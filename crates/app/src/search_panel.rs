//! Find in the folder (Cmd+Shift+F): lines that match, grouped by file; Enter opens the file at the line.

use std::{
    ops::Range,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering::Relaxed},
    },
    time::Duration,
};

use gpui_kit::component::input::{Enter, Escape, Input, InputEvent, InputState, MoveDown, MoveUp};
use gpui_kit::{prelude::FluentBuilder as _, *};
use subwy_project::{
    folders::Folders,
    index::{self, FileIndex},
    paths,
    search::{self, Results},
};

use crate::{
    modal,
    theme::{self, c, size, z},
};

/// Sizes at 100% zoom; the finder's, wider for lines of text.
const WIDTH: f32 = 720.;
/// Below the title bar, over the top of the tab bar.
const TOP_GAP: f32 = 8.;
const INPUT_HEIGHT: f32 = 34.;
const INPUT_LINE: f32 = 20.;
const ROW_HEIGHT: f32 = 24.;
const PADDING_X: f32 = 12.;
const LIST_PADDING_Y: f32 = 4.;
const SMALL_TEXT: f32 = 12.;
/// Wide enough for a five-digit line number at 12 px.
const NUMBER_WIDTH: f32 = 36.;
const NUMBER_GAP: f32 = 8.;
/// Room kept free below the panel when the window is short or zoomed in.
const BOTTOM_GAP: f32 = 24.;

/// The most rows shown at once; more scroll.
const MAX_ROWS: usize = 16;
/// Search only once typing pauses: every search reads the whole folder.
const DEBOUNCE: Duration = Duration::from_millis(150);

pub enum SearchEvent {
    /// Open this file (relative to the open folder) with the cursor on this zero-based line.
    Open { path: PathBuf, line: u32 },
    /// Escape: close and give focus back to where it was.
    Dismiss,
    /// Focus went elsewhere: close and leave it there.
    FocusLost,
}

/// One row of the list: a file's path, or one of its matching lines.
#[derive(Clone, Copy)]
enum Row {
    File(usize),
    Line(usize, usize),
}

pub struct SearchPanel {
    folders: Folders,
    /// The folder searched, as the tree names it, ending in `/`; the whole window when `None`.
    scope: Option<String>,
    /// Alt+F7: with no query, the list is the lines that link to this file.
    links_to: Option<PathBuf>,
    input: Entity<InputState>,
    /// Empty until the walk of the folder, started when the panel opens, comes back.
    index: Option<Arc<FileIndex>>,
    /// The latest finished search; it stays up while the next one runs. `None` for no query.
    results: Option<Results>,
    rows: Vec<Row>,
    /// Always a line row while there are any.
    selected: usize,
    scroll: UniformListScrollHandle,
    /// Stops the search still reading, once a newer query or closing makes it useless.
    cancel: Arc<AtomicBool>,
    _load: Task<()>,
    search: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SearchEvent> for SearchPanel {}

impl Focusable for SearchPanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl Drop for SearchPanel {
    fn drop(&mut self) {
        self.cancel.store(true, Relaxed);
    }
}

impl SearchPanel {
    /// Searches the window's folders, or only the folder `scope` (Cmd+Shift+F on a folder in the
    /// tree, IntelliJ's Directory scope). With `links_to` (Alt+F7) it first lists the lines that
    /// link to that file; typing searches as usual.
    pub fn new(
        folders: Folders,
        scope: Option<PathBuf>,
        links_to: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let scope = scope.map(|dir| format!("{}/", paths::relative(&dir)));
        let placeholder = match (&links_to, &scope) {
            (Some(file), _) => format!(
                "Files linking to {}",
                file.file_name().unwrap_or_default().to_string_lossy()
            ),
            (None, Some(dir)) => format!("Find in {}", dir.trim_end_matches('/')),
            (None, None) => "Find in folder".to_string(),
        };
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let subscriptions = vec![cx.subscribe_in(
            &input,
            window,
            |this, _, event: &InputEvent, _, cx| match event {
                InputEvent::Change => this.schedule(cx),
                InputEvent::Blur => cx.emit(SearchEvent::FocusLost),
                _ => {}
            },
        )];
        let load = cx.spawn({
            let folders = folders.clone();
            let scope = scope.clone();
            async move |this, cx| {
                let index = cx
                    .background_spawn(async move {
                        let mut index = index::build_folders(&folders);
                        if let Some(scope) = scope {
                            index.paths.retain(|path| path.starts_with(&scope));
                        }
                        index
                    })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.index = Some(Arc::new(index));
                    this.schedule(cx);
                });
            }
        });
        Self {
            folders,
            scope,
            links_to,
            input,
            index: None,
            results: None,
            rows: Vec::new(),
            selected: 0,
            scroll: UniformListScrollHandle::new(),
            cancel: Arc::new(AtomicBool::new(false)),
            _load: load,
            search: Task::ready(()),
            _subscriptions: subscriptions,
        }
    }

    /// Searches for the query once typing pauses, off the main thread.
    fn schedule(&mut self, cx: &mut Context<Self>) {
        self.cancel.store(true, Relaxed);
        let query = self.input.read(cx).value().to_string();
        if query.is_empty() {
            self.search = Task::ready(());
            match (self.links_to.clone(), self.index.clone()) {
                (Some(target), Some(index)) => {
                    let folders = self.folders.clone();
                    self.search = cx.spawn(async move |this, cx| {
                        let found = cx
                            .background_spawn(async move {
                                search::backlinks(&folders, &index.paths, &target)
                            })
                            .await;
                        let _ = this.update(cx, |this, cx| this.show(Some(found), cx));
                    });
                }
                _ => self.show(None, cx),
            }
            return;
        }
        // The index arriving schedules the search again.
        let Some(index) = self.index.clone() else {
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = cancel.clone();
        let folders = self.folders.clone();
        // Replacing the task drops a search still waiting out the pause.
        self.search = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            let found = cx
                .background_spawn(async move {
                    search::search_folders(&folders, &index.paths, &query, &cancel)
                })
                .await;
            if let Some(found) = found {
                let _ = this.update(cx, |this, cx| this.show(Some(found), cx));
            }
        });
    }

    fn show(&mut self, results: Option<Results>, cx: &mut Context<Self>) {
        self.rows.clear();
        for (f, file) in results.iter().flat_map(|r| r.files.iter()).enumerate() {
            self.rows.push(Row::File(f));
            self.rows
                .extend((0..file.lines.len()).map(|l| Row::Line(f, l)));
        }
        self.results = results;
        // The first line row: a file row always comes first.
        self.selected = 1;
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    /// Up and Down step over the file rows and stop at the ends.
    fn move_selection(&mut self, down: bool, cx: &mut Context<Self>) {
        if self.rows.is_empty() {
            return;
        }
        let mut next = self.selected;
        loop {
            next = match down {
                true if next + 1 < self.rows.len() => next + 1,
                false if next > 0 => next - 1,
                _ => return,
            };
            if matches!(self.rows[next], Row::Line(..)) {
                break;
            }
        }
        self.selected = next;
        // Moving up onto a file's first line shows the file's path above it too.
        let shown = match self.rows[next - 1] {
            Row::File(_) if !down => next - 1,
            _ => next,
        };
        self.scroll.scroll_to_item(shown, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn open(&mut self, row: usize, cx: &mut Context<Self>) {
        let (Some(results), Some(Row::Line(f, l))) = (&self.results, self.rows.get(row)) else {
            return;
        };
        let file = &results.files[*f];
        cx.emit(SearchEvent::Open {
            path: PathBuf::from(&file.path),
            line: file.lines[*l].line,
        });
    }

    /// "12 matches in 4 files"; `None` before the first search for a query.
    pub fn summary(&self) -> Option<String> {
        let results = self.results.as_ref()?;
        let files = results.files.len();
        Some(if results.matches == 0 {
            "No matches".to_string()
        } else if results.capped {
            format!(
                "First {} matches in {}",
                thousands(results.matches),
                plural(files, "file")
            )
        } else {
            format!(
                "{} in {}",
                plural(results.matches, "match"),
                plural(files, "file")
            )
        })
    }

    fn render_row(&self, row: usize, cx: &mut Context<Self>) -> AnyElement {
        let p = theme::colors(cx);
        let Some(results) = &self.results else {
            return div().into_any_element();
        };
        let base = div()
            .w_full()
            .h(z(ROW_HEIGHT, cx))
            .px(z(PADDING_X, cx))
            .flex()
            .items_center()
            .whitespace_nowrap()
            .overflow_hidden();
        let (f, l) = match self.rows[row] {
            Row::File(f) => {
                // A long path loses its start, keeping the folders nearest the file.
                return base
                    .text_size(z(SMALL_TEXT, cx))
                    .text_color(c(p.sub))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis_start()
                            .child(results.files[f].path.clone()),
                    )
                    .into_any_element();
            }
            Row::Line(f, l) => (f, l),
        };
        let line = &results.files[f].lines[l];
        let selected = row == self.selected;
        let ink = c(if selected { p.sel_ink } else { p.ink });
        base.id(("search-row", row))
            .gap(z(NUMBER_GAP, cx))
            .when(selected, |el| el.bg(c(p.sel_bg)))
            .when(!selected, |el| el.hover(|style| style.bg(c(p.hover))))
            .on_click(cx.listener(move |this, _, _, cx| this.open(row, cx)))
            .child(
                div()
                    .flex_none()
                    .w(z(NUMBER_WIDTH, cx))
                    .flex()
                    .justify_end()
                    .text_size(z(SMALL_TEXT, cx))
                    .text_color(if selected { ink } else { c(p.num) })
                    .child((line.line + 1).to_string()),
            )
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_color(ink)
                    .child(matched_text(&line.text, &line.ranges)),
            )
            .into_any_element()
    }
}

/// The line with its matches in semibold: medium barely shows at 13 px, and green is for the
/// subwy devices only.
fn matched_text(text: &str, ranges: &[Range<usize>]) -> StyledText {
    let matched = HighlightStyle {
        font_weight: Some(FontWeight::SEMIBOLD),
        ..Default::default()
    };
    StyledText::new(text.to_string())
        .with_highlights(ranges.iter().map(|range| (range.clone(), matched)))
}

fn plural(count: usize, noun: &str) -> String {
    let suffix = match (count, noun.ends_with('h')) {
        (1, _) => "",
        (_, true) => "es",
        (_, false) => "s",
    };
    format!("{} {noun}{suffix}", thousands(count))
}

/// 2000 as "2,000".
fn thousands(count: usize) -> String {
    let digits = count.to_string();
    let mut out = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// Places the panel at the top center of the dimmed window, over the tabs, like the finder. A
/// click beside it runs `on_outside`.
pub fn overlay(
    panel: Entity<SearchPanel>,
    on_outside: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    modal::backdrop(on_outside, cx)
        .flex()
        .justify_center()
        .items_start()
        .pt(z(size::TITLE_BAR + TOP_GAP, cx))
        .child(panel)
}

impl Render for SearchPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = theme::colors(cx);
        let zoom = theme::zoom(cx);
        let summary = self.summary();
        // As many rows as fit above the window's bottom edge, at most MAX_ROWS.
        let room = window.viewport_size().height.as_f32() / zoom
            - (size::TITLE_BAR + TOP_GAP + modal::BAND + INPUT_HEIGHT + 2. * ROW_HEIGHT)
            - 2. * LIST_PADDING_Y
            - BOTTOM_GAP;
        let fit = (room / ROW_HEIGHT).max(1.) as usize;
        let shown = self.rows.len().min(MAX_ROWS).min(fit);
        let input = Input::new(&self.input)
            .appearance(false)
            .px(z(PADDING_X, cx))
            .text_size(z(size::UI_TEXT, cx))
            .line_height(z(INPUT_LINE, cx));
        // Input's own `h` sizes only multi-line inputs; the style's height sizes a single line.
        let input = Styled::h(input, z(INPUT_HEIGHT, cx));
        modal::sheet(modal::SEARCH_LINE, cx)
            .id("search")
            // Clicks inside stay here: the dimmed window behind would close it.
            .occlude()
            .on_action(cx.listener(|this, _: &MoveUp, _, cx| this.move_selection(false, cx)))
            .on_action(cx.listener(|this, _: &MoveDown, _, cx| this.move_selection(true, cx)))
            .on_action(cx.listener(|this, _: &Enter, _, cx| this.open(this.selected, cx)))
            .on_action(cx.listener(|_, _: &Escape, _, cx| cx.emit(SearchEvent::Dismiss)))
            .w(z(WIDTH, cx))
            .text_size(z(size::UI_TEXT, cx))
            .text_color(c(p.ink))
            .child(
                div()
                    .flex()
                    .items_center()
                    .when(summary.is_some(), |el| {
                        el.border_b_1().border_color(c(p.line))
                    })
                    .child(div().flex_1().min_w_0().child(input))
                    // The folder searched stays in sight once the query hides the placeholder.
                    .children(self.scope.clone().map(|scope| {
                        div()
                            .flex_none()
                            .max_w(z(WIDTH / 3., cx))
                            .pr(z(PADDING_X, cx))
                            .overflow_hidden()
                            .text_ellipsis_start()
                            .whitespace_nowrap()
                            .text_size(z(SMALL_TEXT, cx))
                            .text_color(c(p.sub))
                            .child(format!("in {scope}"))
                    })),
            )
            .when_some(summary, |el, summary| {
                el.child(
                    div()
                        .h(z(ROW_HEIGHT, cx))
                        .px(z(PADDING_X, cx))
                        .flex()
                        .items_center()
                        .text_size(z(SMALL_TEXT, cx))
                        .text_color(c(p.sub))
                        .child(summary),
                )
            })
            .when(shown > 0, |el| {
                el.child(
                    div().pb(z(LIST_PADDING_Y, cx)).child(
                        uniform_list(
                            "search-rows",
                            self.rows.len(),
                            cx.processor(|this, range: Range<usize>, _, cx| {
                                range.map(|row| this.render_row(row, cx)).collect()
                            }),
                        )
                        .track_scroll(&self.scroll)
                        .w_full()
                        .h(z(ROW_HEIGHT * shown as f32, cx)),
                    ),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: GPUI's glob export includes a `test` macro that would replace the built-in one.
    use super::{plural, thousands};

    #[test]
    fn counts_read_as_english() {
        assert_eq!(plural(1, "match"), "1 match");
        assert_eq!(plural(12, "match"), "12 matches");
        assert_eq!(plural(1, "file"), "1 file");
        assert_eq!(plural(4, "file"), "4 files");
        assert_eq!(thousands(2000), "2,000");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }
}
