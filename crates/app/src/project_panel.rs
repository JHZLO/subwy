//! The folder tree on the left: the route to the selection, key hints, and IntelliJ moves.

use std::{
    collections::VecDeque,
    ops::Range,
    path::{Path, PathBuf},
    time::Instant,
};

use gpui_kit::{
    assets::IconName,
    component::{
        Icon,
        input::{Input, InputEvent, InputState},
        tooltip::Tooltip,
    },
    prelude::FluentBuilder as _,
    *,
};
use subwy_project::{
    folders::Folders,
    fs::{self, EntryKind, NameError},
    nav::{self, NavKey, SearchStep},
    paths,
    tree::{self, ProjectTree, Row},
};

use crate::{
    icons,
    keymap::{
        Collapse, CollapseAll, CopyFileName, CopyItems, CopyPath, CopyRelativePath, CopyTo,
        CutItems, Expand, ExpandAll, ExtendNext, ExtendPrev, FocusEditor, MoveTo, NewFile, Open,
        OpenInTerminal, PROJECT_PANEL, PageDown, PageUp, PassThrough, PasteItems, Rename,
        RevealInFinder, SelectFirst, SelectLast, SelectNext, SelectPrev, SpeedSearch, ToggleSearch,
        Trash, UndoChange,
    },
    route,
    theme::{self, c, size, z},
};

/// Row geometry at 100% zoom, from the design mockup: the chevron slot of a row at depth `d`
/// starts at `10 + 14d`, so a folder's chevron is centered on the guide its children draw.
const ROW_PADDING_LEFT: f32 = 10.;
const ROW_PADDING_RIGHT: f32 = 8.;
const ROW_GAP: f32 = 4.;
const CHEVRON_SLOT: f32 = 12.;
const CHEVRON_SIZE: f32 = 10.;
/// The file type icon after the chevron.
const ICON_SIZE: f32 = 14.;

/// Tree width limits at 100% zoom, and the strip at its right edge that drags it.
const MIN_WIDTH: f32 = 180.;
const MAX_WIDTH: f32 = 480.;

/// The rename input's inner padding at 100% zoom.
const EDIT_PADDING: f32 = 6.;
/// The name input sits inside a row, one point clear of its top and bottom.
const EDIT_HEIGHT: f32 = size::TREE_ROW - 2.;

/// Key hint line spacing at 100% zoom.
const HINT_PADDING: f32 = 8.;
const HINT_GAP: f32 = 8.;
const HINT_KEY_GAP: f32 = 4.;
const KEY_CAP_PADDING: f32 = 3.;

/// The mockup's chevron. Inline bytes keep the tree independent of a registered icon bundle.
const CHEVRON: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10" viewBox="0 0 10 10"><path d="M3.5 2 L7 5 L3.5 8" fill="none" stroke="black" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/></svg>"#;

/// Left padding of a row at `depth`.
fn row_padding(depth: usize) -> f32 {
    ROW_PADDING_LEFT + depth as f32 * size::TREE_INDENT
}

/// The x of the column at `level`, where the route runs: under the chevron center of that
/// level's folders.
fn column_x(level: usize) -> f32 {
    row_padding(level) + CHEVRON_SLOT / 2.
}

fn clamp_width(width: f32) -> f32 {
    width.clamp(MIN_WIDTH, MAX_WIDTH)
}

/// The rows that fit whole in a list `list_height` tall, at least one so a page always moves.
fn rows_per_page(list_height: f32, row_height: f32) -> usize {
    ((list_height / row_height).floor() as usize).max(1)
}

/// `path` with the home folder written as `~`, the way a shell prompt shows it.
fn tilde(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

pub enum PanelEvent {
    /// Open a file. `pass_through` keeps focus in the tree and reuses the pass-through tab.
    Open { path: PathBuf, pass_through: bool },
    /// The header's locate button: select the active tab's file (IntelliJ's Select Opened File).
    Locate,
    /// The header's minus: hide the tree, as Cmd+1 in it does.
    Hide,
    /// The more menu's Add Folder…: ask for a folder to show in this window too.
    AddFolder,
    /// A folder row's Remove from Window (relative path of the folder's row).
    RemoveFolder(PathBuf),
    /// Items went on the clipboard (Cmd+C, Cmd+X): show it on the stem as a path copy is shown.
    Copied { text: String, label: &'static str },
    /// Open in Terminal: a new shell in this folder (relative path).
    OpenTerminal(PathBuf),
}

/// How the clipboard marks the tree's own Cmd+C and Cmd+X, beside the paths as text.
const CLIPBOARD_COPY: &str = "subwy-items-copy";
const CLIPBOARD_CUT: &str = "subwy-items-cut";

/// What a Cmd+V in the tree would paste: files copied in Finder (to copy), or the tree's own
/// Cmd+C (to copy) or Cmd+X (to move), as absolute paths. Anything else on the clipboard is text.
fn clipboard_items(item: &ClipboardItem) -> Option<(Vec<PathBuf>, bool)> {
    let finder = item.entries().iter().find_map(|entry| match entry {
        ClipboardEntry::ExternalPaths(paths) => Some(paths.paths().to_vec()),
        _ => None,
    });
    if let Some(paths) = finder {
        return Some((paths, false));
    }
    let cut = match item.metadata()?.as_str() {
        CLIPBOARD_CUT => true,
        CLIPBOARD_COPY => false,
        _ => return None,
    };
    let paths = item.text()?.lines().map(PathBuf::from).collect();
    Some((paths, cut))
}

/// A small list floating over the tree.
enum Menu {
    /// Cmd+N or the header's plus, as IntelliJ's New popup: whether to make a file or a folder in
    /// `dir`. `choice` is the highlighted line (0 File, 1 Directory).
    New { dir: PathBuf, choice: usize },
    /// The header's more button.
    Options,
    /// Right-click on a row: what can be done to the item at `path`, in groups. `choice` is the
    /// highlighted line, counted across the groups; like a macOS menu, none until an arrow key.
    Context {
        path: PathBuf,
        entries: Vec<Vec<Entry>>,
        choice: Option<usize>,
    },
}

/// One line of a row's right-click menu (IntelliJ's project view menu, without the IDE parts).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Entry {
    NewFile,
    NewDirectory,
    Cut,
    Copy,
    Paste,
    CopyPath,
    CopyRelativePath,
    CopyFileName,
    FindInFolder,
    Rename,
    CopyTo,
    MoveTo,
    Trash,
    RevealInFinder,
    OpenInTerminal,
    OpenInDefaultApp,
    RemoveFromWindow,
}

impl Entry {
    fn label(self) -> &'static str {
        match self {
            Entry::NewFile => "New File",
            Entry::NewDirectory => "New Directory",
            Entry::Cut => "Cut",
            Entry::Copy => "Copy",
            Entry::Paste => "Paste",
            Entry::CopyPath => "Copy Path",
            Entry::CopyRelativePath => "Copy Relative Path",
            Entry::CopyFileName => "Copy File Name",
            Entry::FindInFolder => "Find in Folder…",
            Entry::Rename => "Rename…",
            Entry::CopyTo => "Copy To…",
            Entry::MoveTo => "Move To…",
            Entry::Trash => "Move to Trash",
            Entry::RevealInFinder => "Reveal in Finder",
            Entry::OpenInTerminal => "Open in Terminal",
            Entry::OpenInDefaultApp => "Open in Default App",
            Entry::RemoveFromWindow => "Remove from Window",
        }
    }

    /// The key that does the same, as the hint line writes keys.
    fn key(self) -> Option<&'static str> {
        match self {
            Entry::Cut => Some("⌘X"),
            Entry::Copy => Some("⌘C"),
            Entry::Paste => Some("⌘V"),
            Entry::CopyPath => Some("⌘⇧C"),
            Entry::CopyRelativePath => Some("⌥⌘⇧C"),
            Entry::Rename => Some("⇧F6"),
            Entry::FindInFolder => Some("⌘⇧F"),
            Entry::CopyTo => Some("F5"),
            Entry::MoveTo => Some("F6"),
            Entry::Trash => Some("⌘⌫"),
            Entry::RevealInFinder => Some("⌥⌘R"),
            _ => None,
        }
    }

    fn icon(self) -> Option<IconName> {
        match self {
            Entry::NewFile => Some(IconName::File),
            Entry::NewDirectory => Some(IconName::Folder),
            Entry::RemoveFromWindow => Some(IconName::FolderMinus),
            _ => None,
        }
    }
}

/// The right-click menu of an item, in groups. A folder (`folder`) can be searched. A folder of
/// the window (`own_folder`) is not cut, renamed or trashed; with several folders it can leave the
/// window instead.
fn context_entries(folder: bool, own_folder: bool, several: bool) -> Vec<Vec<Entry>> {
    let clipboard = if own_folder {
        vec![Entry::Copy, Entry::Paste]
    } else {
        vec![Entry::Cut, Entry::Copy, Entry::Paste]
    };
    let mut groups = vec![
        vec![Entry::NewFile, Entry::NewDirectory],
        clipboard,
        vec![
            Entry::CopyPath,
            Entry::CopyRelativePath,
            Entry::CopyFileName,
        ],
    ];
    if folder {
        groups.push(vec![Entry::FindInFolder]);
    }
    groups.push(if own_folder {
        vec![Entry::CopyTo]
    } else {
        vec![Entry::Rename, Entry::CopyTo, Entry::MoveTo, Entry::Trash]
    });
    let mut open = vec![Entry::RevealInFinder, Entry::OpenInTerminal];
    if !folder {
        open.push(Entry::OpenInDefaultApp);
    }
    groups.push(open);
    if own_folder && several {
        groups.push(vec![Entry::RemoveFromWindow]);
    }
    groups
}

/// A file change Cmd+Z can take back. Paths are on disk.
enum Undo {
    /// Items moved or renamed, as (from, to).
    Moved(Vec<(PathBuf, PathBuf)>),
    /// Items made by New, a copy or a paste.
    Made(Vec<PathBuf>),
}

/// How many changes Cmd+Z remembers.
const UNDO_MOST: usize = 50;

/// What Expand All reads at most, so a huge folder does not flood the tree.
const EXPAND_ALL_MOST: usize = 300;

/// Items the tree changed on disk, so open tabs can follow. Paths are relative to the open
/// folder; a folder's event covers everything inside it.
pub enum FileEvent {
    Renamed { from: PathBuf, to: PathBuf },
    Trashed { path: PathBuf },
}

pub struct ProjectPanel {
    tree: ProjectTree,
    /// Where the tree's relative paths are on disk.
    folders: Folders,
    focus_handle: FocusHandle,
    scroll: UniformListScrollHandle,
    pub show_key_hints: bool,
    /// The tree width at 100% zoom, dragged from its right edge. The session keeps it.
    pub width: f32,
    /// The speed search text while a search is on (IntelliJ: type in the tree to find a name).
    search: Option<String>,
    /// Ends the search and drops a name being typed when focus leaves the tree. Made on first
    /// use: `new` has no window.
    focus_out: Option<Subscription>,
    /// The selection has no row yet (its folder is being read); scroll to it when it arrives.
    reveal_pending: bool,
    /// A name being typed into the tree (Cmd+N, Shift+F6).
    edit: Option<Edit>,
    menu: Option<Menu>,
    /// The route to the selection, gliding when the selection moves.
    route: route::Motion,
    /// The row under the pointer: it gets a background and a dashed way to it off the route.
    hovered: Option<PathBuf>,
    /// Where a drag over the tree would drop, drawn as the hovered row is.
    drop_target: Option<PathBuf>,
    /// File changes made here, newest last, for Cmd+Z.
    undo: Vec<Undo>,
}

/// A name being typed into the tree, in an input on its own line.
struct Edit {
    target: EditTarget,
    input: Entity<InputState>,
    /// Why the last Enter was refused, shown after the input.
    error: Option<SharedString>,
    /// The disk work for the last Enter has not come back yet.
    busy: bool,
    /// What the input holds, for the line that shows where a new item will land.
    typed: String,
    _subscription: Subscription,
}

#[derive(Clone)]
enum EditTarget {
    /// A new file, or a folder, inside `dir`. A name ending in `/` is a folder either way.
    New { dir: PathBuf, kind: EntryKind },
    /// A new name for the item at `path`.
    Rename { path: PathBuf },
    /// F5 / F6: where `sources` are copied or, with `cut`, moved.
    Transfer { sources: Vec<PathBuf>, cut: bool },
}

impl EditTarget {
    fn hints(&self) -> &'static [(&'static str, &'static str)] {
        match self {
            EditTarget::New { .. } => &[("↩", "Create"), ("Esc", "Cancel")],
            EditTarget::Rename { .. } => &[("↩", "Rename"), ("Esc", "Cancel")],
            EditTarget::Transfer { cut: false, .. } => &[("↩", "Copy"), ("Esc", "Cancel")],
            EditTarget::Transfer { cut: true, .. } => &[("↩", "Move"), ("Esc", "Cancel")],
        }
    }
}

/// What a rename input selects at first: the name up to its extension, so typing replaces
/// the name and keeps `.md`. A folder, or a name with no extension (`.gitignore`), is selected whole.
fn name_selection(name: &str, kind: EntryKind) -> Range<usize> {
    match name.rfind('.') {
        Some(dot) if dot > 0 && kind == EntryKind::File => 0..dot,
        _ => 0..name.len(),
    }
}

/// One line of the list: a tree row, or where a new item will land while its name is typed.
enum Item {
    Row(Row),
    New {
        depth: usize,
        name: String,
        folder: bool,
    },
}

/// The input line and its refusal, as the list draws them.
struct EditView {
    input: Entity<InputState>,
    error: Option<SharedString>,
}

/// What the gap right of the tree drags; the panel resizes as it moves.
pub struct ResizeDrag;

/// Rows being dragged, on disk. The tree takes them to a folder; the editor opens them and the
/// terminal types their paths.
pub struct DraggedItems {
    pub paths: Vec<PathBuf>,
}

/// What follows the pointer while rows are dragged: the name, or how many items.
struct DragPreview {
    label: SharedString,
    /// Where the row was grabbed. GPUI puts the view's corner that far from the pointer, so the
    /// view pads by it to show the label beside the pointer instead of over the row below.
    grab: Point<Pixels>,
}

impl DragPreview {
    fn new(paths: &[PathBuf], grab: Point<Pixels>) -> Self {
        let label = match paths {
            [one] => file_name(one),
            many => format!("{} items", many.len()),
        };
        Self {
            label: label.into(),
            grab,
        }
    }
}

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = theme::colors(cx);
        div()
            .pl(self.grab.x + z(12., cx))
            .pt(self.grab.y + z(12., cx))
            .child(
                div()
                    .px(z(8., cx))
                    .py(z(2., cx))
                    .rounded(z(4., cx))
                    .border_1()
                    .border_color(c(p.line))
                    .bg(c(p.paper))
                    .text_size(z(size::UI_TEXT, cx))
                    .text_color(c(p.ink))
                    .child(self.label.clone()),
            )
    }
}

impl EventEmitter<PanelEvent> for ProjectPanel {}
impl EventEmitter<FileEvent> for ProjectPanel {}

impl Focusable for ProjectPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ProjectPanel {
    pub fn new(folders: Folders, cx: &mut Context<Self>) -> Self {
        let mut tree = ProjectTree::new(folders.first().to_path_buf());
        // Several folders sit side by side at the top, as the user asked (not under a node).
        if folders.several() {
            tree.hide_root();
        }
        tree.set_show_ignored(crate::settings(cx).show_ignored);
        let mut panel = Self {
            tree,
            folders,
            focus_handle: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            show_key_hints: crate::settings(cx).key_hints,
            width: size::TREE_WIDTH,
            search: None,
            focus_out: None,
            reveal_pending: false,
            edit: None,
            menu: None,
            route: route::Motion::default(),
            hovered: None,
            drop_target: None,
            undo: Vec::new(),
        };
        panel.load_dir(PathBuf::new(), cx);
        panel
    }

    pub fn folders(&self) -> &Folders {
        &self.folders
    }

    /// The settings changed: the hint line and gitignored items follow them.
    pub fn apply_settings(&mut self, cx: &mut Context<Self>) {
        let settings = crate::settings(cx);
        self.show_key_hints = settings.key_hints;
        let show_ignored = settings.show_ignored;
        if self.tree.shows_ignored() != show_ignored {
            self.tree.set_show_ignored(show_ignored);
            self.reveal_selection();
        }
        cx.notify();
    }

    /// Reopens the folders and the selection a session saved; folders that no longer exist stay closed.
    pub fn restore(
        &mut self,
        expanded: &[PathBuf],
        selected: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        for dir in expanded {
            if self.tree.expand(dir) {
                self.load_dir(dir.clone(), cx);
            }
        }
        self.tree.select(selected);
        cx.notify();
    }

    pub fn expanded_dirs(&self) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = self.tree.expanded_dirs().map(Path::to_path_buf).collect();
        dirs.sort();
        dirs
    }

    pub fn selected(&self) -> Option<&Path> {
        self.tree.selected()
    }

    /// The selected folder, when the lead is a folder inside the window rather than the window's
    /// own folder: what Cmd+Shift+F from the tree searches.
    pub fn selected_folder(&self) -> Option<PathBuf> {
        let lead = self.tree.selected()?;
        let rows = self.tree.rows();
        let row = rows.iter().find(|row| row.path == lead)?;
        (row.kind == EntryKind::Dir && !lead.as_os_str().is_empty()).then(|| lead.to_path_buf())
    }

    /// Everything selected, top to bottom.
    pub fn selection(&self) -> Vec<PathBuf> {
        self.tree.selection(&self.tree.rows())
    }

    /// The rows the tree shows, top to bottom; for the UI checks.
    pub fn row_paths(&self) -> Vec<PathBuf> {
        self.tree.rows().into_iter().map(|row| row.path).collect()
    }

    /// Reads one folder off the main thread and shows it when it arrives.
    pub fn load_dir(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        let folders = self.folders.clone();
        cx.spawn(async move |this, cx| {
            let listing = cx.background_spawn({
                let dir = dir.clone();
                async move { list(&folders, &dir) }
            });
            let Ok(entries) = listing.await else { return };
            let _ = this.update(cx, |this, cx| {
                this.tree.set_listing(dir, entries);
                if this.reveal_pending {
                    this.reveal_selection();
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Selects `rel` (relative to the open folder) and opens the folders above it, reading the
    /// ones not read yet, then scrolls to it once its row is there. The workspace calls it to
    /// follow the active tab.
    pub fn reveal(&mut self, rel: &Path, cx: &mut Context<Self>) {
        for dir in self.tree.reveal(rel) {
            self.load_dir(dir, cx);
        }
        self.reveal_selection();
        cx.notify();
    }

    /// Folders changed on disk: re-read the ones on screen, forget the closed ones until they open.
    pub fn refresh_dirs(&mut self, dirs: &[PathBuf], cx: &mut Context<Self>) {
        for dir in dirs {
            if self.tree.listing(dir).is_none() {
                continue;
            }
            if dir.as_os_str().is_empty() || self.tree.is_expanded(dir) {
                self.load_dir(dir.clone(), cx);
            } else {
                self.tree.invalidate(dir);
            }
        }
    }

    fn apply(&mut self, key: NavKey, cx: &mut Context<Self>) {
        // The tree stays put while a name is typed: arrows that reach here were the input's. A
        // row's menu keeps its row, so the tree stays put behind it too.
        if self.edit.is_some() || matches!(self.menu, Some(Menu::Context { .. })) {
            return;
        }
        // Every move but Up and Down ends a speed search.
        self.search = None;
        let outcome = nav::navigate(&self.tree.rows(), self.tree.selected(), key);
        if let Some(dir) = outcome.expand
            && self.tree.expand(&dir)
        {
            self.load_dir(dir, cx);
        }
        if let Some(dir) = outcome.collapse {
            self.tree.collapse(&dir);
        }
        if let Some(path) = outcome.select {
            self.tree.select(Some(path));
        }
        self.reveal_selection();
        cx.notify();
    }

    /// Shift+Up and Shift+Down: the selection grows, or shrinks back, a row at a time from where it
    /// started (IntelliJ).
    fn extend(&mut self, key: NavKey, cx: &mut Context<Self>) {
        if self.edit.is_some() || self.menu.is_some() {
            return;
        }
        self.search = None;
        let rows = self.tree.rows();
        if let Some(path) = nav::navigate(&rows, self.tree.selected(), key).select {
            self.tree.extend_to(&rows, path);
        }
        self.reveal_selection();
        cx.notify();
    }

    /// Up and Down walk the matches while a speed search is on, the rows otherwise.
    fn up_down(&mut self, key: NavKey, cx: &mut Context<Self>) {
        if self.edit.is_some() {
            return;
        }
        match &mut self.menu {
            Some(Menu::New { choice, .. }) => {
                *choice = if key == NavKey::Up { 0 } else { 1 };
                cx.notify();
                return;
            }
            Some(Menu::Context {
                entries, choice, ..
            }) => {
                // Round the ends, as macOS menus do.
                let count = entries.iter().map(Vec::len).sum::<usize>().max(1);
                *choice = Some(match (key, *choice) {
                    (NavKey::Up, Some(at)) => (at + count - 1) % count,
                    (NavKey::Up, None) => count - 1,
                    (_, Some(at)) => (at + 1) % count,
                    (_, None) => 0,
                });
                cx.notify();
                return;
            }
            _ => {}
        }
        match (&self.search, key) {
            (Some(_), NavKey::Up) => self.search_step(SearchStep::Prev, cx),
            (Some(_), _) => self.search_step(SearchStep::Next, cx),
            (None, _) => self.apply(key, cx),
        }
    }

    /// Selects the match `step` points at; with no match the selection stays.
    fn search_step(&mut self, step: SearchStep, cx: &mut Context<Self>) {
        let Some(query) = &self.search else {
            return;
        };
        if let Some(path) = nav::speed_search(&self.tree.rows(), self.tree.selected(), query, step)
        {
            self.tree.select(Some(path));
            self.reveal_selection();
        }
        cx.notify();
    }

    /// Typing a name starts or extends a speed search; Backspace takes a character back.
    /// Keys with a binding (arrows, Enter, Space, Escape) arrive as actions instead.
    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // While a name is being typed, the keys belong to its input.
        if self.edit.is_some() {
            return;
        }
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        // F and D pick in the New popup, as the first letters of its lines.
        if matches!(self.menu, Some(Menu::New { .. })) {
            match keystroke.key.as_str() {
                "f" => self.choose_new(EntryKind::File, window, cx),
                "d" => self.choose_new(EntryKind::Dir, window, cx),
                _ => {}
            }
            cx.stop_propagation();
            return;
        }
        // Letters do not start a search behind a row's menu.
        if matches!(self.menu, Some(Menu::Context { .. })) {
            return;
        }
        // Option types special characters on macOS; leave it, like Cmd and Ctrl, to shortcuts.
        if modifiers.platform || modifiers.control || modifiers.alt || modifiers.function {
            return;
        }
        if keystroke.key == "backspace" {
            let Some(query) = &mut self.search else {
                return;
            };
            query.pop();
            if query.is_empty() {
                self.search = None;
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        let Some(text) = keystroke
            .key_char
            .as_deref()
            .filter(|t| !t.is_empty() && !t.chars().any(char::is_control))
        else {
            return;
        };
        self.watch_focus(window, cx);
        self.search.get_or_insert_default().push_str(text);
        self.search_step(SearchStep::Here, cx);
        cx.stop_propagation();
    }

    /// Cmd+F, as in IntelliJ's tree: opens the speed search empty, so the next letters find a name.
    fn start_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.edit.is_some() || self.menu.is_some() {
            return;
        }
        self.watch_focus(window, cx);
        self.search.get_or_insert_default();
        cx.notify();
    }

    /// Escape cancels a name being typed or clears a speed search; with neither, it goes on
    /// to focus the editor.
    fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            cx.notify();
        } else if self.edit.is_some() {
            self.end_edit(window, cx);
        } else if self.search.take().is_some() {
            cx.notify();
        } else {
            cx.propagate();
        }
    }

    /// Cmd+N: asks whether to make a file or a folder (IntelliJ's New popup), for the selected
    /// folder or the selected file's folder.
    fn new_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.edit.is_some() {
            return;
        }
        self.watch_focus(window, cx);
        self.search = None;
        let dir = self.target_dir(cx);
        self.menu = Some(Menu::New { dir, choice: 0 });
        self.reveal_selection();
        cx.notify();
    }

    /// Where new and pasted items go, opened: the selected folder, else the selected file's folder.
    fn target_dir(&mut self, cx: &mut Context<Self>) -> PathBuf {
        let rows = self.tree.rows();
        let selected = self
            .tree
            .selected()
            .and_then(|s| rows.iter().find(|r| r.path == s));
        let mut dir = match selected {
            Some(row) if row.kind == EntryKind::Dir && !row.loops => row.path.clone(),
            Some(row) => row.path.parent().map(Path::to_path_buf).unwrap_or_default(),
            None => PathBuf::new(),
        };
        // The window's node of several folders is on no disk: the first folder takes the item.
        if self.folders.mount(&dir).is_none() {
            dir = PathBuf::from(&self.folders.keys()[0]);
        }
        if self.tree.expand(&dir) {
            self.load_dir(dir.clone(), cx);
        }
        dir
    }

    /// IntelliJ's Open In > Terminal: a shell in the selected folder, or the selected file's folder.
    fn open_in_terminal(&mut self, cx: &mut Context<Self>) {
        let rows = self.tree.rows();
        let Some(row) = self
            .tree
            .selected()
            .and_then(|s| rows.iter().find(|r| r.path == s))
        else {
            return;
        };
        let dir = match row.kind {
            EntryKind::Dir => row.path.clone(),
            EntryKind::File => row.path.parent().map(Path::to_path_buf).unwrap_or_default(),
        };
        cx.emit(PanelEvent::OpenTerminal(dir));
    }

    /// Cmd+C and Cmd+X: puts the selection on the clipboard for Cmd+V, as its paths one per line
    /// (pasted elsewhere, they are paths), marked as subwy's items to copy or to move.
    fn put_on_clipboard(&mut self, cut: bool, cx: &mut Context<Self>) {
        if self.edit.is_some() || self.menu.is_some() {
            return;
        }
        // A folder of the window does not move; it can be copied somewhere else.
        let paths: Vec<PathBuf> = tree::outermost(&self.selection())
            .iter()
            .filter(|p| !(cut && (p.as_os_str().is_empty() || self.folders.is_folder(p))))
            .filter_map(|p| self.folders.absolute(p))
            .collect();
        if paths.is_empty() {
            return;
        }
        let text = paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("\n");
        let mark = if cut { CLIPBOARD_CUT } else { CLIPBOARD_COPY };
        cx.write_to_clipboard(ClipboardItem::new_string_with_metadata(
            text.clone(),
            mark.to_string(),
        ));
        let label = match (cut, paths.len()) {
            (true, 1) => "Cut file",
            (true, _) => "Cut files",
            (false, 1) => "Copied file",
            (false, _) => "Copied files",
        };
        cx.emit(PanelEvent::Copied { text, label });
    }

    /// Cmd+V: copies, or moves after Cmd+X, what is on the clipboard into the target folder, off
    /// the main thread. Files copied in Finder paste too. A copy whose name is taken gets Finder's
    /// `name copy`; a move never replaces anything.
    fn paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.edit.is_some() || self.menu.is_some() {
            return;
        }
        let Some((sources, cut)) = cx.read_from_clipboard().as_ref().and_then(clipboard_items)
        else {
            return;
        };
        self.search = None;
        let dir = self.target_dir(cx);
        let landing = self.put_into(sources, &dir, cut, "pasted", window, cx);
        cx.spawn(async move |_, cx| {
            let landed = landing.await;
            // Moved once: the clipboard keeps the new paths as plain text, so a second Cmd+V does
            // not try to move them again.
            if cut && !landed.is_empty() {
                let text = landed
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("\n");
                cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string(text)));
            }
        })
        .detach();
    }

    /// Copies or, with `cut`, moves `sources` (on disk) into the tree's folder `dir`, off the main
    /// thread, then shows them there. A copy whose name is taken gets Finder's `name copy`; a move
    /// never replaces anything. Resolves to where they landed.
    fn put_into(
        &mut self,
        sources: Vec<PathBuf>,
        dir: &Path,
        cut: bool,
        verb: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Vec<PathBuf>> {
        let Some(dir) = self.folders.absolute(dir) else {
            return Task::ready(Vec::new());
        };
        let work = cx.background_executor().spawn(async move {
            sources
                .into_iter()
                .map(|from| {
                    let name = from
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let result = if cut {
                        fs::move_into(&from, &dir, &name)
                    } else {
                        let kind = if from.is_dir() {
                            EntryKind::Dir
                        } else {
                            EntryKind::File
                        };
                        fs::copy_into(&from, &dir, &fs::free_name(&dir, &name, kind))
                    };
                    (from, result)
                })
                .collect::<Vec<_>>()
        });
        cx.spawn_in(window, async move |this, cx| {
            let results = work.await;
            this.update_in(cx, |this, window, cx| {
                let landed = this.arrived(cut, results, verb, window, cx);
                this.remember(changed(cut, &landed));
                landed.into_iter().map(|(_, to)| to).collect()
            })
            .unwrap_or_default()
        })
    }

    /// The folder a drop at `position` would go into: the folder of the row there (a file's own
    /// folder), or the open folder below the last row. `lines` are the list's lines' folders.
    fn drop_line(
        &self,
        bounds: Bounds<Pixels>,
        position: Point<Pixels>,
        lines: &[Option<PathBuf>],
        cx: &App,
    ) -> Option<PathBuf> {
        if !bounds.contains(&position) {
            return None;
        }
        let offset = self.scroll.0.borrow().base_handle.offset();
        let row = z(size::TREE_ROW, cx);
        let line = ((position.y - bounds.top() - offset.y) / row).floor();
        if line < 0. {
            return None;
        }
        match lines.get(line as usize) {
            Some(folder) => folder.clone(),
            // A window of several folders has no folder of its own to take it.
            None => (!self.folders.several()).then(PathBuf::new),
        }
    }

    /// A drag over the tree aims at `target`, unless the dragged items hold it. The way to it is
    /// drawn as the hovered row's is.
    fn drag_over(&mut self, target: Option<PathBuf>, dragged: &[PathBuf], cx: &mut Context<Self>) {
        let target = target.filter(|dir| {
            self.folders
                .absolute(dir)
                .is_some_and(|abs| !dragged.iter().any(|item| abs.starts_with(item)))
        });
        if self.drop_target != target {
            self.drop_target = target;
            cx.notify();
        }
    }

    /// Rows dropped on the tree move to the folder under the pointer; with Option held they are
    /// copied (Finder's way). Files from Finder are always copied in.
    fn drop_items(
        &mut self,
        sources: Vec<PathBuf>,
        cut: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dir) = self.drop_target.take() else {
            return;
        };
        if self.edit.is_some() {
            return;
        }
        self.focus_handle.focus(window, cx);
        let verb = if cut { "moved" } else { "copied" };
        self.put_into(sources, &dir, cut, verb, window, cx).detach();
    }

    /// Shows items a paste, F5, F6 or Cmd+Z put somewhere, selected; moved items take their open
    /// tabs along. Reports the ones that could not be `verb` (pasted, copied, moved). Returns the
    /// others as (from, to) on disk.
    fn arrived(
        &mut self,
        cut: bool,
        results: Vec<(PathBuf, Result<PathBuf, NameError>)>,
        verb: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<(PathBuf, PathBuf)> {
        let mut landed = Vec::new();
        let mut failed = Vec::new();
        for (from, result) in results {
            match result {
                Ok(to) => {
                    let moved = self.folders.relative(&from).zip(self.folders.relative(&to));
                    if cut && let Some((from, to)) = moved.filter(|(from, to)| from != to) {
                        self.tree.renamed(&from, &to);
                        self.load_dir(from.parent().map(Path::to_path_buf).unwrap_or_default(), cx);
                        cx.emit(FileEvent::Renamed { from, to });
                    }
                    landed.push((from, to));
                }
                Err(err) => failed.push((from, err)),
            }
        }
        let rels: Vec<PathBuf> = landed
            .iter()
            .filter_map(|(_, abs)| self.folders.relative(abs))
            .collect();
        // Every read folder on the way may have changed: the item, and any folder made for it.
        let changed: std::collections::BTreeSet<PathBuf> = rels
            .iter()
            .flat_map(|rel| rel.ancestors().skip(1))
            .filter(|dir| self.tree.listing(dir).is_some())
            .map(Path::to_path_buf)
            .collect();
        for dir in changed {
            self.load_dir(dir, cx);
        }
        if let Some((first, rest)) = rels.split_first() {
            self.reveal(first, cx);
            for rel in rest {
                self.tree.toggle(rel.clone());
            }
        }
        let title = match failed.as_slice() {
            [] => return landed,
            [(from, _)] => format!("“{}” could not be {verb}", file_name(from)),
            many => format!("{} items could not be {verb}", many.len()),
        };
        drop(window.prompt(
            PromptLevel::Critical,
            &title,
            Some(&failed[0].1.to_string()),
            &["OK"],
            cx,
        ));
        landed
    }

    /// Right-click on a row: selects it and shows what can be done to it.
    fn open_context_menu(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.edit.is_some() {
            return;
        }
        self.focus_handle.focus(window, cx);
        self.watch_focus(window, cx);
        self.search = None;
        // A row of a larger selection keeps it: the menu is for all of them.
        if self.tree.is_selected(&path) {
            self.tree.lead(&path);
        } else {
            self.tree.select(Some(path.clone()));
        }
        let own_folder = path.as_os_str().is_empty() || self.folders.is_folder(&path);
        let folder = self
            .tree
            .rows()
            .iter()
            .any(|row| row.path == path && row.kind == EntryKind::Dir);
        let entries = context_entries(folder, own_folder, self.folders.several());
        self.menu = Some(Menu::Context {
            path,
            entries,
            choice: None,
        });
        cx.notify();
    }

    /// A line of a row's menu, clicked or picked with Enter. Most do what their key does, so
    /// the action is dispatched to the tree, which has focus.
    fn run_entry(&mut self, entry: Entry, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Menu::Context { path, .. }) = self.menu.take() else {
            return;
        };
        match entry {
            Entry::NewFile | Entry::NewDirectory => {
                self.new_file(window, cx);
                let kind = if entry == Entry::NewFile {
                    EntryKind::File
                } else {
                    EntryKind::Dir
                };
                self.choose_new(kind, window, cx);
            }
            Entry::Cut => window.dispatch_action(Box::new(CutItems), cx),
            Entry::Copy => window.dispatch_action(Box::new(CopyItems), cx),
            Entry::Paste => window.dispatch_action(Box::new(PasteItems), cx),
            Entry::CopyPath => window.dispatch_action(Box::new(CopyPath), cx),
            Entry::CopyRelativePath => window.dispatch_action(Box::new(CopyRelativePath), cx),
            Entry::CopyFileName => window.dispatch_action(Box::new(CopyFileName), cx),
            Entry::FindInFolder => window.dispatch_action(Box::new(ToggleSearch), cx),
            Entry::Rename => self.rename(window, cx),
            Entry::CopyTo => self.transfer(false, window, cx),
            Entry::MoveTo => self.transfer(true, window, cx),
            Entry::Trash => self.trash(window, cx),
            Entry::RevealInFinder => window.dispatch_action(Box::new(RevealInFinder), cx),
            Entry::OpenInTerminal => self.open_in_terminal(cx),
            Entry::OpenInDefaultApp => {
                if let Some(abs) = self.folders.absolute(&path) {
                    crate::workspace::open_with_system(&abs, cx);
                }
            }
            Entry::RemoveFromWindow => cx.emit(PanelEvent::RemoveFolder(path)),
        }
        cx.notify();
    }

    /// The New popup's choice: an input for the new item's name, right under its folder.
    fn choose_new(&mut self, kind: EntryKind, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Menu::New { dir, .. }) = self.menu.take() else {
            return;
        };
        self.start_edit(EditTarget::New { dir, kind }, "", 0..0, window, cx);
        if let Some(index) = self
            .items(self.tree.rows())
            .iter()
            .position(|item| matches!(item, Item::New { .. }))
        {
            self.scroll.scroll_to_item(index, ScrollStrategy::Nearest);
        }
    }

    /// The header's Expand All: opens every folder that is not gitignored, reading them off the main
    /// thread, up to [`EXPAND_ALL_MOST`] folders.
    fn expand_all(&mut self, cx: &mut Context<Self>) {
        let folders = self.folders.clone();
        let read = cx.background_spawn(async move {
            let mut queue = VecDeque::from([PathBuf::new()]);
            let mut found = Vec::new();
            while let Some(dir) = queue.pop_front() {
                if found.len() >= EXPAND_ALL_MOST {
                    break;
                }
                let Ok(entries) = list(&folders, &dir) else {
                    continue;
                };
                queue.extend(
                    entries
                        .iter()
                        .filter(|e| e.kind == EntryKind::Dir && !e.ignored && !e.loops)
                        .map(|e| dir.join(&e.name)),
                );
                found.push((dir, entries));
            }
            found
        });
        cx.spawn(async move |this, cx| {
            let found = read.await;
            let _ = this.update(cx, |this, cx| {
                for (dir, entries) in found {
                    this.tree.set_listing(dir.clone(), entries);
                    this.tree.expand(&dir);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Collapse All (the header's button, Cmd+Shift+-): every folder but the top closes.
    fn collapse_all(&mut self, cx: &mut Context<Self>) {
        self.tree.collapse_all();
        self.reveal_selection();
        cx.notify();
    }

    /// IntelliJ's tool window header: the title, then New, Select Opened File, Expand All,
    /// Collapse All, more options and Hide.
    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = theme::colors(cx);
        let options = matches!(self.menu, Some(Menu::Options)).then(|| self.options_popup(cx));
        div()
            .h(z(size::TREE_HEAD, cx))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .pl(z(14., cx))
            .pr(z(6., cx))
            // A small capital label, as on a station sign.
            .child(
                div()
                    .text_size(z(11., cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c(p.sub))
                    .child("PROJECT"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(z(2., cx))
                    .child(
                        // On mouse down, so the tree's own mouse down does not close the popup at once.
                        header_button("tree-new", IconName::Plus, "New…", cx).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                cx.stop_propagation();
                                this.focus_handle.focus(window, cx);
                                this.new_file(window, cx);
                            }),
                        ),
                    )
                    .child(
                        header_button(
                            "tree-locate",
                            IconName::LocateFixed,
                            "Select Opened File",
                            cx,
                        )
                        .on_click(
                            cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(PanelEvent::Locate)),
                        ),
                    )
                    .child(
                        header_button("tree-expand", IconName::ChevronsUpDown, "Expand All", cx)
                            .on_click(
                                cx.listener(|this, _: &ClickEvent, _, cx| this.expand_all(cx)),
                            ),
                    )
                    .child(
                        header_button(
                            "tree-collapse",
                            IconName::ChevronsDownUp,
                            "Collapse All",
                            cx,
                        )
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.collapse_all(cx))),
                    )
                    .child(
                        div()
                            .relative()
                            .child(
                                header_button(
                                    "tree-options",
                                    IconName::EllipsisVertical,
                                    "Options",
                                    cx,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                        cx.stop_propagation();
                                        this.focus_handle.focus(window, cx);
                                        this.watch_focus(window, cx);
                                        this.menu = match this.menu {
                                            Some(Menu::Options) => None,
                                            _ => Some(Menu::Options),
                                        };
                                        cx.notify();
                                    }),
                                ),
                            )
                            .children(options),
                    )
                    .child(
                        header_button("tree-hide", IconName::Minus, "Hide", cx).on_click(
                            cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(PanelEvent::Hide)),
                        ),
                    ),
            )
    }

    /// The more button's list: the key hint line, gitignored items, and the copy and reveal actions
    /// for the selection.
    fn options_popup(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let hints = self.show_key_hints;
        let ignored = self.tree.shows_ignored();
        floating(
            HEADER_BUTTON + 2.,
            Anchor::TopRight,
            popup_box(cx)
                .child(
                    popup_item(
                        "tree-option-add",
                        Some(IconName::FolderPlus),
                        "Add Folder…",
                        false,
                        cx,
                    )
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.menu = None;
                        cx.emit(PanelEvent::AddFolder);
                        cx.notify();
                    })),
                )
                .child(
                    popup_item(
                        "tree-option-hints",
                        hints.then_some(IconName::Check),
                        "Key Hints",
                        false,
                        cx,
                    )
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.menu = None;
                        this.show_key_hints = !this.show_key_hints;
                        cx.notify();
                    })),
                )
                .child(
                    popup_item(
                        "tree-option-ignored",
                        ignored.then_some(IconName::Check),
                        "Show Ignored Files",
                        false,
                        cx,
                    )
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.menu = None;
                        this.tree.set_show_ignored(!this.tree.shows_ignored());
                        this.reveal_selection();
                        cx.notify();
                    })),
                )
                .child(
                    popup_item("tree-option-copy", None, "Copy Path", false, cx).on_click(
                        cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.menu = None;
                            window.dispatch_action(Box::new(CopyPath), cx);
                            cx.notify();
                        }),
                    ),
                )
                .child(
                    popup_item("tree-option-reveal", None, "Reveal in Finder", false, cx).on_click(
                        cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.menu = None;
                            window.dispatch_action(Box::new(RevealInFinder), cx);
                            cx.notify();
                        }),
                    ),
                ),
            cx,
        )
    }

    /// Shift+F6: the selected row's name becomes an input.
    fn rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.edit.is_some() {
            return;
        }
        let rows = self.tree.rows();
        // A folder's own row renames nothing: the window is that folder.
        let Some(row) = self
            .tree
            .selected()
            .filter(|s| !s.as_os_str().is_empty() && !self.folders.is_folder(s))
            .and_then(|s| rows.iter().find(|r| r.path == s))
        else {
            return;
        };
        self.search = None;
        let target = EditTarget::Rename {
            path: row.path.clone(),
        };
        let select = name_selection(&row.name, row.kind);
        self.start_edit(target, &row.name, select, window, cx);
        self.reveal_selection();
    }

    /// F5 and F6 (IntelliJ's Copy and Move): the command line takes where the selection goes, as
    /// the tree names paths. One item shows its own path, its name selected to copy or its folder
    /// to move; several show their folder.
    fn transfer(&mut self, cut: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.edit.is_some() {
            return;
        }
        // Never the window's own folder; a folder of several is copied but does not move.
        let sources: Vec<PathBuf> = tree::outermost(&self.selection())
            .into_iter()
            .filter(|p| !p.as_os_str().is_empty() && self.folders.mount(p).is_some())
            .filter(|p| !(cut && self.folders.is_folder(p)))
            .collect();
        let Some(first) = sources.first() else {
            return;
        };
        self.search = None;
        let folder = match first.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => {
                format!("{}/", paths::relative(parent))
            }
            _ => String::new(),
        };
        let (text, select) = match sources.as_slice() {
            [one] => {
                let name = file_name(one);
                let kind = self
                    .tree
                    .rows()
                    .iter()
                    .find(|row| row.path == *one)
                    .map_or(EntryKind::File, |row| row.kind);
                let select = if cut {
                    0..folder.len()
                } else {
                    let stem = name_selection(&name, kind);
                    folder.len() + stem.start..folder.len() + stem.end
                };
                (format!("{folder}{name}"), select)
            }
            _ => (folder.clone(), 0..folder.len()),
        };
        self.start_edit(
            EditTarget::Transfer { sources, cut },
            &text,
            select,
            window,
            cx,
        );
    }

    /// Cmd+Backspace: asks, then moves the selection to the Trash off the main thread.
    fn trash(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.edit.is_some() {
            return;
        }
        // Never a folder of the window itself (several leave by Remove from Window), and a folder
        // takes what it holds.
        let paths: Vec<PathBuf> = tree::outermost(&self.selection())
            .into_iter()
            .filter(|s| !s.as_os_str().is_empty() && !self.folders.is_folder(s))
            .collect();
        let jobs: Vec<(PathBuf, PathBuf, PathBuf)> = paths
            .iter()
            .filter_map(|path| {
                let (root, rest, _) = self.folders.mount(path)?;
                Some((path.clone(), root.to_path_buf(), rest))
            })
            .collect();
        if jobs.is_empty() {
            return;
        }
        self.search = None;
        let question = match jobs.as_slice() {
            [(path, ..)] => format!("Move “{}” to the Trash?", file_name(path)),
            many => format!("Move {} items to the Trash?", many.len()),
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            &question,
            None,
            &["Move to Trash", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let results = cx.background_executor().spawn(trash_all(jobs)).await;
            let _ = this.update_in(cx, |this, window, cx| this.trashed(results, window, cx));
        })
        .detach();
    }

    /// Items went to the Trash (`results` by tree path): the tree forgets them and their tabs
    /// close. Reports the ones that did not go.
    fn trashed(
        &mut self,
        results: Vec<(PathBuf, std::io::Result<()>)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut failed = Vec::new();
        for (path, result) in results {
            match result {
                Ok(()) => {
                    self.tree.forget(&path);
                    // Reading the folder again moves the selection to a neighbor.
                    self.load_dir(path.parent().map(Path::to_path_buf).unwrap_or_default(), cx);
                    cx.emit(FileEvent::Trashed { path });
                }
                Err(err) => failed.push((path, err)),
            }
        }
        let title = match failed.as_slice() {
            [] => return,
            [(path, _)] => format!("“{}” could not be moved to the Trash", file_name(path)),
            many => format!("{} items could not be moved to the Trash", many.len()),
        };
        // Only a report: there is no answer to wait for.
        drop(window.prompt(
            PromptLevel::Critical,
            &title,
            Some(&failed[0].1.to_string()),
            &["OK"],
            cx,
        ));
    }

    /// Cmd+Z in the tree: takes back the last rename, move, copy, paste or new item, newest first.
    /// A moved item goes back where it was; a made one goes to the Trash. The Trash itself is not
    /// taken back (Finder's Put Back does that).
    fn undo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.edit.is_some() || self.menu.is_some() {
            return;
        }
        let Some(change) = self.undo.pop() else {
            return;
        };
        self.search = None;
        match change {
            Undo::Moved(pairs) => {
                let work = cx.background_executor().spawn(async move {
                    pairs
                        .into_iter()
                        .rev()
                        .map(|(from, to)| {
                            let back = match (from.parent(), from.file_name()) {
                                (Some(dir), Some(name)) => {
                                    fs::move_into(&to, dir, &name.to_string_lossy())
                                }
                                _ => Err(NameError::Invalid),
                            };
                            (to, back)
                        })
                        .collect::<Vec<_>>()
                });
                cx.spawn_in(window, async move |this, cx| {
                    let results = work.await;
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.arrived(true, results, "moved back", window, cx);
                    });
                })
                .detach();
            }
            Undo::Made(paths) => {
                let jobs: Vec<(PathBuf, PathBuf, PathBuf)> = paths
                    .iter()
                    .filter_map(|abs| {
                        let rel = self.folders.relative(abs)?;
                        let (root, rest, _) = self.folders.mount(&rel)?;
                        Some((rel, root.to_path_buf(), rest))
                    })
                    .collect();
                cx.spawn_in(window, async move |this, cx| {
                    let results = cx.background_executor().spawn(trash_all(jobs)).await;
                    let _ =
                        this.update_in(cx, |this, window, cx| this.trashed(results, window, cx));
                })
                .detach();
            }
        }
    }

    /// Keeps what a change did, for Cmd+Z: moves as (from, to) on disk, or the items it made.
    fn remember(&mut self, change: Undo) {
        let empty = match &change {
            Undo::Moved(pairs) => pairs.is_empty(),
            Undo::Made(paths) => paths.is_empty(),
        };
        if empty {
            return;
        }
        if self.undo.len() == UNDO_MOST {
            self.undo.remove(0);
        }
        self.undo.push(change);
    }

    /// Puts an input with `text` in the tree, `select` selected, and gives it focus.
    fn start_edit(
        &mut self,
        target: EditTarget,
        text: &str,
        select: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| InputState::new(window, cx));
        input.update(cx, |state, cx| {
            state.set_value(text.to_string(), window, cx);
            state.set_selected_range(select, cx);
            state.focus(window, cx);
        });
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.commit_edit(window, cx),
                InputEvent::Change => {
                    if let Some(edit) = &mut this.edit {
                        edit.typed = edit.input.read(cx).value().to_string();
                        edit.error = None;
                        cx.notify();
                    }
                }
                _ => {}
            },
        );
        self.watch_focus(window, cx);
        self.menu = None;
        self.edit = Some(Edit {
            target,
            input,
            error: None,
            busy: false,
            typed: text.to_string(),
            _subscription: subscription,
        });
        cx.notify();
    }

    /// Enter in the input: does the disk work off the main thread.
    fn commit_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = &mut self.edit else {
            return;
        };
        if edit.busy {
            return;
        }
        edit.busy = true;
        let typed = edit.input.read(cx).value().to_string();
        let target = match &edit.target {
            EditTarget::New { dir, .. } => dir,
            EditTarget::Rename { path } => path,
            EditTarget::Transfer { sources, cut } => {
                let (sources, cut) = (sources.clone(), *cut);
                return self.commit_transfer(sources, cut, &typed, window, cx);
            }
        };
        let Some((root, rest, prefix)) = self.folders.mount(target) else {
            return;
        };
        let root = root.to_path_buf();
        match edit.target.clone() {
            EditTarget::New { dir, kind } => {
                let typed = match kind {
                    EntryKind::Dir if !typed.trim_end().ends_with('/') => format!("{typed}/"),
                    _ => typed,
                };
                // Made inside its folder on disk, then named as the tree names it.
                let work = cx.background_executor().spawn(async move {
                    fs::create(&root, &rest, &typed).map(|(made, kind)| (prefix.join(made), kind))
                });
                cx.spawn_in(window, async move |this, cx| {
                    let result = work.await;
                    let _ = this
                        .update_in(cx, |this, window, cx| this.created(dir, result, window, cx));
                })
                .detach();
            }
            EditTarget::Transfer { .. } => {}
            EditTarget::Rename { path } => {
                let work = cx.background_executor().spawn(async move {
                    fs::rename(&root, &rest, &typed).map(|renamed| prefix.join(renamed))
                });
                cx.spawn_in(window, async move |this, cx| {
                    let result = work.await;
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.renamed(path, result, window, cx)
                    });
                })
                .detach();
            }
        }
    }

    /// Enter on F5 / F6's command line: copies or moves off the main thread. The folder is made
    /// when it is not there. A single item typed onto a folder goes inside it, as `mv` does.
    fn commit_transfer(
        &mut self,
        sources: Vec<PathBuf>,
        cut: bool,
        typed: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = fs::typed_target(typed, sources.len() > 1).and_then(|(dir, name)| {
            // With several folders, the first part of a path names one of them.
            let dir = self.folders.absolute(&dir).ok_or(NameError::Invalid)?;
            Ok((dir, name))
        });
        let (dir, name) = match target {
            Ok(target) => target,
            Err(err) => return self.refused(err, cx),
        };
        let sources: Vec<PathBuf> = sources
            .iter()
            .filter_map(|path| self.folders.absolute(path))
            .collect();
        let work = cx.background_executor().spawn(async move {
            std::fs::create_dir_all(&dir)?;
            Ok::<_, NameError>(
                sources
                    .into_iter()
                    .map(|from| {
                        let own = from
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        let (into, name) = match &name {
                            Some(name) if dir.join(name) != from && dir.join(name).is_dir() => {
                                (dir.join(name), own)
                            }
                            Some(name) => (dir.clone(), name.clone()),
                            None => (dir.clone(), own),
                        };
                        let result = if cut {
                            fs::move_into(&from, &into, &name)
                        } else {
                            fs::copy_into(&from, &into, &name)
                        };
                        (from, result)
                    })
                    .collect::<Vec<_>>(),
            )
        });
        cx.spawn_in(window, async move |this, cx| {
            let outcome = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                // Nothing went: the command line stays with the reason, to try another path.
                let results = match outcome {
                    Err(err) => return this.refused(err, cx),
                    Ok(results) if results.iter().all(|(_, result)| result.is_err()) => {
                        if let Some(err) = results.into_iter().find_map(|(_, r)| r.err()) {
                            this.refused(err, cx);
                        }
                        return;
                    }
                    Ok(results) => results,
                };
                this.end_edit(window, cx);
                let verb = if cut { "moved" } else { "copied" };
                let landed = this.arrived(cut, results, verb, window, cx);
                this.remember(changed(cut, &landed));
            });
        })
        .detach();
    }

    fn renamed(
        &mut self,
        from: PathBuf,
        result: Result<PathBuf, NameError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let to = match result {
            Ok(to) => to,
            Err(err) => return self.refused(err, cx),
        };
        self.end_edit(window, cx);
        if to != from {
            if let (Some(was), Some(now)) =
                (self.folders.absolute(&from), self.folders.absolute(&to))
            {
                self.remember(Undo::Moved(vec![(was, now)]));
            }
            self.tree.renamed(&from, &to);
            self.load_dir(from.parent().map(Path::to_path_buf).unwrap_or_default(), cx);
            cx.emit(FileEvent::Renamed { from, to });
        }
        self.reveal_selection();
    }

    fn created(
        &mut self,
        dir: PathBuf,
        result: Result<(PathBuf, EntryKind), NameError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (path, kind) = match result {
            Ok(created) => created,
            Err(err) => return self.refused(err, cx),
        };
        self.end_edit(window, cx);
        if let Some(abs) = self.folders.absolute(&path) {
            self.remember(Undo::Made(vec![abs]));
        }
        // The folder it went into changed; `reveal` reads the folders made on the way.
        self.load_dir(dir, cx);
        self.reveal(&path, cx);
        if kind == EntryKind::File {
            cx.emit(PanelEvent::Open {
                path,
                pass_through: false,
            });
        }
    }

    /// Keeps the input open with the reason after it.
    fn refused(&mut self, err: NameError, cx: &mut Context<Self>) {
        if let Some(edit) = &mut self.edit {
            edit.busy = false;
            edit.error = Some(err.to_string().into());
        }
        cx.notify();
    }

    /// Drops the input and gives the tree its focus back.
    fn end_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.edit.take().is_some() {
            self.focus_handle.focus(window, cx);
        }
        cx.notify();
    }

    /// The list lines: the rows, with the line where a new item will land among its folder's
    /// items, named as typed so far.
    fn items(&self, rows: Vec<Row>) -> Vec<Item> {
        let new = self.edit.as_ref().and_then(|edit| match &edit.target {
            EditTarget::New { dir, kind } => Some((dir, *kind, &edit.typed)),
            EditTarget::Rename { .. } | EditTarget::Transfer { .. } => None,
        });
        let Some((dir, kind, typed)) = new else {
            return rows.into_iter().map(Item::Row).collect();
        };
        let at = tree::landing(&rows, dir, typed, kind);
        let depth = rows
            .iter()
            .find(|r| &r.path == dir)
            .map_or(0, |r| r.depth + 1);
        let folder = kind == EntryKind::Dir || typed.ends_with('/');
        let mut items: Vec<Item> = rows.into_iter().map(Item::Row).collect();
        items.insert(
            at.min(items.len()),
            Item::New {
                depth,
                name: typed.clone(),
                folder,
            },
        );
        items
    }

    fn watch_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.focus_out.is_none() {
            self.focus_out = Some(
                cx.on_focus_out(&self.focus_handle, window, |this, _, _, cx| {
                    this.search = None;
                    this.edit = None;
                    this.menu = None;
                    cx.notify();
                }),
            );
        }
    }

    /// Scrolls just enough to show the selected row. A selection whose folder is still being
    /// read has no row yet; the scroll waits for the listing.
    fn reveal_selection(&mut self) {
        let selected = self.tree.selected();
        let index = self
            .items(self.tree.rows())
            .iter()
            .position(|item| matches!(item, Item::Row(r) if Some(r.path.as_path()) == selected));
        if let Some(index) = index {
            self.scroll.scroll_to_item(index, ScrollStrategy::Nearest);
        }
        self.reveal_pending = index.is_none() && selected.is_some();
    }

    /// How many rows PageUp and PageDown move: as many as the list showed in the last frame.
    fn page_rows(&self, cx: &App) -> usize {
        let list_height = self
            .scroll
            .0
            .borrow()
            .last_item_size
            .map_or(0., |size| size.item.height.as_f32());
        rows_per_page(list_height, z(size::TREE_ROW, cx).as_f32())
    }

    fn open_selected(&mut self, pass_through: bool, cx: &mut Context<Self>) {
        // Enter on several items opens their files, as in IntelliJ; Space still shows the lead.
        let rows = self.tree.rows();
        let chosen = self.tree.selection(&rows);
        if chosen.len() > 1 && !pass_through {
            for row in rows.iter().filter(|r| r.kind == EntryKind::File) {
                if chosen.contains(&row.path) {
                    cx.emit(PanelEvent::Open {
                        path: row.path.clone(),
                        pass_through: false,
                    });
                }
            }
            return;
        }
        let Some(selected) = self.tree.selected().map(Path::to_path_buf) else {
            return;
        };
        let is_dir = self
            .tree
            .rows()
            .iter()
            .any(|r| r.path == selected && r.kind == EntryKind::Dir);
        if is_dir {
            if self.tree.is_expanded(&selected) {
                self.tree.collapse(&selected);
            } else if self.tree.expand(&selected) {
                self.load_dir(selected, cx);
            }
            self.reveal_selection();
            cx.notify();
        } else {
            cx.emit(PanelEvent::Open {
                path: selected,
                pass_through,
            });
        }
    }

    /// A click selects; a folder toggles on the first click and a file opens on the second.
    /// Folders ignore the second click so that a double click does not undo itself. Shift selects
    /// the rows up to here and Cmd adds or takes out this one, opening nothing.
    fn click_row(
        &mut self,
        path: PathBuf,
        kind: EntryKind,
        count: usize,
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) {
        if modifiers.shift || modifiers.platform {
            if modifiers.shift {
                let rows = self.tree.rows();
                self.tree.extend_to(&rows, path);
            } else {
                self.tree.toggle(path);
            }
            self.reveal_selection();
            cx.notify();
            return;
        }
        self.tree.select(Some(path));
        let open = match kind {
            EntryKind::Dir => count == 1,
            EntryKind::File if crate::settings(cx).single_click_open => count == 1,
            EntryKind::File => count == 2,
        };
        if open {
            self.open_selected(false, cx);
        }
        self.reveal_selection();
        cx.notify();
    }

    /// The pointer entered or left the row at `path`.
    fn hover_row(&mut self, path: &Path, hovered: bool, cx: &mut Context<Self>) {
        if hovered && self.hovered.as_deref() != Some(path) {
            self.hovered = Some(path.to_path_buf());
            cx.notify();
        } else if !hovered && self.hovered.as_deref() == Some(path) {
            self.hovered = None;
            cx.notify();
        }
    }

    /// A tree line at `depth`: its height and indent.
    fn row_frame(id: impl Into<ElementId>, depth: usize, cx: &App) -> Stateful<Div> {
        div()
            .id(id)
            .relative()
            .w_full()
            .flex()
            .items_center()
            .gap(z(ROW_GAP, cx))
            .h(z(size::TREE_ROW, cx))
            .pl(z(row_padding(depth), cx))
            .pr(z(ROW_PADDING_RIGHT, cx))
            .whitespace_nowrap()
    }

    /// `selection` is `Some(focused)` on the selected row, whose ink dims when the tree has no
    /// focus. On a row `on_route`, the chevron gives way to the route's ring and the icon and name
    /// stand out from the rest of the tree, whose icons stay faint. The backgrounds and the route
    /// itself are painted under the rows (see `route`).
    /// The part of the name a speed search for `query` matched is marked, and a row that neither
    /// matches nor lies on the route steps back.
    /// With `rename`, the name is an input. A folder of the window (and the window's own node) is
    /// set in medium and shows `hint`, where it is on disk.
    fn render_row(
        row: &Row,
        selection: Option<bool>,
        on_route: bool,
        query: Option<&str>,
        rename: Option<&EditView>,
        hint: Option<&SharedString>,
        cx: &App,
    ) -> Stateful<Div> {
        let p = theme::colors(cx);
        let is_dir = row.kind == EntryKind::Dir;
        let folder_row = row.path.as_os_str().is_empty() || hint.is_some();
        let (ink, chevron_color) = match selection {
            Some(true) => (p.sel_ink, p.sel_ink),
            Some(false) => (p.ink, p.sub),
            None if row.ignored => (p.ignored, p.sub),
            None => (p.ink, p.sub),
        };
        let mark = c(p.accent).opacity(0.3);
        let dim = !on_route && query.is_some_and(|q| nav::name_match(&row.name, q).is_none());
        Self::row_frame(
            ElementId::Name(row.path.to_string_lossy().to_string().into()),
            row.depth,
            cx,
        )
        .text_color(c(ink))
        .when(dim, |el| el.opacity(0.38))
        .when(on_route && !folder_row, |el| {
            el.font_weight(FontWeight::MEDIUM)
        })
        .child(
            div()
                .size(z(CHEVRON_SLOT, cx))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .when(is_dir && !on_route, |slot| {
                    slot.child(
                        Icon::default()
                            .data(CHEVRON)
                            .size(z(CHEVRON_SIZE, cx))
                            .text_color(c(chevron_color))
                            .when(row.expanded, |icon| {
                                icon.rotate(radians(std::f32::consts::FRAC_PI_2))
                            }),
                    )
                }),
        )
        .child({
            let icon = if is_dir {
                icons::folder_icon(row.expanded)
            } else {
                icons::file_icon(&row.name)
            };
            let color = if on_route { p.sub } else { p.ignored };
            Icon::new(icon)
                .size(z(ICON_SIZE, cx))
                .flex_none()
                .text_color(c(color))
        })
        .map(|el| match rename {
            Some(edit) => el.child(edit_field(edit, cx)),
            None if folder_row => el
                .child(
                    div()
                        .flex_none()
                        .font_weight(FontWeight::MEDIUM)
                        .child(name_label(&row.name, query, mark)),
                )
                .child(
                    // A long path loses its start, keeping the folders nearest the name.
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis_start()
                        .text_size(z(12., cx))
                        .text_color(c(p.sub))
                        .children(hint.cloned()),
                ),
            None => el.child(name_label(&row.name, query, mark)),
        })
    }

    /// Where a new item will land while its name is typed on the command line: its icon and the
    /// name so far, in the color of the line that will reach it (red once a name is refused).
    fn render_new(
        depth: usize,
        name: &str,
        folder: bool,
        refused: bool,
        cx: &App,
    ) -> Stateful<Div> {
        let p = theme::colors(cx);
        let color = if refused {
            p.ansi[1]
        } else {
            p.route[depth.saturating_sub(1) % p.route.len()]
        };
        let icon = if folder {
            icons::folder_icon(false)
        } else {
            icons::file_icon(name)
        };
        let shown = match (name.is_empty(), folder) {
            (false, _) => name.to_string(),
            (true, true) => "folder name".to_string(),
            (true, false) => "name".to_string(),
        };
        Self::row_frame("tree-new", depth, cx)
            .child(div().size(z(CHEVRON_SLOT, cx)).flex_none())
            .child(
                Icon::new(icon)
                    .size(z(ICON_SIZE, cx))
                    .flex_none()
                    .text_color(c(color)),
            )
            .child(
                div()
                    .italic()
                    .text_color(c(if refused { p.ansi[1] } else { p.sub }))
                    .child(shown),
            )
    }

    /// One of the speed search's buttons; `step` moves among the matches, none closes the search.
    fn search_button(
        id: &'static str,
        icon: IconName,
        tooltip: &'static str,
        step: Option<SearchStep>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        // On mouse down, so the tree's own mouse down does not close the search first.
        header_button(id, icon, tooltip, cx).on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                this.focus_handle.focus(window, cx);
                match step {
                    Some(step) => this.search_step(step, cx),
                    None => {
                        this.search = None;
                        cx.notify();
                    }
                }
            }),
        )
    }

    /// The speed search in place of the header (Cmd+F, or typing in the tree): what is looked for,
    /// which match is selected out of how many, and buttons to step and close. The tree takes the
    /// keys, so the caret is drawn here.
    fn render_search(
        &self,
        query: &str,
        found: Option<(usize, Option<usize>)>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let p = theme::colors(cx);
        let count = match found {
            None => String::new(),
            Some((0, _)) => "No match".to_string(),
            Some((total, Some(at))) => format!("{} / {total}", at + 1),
            Some((total, None)) => total.to_string(),
        };
        let missed = matches!(found, Some((0, _)));
        let caret = div().flex_none().w(px(1.5)).h(z(14., cx)).bg(c(p.accent));
        div()
            .h(z(size::TREE_HEAD, cx))
            .flex_none()
            .flex()
            .items_center()
            .px(z(6., cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h(z(26., cx))
                    .flex()
                    .items_center()
                    .gap(z(6., cx))
                    .pl(z(8., cx))
                    .pr(z(2., cx))
                    .rounded(z(7., cx))
                    .bg(c(p.cur))
                    .border_1()
                    .border_color(c(if missed { p.ansi[1] } else { p.line }))
                    .child(
                        Icon::new(IconName::Search)
                            .size(z(13., cx))
                            .flex_none()
                            .text_color(c(p.sub)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .map(|el| {
                                if query.is_empty() {
                                    el.child(caret)
                                        .child(div().text_color(c(p.ignored)).child("Find a name"))
                                } else {
                                    el.child(div().text_color(c(p.ink)).child(query.to_string()))
                                        .child(caret)
                                }
                            }),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(z(11.5, cx))
                            .text_color(c(p.sub))
                            .child(count),
                    )
                    .child(Self::search_button(
                        "search-prev",
                        IconName::ChevronUp,
                        "Previous match",
                        Some(SearchStep::Prev),
                        cx,
                    ))
                    .child(Self::search_button(
                        "search-next",
                        IconName::ChevronDown,
                        "Next match",
                        Some(SearchStep::Next),
                        cx,
                    ))
                    .child(Self::search_button(
                        "search-close",
                        IconName::X,
                        "Close",
                        None,
                        cx,
                    )),
            )
    }
}

/// The last part of a tree path, as a prompt names the item.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// What a copy (`cut` false) or a move put on disk, as Cmd+Z takes it back. A move to where the
/// item already was changed nothing.
fn changed(cut: bool, landed: &[(PathBuf, PathBuf)]) -> Undo {
    if cut {
        Undo::Moved(
            landed
                .iter()
                .filter(|(from, to)| from != to)
                .cloned()
                .collect(),
        )
    } else {
        Undo::Made(landed.iter().map(|(_, to)| to.clone()).collect())
    }
}

/// Moves items to the Trash, given as (tree path, folder on disk, path inside it), one after
/// another; run off the main thread.
async fn trash_all(jobs: Vec<(PathBuf, PathBuf, PathBuf)>) -> Vec<(PathBuf, std::io::Result<()>)> {
    jobs.into_iter()
        .map(|(path, root, rest)| {
            let result = fs::trash(&root, &rest);
            (path, result)
        })
        .collect()
}

/// A folder's listing. With several folders the window's node lists them.
fn list(folders: &Folders, dir: &Path) -> std::io::Result<Vec<subwy_project::fs::DirEntry>> {
    if folders.several() && dir.as_os_str().is_empty() {
        return Ok(folders.entries());
    }
    match folders.mount(dir) {
        Some((root, rest, _)) => fs::list_dir(root, &rest),
        None => Err(std::io::ErrorKind::NotFound.into()),
    }
}

/// The header's icon buttons: square, the icon in `sub`, a hover fill, a tooltip.
const HEADER_BUTTON: f32 = 22.;

pub(crate) fn header_button(
    id: &'static str,
    icon: IconName,
    tooltip: &'static str,
    cx: &App,
) -> Stateful<Div> {
    let p = theme::colors(cx);
    div()
        .id(id)
        .size(z(HEADER_BUTTON, cx))
        .flex()
        .items_center()
        .justify_center()
        .rounded(z(4., cx))
        .text_color(c(p.sub))
        .hover(|style| style.bg(c(p.hover)).text_color(c(p.ink)))
        .tooltip(move |window, cx| Tooltip::new(tooltip).build(window, cx))
        .child(Icon::new(icon).size(z(15., cx)))
}

/// A list that floats `top` below where it is placed, drawn over everything and kept on screen.
/// `corner` is the list's corner that sits there.
fn floating<E: IntoElement>(
    top: f32,
    corner: Anchor,
    list: E,
    cx: &App,
) -> impl IntoElement + use<E> {
    let right = matches!(corner, Anchor::TopRight | Anchor::BottomRight);
    div()
        .absolute()
        .top(z(top, cx))
        .map(|el| if right { el.right_0() } else { el.left_0() })
        .child(
            deferred(
                anchored()
                    .anchor(corner)
                    .snap_to_window_with_margin(px(8.))
                    .child(list),
            )
            .with_priority(1),
        )
}

fn popup_box(cx: &App) -> Div {
    let p = theme::colors(cx);
    div()
        .occlude()
        .min_w(z(180., cx))
        .py(z(4., cx))
        .flex()
        .flex_col()
        .bg(c(p.paper))
        .border_1()
        .border_color(c(p.line))
        .rounded(z(6., cx))
        .text_size(z(size::UI_TEXT, cx))
        .text_color(c(p.ink))
}

/// One line of a floating list; `highlighted` is the one Enter picks.
fn popup_item(
    id: impl Into<ElementId>,
    icon: Option<IconName>,
    label: &'static str,
    highlighted: bool,
    cx: &App,
) -> Stateful<Div> {
    let p = theme::colors(cx);
    div()
        .id(id)
        .mx(z(4., cx))
        .px(z(8., cx))
        .h(z(size::TREE_ROW, cx))
        .flex()
        .items_center()
        .gap(z(8., cx))
        .rounded(z(4., cx))
        .when(highlighted, |el| {
            el.bg(c(p.sel_bg)).text_color(c(p.sel_ink))
        })
        .when(!highlighted, |el| el.hover(|style| style.bg(c(p.hover))))
        .child(
            div()
                .size(z(14., cx))
                .flex_none()
                .children(icon.map(|icon| Icon::new(icon).size(z(14., cx)).text_color(c(p.sub)))),
        )
        .child(label)
}

/// IntelliJ's New popup under the selected row: a file or a folder. Up, Down and Enter pick, as do
/// F and D; Escape closes it.
fn new_popup(
    depth: usize,
    choice: usize,
    panel: WeakEntity<ProjectPanel>,
    cx: &App,
) -> impl IntoElement {
    let p = theme::colors(cx);
    let kinds = [
        (EntryKind::File, IconName::File, "File"),
        (EntryKind::Dir, IconName::Folder, "Directory"),
    ];
    div()
        .absolute()
        .left(z(row_padding(depth) + CHEVRON_SLOT, cx))
        .top_0()
        .child(floating(
            size::TREE_ROW,
            Anchor::TopLeft,
            popup_box(cx)
                .child(
                    div()
                        .px(z(12., cx))
                        .pb(z(4., cx))
                        .text_size(z(11.5, cx))
                        .text_color(c(p.sub))
                        .child("New"),
                )
                .children(
                    kinds
                        .into_iter()
                        .enumerate()
                        .map(|(index, (kind, icon, label))| {
                            let panel = panel.clone();
                            popup_item(
                                ("tree-new-kind", index),
                                Some(icon),
                                label,
                                index == choice,
                                cx,
                            )
                            .on_click(move |_, window, cx| {
                                let _ =
                                    panel.update(cx, |this, cx| this.choose_new(kind, window, cx));
                            })
                        }),
                ),
            cx,
        ))
}

/// A row's right-click menu under its name: the groups, a line between them, each line's key at
/// its right. `choice` is the line Enter picks.
fn context_popup(
    depth: usize,
    entries: &[Vec<Entry>],
    choice: Option<usize>,
    panel: WeakEntity<ProjectPanel>,
    cx: &App,
) -> impl IntoElement {
    let p = theme::colors(cx);
    let mut list = popup_box(cx);
    let mut index = 0;
    for (group, entries) in entries.iter().enumerate() {
        if group > 0 {
            list = list.child(div().my(z(4., cx)).h(px(1.)).bg(c(p.line)));
        }
        for &entry in entries {
            let panel = panel.clone();
            list = list.child(
                popup_item(
                    ("tree-menu", index),
                    entry.icon(),
                    entry.label(),
                    choice == Some(index),
                    cx,
                )
                .children(entry.key().map(|key| {
                    div()
                        .ml_auto()
                        .pl(z(16., cx))
                        .text_size(z(11.5, cx))
                        .text_color(c(p.sub))
                        .child(key)
                }))
                .on_click(move |_, window, cx| {
                    let _ = panel.update(cx, |this, cx| this.run_entry(entry, window, cx));
                }),
            );
            index += 1;
        }
    }
    div()
        .absolute()
        .left(z(row_padding(depth) + CHEVRON_SLOT, cx))
        .top_0()
        .child(floating(size::TREE_ROW, Anchor::TopLeft, list, cx))
}

/// A row's name, its speed search match in semibold (medium barely shows at 13 px).
fn name_label(name: &str, query: Option<&str>, mark: Hsla) -> StyledText {
    let text = StyledText::new(name.to_string());
    match query.and_then(|query| nav::name_match(name, query)) {
        Some(range) => text.with_highlights([(
            range,
            HighlightStyle {
                background_color: Some(mark),
                ..Default::default()
            },
        )]),
        None => text,
    }
}

/// The name input of a tree line and, after it, why the last Enter was refused.
fn edit_field(edit: &EditView, cx: &App) -> impl IntoElement {
    let p = theme::colors(cx);
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .items_center()
        .gap(z(ROW_GAP, cx))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .h(z(EDIT_HEIGHT, cx))
                .overflow_hidden()
                .rounded(z(4., cx))
                .border_1()
                .border_color(c(p.line))
                .bg(c(p.paper))
                // Plain ink even on the selected row, like the speed search field.
                .text_color(c(p.ink))
                // A click inside stays there; the tree's own mouse-down would take the focus back.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    Input::new(&edit.input)
                        .appearance(false)
                        .size_full()
                        .px(z(EDIT_PADDING, cx))
                        .py_0()
                        .text_size(z(size::UI_TEXT, cx))
                        .line_height(z(EDIT_HEIGHT - 2., cx)),
                ),
        )
        .when_some(edit.error.clone(), |el, error| {
            el.child(
                div()
                    .flex_none()
                    .text_size(z(12., cx))
                    .text_color(c(p.sub))
                    .child(error),
            )
        })
}

/// The command line in place of the key hints, for Cmd+N, F5 and F6: what is done (`badge`), in
/// which folder (`place`, when the input holds a name alone), the input, and why the last Enter
/// was refused.
fn command_line(
    edit: &EditView,
    badge: SharedString,
    place: Option<String>,
    cx: &App,
) -> impl IntoElement {
    let p = theme::colors(cx);
    div()
        .h(z(size::KEY_HINTS, cx))
        .flex_none()
        .flex()
        .items_center()
        .gap(z(8., cx))
        .px(z(HINT_PADDING, cx))
        .border_t_1()
        .border_color(c(p.line))
        .bg(c(p.cur))
        .whitespace_nowrap()
        .overflow_hidden()
        .text_size(z(12.5, cx))
        // A click inside stays there; the tree's own mouse-down would end the naming.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .flex_none()
                .h(z(18., cx))
                .px(z(6., cx))
                .flex()
                .items_center()
                .rounded(z(5., cx))
                .bg(c(p.tree_sel))
                .text_size(z(10.5, cx))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(c(p.sub))
                .child(badge),
        )
        .children(place.map(|place| {
            div()
                .flex_none()
                .text_color(c(p.ignored))
                .child(format!("{place}/"))
        }))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .h(z(EDIT_HEIGHT, cx))
                .text_color(c(p.ink))
                .child(
                    Input::new(&edit.input)
                        .appearance(false)
                        .size_full()
                        .px_0()
                        .py_0()
                        .text_size(z(12.5, cx))
                        .line_height(z(EDIT_HEIGHT - 2., cx)),
                ),
        )
        .child(
            div()
                .flex_none()
                .text_size(z(11.5, cx))
                .text_color(c(if edit.error.is_some() {
                    p.ansi[1]
                } else {
                    p.ignored
                }))
                .child(edit.error.clone().unwrap_or_else(|| "↩".into())),
        )
}

impl Render for ProjectPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = theme::colors(cx);
        let focused = self.focus_handle.contains_focused(window, cx);
        let rows = self.tree.rows();
        let selected = self.tree.selected().map(Path::to_path_buf);
        let home = std::env::var_os("HOME").map(PathBuf::from);
        // Where the folders are: one folder's row shows the folder it sits in (IntelliJ), several
        // show their own places under the window's node.
        let places: Vec<(PathBuf, SharedString)> = if self.folders.several() {
            self.folders
                .roots()
                .iter()
                .zip(self.folders.keys())
                .map(|(root, key)| (PathBuf::from(key), tilde(root, home.as_deref()).into()))
                .collect()
        } else {
            let parent = self.folders.first().parent();
            vec![(
                PathBuf::new(),
                parent
                    .map(|parent| tilde(parent, home.as_deref()))
                    .unwrap_or_default()
                    .into(),
            )]
        };
        let selected_row = selected
            .as_ref()
            .and_then(|s| rows.iter().find(|r| &r.path == s))
            .cloned();
        // The way to the selection and to the hovered row, as list lines: the input for a new
        // item is a line of its own, and the lines below it move down one.
        let stops = selected
            .as_deref()
            .map_or_else(Vec::new, |s| tree::route(&rows, s));
        // While a new item is named, the dashed way runs to where it will land instead.
        let new_dir = match self.edit.as_ref().map(|e| &e.target) {
            Some(EditTarget::New { dir, .. }) => Some(dir.clone()),
            _ => None,
        };
        // A drag that ended anywhere leaves no target behind.
        if !cx.has_active_drag() {
            self.drop_target = None;
        }
        let hovered = match &new_dir {
            Some(dir) => tree::route(&rows, dir),
            None => self
                .drop_target
                .as_deref()
                .or(self.hovered.as_deref())
                .map_or_else(Vec::new, |h| tree::route(&rows, h)),
        };
        // How many shown rows the speed search matches, and which of them is selected.
        let found = self
            .search
            .as_deref()
            .filter(|q| !q.is_empty())
            .map(|query| {
                let hits: Vec<&PathBuf> = rows
                    .iter()
                    .filter(|r| nav::name_match(&r.name, query).is_some())
                    .map(|r| &r.path)
                    .collect();
                let at = selected
                    .as_ref()
                    .and_then(|s| hits.iter().position(|h| *h == s));
                (hits.len(), at)
            });
        // The other selected rows, by list line: they wear the selection without the route.
        let chosen = self.tree.selection(&rows);
        // What a drag from a selected row carries: the selection, without the window's folders.
        let chosen_abs: Vec<PathBuf> = tree::outermost(&chosen)
            .iter()
            .filter(|p| !p.as_os_str().is_empty() && !self.folders.is_folder(p))
            .filter_map(|p| self.folders.absolute(p))
            .collect();
        let items = self.items(rows);
        // Each list line's folder, where a drop on it goes.
        let drop_lines: Vec<Option<PathBuf>> = items
            .iter()
            .map(|item| match item {
                Item::Row(row) if row.kind == EntryKind::Dir && !row.loops => {
                    Some(row.path.clone())
                }
                Item::Row(row) => {
                    Some(row.path.parent().map(Path::to_path_buf).unwrap_or_default())
                }
                Item::New { .. } => None,
            })
            .collect();
        let count = items.len();
        let marked: Vec<(usize, usize)> = items
            .iter()
            .enumerate()
            .filter_map(|(line, item)| match item {
                Item::Row(row)
                    if Some(&row.path) != selected.as_ref() && chosen.contains(&row.path) =>
                {
                    Some((row.depth, line))
                }
                _ => None,
            })
            .collect();
        let new_at = items
            .iter()
            .position(|item| matches!(item, Item::New { .. }));
        let to_line = |row: usize| row + usize::from(new_at.is_some_and(|at| row >= at));
        let stops: Vec<usize> = stops.into_iter().map(to_line).collect();
        let mut hovered: Vec<usize> = hovered.into_iter().map(to_line).collect();
        if let (Some(_), Some(at)) = (&new_dir, new_at) {
            hovered.push(at);
        }
        let now = Instant::now();
        self.route
            .aim(selected.as_deref(), &stops, now, !cx.reduce_motion());
        if self.route.moving(now) {
            window.request_animation_frame();
        }
        let on_route = stops.clone();
        let route_layer = {
            let (shape, ping) = (self.route.at(now), self.route.ping(now));
            let scroll = self.scroll.clone();
            canvas(
                |_, _, _| {},
                move |bounds, _, window, cx| {
                    let offset = scroll.0.borrow().base_handle.offset();
                    let look = route::Look {
                        origin: point(bounds.left(), bounds.top() + offset.y),
                        row: z(size::TREE_ROW, cx),
                        column: z(column_x(0), cx),
                        indent: z(size::TREE_INDENT, cx),
                        right: bounds.right() - z(ROW_PADDING_RIGHT, cx),
                        zoom: theme::zoom(cx),
                        lines: p.route.map(c),
                        ring: c(p.ink),
                        paper: c(p.paper),
                        // Without focus the band turns neutral and fainter, like the current line.
                        selection: c(if focused { p.tree_sel } else { p.cur }),
                        hover: c(p.hover),
                    };
                    route::paint_marked(window, bounds, &look, &marked);
                    route::paint(window, bounds, &look, &shape, ping, &stops, &hovered);
                },
            )
            .absolute()
            .size_full()
        };
        let hints = match &self.edit {
            Some(edit) => edit.target.hints(),
            None if self.search.is_some() => SEARCH_HINTS,
            None => hint_keys(selected_row.as_ref()),
        };
        let command = self.edit.as_ref().and_then(|edit| match &edit.target {
            EditTarget::New { dir, kind } => {
                let place = if dir.as_os_str().is_empty() {
                    self.folders
                        .first()
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default()
                } else {
                    dir.display().to_string()
                };
                let badge = if *kind == EntryKind::Dir {
                    "NEW FOLDER"
                } else {
                    "NEW FILE"
                };
                Some((badge.into(), Some(place)))
            }
            EditTarget::Transfer { sources, cut } => {
                let verb = if *cut { "MOVE" } else { "COPY" };
                let badge: SharedString = match sources.len() {
                    1 => verb.into(),
                    n => format!("{verb} {n}").into(),
                };
                Some((badge, None))
            }
            EditTarget::Rename { .. } => None,
        });
        let command = command
            .zip(self.edit.as_ref())
            .map(|((badge, place), edit)| {
                let view = EditView {
                    input: edit.input.clone(),
                    error: edit.error.clone(),
                };
                (view, badge, place)
            });
        div()
            .key_context(PROJECT_PANEL)
            .track_focus(&self.focus_handle)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.search = None;
                    this.edit = None;
                    this.menu = None;
                    this.focus_handle.focus(window, cx)
                }),
            )
            .on_key_down(cx.listener(Self::key_down))
            .on_action(cx.listener(|this, _: &SelectPrev, _, cx| this.up_down(NavKey::Up, cx)))
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.up_down(NavKey::Down, cx)))
            .on_action(cx.listener(|this, _: &ExtendPrev, _, cx| this.extend(NavKey::Up, cx)))
            .on_action(cx.listener(|this, _: &ExtendNext, _, cx| this.extend(NavKey::Down, cx)))
            .on_action(cx.listener(|this, _: &Expand, _, cx| this.apply(NavKey::Right, cx)))
            .on_action(cx.listener(|this, _: &Collapse, _, cx| this.apply(NavKey::Left, cx)))
            .on_action(cx.listener(|this, _: &SelectFirst, _, cx| this.apply(NavKey::First, cx)))
            .on_action(cx.listener(|this, _: &SelectLast, _, cx| this.apply(NavKey::Last, cx)))
            .on_action(cx.listener(|this, _: &PageUp, _, cx| {
                let page = this.page_rows(cx);
                this.apply(NavKey::PageUp(page), cx)
            }))
            .on_action(cx.listener(|this, _: &PageDown, _, cx| {
                let page = this.page_rows(cx);
                this.apply(NavKey::PageDown(page), cx)
            }))
            .on_action(cx.listener(|this, _: &Open, window, cx| {
                if let Some(Menu::New { choice, .. }) = &this.menu {
                    let kind = if *choice == 0 {
                        EntryKind::File
                    } else {
                        EntryKind::Dir
                    };
                    this.choose_new(kind, window, cx);
                    return;
                }
                if let Some(Menu::Context {
                    entries, choice, ..
                }) = &this.menu
                {
                    if let Some(&entry) = choice.and_then(|at| entries.iter().flatten().nth(at)) {
                        this.run_entry(entry, window, cx);
                    }
                    return;
                }
                // Enter in a name input reaches here after the input took it.
                if this.edit.is_none() {
                    this.search = None;
                    this.open_selected(false, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &FocusEditor, window, cx| this.escape(window, cx)))
            .on_action(cx.listener(|this, _: &PassThrough, _, cx| {
                if this.menu.is_some() {
                    return;
                }
                if this.edit.is_some() {
                    // Space is bound in the tree; let it go on and type into the name.
                    cx.propagate();
                } else {
                    this.open_selected(true, cx)
                }
            }))
            .on_action(
                cx.listener(|this, _: &SpeedSearch, window, cx| this.start_search(window, cx)),
            )
            .on_action(cx.listener(|this, _: &NewFile, window, cx| this.new_file(window, cx)))
            .on_action(cx.listener(|this, _: &Rename, window, cx| this.rename(window, cx)))
            .on_action(cx.listener(|this, _: &Trash, window, cx| this.trash(window, cx)))
            .on_action(cx.listener(|this, _: &CutItems, _, cx| this.put_on_clipboard(true, cx)))
            .on_action(cx.listener(|this, _: &CopyItems, _, cx| this.put_on_clipboard(false, cx)))
            .on_action(cx.listener(|this, _: &PasteItems, window, cx| this.paste(window, cx)))
            .on_action(cx.listener(|this, _: &CopyTo, window, cx| this.transfer(false, window, cx)))
            .on_action(cx.listener(|this, _: &MoveTo, window, cx| this.transfer(true, window, cx)))
            .on_action(cx.listener(|this, _: &OpenInTerminal, _, cx| this.open_in_terminal(cx)))
            .on_action(cx.listener(|this, _: &ExpandAll, _, cx| this.expand_all(cx)))
            .on_action(cx.listener(|this, _: &CollapseAll, _, cx| this.collapse_all(cx)))
            .on_action(cx.listener(|this, _: &UndoChange, window, cx| this.undo(window, cx)))
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<ResizeDrag>, _, cx| {
                    // The pointer holds the middle of the gap, half a gap past the tree's edge.
                    let zoom = theme::zoom(cx);
                    let width = (event.event.position.x - event.bounds.left()).as_f32() / zoom
                        - size::ISLAND_GAP / 2.;
                    this.width = clamp_width(width);
                    cx.notify();
                }),
            )
            .relative()
            // Clamped here too: the width may come back from a saved session.
            .w(z(clamp_width(self.width), cx))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            // An island, like the editor and the terminal: paper with rounded corners on the desk.
            .rounded(z(size::ISLAND_RADIUS, cx))
            .overflow_hidden()
            .bg(c(p.paper))
            .text_size(z(size::UI_TEXT, cx))
            .child(match self.search.clone() {
                Some(query) => self.render_search(&query, found, cx).into_any_element(),
                None => self.render_header(cx).into_any_element(),
            })
            .child(
                // The route and the row backgrounds are painted under the list's rows.
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .on_drag_move(cx.listener({
                        let lines = drop_lines.clone();
                        move |this, event: &DragMoveEvent<DraggedItems>, _, cx| {
                            let target =
                                this.drop_line(event.bounds, event.event.position, &lines, cx);
                            let dragged = event.drag(cx).paths.clone();
                            this.drag_over(target, &dragged, cx);
                        }
                    }))
                    .on_drag_move(cx.listener(
                        move |this, event: &DragMoveEvent<ExternalPaths>, _, cx| {
                            let target =
                                this.drop_line(event.bounds, event.event.position, &drop_lines, cx);
                            this.drag_over(target, &[], cx);
                        },
                    ))
                    .on_drop(cx.listener(|this, items: &DraggedItems, window, cx| {
                        let cut = !window.modifiers().alt;
                        this.drop_items(items.paths.clone(), cut, window, cx);
                    }))
                    .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                        this.drop_items(paths.paths().to_vec(), false, window, cx);
                    }))
                    .child(route_layer)
                    .child(
                        uniform_list("project-tree", count, {
                            let selected = selected.clone();
                            // Matched names are marked and the rest step back, once something matches.
                            let query = self
                                .search
                                .clone()
                                .filter(|_| found.is_some_and(|(total, _)| total > 0));
                            let edit = self.edit.as_ref().map(|edit| EditView {
                                input: edit.input.clone(),
                                error: edit.error.clone(),
                            });
                            let renaming = match self.edit.as_ref().map(|edit| &edit.target) {
                                Some(EditTarget::Rename { path }) => Some(path.clone()),
                                _ => None,
                            };
                            let panel = cx.weak_entity();
                            let new_choice = match &self.menu {
                                Some(Menu::New { choice, .. }) => Some(*choice),
                                _ => None,
                            };
                            let folders = self.folders.clone();
                            let context_menu = match &self.menu {
                                Some(Menu::Context {
                                    path,
                                    entries,
                                    choice,
                                }) => Some((path.clone(), entries.clone(), *choice)),
                                _ => None,
                            };
                            move |range, _, cx| {
                                items[range.clone()]
                                    .iter()
                                    .zip(range)
                                    .map(|(item, line)| (item, on_route.contains(&line)))
                                    .map(|(item, route)| match (item, &edit) {
                                        (Item::Row(row), Some(edit))
                                            if renaming.as_deref() == Some(row.path.as_path()) =>
                                        {
                                            Self::render_row(
                                                row,
                                                Some(focused),
                                                route,
                                                None,
                                                Some(edit),
                                                places
                                                    .iter()
                                                    .find(|(path, _)| *path == row.path)
                                                    .map(|(_, place)| place),
                                                cx,
                                            )
                                            .into_any_element()
                                        }
                                        (Item::Row(row), _) => {
                                            let (path, kind) = (row.path.clone(), row.kind);
                                            let is_selected =
                                                selected.as_deref() == Some(row.path.as_path());
                                            let is_chosen =
                                                is_selected || chosen.contains(&row.path);
                                            // A drag takes the selection when it starts on it,
                                            // else this row; the window's folders stay put.
                                            let own = row.path.as_os_str().is_empty()
                                                || folders.is_folder(&row.path);
                                            let dragged = if own {
                                                None
                                            } else if is_chosen && !chosen_abs.is_empty() {
                                                Some(chosen_abs.clone())
                                            } else {
                                                folders.absolute(&row.path).map(|abs| vec![abs])
                                            };
                                            let popup =
                                                new_choice.filter(|_| is_selected).map(|choice| {
                                                    new_popup(row.depth, choice, panel.clone(), cx)
                                                        .into_any_element()
                                                });
                                            let context_popup = context_menu
                                                .as_ref()
                                                .filter(|(path, ..)| *path == row.path)
                                                .map(|(_, entries, choice)| {
                                                    context_popup(
                                                        row.depth,
                                                        entries,
                                                        *choice,
                                                        panel.clone(),
                                                        cx,
                                                    )
                                                    .into_any_element()
                                                });
                                            let panel = panel.clone();
                                            Self::render_row(
                                                row,
                                                is_chosen.then_some(focused),
                                                route,
                                                query.as_deref(),
                                                None,
                                                places
                                                    .iter()
                                                    .find(|(path, _)| *path == row.path)
                                                    .map(|(_, place)| place),
                                                cx,
                                            )
                                            .children(popup)
                                            .children(context_popup)
                                            .when_some(dragged, |el, paths| {
                                                el.on_drag(
                                                    DraggedItems { paths },
                                                    |items, grab, _, cx| {
                                                        cx.new(|_| {
                                                            DragPreview::new(&items.paths, grab)
                                                        })
                                                    },
                                                )
                                            })
                                            .on_mouse_down(MouseButton::Right, {
                                                let (panel, path) =
                                                    (panel.clone(), row.path.clone());
                                                move |_, window, cx| {
                                                    let _ = panel.update(cx, |this, cx| {
                                                        this.open_context_menu(
                                                            path.clone(),
                                                            window,
                                                            cx,
                                                        )
                                                    });
                                                }
                                            })
                                            .on_hover({
                                                let (panel, path) =
                                                    (panel.clone(), row.path.clone());
                                                move |&hovered, _, cx| {
                                                    let _ = panel.update(cx, |this, cx| {
                                                        this.hover_row(&path, hovered, cx)
                                                    });
                                                }
                                            })
                                            .on_click(move |event, _, cx| {
                                                let count = event.click_count();
                                                let modifiers = event.modifiers();
                                                let _ = panel.update(cx, |this, cx| {
                                                    this.click_row(
                                                        path.clone(),
                                                        kind,
                                                        count,
                                                        modifiers,
                                                        cx,
                                                    )
                                                });
                                            })
                                            .into_any_element()
                                        }
                                        (
                                            Item::New {
                                                depth,
                                                name,
                                                folder,
                                            },
                                            Some(edit),
                                        ) => Self::render_new(
                                            *depth,
                                            name,
                                            *folder,
                                            edit.error.is_some(),
                                            cx,
                                        )
                                        .into_any_element(),
                                        (Item::New { .. }, None) => div().into_any_element(),
                                    })
                                    .collect()
                            }
                        })
                        .track_scroll(&self.scroll)
                        .size_full(),
                    ),
            )
            // Naming a new item takes the hint line for its command line, even with hints hidden.
            .map(|el| match command {
                Some((edit, badge, place)) => el.child(command_line(&edit, badge, place, cx)),
                None => el.when(self.show_key_hints, |el| el.child(key_hints(hints, cx))),
            })
            .child(crate::workspace::island_corners(cx))
    }
}

/// The keys the hint line offers while a speed search is on.
const SEARCH_HINTS: &[(&str, &str)] = &[("↑↓", "Matches"), ("↩", "Open"), ("Esc", "Close")];

/// The keys the hint line offers for the selection, as (key, label).
fn hint_keys(selected: Option<&Row>) -> &'static [(&'static str, &'static str)] {
    match selected {
        Some(row) if row.kind == EntryKind::Dir && row.expanded => &[
            ("←", "Collapse"),
            ("→", "First child"),
            ("⌘⇧C", "Copy path"),
        ],
        // A folder that leads back up never opens, so offering Expand would be a lie.
        Some(row) if row.kind == EntryKind::Dir && row.loops => {
            &[("←", "Parent"), ("⌘⇧C", "Copy path")]
        }
        Some(row) if row.path.as_os_str().is_empty() => &[("→", "Expand"), ("⌘⇧C", "Copy path")],
        Some(row) if row.kind == EntryKind::Dir => {
            &[("→", "Expand"), ("←", "Parent"), ("⌘⇧C", "Copy path")]
        }
        _ => &[
            ("↩", "Open"),
            ("Space", "Pass through"),
            ("⌘⇧C", "Copy path"),
        ],
    }
}

pub(crate) fn key_hints(
    keys: &'static [(&'static str, &'static str)],
    cx: &App,
) -> impl IntoElement {
    let p = theme::colors(cx);
    // Tighter than the mockup (12 / 12 / 5 / 5): at the default width the file hints need every
    // point. A hint that still does not fit wraps onto a hidden second line instead of being cut.
    div()
        .h(z(size::KEY_HINTS, cx))
        .flex_none()
        .flex()
        .flex_wrap()
        .gap_x(z(HINT_GAP, cx))
        .px(z(HINT_PADDING, cx))
        .overflow_hidden()
        .whitespace_nowrap()
        .border_t_1()
        .border_color(c(p.line))
        .text_size(z(11.5, cx))
        .text_color(c(p.sub))
        .children(keys.iter().map(|&(key, label)| {
            div()
                .h_full()
                .flex()
                .items_center()
                .gap(z(HINT_KEY_GAP, cx))
                .child(
                    div()
                        .h(z(18., cx))
                        .flex()
                        .items_center()
                        .px(z(KEY_CAP_PADDING, cx))
                        .rounded(z(5., cx))
                        .border_1()
                        .border_color(c(p.line))
                        .bg(c(p.cur))
                        .text_color(c(p.sub))
                        .text_size(z(10.5, cx))
                        .child(key),
                )
                .child(label)
        }))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use subwy_project::{fs::EntryKind, tree::Row};

    use super::{
        CHEVRON_SLOT, Entry, clamp_width, column_x, context_entries, hint_keys, name_selection,
        row_padding, rows_per_page, tilde,
    };

    fn folder(expanded: bool, loops: bool) -> Row {
        Row {
            path: PathBuf::from("self"),
            name: "self".into(),
            depth: 0,
            kind: EntryKind::Dir,
            expanded,
            ignored: false,
            loops,
        }
    }

    fn labels(row: Option<&Row>) -> Vec<&'static str> {
        hint_keys(row).iter().map(|&(_, label)| label).collect()
    }

    #[test]
    fn a_rename_selects_the_name_up_to_its_extension() {
        assert_eq!(name_selection("paths.md", EntryKind::File), 0..5);
        assert_eq!(name_selection("archive.tar.gz", EntryKind::File), 0..11);
        assert_eq!(name_selection(".gitignore", EntryKind::File), 0..10);
        assert_eq!(name_selection("Makefile", EntryKind::File), 0..8);
        assert_eq!(name_selection("v1.2", EntryKind::Dir), 0..4);
    }

    #[test]
    fn a_folder_that_loops_offers_no_expand() {
        assert_eq!(
            labels(Some(&folder(false, false))),
            ["Expand", "Parent", "Copy path"]
        );
        assert_eq!(labels(Some(&folder(false, true))), ["Parent", "Copy path"]);
        assert_eq!(labels(None), ["Open", "Pass through", "Copy path"]);
    }

    #[test]
    fn the_open_folder_offers_no_parent() {
        let root = Row {
            path: PathBuf::new(),
            ..folder(false, false)
        };
        assert_eq!(labels(Some(&root)), ["Expand", "Copy path"]);
        let open = Row {
            expanded: true,
            ..root
        };
        assert_eq!(
            labels(Some(&open)),
            ["Collapse", "First child", "Copy path"]
        );
    }

    #[test]
    fn a_folder_of_the_window_is_not_renamed_or_trashed_from_its_menu() {
        let flat = |own, several| -> Vec<Entry> {
            context_entries(own, own, several)
                .into_iter()
                .flatten()
                .collect()
        };
        let item = flat(false, false);
        assert!(item.contains(&Entry::Rename) && item.contains(&Entry::Trash));
        assert!(!item.contains(&Entry::RemoveFromWindow));
        let folder = flat(true, false);
        assert!(!folder.contains(&Entry::Rename) && !folder.contains(&Entry::Trash));
        assert!(!folder.contains(&Entry::Cut) && folder.contains(&Entry::Paste));
        assert!(!folder.contains(&Entry::RemoveFromWindow));
        // One of several folders leaves the window instead.
        assert!(flat(true, true).contains(&Entry::RemoveFromWindow));
        assert!(!flat(false, true).contains(&Entry::RemoveFromWindow));
        // Only a folder is searched.
        let file: Vec<Entry> = context_entries(false, false, false)
            .into_iter()
            .flatten()
            .collect();
        assert!(!file.contains(&Entry::FindInFolder));
        assert!(flat(true, false).contains(&Entry::FindInFolder));
    }

    #[test]
    fn columns_follow_the_design_formula() {
        // The design puts the column of depth L at x = 16 + 14L.
        for level in 0..5 {
            assert_eq!(column_x(level), 16. + 14. * level as f32);
        }
    }

    #[test]
    fn a_folder_chevron_sits_on_its_column() {
        for depth in 0..5 {
            assert_eq!(row_padding(depth) + CHEVRON_SLOT / 2., column_x(depth));
        }
    }

    #[test]
    fn the_home_folder_shows_as_a_tilde() {
        let home = Some(Path::new("/Users/you"));
        assert_eq!(
            tilde(Path::new("/Users/you/Desktop/work"), home),
            "~/Desktop/work"
        );
        assert_eq!(tilde(Path::new("/Users/you"), home), "~");
        // Only whole folder names match: `/Users/younger` is not under `/Users/you`.
        assert_eq!(tilde(Path::new("/Users/younger"), home), "/Users/younger");
        assert_eq!(tilde(Path::new("/opt/work"), home), "/opt/work");
        assert_eq!(tilde(Path::new("/opt/work"), None), "/opt/work");
    }

    #[test]
    fn a_page_is_the_rows_that_fit_whole() {
        assert_eq!(rows_per_page(220., 22.), 10);
        assert_eq!(rows_per_page(230., 22.), 10);
        // Zoom scales both, so the count stays.
        assert_eq!(rows_per_page(220. * 1.5, 22. * 1.5), 10);
        // Before the first layout the list has no height; a page is still one row.
        assert_eq!(rows_per_page(0., 22.), 1);
    }

    #[test]
    fn the_tree_width_stays_between_its_limits() {
        assert_eq!(clamp_width(120.), 180.);
        assert_eq!(clamp_width(300.), 300.);
        assert_eq!(clamp_width(900.), 480.);
    }
}
