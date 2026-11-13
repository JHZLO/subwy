//! Find in the folder (Cmd+Shift+F), after IntelliJ's Find in Files: a line for each match with
//! its file and line number, and below them the selected match's file, read-only, at the match.
//! Enter opens the file at the line.

use std::{
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering::Relaxed},
    },
    time::Duration,
};

use gpui_kit::component::{
    ActiveTheme as _,
    input::{
        Editor, EditorState, Enter, Escape, Input, InputEvent, InputState, MoveDown, MoveUp,
        RopeExt as _,
    },
};
use gpui_kit::{prelude::FluentBuilder as _, *};
use subwy_project::{
    folders::Folders,
    fs,
    index::{self, FileIndex},
    paths,
    search::{self, Results},
};

use crate::{
    editor_tabs, modal,
    theme::{self, c, size, z},
};

/// Sizes at 100% zoom.
const WIDTH: f32 = 960.;
/// Below the title bar, over the top of the tab bar.
const TOP_GAP: f32 = 8.;
/// Room kept free beside and below the panel.
const SIDE_GAP: f32 = 24.;
const BOTTOM_GAP: f32 = 24.;
const HEADER_HEIGHT: f32 = 34.;
const INPUT_HEIGHT: f32 = 34.;
const INPUT_LINE: f32 = 20.;
const ROW_HEIGHT: f32 = 24.;
const PATH_HEIGHT: f32 = 28.;
const PADDING_X: f32 = 12.;
const LIST_PADDING_Y: f32 = 4.;
const SMALL_TEXT: f32 = 12.;
const GAP: f32 = 8.;
/// The preview keeps at least this much when the window is short.
const PREVIEW_MIN: f32 = 120.;

/// The most rows shown above the preview; more scroll.
const LIST_ROWS: usize = 10;
/// Lines of the file shown above the match in the preview.
const PREVIEW_CONTEXT: usize = 3;
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
    /// Each match as (file, line) in `results`.
    rows: Vec<(usize, usize)>,
    selected: usize,
    scroll: UniformListScrollHandle,
    /// The selected match's file, read-only with its syntax colors, and its path.
    preview: Option<(String, Entity<EditorState>)>,
    /// Reads the file for the preview; replacing it drops a read for an older selection.
    preview_load: Task<()>,
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
        let subscriptions =
            vec![
                cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                    match event {
                        InputEvent::Change => this.schedule(window, cx),
                        // A click in the preview reads it; the panel stays.
                        InputEvent::Blur if !this.preview_focused(window, cx) => {
                            cx.emit(SearchEvent::FocusLost)
                        }
                        _ => {}
                    }
                }),
            ];
        let load = cx.spawn_in(window, {
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
                let _ = this.update_in(cx, |this, window, cx| {
                    this.index = Some(Arc::new(index));
                    this.schedule(window, cx);
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
            preview: None,
            preview_load: Task::ready(()),
            cancel: Arc::new(AtomicBool::new(false)),
            _load: load,
            search: Task::ready(()),
            _subscriptions: subscriptions,
        }
    }

    /// Searches for the query once typing pauses, off the main thread.
    fn schedule(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel.store(true, Relaxed);
        let query = self.input.read(cx).value().to_string();
        if query.is_empty() {
            self.search = Task::ready(());
            match (self.links_to.clone(), self.index.clone()) {
                (Some(target), Some(index)) => {
                    let folders = self.folders.clone();
                    self.search = cx.spawn_in(window, async move |this, cx| {
                        let found = cx
                            .background_spawn(async move {
                                search::backlinks(&folders, &index.paths, &target)
                            })
                            .await;
                        let _ = this
                            .update_in(cx, |this, window, cx| this.show(Some(found), window, cx));
                    });
                }
                _ => self.show(None, window, cx),
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
        self.search = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            let found = cx
                .background_spawn(async move {
                    search::search_folders(&folders, &index.paths, &query, &cancel)
                })
                .await;
            if let Some(found) = found {
                let _ = this.update_in(cx, |this, window, cx| this.show(Some(found), window, cx));
            }
        });
    }

    fn show(&mut self, results: Option<Results>, window: &mut Window, cx: &mut Context<Self>) {
        self.rows = results
            .iter()
            .flat_map(|r| r.files.iter().enumerate())
            .flat_map(|(f, file)| (0..file.lines.len()).map(move |l| (f, l)))
            .collect();
        self.results = results;
        self.selected = 0;
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.preview_selected(window, cx);
        cx.notify();
    }

    /// Up and Down stop at the ends.
    fn move_selection(&mut self, down: bool, window: &mut Window, cx: &mut Context<Self>) {
        let next = match down {
            true => self.selected + 1,
            false => self.selected.wrapping_sub(1),
        };
        if next >= self.rows.len() {
            return;
        }
        self.selected = next;
        self.scroll.scroll_to_item(next, ScrollStrategy::Nearest);
        self.preview_selected(window, cx);
        cx.notify();
    }

    fn open(&mut self, row: usize, cx: &mut Context<Self>) {
        let (Some(results), Some(&(f, l))) = (&self.results, self.rows.get(row)) else {
            return;
        };
        let file = &results.files[f];
        cx.emit(SearchEvent::Open {
            path: PathBuf::from(&file.path),
            line: file.lines[l].line,
        });
    }

    /// The selected match: its file's path, its zero-based line and its first matched text.
    fn selected_match(&self) -> Option<(String, u32, String)> {
        let &(f, l) = self.rows.get(self.selected)?;
        let file = &self.results.as_ref()?.files[f];
        let line = &file.lines[l];
        let matched = line
            .ranges
            .first()
            .map_or(String::new(), |range| line.text[range.clone()].to_string());
        Some((file.path.clone(), line.line, matched))
    }

    /// Shows the selected match's file under the list, read off the main thread when it is
    /// another file than the one shown.
    fn preview_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, line, matched)) = self.selected_match() else {
            self.preview = None;
            self.preview_load = Task::ready(());
            return;
        };
        if let Some((shown, editor)) = &self.preview
            && *shown == path
        {
            self.preview_load = Task::ready(());
            reveal(editor, line, &matched, cx);
            return;
        }
        let Some(abs) = self.folders.absolute(Path::new(&path)) else {
            return;
        };
        self.preview_load = cx.spawn_in(window, async move |this, cx| {
            let file = cx
                .background_spawn(async move { fs::read_text(&abs) })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                let Ok(file) = file else {
                    this.preview = None;
                    cx.notify();
                    return;
                };
                let language =
                    editor_tabs::tab_language(Path::new(&path), &file.text, file.stamp.len);
                let editor = cx.new(|cx| {
                    EditorState::new(window, cx)
                        .language(language)
                        .line_number(true)
                        .soft_wrap(false)
                        .default_value(file.text)
                });
                reveal(&editor, line, &matched, cx);
                // The scroll to the line needs the line height, known once the editor is laid out.
                cx.on_next_frame(window, {
                    let editor = editor.clone();
                    move |_, _, cx| reveal(&editor, line, &matched, cx)
                });
                this.preview = Some((path, editor));
                cx.notify();
            });
        });
    }

    /// The preview's file and the text it has selected (the match).
    pub fn preview(&self, cx: &App) -> Option<(String, String)> {
        let (path, editor) = self.preview.as_ref()?;
        Some((path.clone(), editor.read(cx).selected_value().to_string()))
    }

    fn preview_focused(&self, window: &Window, cx: &App) -> bool {
        self.preview
            .as_ref()
            .is_some_and(|(_, editor)| editor.focus_handle(cx).is_focused(window))
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

    /// A match's line, then on the right its file and line number, as IntelliJ lists them.
    fn render_row(&self, row: usize, cx: &mut Context<Self>) -> AnyElement {
        let p = theme::colors(cx);
        let (Some(results), Some(&(f, l))) = (&self.results, self.rows.get(row)) else {
            return div().into_any_element();
        };
        let file = &results.files[f];
        let line = &file.lines[l];
        let selected = row == self.selected;
        let ink = c(if selected { p.sel_ink } else { p.ink });
        div()
            .id(("search-row", row))
            .w_full()
            .h(z(ROW_HEIGHT, cx))
            .px(z(PADDING_X, cx))
            .flex()
            .items_center()
            .gap(z(GAP, cx))
            .whitespace_nowrap()
            .overflow_hidden()
            .when(selected, |el| el.bg(c(p.sel_bg)))
            .when(!selected, |el| el.hover(|style| style.bg(c(p.hover))))
            .on_click(cx.listener(move |this, _, _, cx| this.open(row, cx)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_color(ink)
                    .child(matched_text(&line.text, &line.ranges)),
            )
            .child(
                // A long path loses its start, keeping the folders nearest the file.
                div()
                    .flex_none()
                    .max_w(relative(0.45))
                    .overflow_hidden()
                    .text_ellipsis_start()
                    .text_size(z(SMALL_TEXT, cx))
                    .text_color(if selected { ink } else { c(p.sub) })
                    .child(file.path.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(z(SMALL_TEXT, cx))
                    .text_color(if selected { ink } else { c(p.num) })
                    .child((line.line + 1).to_string()),
            )
            .into_any_element()
    }
}

/// Selects `matched` on `line` of the preview and scrolls the line a few lines below the top,
/// without taking focus from the query.
fn reveal(editor: &Entity<EditorState>, line: u32, matched: &str, cx: &mut App) {
    editor.update(cx, |state, cx| {
        let row = line as usize;
        let text = state.text();
        let start = text.line_start_offset(row);
        let range = text
            .slice_line(row)
            .to_string()
            .find(matched)
            .filter(|_| !matched.is_empty())
            .map_or(start..start, |at| start + at..start + at + matched.len());
        state.set_selected_range(range, cx);
        if let Some(height) = state.line_height() {
            let top = row.saturating_sub(PREVIEW_CONTEXT) as f32;
            state.set_scroll_offset(point(px(0.), -(height * top)), cx);
        }
    });
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
        let viewport = window.viewport_size();
        let width = (WIDTH * zoom).min(viewport.width.as_f32() - 2. * SIDE_GAP * zoom);
        // With matches the panel reaches down the window, the preview taking what the list leaves.
        let height = viewport.height.as_f32() / zoom - size::TITLE_BAR - TOP_GAP - BOTTOM_GAP;
        let room = height
            - (modal::BAND + HEADER_HEIGHT + INPUT_HEIGHT + 2. + 2. * LIST_PADDING_Y)
            - (PATH_HEIGHT + 1. + PREVIEW_MIN);
        let fit = (room / ROW_HEIGHT).max(1.) as usize;
        let shown = self.rows.len().min(LIST_ROWS).min(fit);
        let selected = self.selected_match().map(|(path, ..)| path);
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
            .on_action(
                cx.listener(|this, _: &MoveUp, window, cx| this.move_selection(false, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &MoveDown, window, cx| this.move_selection(true, window, cx)),
            )
            .on_action(cx.listener(|this, _: &Enter, _, cx| this.open(this.selected, cx)))
            .on_action(cx.listener(|_, _: &Escape, _, cx| cx.emit(SearchEvent::Dismiss)))
            .w(px(width.max(320.)))
            .when(shown > 0, |el| el.h(z(height, cx)))
            .text_size(z(size::UI_TEXT, cx))
            .text_color(c(p.ink))
            .child(
                div()
                    .flex_none()
                    .h(z(HEADER_HEIGHT, cx))
                    .px(z(PADDING_X, cx))
                    .flex()
                    .items_center()
                    .gap(z(GAP + 4., cx))
                    .whitespace_nowrap()
                    .child(
                        div()
                            .flex_none()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Find in Files"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_size(z(SMALL_TEXT, cx))
                            .text_color(c(p.sub))
                            .children(self.summary()),
                    )
                    // The folder searched stays in sight once the query hides the placeholder.
                    .children(self.scope.clone().map(|scope| {
                        div()
                            .flex_none()
                            .max_w(z(WIDTH / 3., cx))
                            .overflow_hidden()
                            .text_ellipsis_start()
                            .text_size(z(SMALL_TEXT, cx))
                            .text_color(c(p.sub))
                            .child(format!("in {scope}"))
                    })),
            )
            .child(
                div()
                    .flex_none()
                    .border_t_1()
                    .border_color(c(p.line))
                    .when(shown > 0, |el| el.border_b_1())
                    .child(input),
            )
            .when(shown > 0, |el| {
                el.child(
                    div().flex_none().py(z(LIST_PADDING_Y, cx)).child(
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
                .children(selected.map(|path| path_bar(&path, cx)))
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .border_t_1()
                        .border_color(c(p.line))
                        .children(self.preview.as_ref().map(|(_, editor)| {
                            Editor::new(editor)
                                .bordered(false)
                                .readonly(true)
                                .h_full()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_size(z(theme::editor_text(cx), cx))
                        })),
                )
            })
    }
}

/// The selected match's file over the preview: its name, then the folders it is in.
fn path_bar(path: &str, cx: &App) -> Div {
    let p = theme::colors(cx);
    let (folder, name) = match path.rsplit_once('/') {
        Some((folder, name)) => (folder.to_string(), name.to_string()),
        None => (String::new(), path.to_string()),
    };
    div()
        .flex_none()
        .h(z(PATH_HEIGHT, cx))
        .px(z(PADDING_X, cx))
        .flex()
        .items_center()
        .gap(z(GAP, cx))
        .whitespace_nowrap()
        .border_t_1()
        .border_color(c(p.line))
        .child(
            div()
                .flex_none()
                .font_weight(FontWeight::MEDIUM)
                .child(name),
        )
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis_start()
                .text_size(z(SMALL_TEXT, cx))
                .text_color(c(p.sub))
                .child(folder),
        )
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
