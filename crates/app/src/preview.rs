//! The Markdown preview beside the editor (Cmd+Shift+V).

use std::{
    cell::Cell,
    ops::Range,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::Duration,
};

use gpui_kit::base::{TextView, TextViewStyle};
use gpui_kit::component::{
    ActiveTheme as _,
    input::EditorState,
    text::{
        MarkdownExtensions, MarkdownNode, MarkdownParseContext, RenderedText, TextViewState,
        markdown_ast::Node,
    },
};
use gpui_kit::{TestSupportExt as _, prelude::FluentBuilder as _, *};

use crate::theme::{self, Palette, c, z};

/// Re-render only after typing pauses this long, so a long document does not re-render per key.
const DEBOUNCE: Duration = Duration::from_millis(150);
/// Body text size at 100% zoom; headings below h2 keep gpui-kit's own ratios to it.
const BODY: f32 = 14.;
/// Top padding at 100% zoom; a line the preview follows lands this far below the top edge.
const TOP: f32 = 26.;
/// Padding at the sides. Kept small: the text it renders sits just across the divider.
const SIDE: f32 = 24.;
/// Frames a reveal may take to report its line before the preview stops waiting for it.
const REVEAL_FRAMES: u8 = 2;
/// The custom block of an image on disk, see [`local_image`].
const LOCAL_IMAGE: &str = "subwy-local-image";

/// What a click on a link in the preview does with the link's target.
type OnLink = Arc<dyn Fn(&str, &mut Window, &mut App) + Send + Sync>;

pub struct Preview {
    state: Entity<TextViewState>,
    /// The Markdown editor shown; none when the active tab is not Markdown.
    source: Option<Entity<EditorState>>,
    /// The shown file's folder, where relative image paths start.
    folder: Option<PathBuf>,
    extensions: MarkdownExtensions,
    task: Task<()>,
    scroll: ScrollHandle,
    /// The editor's top visible line, which the preview keeps at its own top.
    top_line: Option<usize>,
    /// `top_line` changed and the preview has not moved there yet.
    follow_pending: bool,
    /// The line the text view is revealing for the preview, until it reports where that line is.
    reveal: Rc<Cell<Option<Reveal>>>,
    /// A click on a link: its target, as written, for the tabs to follow from the shown file.
    on_link: OnLink,
    _state: Subscription,
}

/// A line [`TextViewState::reveal_range`] was asked to find, and which of its edges goes to the top.
#[derive(Clone, Copy)]
struct Reveal {
    edge: Edge,
    /// Frames left in which the preview hides every line, see [`HideLines`].
    frames: u8,
}

#[derive(Clone, Copy)]
enum Edge {
    Top,
    Bottom,
}

/// Where the preview goes to follow an editor line.
enum Target {
    Start,
    End,
    /// A line of the rendered text, by an offset into it.
    Line {
        at: usize,
        edge: Edge,
    },
}

impl Preview {
    pub fn new(
        on_link: impl Fn(&str, &mut Window, &mut App) + Send + Sync + 'static,
        cx: &mut Context<Self>,
    ) -> Self {
        let state = cx.new(|cx| TextViewState::markdown("", cx));
        // A parse that lands may be the one a move of the editor was waiting for.
        let _state = cx.observe(&state, |this, _, cx| this.line_up(cx));
        Self {
            state,
            source: None,
            folder: None,
            extensions: extensions(None),
            task: Task::ready(()),
            scroll: ScrollHandle::new(),
            top_line: None,
            follow_pending: false,
            reveal: Rc::default(),
            on_link: Arc::new(on_link),
            _state,
        }
    }

    /// Shows `source`, a file in `folder`, now, dropping a pending update. Images with a relative
    /// path load from `folder`; without one only absolute paths do.
    pub fn show(
        &mut self,
        source: Option<Entity<EditorState>>,
        folder: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if self.folder != folder {
            // Only the renderer reads the folder, so the document is not parsed again.
            self.extensions = extensions(folder.clone());
            self.folder = folder;
        }
        self.task = Task::ready(());
        if self.source != source {
            self.top_line = None;
            self.scroll.set_offset(Point::default());
        }
        self.source = source;
        self.update_text(cx);
        self.follow(cx);
    }

    /// Shows the source's new text once typing pauses.
    pub fn schedule(&mut self, cx: &mut Context<Self>) {
        self.task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| this.update_text(cx));
        });
    }

    fn update_text(&mut self, cx: &mut Context<Self>) {
        if let Some(source) = &self.source {
            // Read here, after the pause, so typing does not copy the document on every key.
            let text = source.read(cx).value();
            self.state.update(cx, |state, cx| state.set_text(&text, cx));
        }
        cx.notify();
    }

    /// Follows the editor when its top visible line changed. Called after each frame: the editor
    /// learns which lines it shows while it paints, a frame after it scrolled.
    fn follow(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = &self.source else {
            return;
        };
        let line = editor.read(cx).visible_row_range().map(|rows| rows.start);
        if line.is_none() || line == self.top_line {
            return;
        }
        self.top_line = line;
        self.follow_pending = true;
        self.line_up(cx);
    }

    /// Scrolls so the editor's top line is at the top, once the text view shows the editor's text.
    ///
    /// gpui-kit maps a source range to the rendered text, and `reveal_range` finds the line that
    /// text was laid out on; the reveal reports that line's bounds to `on_reveal`, which scrolls.
    fn line_up(&mut self, cx: &mut Context<Self>) {
        let (Some(editor), Some(line), true) = (&self.source, self.top_line, self.follow_pending)
        else {
            return;
        };
        let rendered = self.state.read(cx).rendered_text();
        // Until its parse lands the text view holds older text, whose lines are not the editor's.
        if rendered.source().len() != editor.read(cx).text().len() {
            return;
        }
        self.follow_pending = false;
        self.reveal.set(None);
        match target(&rendered, line) {
            Target::Start => self.scroll.set_offset(Point::default()),
            Target::End => self.scroll.scroll_to_bottom(),
            Target::Line { at, edge } => {
                let requested = self
                    .state
                    .update(cx, |state, cx| state.reveal_range(at..at, cx));
                if requested.is_ok() {
                    self.reveal.set(Some(Reveal {
                        edge,
                        frames: REVEAL_FRAMES,
                    }));
                }
            }
        }
        cx.notify();
    }

    /// Whether this frame hides every line from the reveal, counting the frame against its budget.
    fn take_hide_frame(&self) -> bool {
        match self.reveal.get() {
            Some(reveal) if reveal.frames > 0 => {
                self.reveal.set(Some(Reveal {
                    frames: reveal.frames - 1,
                    ..reveal
                }));
                true
            }
            _ => false,
        }
    }

    fn text_view(&self, p: &Palette, cx: &App) -> TextView {
        let scroll = self.scroll.clone();
        let reveal = self.reveal.clone();
        let on_link = self.on_link.clone();
        let top = z(TOP, cx);
        TextView::new(&self.state)
            .style(preview_style(p, cx))
            .markdown_extensions(self.extensions.clone())
            // Instead of the browser for every link, which leaves a link to a file going nowhere.
            .on_link_click(move |url, event, window, cx| {
                if event.standard_click() {
                    on_link(url, window, cx);
                }
            })
            // Called while the revealed line is off screen; it lands at once, no animation.
            .on_reveal(move |line, window, _| {
                let Some(Reveal { edge, .. }) = reveal.take() else {
                    return;
                };
                let y = match edge {
                    Edge::Top => line.top(),
                    Edge::Bottom => line.bottom(),
                };
                let offset = scroll.offset();
                let max = scroll.max_offset().y;
                let goal = offset.y + scroll.bounds().top() + top - y;
                scroll.set_offset(point(offset.x, goal.clamp(-max, px(0.))));
                window.refresh();
            })
    }
}

/// Where the editor's line `line` (from 0) of the rendered text's source shows in the preview.
fn target(rendered: &RenderedText, line: usize) -> Target {
    if line == 0 {
        return Target::Start;
    }
    let source = rendered.source();
    let Some(range) = line_range(source, line) else {
        return Target::End;
    };
    // The line's own text, when it renders any, goes to the top.
    if let Some(text) = rendered.range_for_source(range.clone()) {
        return Target::Line {
            at: text.start,
            edge: Edge::Top,
        };
    }
    // A line that renders no text (blank, a fence, an image, a rule) is what follows the text
    // before it, so that text goes just above the top.
    match rendered.range_for_source(0..range.start) {
        Some(before) => Target::Line {
            at: rendered.as_str()[..before.end]
                .char_indices()
                .next_back()
                .map_or(0, |(i, _)| i),
            edge: Edge::Bottom,
        },
        None => Target::Start,
    }
}

/// The byte range of line `line` (from 0) of `text`, without its line break.
fn line_range(text: &str, line: usize) -> Option<Range<usize>> {
    let start = match line {
        0 => 0,
        _ => text.match_indices('\n').nth(line - 1)?.0 + 1,
    };
    let end = text[start..].find('\n').map_or(text.len(), |i| start + i);
    Some(start..end)
}

/// gpui-kit fetches every image as a URL, so images on disk would not show, and one that fails to
/// load leaves nothing behind. A paragraph that is only an image on disk becomes a custom block,
/// drawn by [`image_block`]; web and data images keep gpui-kit's own handling.
fn extensions(folder: Option<PathBuf>) -> MarkdownExtensions {
    MarkdownExtensions::default()
        .block_parser(local_image)
        .block_renderer(LOCAL_IMAGE, move |node, _, cx| {
            image_block(node, folder.as_deref(), cx)
        })
}

/// What a [`LOCAL_IMAGE`] block carries.
struct LocalImage {
    url: String,
    alt: SharedString,
}

/// Claims a paragraph holding nothing but an image on disk. Runs while the text view parses.
fn local_image(node: &Node, cx: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
    let Node::Paragraph(paragraph) = node else {
        return None;
    };
    let [Node::Image(image)] = paragraph.children.as_slice() else {
        return None;
    };
    if !is_local(&image.url) {
        return None;
    }
    let block = LocalImage {
        url: image.url.clone(),
        alt: image.alt.clone().into(),
    };
    // No text of its own, like gpui-kit's images, so copying the preview leaves it out.
    Some(
        MarkdownNode::new(LOCAL_IMAGE, block)
            .markdown(cx.node_source(node).unwrap_or_default())
            .accessibility_label(image.alt.clone()),
    )
}

/// The image, read off the main thread by gpui, or its alt text in `sub` when it cannot be shown.
fn image_block(node: &MarkdownNode, folder: Option<&Path>, cx: &App) -> AnyElement {
    let Some(image) = node.data::<LocalImage>() else {
        return div().into_any_element();
    };
    let sub = c(theme::colors(cx).sub);
    let alt = if image.alt.is_empty() {
        SharedString::from(image.url.clone())
    } else {
        image.alt.clone()
    };
    let missing = move || div().text_color(sub).child(alt.clone()).into_any_element();
    match image_path(&image.url, folder) {
        Some(path) => img(path)
            .object_fit(ObjectFit::Contain)
            .max_w(relative(1.))
            .with_fallback(missing)
            .into_any_element(),
        None => missing(),
    }
}

/// Whether an image URL names a file on disk (a path or a `file:` URL), not a web or data URL.
fn is_local(url: &str) -> bool {
    match url.split_once(':') {
        Some((scheme, _)) if is_scheme(scheme) => scheme.eq_ignore_ascii_case("file"),
        _ => !url.is_empty() && !url.starts_with("//"),
    }
}

fn is_scheme(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// The file a local image URL names: absolute as written, or relative to `folder`.
fn image_path(url: &str, folder: Option<&Path>) -> Option<PathBuf> {
    let path = url
        .strip_prefix("file://")
        .or_else(|| url.strip_prefix("file:"))
        .unwrap_or(url);
    let path = PathBuf::from(percent_decoded(path));
    if path.is_absolute() {
        Some(path)
    } else {
        folder.map(|folder| folder.join(path))
    }
}

/// `text` with `%XX` escapes decoded, as Markdown writes a space in a path (`my%20photo.png`).
fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = text
            .get(i + 1..i + 3)
            .filter(|hex| hex.bytes().all(|b| b.is_ascii_hexdigit()));
        match (bytes[i], hex) {
            (b'%', Some(hex)) => {
                out.push(u8::from_str_radix(hex, 16).unwrap_or_default());
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Lays out and paints its child as is, but can prepaint it under an empty content mask, which
/// makes every line count as off screen. gpui-kit reports where a revealed line is only when that
/// line is off screen; hiding them all for one frame makes it report a line in view too.
struct HideLines {
    child: AnyElement,
    hide: bool,
}

impl IntoElement for HideLines {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for HideLines {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if self.hide {
            let mask = ContentMask {
                bounds: Bounds::new(bounds.origin, size(bounds.size.width, px(0.))),
            };
            window.with_content_mask(Some(mask), |window| {
                self.child.prepaint(window, cx);
            });
        } else {
            self.child.prepaint(window, cx);
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

/// The design's preview typography, as far as the gpui-kit text view takes it.
fn preview_style(p: &Palette, cx: &App) -> TextViewStyle {
    let zoom = theme::zoom(cx);
    let title = c(p.syn_title);
    let none = transparent_black();
    TextViewStyle::default()
        .with_foreground(c(p.ink))
        // Blockquotes.
        .with_muted_foreground(c(p.sub))
        .with_link(c(p.syn_type))
        .with_selection(c(p.sel_bg))
        .with_code_background(c(p.side))
        .with_border(c(p.line))
        .with_heading(move |level| {
            let size = match level {
                1 => 24.,
                2 => 18.,
                3 => BODY * 17. / 15.,
                4 => BODY,
                5 => BODY * 14. / 15.,
                _ => BODY * 13. / 15.,
            };
            StyleRefinement::default()
                .text_size(px(size * zoom))
                .line_height(relative(1.3))
                .text_color(title)
        })
        .with_inline_code(HighlightStyle {
            color: Some(c(p.syn_literal)),
            background_color: Some(c(p.side)),
            ..Default::default()
        })
        .with_code_block(StyleRefinement::default().rounded(z(4., cx)))
        // Tables are 1 px rules under each row and nothing else: no frame, no column lines, no fills.
        .with_table(
            StyleRefinement::default()
                .text_size(z(13., cx))
                .bg(none)
                .border_0()
                .border_b_1(),
        )
        .with_table_head(
            StyleRefinement::default()
                .bg(none)
                .font_weight(FontWeight::SEMIBOLD),
        )
        .with_table_cell(
            StyleRefinement::default()
                .border_color(none)
                .pl(px(0.))
                .pr(z(10., cx))
                .py(z(6., cx)),
        )
        .with_dark(cx.theme().is_dark())
}

impl Render for Preview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.source.is_some() {
            cx.defer_in(window, |this, _, cx| this.follow(cx));
        }
        let p = theme::colors(cx);
        div()
            .id("preview")
            .track_scroll(&self.scroll)
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .bg(c(p.paper))
            .border_l_1()
            .border_color(c(p.line))
            .px(z(SIDE, cx))
            .py(z(TOP, cx))
            .text_color(c(p.ink))
            .map(|el| {
                if self.source.is_some() {
                    let lines = HideLines {
                        child: self.text_view(p, cx).into_any_element(),
                        hide: self.take_hide_frame(),
                    };
                    el.text_size(z(BODY, cx))
                        .line_height(relative(1.7))
                        // Observed so the offscreen checks can see how far the page scrolled.
                        .child(div().id("preview-page").child(lines).test_support())
                } else {
                    el.text_size(z(13., cx))
                        .text_color(c(p.sub))
                        .child("Preview shows Markdown files")
                }
            })
            .test_support()
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{image_path, is_local, line_range};

    #[test]
    fn local_images_are_paths_and_file_urls() {
        for url in [
            "pic.png",
            "../img/pic.png",
            "/abs/pic.png",
            "file:///abs/pic.png",
        ] {
            assert!(is_local(url), "{url}");
        }
        for url in [
            "https://example.com/pic.png",
            "http://example.com/pic.png",
            "data:image/png;base64,iVBORw0KGgo=",
            "//example.com/pic.png",
            "",
        ] {
            assert!(!is_local(url), "{url}");
        }
    }

    #[test]
    fn image_paths_resolve_against_the_folder() {
        let folder = Path::new("/notes/docs");
        assert_eq!(
            image_path("img/my%20pic.png", Some(folder)),
            Some(PathBuf::from("/notes/docs/img/my pic.png"))
        );
        assert_eq!(
            image_path("../pic.png", Some(folder)),
            Some(PathBuf::from("/notes/docs/../pic.png"))
        );
        assert_eq!(
            image_path("file:///abs/pic.png", None),
            Some(PathBuf::from("/abs/pic.png"))
        );
        assert_eq!(
            image_path("/abs/100%.png", None),
            Some(PathBuf::from("/abs/100%.png"))
        );
        // Without the file's folder a relative path names nothing, and the alt text shows.
        assert_eq!(image_path("pic.png", None), None);
    }

    #[test]
    fn line_range_finds_each_line_without_its_break() {
        let text = "# Title\n\nbody\r\nlast";
        let lines: Vec<_> = (0..4)
            .map(|i| line_range(text, i).map(|r| &text[r]))
            .collect();
        assert_eq!(
            lines,
            [Some("# Title"), Some(""), Some("body\r"), Some("last")]
        );
        assert_eq!(line_range(text, 4), None);
        assert_eq!(line_range("a\n", 1), Some(2..2));
    }
}
