//! Open files as tabs, each over one gpui-kit editor: the pass-through tab, the paper sheet, saving.

use std::{
    io,
    ops::Range,
    path::{Path, PathBuf},
};

use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, WindowExt as _,
    button::Button,
    dialog::{DialogEntrance, DialogFooter},
    input::{
        Editor, EditorState, InputEvent, MoveDown, MoveUp, Position, Rope, RopeExt as _, TabSize,
    },
    tooltip::Tooltip,
};
use gpui_kit::{assets::IconName, prelude::FluentBuilder as _, *};
use subwy_project::{
    folders::Folders,
    fs::{self, FileStamp, LineEnding, MAX_HIGHLIGHT_BYTES, ReadError, TextFile},
    markdown::{self, Edit, Emphasis},
    session::TabState,
};

use crate::{
    keymap::{DeleteIndent, IndentItem, MARKDOWN, Newline, OutdentItem, ToggleBold, ToggleItalic},
    preview::Preview,
    theme::{self, Palette, c, size, z},
};

/// How far up and down from a list item Tab looks for its list's indent unit; nearby lines
/// show it, and the bound keeps the key cheap in a long block.
const LIST_LOOKAROUND: usize = 100;

pub struct Tab {
    /// Relative to the open folder.
    pub path: PathBuf,
    pub editor: Entity<EditorState>,
    /// Opened with Space from the tree; replaced by the next pass-through, kept once edited or opened with Enter.
    pub pass_through: bool,
    pub dirty: bool,
    pub read_only: bool,
    pub line_ending: LineEnding,
    pub bom: bool,
    pub stamp: FileStamp,
    pub language: SharedString,
    /// The file is gone from disk; saving writes it again.
    deleted: bool,
    /// The file changed on disk under unsaved edits: its stamp there, until the user picks a side.
    conflict: Option<FileStamp>,
    /// Counts edits, so a save that finishes after more typing leaves the tab dirty.
    edits: u64,
    saving: bool,
    _subscriptions: Vec<Subscription>,
}

impl Tab {
    pub fn is_markdown(&self) -> bool {
        self.language == "markdown"
    }
}

/// Something the workspace shows (title, stem, status) may have changed.
pub struct TabsChanged;

/// A file being read on the background executor for [`EditorTabs::open`].
struct PendingOpen {
    id: u64,
    path: PathBuf,
    pass_through: bool,
    /// Where to put the cursor once open (zero-based line and column), for a restored session.
    cursor: Option<(u32, u32)>,
}

pub struct EditorTabs {
    /// Where the tabs' relative paths are on disk.
    folders: Folders,
    tabs: Vec<Tab>,
    active: Option<usize>,
    pending: Vec<PendingOpen>,
    next_open: u64,
    /// While a session's tabs load, only this one takes the active place, so the order reads finish in does not matter.
    restoring_active: Option<PathBuf>,
    /// The active tab's cursor lines as last reported, so caret blinks do not count as changes.
    last_lines: Option<(u32, u32)>,
    preview: Option<Entity<Preview>>,
    /// IntelliJ's Preview only: the preview takes the whole pane for a Markdown tab.
    preview_only: bool,
    /// The editor's share of the width beside the preview, dragged from the line between them.
    split: f32,
    focus_handle: FocusHandle,
    /// The last file that could not be opened, and why; shown in the status bar.
    pub error: Option<String>,
}

impl EventEmitter<TabsChanged> for EditorTabs {}

impl Focusable for EditorTabs {
    /// The active tab's editor, unless only its preview shows: keys must not edit hidden text.
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self.active_tab() {
            Some(tab) if !self.editor_hidden() => tab.editor.focus_handle(cx),
            _ => self.focus_handle.clone(),
        }
    }
}

/// How a Markdown tab shows (IntelliJ's three buttons at the editor's top right).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Editor,
    Split,
    Preview,
}

/// What the line between the editor and the preview drags.
struct SplitDrag;

fn file_name(path: &Path) -> SharedString {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
        .into()
}

/// The active tab after the tab at `closed` is removed, leaving `len` tabs.
fn active_after_close(active: Option<usize>, closed: usize, len: usize) -> Option<usize> {
    let active = active?;
    if len == 0 {
        None
    } else if active > closed {
        Some(active - 1)
    } else {
        Some(active.min(len - 1))
    }
}

/// What Tab inserts, from the settings.
fn tab_size(settings: &subwy_project::settings::Settings) -> TabSize {
    TabSize {
        tab_size: settings.tab_size,
        hard_tabs: settings.hard_tabs,
    }
}

/// A tab's language: its file's, or plain text past the size highlighting stops at.
fn tab_language(path: &Path, len: u64) -> SharedString {
    match len > MAX_HIGHLIGHT_BYTES {
        true => "text".into(),
        false => language_for(path),
    }
}

fn language_for(path: &Path) -> SharedString {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "md" | "markdown" | "mdx" => "markdown".into(),
        "yml" => "yaml".into(),
        "ts" | "mts" | "cts" => "typescript".into(),
        "js" | "mjs" | "cjs" => "javascript".into(),
        "" => "text".into(),
        other => other.to_string().into(),
    }
}

impl EditorTabs {
    /// Where `rel` is on disk.
    fn abs(&self, rel: &Path) -> PathBuf {
        self.folders
            .absolute(rel)
            .unwrap_or_else(|| rel.to_path_buf())
    }

    /// The window's folders changed: each tab keeps its file under the file's new path. A tab whose
    /// folder left the window closes; the workspace keeps a folder with unsaved tabs from leaving.
    pub fn set_folders(&mut self, folders: Folders, window: &mut Window, cx: &mut Context<Self>) {
        let moved =
            |old: &Folders, rel: &Path| old.absolute(rel).and_then(|abs| folders.relative(&abs));
        let mapped: Vec<Option<PathBuf>> = self
            .tabs
            .iter()
            .map(|tab| moved(&self.folders, &tab.path))
            .collect();
        for index in (0..mapped.len()).rev() {
            if mapped[index].is_none() {
                self.remove_tab(index, window, cx);
            }
        }
        for (tab, path) in self.tabs.iter_mut().zip(mapped.into_iter().flatten()) {
            tab.path = path;
        }
        let old = std::mem::replace(&mut self.folders, folders.clone());
        self.pending
            .retain_mut(|open| match moved(&old, &open.path) {
                Some(path) => {
                    open.path = path;
                    true
                }
                None => false,
            });
        self.refresh_preview(cx);
        cx.emit(TabsChanged);
        cx.notify();
    }

    /// Whether a tab under `rel` has unsaved changes.
    pub fn unsaved_under(&self, rel: &Path) -> bool {
        self.tabs
            .iter()
            .any(|tab| tab.dirty && tab.path.starts_with(rel))
    }

    pub fn new(folders: Folders, cx: &mut Context<Self>) -> Self {
        Self {
            folders,
            tabs: Vec::new(),
            active: None,
            pending: Vec::new(),
            next_open: 0,
            restoring_active: None,
            last_lines: None,
            preview: None,
            preview_only: false,
            split: 0.5,
            focus_handle: cx.focus_handle(),
            error: None,
        }
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.active.and_then(|i| self.tabs.get(i))
    }

    /// The settings changed: open editors take them as new ones would.
    pub fn apply_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let settings = crate::settings(cx).clone();
        for tab in &self.tabs {
            let wrap = settings.wraps(tab.is_markdown());
            tab.editor.update(cx, |state, cx| {
                state.set_line_number(settings.line_numbers, window, cx);
                state.set_soft_wrap(wrap, window, cx);
                state.set_tab_size(tab_size(&settings), cx);
                state.set_show_whitespaces(settings.show_whitespace, window, cx);
                state.set_indent_guides(settings.indent_guides, window, cx);
                state.set_folding(settings.folding, window, cx);
            });
        }
        cx.notify();
    }

    /// The outline of a Markdown tab with two headings or more: each heading a station on the
    /// file's line, the section holding the cursor marked, the way to it drawn in the line's color.
    /// A click on a station puts the cursor on its heading.
    fn render_outline(&self, tab: &Tab, cx: &mut Context<Self>) -> Option<AnyElement> {
        const ROW: f32 = 30.;
        const COLUMN: f32 = 24.;
        let p = theme::colors(cx);
        let state = tab.editor.read(cx);
        let text = state.value();
        let headings = markdown::outline(&text);
        if headings.len() < 2 {
            return None;
        }
        let cursor = state.cursor_position().line;
        let lines = text.lines().count().max(1) as u32;
        let current = markdown::section_at(&headings, cursor);
        let line_color = c(self.station_color(&tab.path, p));
        let last = headings.len() - 1;
        let reached = current.unwrap_or(0);
        let rows = headings.into_iter().enumerate().map(|(index, heading)| {
            let here = Some(index) == current;
            let passed = current.is_some_and(|at| index <= at);
            let size = match (here, heading.level) {
                (true, _) => 15.,
                (false, 1) => 14.,
                (false, 2) => 9.,
                (false, _) => 5.,
            };
            let station = div()
                .absolute()
                .left(z(COLUMN - size / 2., cx))
                .top(z((ROW - size) / 2., cx))
                .size(z(size, cx))
                .rounded_full()
                .map(|el| match (here, heading.level, passed) {
                    (true, ..) => el.bg(line_color).shadow(vec![BoxShadow {
                        color: line_color.opacity(0.22),
                        offset: point(px(0.), px(0.)),
                        blur_radius: px(0.),
                        spread_radius: z(4., cx),
                        inset: false,
                    }]),
                    // A top heading is a transfer station, as a folder is in the tree.
                    (false, 1, _) => el.bg(c(p.paper)).border(z(2.5, cx)).border_color(c(p.ink)),
                    (false, 2, true) => el.bg(line_color),
                    (false, 2, false) => el
                        .bg(c(p.paper))
                        .border(z(2., cx))
                        .border_color(c(p.ignored)),
                    (false, _, true) => el.bg(c(p.sub)),
                    (false, _, false) => el.bg(c(p.ignored)),
                });
            let editor = tab.editor.clone();
            let line = heading.line;
            div()
                .id(("outline", index))
                .relative()
                .h(z(ROW, cx))
                .flex()
                .items_center()
                .gap(z(8., cx))
                .pl(z(COLUMN * 2. - 4., cx))
                .pr(z(10., cx))
                .whitespace_nowrap()
                .overflow_hidden()
                .text_size(z(
                    if heading.level <= 2 {
                        size::UI_TEXT
                    } else {
                        12.
                    },
                    cx,
                ))
                .text_color(c(match (here, heading.level, passed) {
                    (true, ..) => p.ink,
                    (false, 1, _) => p.ink,
                    (false, 2, _) => p.sub,
                    (false, _, true) => p.sub,
                    (false, _, false) => p.ignored,
                }))
                .when(here || heading.level == 1, |el| {
                    el.font_weight(FontWeight::SEMIBOLD)
                })
                .hover(|style| style.bg(c(p.hover)))
                .child(station)
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(heading.title),
                )
                .when(here, |el| {
                    el.child(
                        div()
                            .flex_none()
                            .text_size(z(11., cx))
                            .font_weight(FontWeight::NORMAL)
                            .text_color(c(p.sub))
                            .child("You are here"),
                    )
                })
                .when(index == last && !here, |el| {
                    el.child(
                        div()
                            .flex_none()
                            .text_size(z(11., cx))
                            .font_weight(FontWeight::NORMAL)
                            .text_color(c(p.ignored))
                            .child("Last stop"),
                    )
                })
                .on_click(cx.listener(move |_, _: &ClickEvent, window, cx| {
                    editor.update(cx, |state, cx| {
                        state.set_cursor_position(Position::new(line, 0), window, cx)
                    });
                    editor.read(cx).focus_handle(cx).focus(window, cx);
                }))
        });
        let rows: Vec<_> = rows.collect();
        Some(
            div()
                .w(z(230., cx))
                .flex_none()
                .h_full()
                .flex()
                .flex_col()
                .border_l_1()
                .border_color(c(p.line))
                .child(
                    div()
                        .h(z(34., cx))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px(z(14., cx))
                        .text_size(z(11., cx))
                        .text_color(c(p.sub))
                        .child(div().font_weight(FontWeight::SEMIBOLD).child("OUTLINE"))
                        .child(format!("{}%", (cursor + 1).min(lines) * 100 / lines)),
                )
                .child(
                    div()
                        .id("outline-stations")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .child(
                            div()
                                .relative()
                                // The line from the first station to the last: the way to the
                                // section in the file's color, the rest still to go.
                                .child(
                                    div()
                                        .absolute()
                                        .left(z(COLUMN - 1.5, cx))
                                        .top(z(ROW / 2., cx))
                                        .w(z(3., cx))
                                        .h(z(ROW * last as f32, cx))
                                        .rounded(z(1.5, cx))
                                        .bg(c(p.guide)),
                                )
                                .child(
                                    div()
                                        .absolute()
                                        .left(z(COLUMN - 1.5, cx))
                                        .top(z(ROW / 2., cx))
                                        .w(z(3., cx))
                                        .h(z(ROW * reached as f32, cx))
                                        .rounded(z(1.5, cx))
                                        .bg(line_color),
                                )
                                .children(rows),
                        ),
                )
                .into_any_element(),
        )
    }

    /// The color of the route line that reaches `path` in the tree, so a file's tab and its
    /// station agree. The open folder is depth 0; with several folders, each folder's row is.
    fn station_color(&self, path: &Path, p: &Palette) -> u32 {
        let depth = path.components().count() - usize::from(self.folders.several());
        p.route[depth.saturating_sub(1) % p.route.len()]
    }

    /// The active file's place under the tab bar: its folders, a diagonal between each as on a
    /// metro map, then the file at its station.
    fn render_route_path(&self, tab: &Tab, cx: &App) -> impl IntoElement + use<> {
        let p = theme::colors(cx);
        let parts: Vec<String> = tab
            .path
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect();
        let (file, folders) = parts
            .split_last()
            .map_or(("", &[][..]), |(f, d)| (f.as_str(), d));
        let slash = |cx: &App| {
            let color = c(p.num);
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let mut path = PathBuilder::stroke(bounds.size.width / 5.);
                    path.move_to(point(bounds.right(), bounds.top()));
                    path.line_to(point(bounds.left(), bounds.bottom()));
                    if let Ok(path) = path.build() {
                        window.paint_path(path, color);
                    }
                },
            )
            .flex_none()
            .w(z(6., cx))
            .h(z(11., cx))
        };
        div()
            .h(z(30., cx))
            .flex_none()
            .flex()
            .items_center()
            .gap(z(8., cx))
            .px(z(18., cx))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_size(z(12., cx))
            .text_color(c(p.sub))
            .children(folders.iter().flat_map(|name| {
                [
                    div().flex_none().child(name.clone()).into_any_element(),
                    slash(cx).into_any_element(),
                ]
            }))
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(z(6., cx))
                    .text_color(c(p.ink))
                    .font_weight(FontWeight::MEDIUM)
                    .child(
                        div()
                            .flex_none()
                            .size(z(6., cx))
                            .rounded_full()
                            .bg(c(self.station_color(&tab.path, p))),
                    )
                    .child(file.to_string()),
            )
    }

    /// How many tabs have edits that are not on disk.
    pub fn tabs_unsaved(&self) -> usize {
        self.tabs.iter().filter(|t| t.dirty).count()
    }

    /// The cursor or selection lines of the active tab, counted from 1.
    pub fn active_lines(&self, cx: &App) -> Option<(u32, u32)> {
        let tab = self.active_tab()?;
        let state = tab.editor.read(cx);
        let range = state.selected_range();
        let start = state.text().offset_to_position(range.start).line + 1;
        let mut end = state.text().offset_to_position(range.end).line + 1;
        // A selection that ends at the start of a line does not include that line.
        if end > start && state.text().offset_to_position(range.end).character == 0 {
            end -= 1;
        }
        Some((start, end))
    }

    /// Opens `path` (relative to the open folder), or switches to it when it is already open.
    /// The file is read off the main thread; the tab appears when it has been read.
    pub fn open(
        &mut self,
        path: &Path,
        pass_through: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(index) = self.tabs.iter().position(|t| t.path == path) {
            if !pass_through {
                self.tabs[index].pass_through = false;
            }
            self.activate(index, window, cx);
            return;
        }
        if let Some(pending) = self.pending.iter_mut().find(|p| p.path == path) {
            // Already being read; Enter on a file still loading from Space makes it a normal tab.
            pending.pass_through &= pass_through;
            return;
        }
        if pass_through {
            // Walking the tree with Space, only the newest file is worth showing.
            self.pending.retain(|p| !p.pass_through);
        }
        let id = self.next_open;
        self.next_open += 1;
        self.pending.push(PendingOpen {
            id,
            path: path.to_path_buf(),
            pass_through,
            cursor: None,
        });
        let abs = self.abs(path);
        let read = cx
            .background_executor()
            .spawn(async move { fs::read_text(&abs) });
        cx.spawn_in(window, async move |this, cx| {
            let file = read.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.finish_open(id, file, window, cx)
            });
        })
        .detach();
    }

    fn finish_open(
        &mut self,
        id: u64,
        file: Result<TextFile, ReadError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A newer pass-through may have replaced this request while the file was read.
        let Some(ix) = self.pending.iter().position(|p| p.id == id) else {
            return;
        };
        let PendingOpen {
            path,
            pass_through,
            cursor,
            ..
        } = self.pending.remove(ix);
        let file = match file {
            Ok(file) => file,
            // An image or a PDF goes to its own app, as from Finder; Space only says what it is,
            // so walking the tree never brings another app to the front.
            Err(ReadError::Binary) if !pass_through => {
                crate::workspace::open_with_system(&self.abs(&path), cx);
                self.error = Some(format!("{}: opened in the default app", path.display()));
                cx.emit(TabsChanged);
                cx.notify();
                return;
            }
            Err(err) => {
                self.error = Some(format!("{}: {err}", path.display()));
                cx.emit(TabsChanged);
                cx.notify();
                return;
            }
        };
        self.error = None;
        let tab = self.new_tab(path, pass_through, file, window, cx);
        if let Some((line, column)) = cursor {
            tab.editor.update(cx, |state, cx| {
                state.set_cursor_position(Position::new(line, column), window, cx)
            });
        }
        // A new pass-through replaces the previous one in place.
        let index = match self.tabs.iter().position(|t| t.pass_through) {
            Some(i) if pass_through => {
                self.tabs[i] = tab;
                i
            }
            _ => {
                self.tabs.push(tab);
                self.tabs.len() - 1
            }
        };
        match &self.restoring_active {
            Some(active) if *active != self.tabs[index].path && self.active.is_some() => {
                cx.emit(TabsChanged);
                cx.notify();
            }
            _ => {
                if self.restoring_active.as_ref() == Some(&self.tabs[index].path) {
                    self.restoring_active = None;
                }
                self.activate(index, window, cx);
            }
        }
    }

    fn new_tab(
        &mut self,
        path: PathBuf,
        pass_through: bool,
        file: TextFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Tab {
        let language = tab_language(&path, file.stamp.len);
        let markdown = language == "markdown";
        let editor = cx.new(|cx| {
            let settings = crate::settings(cx).clone();
            EditorState::new(window, cx)
                .language(language.clone())
                .line_number(settings.line_numbers)
                .soft_wrap(settings.wraps(markdown))
                .tab_size(tab_size(&settings))
                .show_whitespaces(settings.show_whitespace)
                .indent_guides(settings.indent_guides)
                .folding(settings.folding)
                .default_value(file.text)
        });
        let subscriptions = vec![
            cx.subscribe(&editor, |this, editor, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.edited(&editor, cx);
                }
            }),
            // The editor notifies on every caret blink; pass on only a move to another line.
            cx.observe(&editor, |this, editor, cx| {
                if this.active_tab().is_some_and(|t| t.editor == editor) {
                    let lines = this.active_lines(cx);
                    if lines != this.last_lines {
                        this.last_lines = lines;
                        cx.emit(TabsChanged);
                        // The outline's station follows the cursor's line.
                        cx.notify();
                    }
                }
            }),
        ];
        Tab {
            path,
            editor,
            pass_through,
            dirty: false,
            read_only: file.mixed_line_endings,
            line_ending: file.line_ending,
            bom: file.bom,
            stamp: file.stamp,
            language,
            deleted: false,
            conflict: None,
            edits: 0,
            saving: false,
            _subscriptions: subscriptions,
        }
    }

    fn edited(&mut self, editor: &Entity<EditorState>, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.iter_mut().find(|t| &t.editor == editor) {
            tab.dirty = true;
            tab.pass_through = false;
            tab.edits += 1;
        }
        if let Some(preview) = self.preview.clone()
            && self.preview_source().as_ref() == Some(editor)
        {
            preview.update(cx, |preview, cx| preview.schedule(cx));
        }
        cx.emit(TabsChanged);
        cx.notify();
    }

    fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        // Focus follows the switch only when it was already in the editor, so Space in the tree keeps it there.
        let had_focus = self.focus_handle.contains_focused(window, cx);
        self.active = Some(index);
        if had_focus {
            self.focus_active(window, cx);
        }
        self.refresh_preview(cx);
        cx.emit(TabsChanged);
        cx.notify();
    }

    pub fn focus_active(&self, window: &mut Window, cx: &mut App) {
        self.focus_handle(cx).focus(window, cx);
    }

    pub fn activate_offset(&mut self, offset: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(active) = self.active else { return };
        let len = self.tabs.len() as isize;
        let next = (active as isize + offset).rem_euclid(len) as usize;
        self.activate(next, window, cx);
        self.focus_active(window, cx);
    }

    /// Saves the active tab off the main thread. Failures go to [`Self::error`].
    pub fn save_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.active_tab() {
            let id = tab.editor.entity_id();
            self.save(id, false, false, window, cx);
        }
    }

    /// Writes the tab to disk. Unless `overwrite`, asks first when the file changed on disk since it
    /// was read or written; `then_close` closes the tab once the file is written.
    fn save(
        &mut self,
        id: EntityId,
        overwrite: bool,
        then_close: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.index_of(id) else {
            return;
        };
        let tab = &mut self.tabs[index];
        if tab.read_only {
            self.error = Some(format!(
                "{}: mixed line endings, read-only",
                tab.path.display()
            ));
            cx.emit(TabsChanged);
            cx.notify();
            return;
        }
        // One write per tab at a time, so an older text never lands after a newer one.
        if tab.saving {
            return;
        }
        tab.saving = true;
        let edits = tab.edits;
        // The rope is shared, not copied; it becomes a string on the background executor.
        let text = tab.editor.read(cx).text().clone();
        // The field, not `abs`: `tab` borrows the tabs.
        let path = self
            .folders
            .absolute(&tab.path)
            .unwrap_or_else(|| tab.path.clone());
        // A deleted file is written again without asking.
        let expected = (!overwrite && !tab.deleted).then_some(tab.stamp);
        let (line_ending, bom) = (tab.line_ending, tab.bom);
        let task = cx
            .background_executor()
            .spawn(async move { write_file(&path, &text.to_string(), expected, line_ending, bom) });
        cx.spawn_in(window, async move |this, cx| {
            let saved = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.finish_save(id, edits, then_close, saved, window, cx)
            });
        })
        .detach();
    }

    fn finish_save(
        &mut self,
        id: EntityId,
        edits: u64,
        then_close: bool,
        saved: Saved,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.index_of(id) else {
            return;
        };
        let tab = &mut self.tabs[index];
        tab.saving = false;
        match saved {
            Saved::Written(stamp) => {
                tab.stamp = stamp;
                tab.deleted = false;
                tab.conflict = None;
                tab.pass_through = false;
                // Typing while the file was written leaves edits the file does not have yet.
                if tab.edits == edits {
                    tab.dirty = false;
                }
                self.error = None;
                if then_close {
                    self.remove_tab(index, window, cx);
                }
            }
            Saved::ChangedOnDisk => self.confirm_overwrite(id, then_close, window, cx),
            Saved::Failed(err) => {
                self.error = Some(format!("{}: {err}", tab.path.display()));
            }
        }
        cx.emit(TabsChanged);
        cx.notify();
    }

    fn confirm_overwrite(
        &mut self,
        id: EntityId,
        then_close: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.index_of(id) else {
            return;
        };
        let name = file_name(&self.tabs[index].path);
        let this = cx.entity().downgrade();
        // Each choice closes the dialog itself first, so focus is back in the editor before the tab changes.
        let overwrite = move |window: &mut Window, cx: &mut App| {
            window.close_dialog(cx);
            let _ = this.update(cx, |tabs, cx| tabs.save(id, true, then_close, window, cx));
        };
        window.open_alert_dialog(cx, move |alert, _, cx| {
            let ok = overwrite.clone();
            let click = overwrite.clone();
            alert
                .title(format!("{name} changed on disk"))
                .description("Saving replaces the version on disk with yours.")
                .entrance(DialogEntrance::None)
                .on_ok(move |_, window, cx| {
                    ok(window, cx);
                    false
                })
                .footer(
                    DialogFooter::new()
                        .child(
                            subwy_button("cancel", "Cancel", cx)
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            subwy_button("overwrite", "Overwrite", cx)
                                .on_click(move |_, window, cx| click(window, cx)),
                        ),
                )
        });
    }

    /// The tree renamed `from` (a file or a folder) to `to`; tabs inside follow the new path.
    pub fn renamed(&mut self, from: &Path, to: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let moved = |path: &mut PathBuf| {
            if let Ok(rest) = path.strip_prefix(from) {
                // `join("")` would leave a trailing slash, and the disk would then take the file
                // for a folder ("Not a directory").
                *path = if rest.as_os_str().is_empty() {
                    to.to_path_buf()
                } else {
                    to.join(rest)
                };
            }
        };
        self.tabs.iter_mut().for_each(|tab| moved(&mut tab.path));
        self.pending
            .iter_mut()
            .for_each(|open| moved(&mut open.path));
        // A new extension is a new language: `notes` renamed `notes.md` is Markdown from now on.
        let settings = crate::settings(cx).clone();
        for tab in &mut self.tabs {
            let language = tab_language(&tab.path, tab.stamp.len);
            if language != tab.language {
                let wrap = settings.wraps(language == "markdown");
                tab.editor.update(cx, |state, cx| {
                    state.set_highlighter(language.clone(), cx);
                    state.set_soft_wrap(wrap, window, cx);
                });
                tab.language = language;
            }
        }
        self.refresh_preview(cx);
        cx.emit(TabsChanged);
        cx.notify();
    }

    /// The tree moved `path` to the Trash: clean tabs inside close; edited ones stay, marked deleted,
    /// so nothing typed is lost.
    pub fn trashed(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        for index in (0..self.tabs.len()).rev() {
            if !self.tabs[index].path.starts_with(path) {
                continue;
            }
            if self.tabs[index].dirty {
                self.tabs[index].deleted = true;
            } else {
                self.remove_tab(index, window, cx);
            }
        }
        cx.emit(TabsChanged);
        cx.notify();
    }

    /// Closes the active tab, asking first when it has unsaved edits.
    pub fn close_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.active {
            self.close_at(index, window, cx);
        }
    }

    /// Closes a tab, asking first when it has unsaved changes (the tab's x, Cmd+W).
    pub fn close_at(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        if tab.dirty {
            self.confirm_close(tab.editor.entity_id(), window, cx);
        } else {
            self.remove_tab(index, window, cx);
        }
    }

    /// Option-click on a tab's x, as in IntelliJ: closes every other tab. Tabs with unsaved changes
    /// stay open rather than asking about each one.
    pub fn close_others(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(keep) = self.tabs.get(index).map(|t| t.editor.entity_id()) else {
            return;
        };
        let doomed: Vec<EntityId> = self
            .tabs
            .iter()
            .filter(|t| !t.dirty && t.editor.entity_id() != keep)
            .map(|t| t.editor.entity_id())
            .collect();
        for id in doomed {
            if let Some(index) = self.index_of(id) {
                self.remove_tab(index, window, cx);
            }
        }
        if let Some(index) = self.index_of(keep) {
            self.activate(index, window, cx);
        }
    }

    fn confirm_close(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.index_of(id) else {
            return;
        };
        let name = file_name(&self.tabs[index].path);
        let this = cx.entity().downgrade();
        let save = {
            let this = this.clone();
            move |window: &mut Window, cx: &mut App| {
                window.close_dialog(cx);
                let _ = this.update(cx, |tabs, cx| tabs.save(id, false, true, window, cx));
            }
        };
        let discard = move |window: &mut Window, cx: &mut App| {
            window.close_dialog(cx);
            let _ = this.update(cx, |tabs, cx| {
                if let Some(index) = tabs.index_of(id) {
                    tabs.remove_tab(index, window, cx);
                }
            });
        };
        window.open_alert_dialog(cx, move |alert, _, cx| {
            let ok = save.clone();
            let click = save.clone();
            let discard = discard.clone();
            alert
                .title(format!("Save changes to {name}?"))
                .description("Your changes are lost if you don't save them.")
                .entrance(DialogEntrance::None)
                .on_ok(move |_, window, cx| {
                    ok(window, cx);
                    false
                })
                .footer(
                    DialogFooter::new()
                        .child(
                            subwy_button("dont-save", "Don't save", cx)
                                .on_click(move |_, window, cx| discard(window, cx)),
                        )
                        .child(
                            subwy_button("cancel", "Cancel", cx)
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            subwy_button("save", "Save", cx)
                                .on_click(move |_, window, cx| click(window, cx)),
                        ),
                )
        });
    }

    fn remove_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let had_focus = self.focus_handle.contains_focused(window, cx);
        self.tabs.remove(index);
        self.active = active_after_close(self.active, index, self.tabs.len());
        if had_focus {
            self.focus_active(window, cx);
        }
        self.refresh_preview(cx);
        cx.emit(TabsChanged);
        cx.notify();
    }

    /// Files changed on disk outside subwy (relative to the open folder, as the watcher reports them).
    /// A clean tab reloads, a tab with edits shows a banner, a deleted file marks its tab.
    pub fn files_changed(
        &mut self,
        files: &[PathBuf],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A folder that was renamed or removed counts for every file under it.
        let checks: Vec<_> = self
            .tabs
            .iter()
            .filter(|t| files.iter().any(|f| t.path.starts_with(f)))
            .map(|t| (t.editor.entity_id(), self.abs(&t.path), t.stamp, t.dirty))
            .collect();
        if checks.is_empty() {
            return;
        }
        let task = cx.background_executor().spawn(async move {
            checks
                .into_iter()
                .map(|(id, path, stamp, dirty)| (id, check_disk(&path, stamp, dirty)))
                .collect::<Vec<_>>()
        });
        cx.spawn_in(window, async move |this, cx| {
            let states = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                for (id, state) in states {
                    this.apply_disk_state(id, state, window, cx);
                }
                cx.emit(TabsChanged);
                cx.notify();
            });
        })
        .detach();
    }

    fn index_of(&self, id: EntityId) -> Option<usize> {
        self.tabs.iter().position(|t| t.editor.entity_id() == id)
    }

    fn apply_disk_state(
        &mut self,
        id: EntityId,
        state: DiskState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.index_of(id) else {
            return;
        };
        let tab = &mut self.tabs[index];
        // The tab may have saved or reloaded while the disk was checked.
        let seen = match &state {
            DiskState::Changed(stamp) => Some(*stamp),
            DiskState::Reread(Ok(file)) => Some(file.stamp),
            _ => None,
        };
        if seen == Some(tab.stamp) {
            tab.deleted = false;
            return;
        }
        match state {
            DiskState::Same => tab.deleted = false,
            DiskState::Gone => {
                tab.deleted = true;
                tab.conflict = None;
            }
            DiskState::Changed(stamp) if tab.dirty => {
                tab.deleted = false;
                tab.conflict = Some(stamp);
            }
            DiskState::Changed(_) => self.reload_from_disk(id, window, cx),
            DiskState::Reread(Ok(file)) if tab.dirty => {
                tab.deleted = false;
                tab.conflict = Some(file.stamp);
            }
            DiskState::Reread(Ok(file)) => self.reload(index, file, window, cx),
            DiskState::Reread(Err(err)) => {
                self.error = Some(format!("{}: {err}", tab.path.display()));
            }
        }
    }

    /// Replaces the tab's text with `file`, keeping the cursor where it was (on the last line if the file got shorter).
    fn reload(
        &mut self,
        index: usize,
        file: TextFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab = &mut self.tabs[index];
        tab.editor.update(cx, |state, cx| {
            let mut cursor = state.cursor_position();
            state.set_value(file.text, window, cx);
            let last_line = state.text().lines_len().saturating_sub(1);
            cursor.line = cursor.line.min(last_line as u32);
            let offset = state.text().position_to_offset(&cursor);
            state.set_selected_range(offset..offset, cx);
        });
        tab.stamp = file.stamp;
        tab.line_ending = file.line_ending;
        tab.bom = file.bom;
        tab.read_only = file.mixed_line_endings;
        tab.dirty = false;
        tab.deleted = false;
        tab.conflict = None;
        if self.active == Some(index) {
            self.refresh_preview(cx);
        }
    }

    fn reload_from_disk(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.index_of(id) else {
            return;
        };
        let path = self.abs(&self.tabs[index].path);
        let read = cx
            .background_executor()
            .spawn(async move { fs::read_text(&path) });
        cx.spawn_in(window, async move |this, cx| {
            let file = read.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let Some(index) = this.index_of(id) else {
                    return;
                };
                match file {
                    Ok(file) => this.reload(index, file, window, cx),
                    Err(ReadError::Io(err)) if err.kind() == io::ErrorKind::NotFound => {
                        this.tabs[index].deleted = true;
                        this.tabs[index].conflict = None;
                    }
                    Err(err) => {
                        this.error = Some(format!("{}: {err}", this.tabs[index].path.display()));
                    }
                }
                cx.emit(TabsChanged);
                cx.notify();
            });
        })
        .detach();
    }

    /// Keeps the edits over a file changed on disk; saving then writes them without asking again.
    fn keep_mine(&mut self, id: EntityId, cx: &mut Context<Self>) {
        let Some(index) = self.index_of(id) else {
            return;
        };
        let tab = &mut self.tabs[index];
        if let Some(stamp) = tab.conflict.take() {
            tab.stamp = stamp;
        }
        cx.notify();
    }

    fn render_banner(&self, id: EntityId, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = theme::colors(cx);
        div()
            .flex_none()
            .h(z(size::TAB_BAR, cx))
            .flex()
            .items_center()
            .justify_between()
            .px(z(16., cx))
            .bg(c(p.side))
            .border_b_1()
            .border_color(c(p.line))
            .text_size(z(12.5, cx))
            .text_color(c(p.ink))
            .child("Changed on disk")
            .child(
                div()
                    .flex()
                    .gap(z(8., cx))
                    .child(subwy_button("reload", "Reload", cx).on_click(cx.listener(
                        move |this, _: &ClickEvent, window, cx| {
                            this.reload_from_disk(id, window, cx)
                        },
                    )))
                    .child(subwy_button("keep-mine", "Keep mine", cx).on_click(
                        cx.listener(move |this, _: &ClickEvent, _, cx| this.keep_mine(id, cx)),
                    )),
            )
    }

    /// Opens `path` with the cursor on `line` (zero-based), as a normal tab; for search results.
    pub fn open_at(&mut self, path: &Path, line: u32, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.tabs.iter().position(|t| t.path == path) {
            self.tabs[index].pass_through = false;
            let editor = self.tabs[index].editor.clone();
            editor.update(cx, |state, cx| {
                state.set_cursor_position(Position::new(line, 0), window, cx)
            });
            self.activate(index, window, cx);
            return;
        }
        self.open(path, false, window, cx);
        if let Some(pending) = self.pending.iter_mut().find(|p| p.path == path) {
            pending.cursor = Some((line, 0));
        }
    }

    /// Reopens a saved session's tabs, each at its cursor, with `active` as the active tab.
    pub fn restore(
        &mut self,
        tabs: &[TabState],
        active: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.restoring_active = active.and_then(|i| tabs.get(i)).map(|t| t.path.clone());
        for tab in tabs {
            self.open(&tab.path, false, window, cx);
            if let Some(pending) = self.pending.iter_mut().find(|p| p.path == tab.path) {
                pending.cursor = Some((tab.line, tab.column));
            }
        }
    }

    /// The tabs as a session keeps them, and which one is active.
    pub fn session_tabs(&self, cx: &App) -> (Vec<TabState>, Option<usize>) {
        let tabs = self
            .tabs
            .iter()
            .map(|tab| {
                let cursor = tab.editor.read(cx).cursor_position();
                TabState {
                    path: tab.path.clone(),
                    line: cursor.line,
                    column: cursor.character,
                }
            })
            .collect();
        (tabs, self.active)
    }

    pub fn preview_open(&self) -> bool {
        self.preview.is_some()
    }

    /// Cmd+Shift+V: the preview beside the editor, or no preview.
    pub fn toggle_preview(&mut self, cx: &mut Context<Self>) {
        self.preview_only = false;
        if self.preview.take().is_none() {
            self.preview = Some(cx.new(Preview::new));
            self.refresh_preview(cx);
        }
        cx.notify();
    }

    pub fn view(&self) -> View {
        match (&self.preview, self.preview_only) {
            (None, _) => View::Editor,
            (Some(_), false) => View::Split,
            (Some(_), true) => View::Preview,
        }
    }

    /// The view buttons: editor only, editor and preview, preview only.
    pub fn set_view(&mut self, view: View, window: &mut Window, cx: &mut Context<Self>) {
        let had_focus = self.focus_handle.contains_focused(window, cx);
        if view == View::Editor {
            self.preview = None;
        } else if self.preview.is_none() {
            self.preview = Some(cx.new(Preview::new));
            self.refresh_preview(cx);
        }
        self.preview_only = view == View::Preview;
        // The keyboard follows: to the editor when it shows again, off it when it hides.
        if had_focus {
            self.focus_active(window, cx);
        }
        cx.notify();
    }

    /// Whether the editor and the preview show, and the editor's share beside the preview; for the
    /// UI checks.
    pub fn panes(&self) -> (bool, bool, f32) {
        (!self.editor_hidden(), self.preview.is_some(), self.split)
    }

    /// Only the preview shows: Preview only, on a Markdown tab (other files always show the editor).
    fn editor_hidden(&self) -> bool {
        self.preview_only
            && self.preview.is_some()
            && self.active_tab().is_some_and(|t| t.is_markdown())
    }

    /// The editor the preview shows: the active tab, when it is Markdown.
    fn preview_source(&self) -> Option<Entity<EditorState>> {
        self.active_tab()
            .filter(|t| t.is_markdown())
            .map(|t| t.editor.clone())
    }

    /// Shows the active tab in the preview at once (switching tabs is not typing).
    fn refresh_preview(&mut self, cx: &mut Context<Self>) {
        let Some(preview) = self.preview.clone() else {
            return;
        };
        let source = self.preview_source();
        // Relative image paths in the Markdown resolve against the file's own folder.
        let folder = self
            .active_tab()
            .and_then(|t| self.abs(&t.path).parent().map(Path::to_path_buf));
        preview.update(cx, |preview, cx| preview.show(source, folder, cx));
    }

    /// The active tab's editor when it is Markdown, has the keyboard and takes edits. Anything
    /// else (the find bar, a read-only file, an IME composition) keeps gpui-kit's own keys.
    fn markdown_editor(&self, window: &mut Window, cx: &mut App) -> Option<Entity<EditorState>> {
        let tab = self
            .active_tab()
            .filter(|t| t.is_markdown() && !t.read_only)?;
        let editor = tab.editor.clone();
        if !editor.focus_handle(cx).is_focused(window) {
            return None;
        }
        let composing = editor.update(cx, |state, cx| {
            state.marked_text_range(window, cx).is_some()
        });
        (!composing).then_some(editor)
    }

    /// Runs a Markdown editing key. `edit` gets the text and the selection and says what to
    /// change, relative to an offset. With nothing to change the key propagates, and GPUI goes on
    /// to gpui-kit's binding for the same key (its newline, its tab).
    fn edit_markdown(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&Rope, Range<usize>) -> Option<(usize, Edit)>,
    ) {
        let applied = self.markdown_editor(window, cx).is_some_and(|editor| {
            editor.update(cx, |state, cx| {
                let Some((base, edit)) = edit(state.text(), state.selected_range()) else {
                    return false;
                };
                // A single replace, so one undo takes it back.
                state.set_selected_range(base + edit.range.start..base + edit.range.end, cx);
                state.replace(edit.text, window, cx);
                let selection = edit.selection;
                state.set_selected_range(base + selection.start..base + selection.end, cx);
                true
            })
        });
        if !applied {
            cx.propagate();
        }
    }

    fn delete_indent(&mut self, _: &DeleteIndent, window: &mut Window, cx: &mut Context<Self>) {
        let tab = crate::settings(cx).tab_size;
        self.edit_markdown(window, cx, |text, selection| {
            if !selection.is_empty() {
                return None;
            }
            let (row, start, line) = line_at(text, selection.start);
            markdown::unindent(&line, selection.start - start, &list_unit(text, row), tab)
                .map(|edit| (start, edit))
        });
    }

    /// Down on the last line goes to the end of the text and Up on the first to its start, as in
    /// macOS text views; gpui-kit's editor stays put there. Only the editor knows its rows (soft
    /// wrap, folds), so its own move runs first and a caret it left in place goes on.
    fn past_edge(&mut self, to_end: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.active_tab().map(|tab| tab.editor.clone()) else {
            return;
        };
        let state = editor.read(cx);
        if !state.focus_handle(cx).is_focused(window) || !state.selected_range().is_empty() {
            return;
        }
        let before = state.cursor();
        cx.defer_in(window, move |_, window, cx| {
            editor.update(cx, |state, cx| {
                if state.cursor() == before && state.selected_range().is_empty() {
                    let offset = if to_end { state.text().len() } else { 0 };
                    let position = state.text().offset_to_position(offset);
                    state.set_cursor_position(position, window, cx);
                }
            });
        });
    }

    fn newline(&mut self, _: &Newline, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_markdown(window, cx, |text, selection| {
            if !selection.is_empty() {
                return None;
            }
            let (_, start, line) = line_at(text, selection.start);
            markdown::enter(&line, selection.start - start).map(|edit| (start, edit))
        });
    }

    fn indent_item(&mut self, _: &IndentItem, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_markdown(window, cx, |text, selection| {
            let (row, start, line) = single_line(text, &selection)?;
            let local = selection.start - start..selection.end - start;
            markdown::indent(&line, local, &list_unit(text, row)).map(|edit| (start, edit))
        });
    }

    fn outdent_item(&mut self, _: &OutdentItem, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_markdown(window, cx, |text, selection| {
            let (row, start, line) = single_line(text, &selection)?;
            let local = selection.start - start..selection.end - start;
            markdown::outdent(&line, local, &list_unit(text, row)).map(|edit| (start, edit))
        });
    }

    fn toggle_emphasis(&mut self, emphasis: Emphasis, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_markdown(window, cx, |text, selection| {
            let (_, start, _) = line_at(text, selection.start);
            let end = text.line_end_offset(text.offset_to_point(selection.end).row);
            let lines = text.slice(start..end).to_string();
            let local = selection.start - start..selection.end - start;
            Some((start, markdown::toggle_emphasis(&lines, local, emphasis)))
        });
    }

    fn render_tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = theme::colors(cx);
        let tabs: Vec<_> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(i, tab)| {
                let active = self.active == Some(i);
                let name = file_name(&tab.path);
                let station = self.station_color(&tab.path, p);
                let group = SharedString::from(format!("editor-tab-{i}"));
                div()
                    .id(("tab", i))
                    .group(group.clone())
                    .relative()
                    .flex_none()
                    .h_full()
                    .flex()
                    .items_center()
                    .gap(z(8., cx))
                    .px(z(12., cx))
                    .text_color(c(if active && !tab.deleted { p.ink } else { p.sub }))
                    .when(active, |el| el.font_weight(FontWeight::MEDIUM))
                    .when(!active, |el| el.hover(|style| style.text_color(c(p.ink))))
                    .when(tab.pass_through, |el| el.italic())
                    .child(div().when(tab.deleted, |el| el.line_through()).child(name))
                    .when(tab.dirty, |el| {
                        el.child(
                            div()
                                .flex_none()
                                .size(z(6., cx))
                                .rounded_full()
                                .bg(c(p.sub)),
                        )
                    })
                    // IntelliJ's x: on the tab in front, on the others when hovered. Option-click closes
                    // the other tabs.
                    .child(
                        div()
                            .id(("tab-close", i))
                            .flex_none()
                            .size(z(16., cx))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(z(4., cx))
                            .text_color(c(p.sub))
                            .when(!active, |el| {
                                el.invisible().group_hover(group, |style| style.visible())
                            })
                            .hover(|style| style.bg(c(p.guide)).text_color(c(p.ink)))
                            .child(Icon::new(IconName::X).size(z(12., cx)))
                            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                                cx.stop_propagation();
                                if event.modifiers().alt {
                                    this.close_others(i, window, cx);
                                } else {
                                    this.close_at(i, window, cx);
                                }
                            })),
                    )
                    // Drawn over the tab rather than as a border, so the active tab is as tall as the
                    // rest. It takes the color of the route line that reaches the file in the tree.
                    .when(active, |el| {
                        el.child(
                            div()
                                .absolute()
                                .left(z(12., cx))
                                .right(z(12., cx))
                                .bottom_0()
                                .h(z(2., cx))
                                .rounded(z(1., cx))
                                .bg(c(station)),
                        )
                    })
                    .when(tab.pass_through, |el| {
                        el.child(folded_corner(z(9., cx), c(p.fold)))
                    })
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        if i < this.tabs.len() {
                            this.activate(i, window, cx);
                            this.focus_active(window, cx);
                        }
                    }))
            })
            .collect();
        let markdown = self.active_tab().is_some_and(|t| t.is_markdown());
        let current = self.view();
        div()
            .h(z(size::TAB_BAR, cx))
            .flex_none()
            .flex()
            .bg(c(p.paper))
            .border_b_1()
            .border_color(c(p.line))
            .text_size(z(12.5, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .overflow_x_hidden()
                    .children(tabs),
            )
            .when(markdown, |el| {
                el.child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(z(2., cx))
                        .px(z(8., cx))
                        .children(
                            [
                                (View::Editor, IconName::Code, "Editor Only"),
                                (View::Split, IconName::SquareCode, "Editor and Preview"),
                                (View::Preview, IconName::Eye, "Preview Only"),
                            ]
                            .map(|(view, icon, label)| {
                                view_button(view, icon, label, view == current, cx).on_click(
                                    cx.listener(move |this, _: &ClickEvent, window, cx| {
                                        this.set_view(view, window, cx)
                                    }),
                                )
                            }),
                        ),
                )
            })
    }
}

/// One of the Markdown view buttons; the one in use is filled.
fn view_button(
    view: View,
    icon: IconName,
    label: &'static str,
    selected: bool,
    cx: &App,
) -> Stateful<Div> {
    let p = theme::colors(cx);
    div()
        .id(("markdown-view", view as usize))
        .size(z(24., cx))
        .flex()
        .items_center()
        .justify_center()
        .rounded(z(4., cx))
        .text_color(c(if selected { p.accent } else { p.sub }))
        .when(selected, |el| el.bg(c(p.sel_bg)))
        .when(!selected, |el| {
            el.hover(|style| style.bg(c(p.hover)).text_color(c(p.ink)))
        })
        .tooltip(move |window, cx| Tooltip::new(label).build(window, cx))
        .child(Icon::new(icon).size(z(15., cx)))
}

/// The line holding `offset`: its row, where it starts, and its text.
fn line_at(text: &Rope, offset: usize) -> (usize, usize, String) {
    let row = text.offset_to_point(offset).row;
    (
        row,
        text.line_start_offset(row),
        text.slice_line(row).to_string(),
    )
}

/// [`line_at`] for a selection on one line; a selection over several lines is gpui-kit's to indent.
fn single_line(text: &Rope, selection: &Range<usize>) -> Option<(usize, usize, String)> {
    let line = line_at(text, selection.start);
    (text.offset_to_point(selection.end).row == line.0).then_some(line)
}

/// The indent unit of the list around `row`, read from its lines up to a blank line each way.
fn list_unit(text: &Rope, row: usize) -> String {
    let line = |r: usize| text.slice_line(r).to_string();
    let filled = |l: &String| !l.trim().is_empty();
    let mut lines: Vec<String> = (row.saturating_sub(LIST_LOOKAROUND)..row)
        .rev()
        .map(line)
        .take_while(filled)
        .collect();
    lines.reverse();
    let end = text.lines_len().min(row + LIST_LOOKAROUND);
    lines.extend((row..end).map(line).take_while(filled));
    markdown::indent_unit(lines.iter().map(String::as_str))
}

/// subwy's one button look (the design's Open folder button): `side` fill, `line` border, radius 6.
fn subwy_button(id: &'static str, label: &'static str, cx: &App) -> Button {
    let p = theme::colors(cx);
    Button::new(id)
        .label(label)
        .small()
        .rounded(z(6., cx))
        .bg(c(p.side))
        .text_color(c(p.ink))
}

/// The pass-through mark: the tab's top right corner folded over, a right triangle of `size`.
fn folded_corner(size: Pixels, color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let (right, top) = (bounds.right(), bounds.top());
            let mut corner = gpui_kit::Path::new(point(right - size, top));
            corner.line_to(point(right, top));
            corner.line_to(point(right, top + size));
            window.paint_path(corner, color);
        },
    )
    .absolute()
    .top_0()
    .right_0()
    .size(size)
}

impl Render for EditorTabs {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = theme::colors(cx);
        // In a column under the banner, so it takes the height left rather than all of it.
        let body = div()
            .flex_1()
            .min_h_0()
            .capture_action(
                cx.listener(|this, _: &MoveDown, window, cx| this.past_edge(true, window, cx)),
            )
            .capture_action(
                cx.listener(|this, _: &MoveUp, window, cx| this.past_edge(false, window, cx)),
            );
        let body = match self.active_tab() {
            None => body.bg(c(p.paper)),
            Some(tab) => {
                let editor = Editor::new(&tab.editor)
                    .bordered(false)
                    .readonly(tab.read_only)
                    .h_full()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(z(theme::editor_text(cx), cx));
                if tab.is_markdown() {
                    // The outline beside the text, in the editor view only: the preview shows the
                    // text itself.
                    let outline = (crate::settings(cx).outline && self.preview.is_none())
                        .then(|| self.render_outline(tab, cx))
                        .flatten();
                    let editor = div()
                        .size_full()
                        .flex()
                        .child(div().flex_1().min_w_0().h_full().child(editor))
                        .children(outline);
                    // The full width, as code and as IntelliJ shows Markdown (the user asked for it
                    // over a centered reading-width page), with the Markdown editing keys.
                    body.key_context(MARKDOWN)
                        .on_action(cx.listener(Self::newline))
                        .on_action(cx.listener(Self::indent_item))
                        .on_action(cx.listener(Self::outdent_item))
                        .on_action(cx.listener(Self::delete_indent))
                        .on_action(cx.listener(|this, _: &ToggleBold, window, cx| {
                            this.toggle_emphasis(Emphasis::Bold, window, cx)
                        }))
                        .on_action(cx.listener(|this, _: &ToggleItalic, window, cx| {
                            this.toggle_emphasis(Emphasis::Italic, window, cx)
                        }))
                        .bg(c(p.paper))
                        .child(editor)
                } else {
                    body.bg(c(p.paper)).child(editor)
                }
            }
        };
        let banner = self
            .active_tab()
            .filter(|t| t.conflict.is_some())
            .map(|t| t.editor.entity_id())
            .map(|id| self.render_banner(id, cx));
        div()
            .track_focus(&self.focus_handle)
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(self.render_tab_bar(cx))
            .children(self.active_tab().map(|tab| self.render_route_path(tab, cx)))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .on_drag_move(
                        cx.listener(|this, event: &DragMoveEvent<SplitDrag>, _, cx| {
                            let width = event.bounds.size.width;
                            if width > px(0.) {
                                this.split = ((event.event.position.x - event.bounds.left())
                                    / width)
                                    .clamp(0.2, 0.8);
                                cx.notify();
                            }
                        }),
                    )
                    .when(!self.editor_hidden(), |el| {
                        el.child(
                            div()
                                .map(|el| match &self.preview {
                                    // Beside the preview the editor keeps its dragged share.
                                    Some(_) => el.flex_none().w(relative(self.split)),
                                    None => el.flex_1(),
                                })
                                .min_w_0()
                                .h_full()
                                .flex()
                                .flex_col()
                                .children(banner)
                                .child(body),
                        )
                        .when(self.preview.is_some(), |el| {
                            // The grip just left of the preview's divider line.
                            el.child(
                                div()
                                    .id("preview-split")
                                    .flex_none()
                                    .w(z(5., cx))
                                    .h_full()
                                    .cursor_col_resize()
                                    .on_drag(SplitDrag, |_, _, _, cx| cx.new(|_| EmptyView)),
                            )
                        })
                    })
                    .children(self.preview.clone()),
            )
    }
}

/// How a save ended.
#[derive(Debug)]
enum Saved {
    Written(FileStamp),
    /// The file is no longer what the tab read or last wrote; nothing was written.
    ChangedOnDisk,
    Failed(String),
}

/// Writes `text` to `path` (a symlink's target), unless the file's stamp is no longer `expected`.
/// Runs on the background executor.
fn write_file(
    path: &Path,
    text: &str,
    expected: Option<FileStamp>,
    line_ending: LineEnding,
    bom: bool,
) -> Saved {
    let target = fs::write_target(path);
    if let Some(expected) = expected
        && let Ok(stamp) = fs::stamp(&target)
        && stamp != expected
    {
        return Saved::ChangedOnDisk;
    }
    match fs::write_text(&target, text, line_ending, bom) {
        Ok(stamp) => Saved::Written(stamp),
        Err(err) => Saved::Failed(err.to_string()),
    }
}

/// What the disk says about an open tab's file.
#[derive(Debug)]
enum DiskState {
    /// As the tab last read or wrote it.
    Same,
    Gone,
    /// Changed under unsaved edits: the stamp on disk.
    Changed(FileStamp),
    /// Changed under a clean tab, and read again.
    Reread(Result<TextFile, ReadError>),
}

/// Checks `path` against the stamp its tab knows. Runs on the background executor.
fn check_disk(path: &Path, known: FileStamp, dirty: bool) -> DiskState {
    match fs::stamp(path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => DiskState::Gone,
        // Unreadable for now (permissions, a save in progress): the next event checks again.
        Err(_) => DiskState::Same,
        Ok(stamp) if stamp == known => DiskState::Same,
        Ok(stamp) if dirty => DiskState::Changed(stamp),
        Ok(_) => DiskState::Reread(fs::read_text(path)),
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: GPUI's glob export includes a `test` macro that would replace the built-in one.
    use super::{DiskState, Saved, active_after_close, check_disk, write_file};
    use subwy_project::fs::{self, LineEnding};

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("subwy-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn closing_a_tab_keeps_the_active_one_when_it_can() {
        assert_eq!(active_after_close(Some(2), 0, 3), Some(1));
        assert_eq!(active_after_close(Some(0), 2, 3), Some(0));
        assert_eq!(active_after_close(Some(1), 1, 2), Some(1));
        assert_eq!(active_after_close(Some(2), 2, 2), Some(1));
        assert_eq!(active_after_close(Some(0), 0, 0), None);
        assert_eq!(active_after_close(None, 0, 1), None);
    }

    #[test]
    fn write_file_asks_before_replacing_a_changed_file() {
        let dir = temp_dir("write-file");
        let path = dir.join("a.md");
        std::fs::write(&path, "one\n").unwrap();
        let read = fs::stamp(&path).unwrap();

        let written = match write_file(&path, "two\n", Some(read), LineEnding::Lf, false) {
            Saved::Written(stamp) => stamp,
            other => panic!("expected a write, got {other:?}"),
        };
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "two\n");

        std::fs::write(&path, "changed elsewhere\n").unwrap();
        assert!(matches!(
            write_file(&path, "three\n", Some(written), LineEnding::Lf, false),
            Saved::ChangedOnDisk
        ));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "changed elsewhere\n"
        );

        // Overwriting, or writing a deleted file again, does not check.
        assert!(matches!(
            write_file(&path, "three\n", None, LineEnding::Lf, false),
            Saved::Written(_)
        ));
        std::fs::remove_file(&path).unwrap();
        assert!(matches!(
            write_file(&path, "four\n", None, LineEnding::Lf, false),
            Saved::Written(_)
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "four\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn check_disk_follows_the_file() {
        let dir = temp_dir("check-disk");
        let path = dir.join("a.md");
        std::fs::write(&path, "one\n").unwrap();
        let known = fs::stamp(&path).unwrap();

        assert!(matches!(check_disk(&path, known, false), DiskState::Same));

        std::fs::write(&path, "one\ntwo\n").unwrap();
        assert!(matches!(
            check_disk(&path, known, true),
            DiskState::Changed(_)
        ));
        match check_disk(&path, known, false) {
            DiskState::Reread(Ok(file)) => assert_eq!(file.text, "one\ntwo\n"),
            other => panic!("expected a reread, got {other:?}"),
        }

        std::fs::remove_file(&path).unwrap();
        assert!(matches!(check_disk(&path, known, false), DiskState::Gone));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
