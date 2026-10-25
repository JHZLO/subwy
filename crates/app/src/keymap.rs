//! Every key binding in subwy, in one table (IntelliJ keys; the design doc's keymap section).

use gpui_kit::*;
use subwy_project::settings::{Binding, KeyOverride};

/// Anywhere in the window.
pub const WORKSPACE: &str = "Workspace";
/// The folder tree has focus.
pub const PROJECT_PANEL: &str = "ProjectPanel";

actions!(
    workspace,
    [
        OpenFolder,
        FocusTree,
        CopyPath,
        CopyRelativePath,
        CopyFileName,
        Save,
        CloseTab,
        PrevTab,
        NextTab,
        TogglePreview,
        ZoomIn,
        ZoomOut,
        ZoomReset,
        Quit,
        ToggleFinder,
        RevealInFinder,
        ToggleSearch,
        NewWindow,
        ToggleTerminal,
        OpenSettings,
    ]
);

actions!(
    project_panel,
    [
        SelectPrev,
        SelectNext,
        ExtendPrev,
        ExtendNext,
        Expand,
        Collapse,
        SelectFirst,
        SelectLast,
        PageUp,
        PageDown,
        Open,
        PassThrough,
        SpeedSearch,
        FocusEditor,
        NewFile,
        Rename,
        Trash,
        CutItems,
        CopyItems,
        PasteItems,
        CopyTo,
        MoveTo,
        OpenInTerminal,
        ExpandAll,
        CollapseAll,
        UndoChange,
    ]
);

/// The Settings dialog has focus.
pub const SETTINGS: &str = "Settings";

actions!(settings, [CloseSettings]);

/// The terminal panel has focus.
pub const TERMINAL: &str = "Terminal";

actions!(
    terminal,
    [
        CopySelection,
        Paste,
        SendTab,
        SendBackTab,
        NewTerminal,
        CloseTerminal,
        PrevTerminal,
        NextTerminal,
    ]
);

/// Around the editor of a Markdown tab.
pub const MARKDOWN: &str = "Markdown";
/// gpui-kit's editor ("Input") inside a Markdown tab.
const MARKDOWN_EDITOR: &str = "Markdown > Input";

actions!(
    markdown,
    [
        Newline,
        IndentItem,
        OutdentItem,
        DeleteIndent,
        ToggleBold,
        ToggleItalic
    ]
);

/// Binds the user's extra keys from the settings file. Returns the first one that could not be bound.
pub fn bind_overrides(overrides: &[KeyOverride], cx: &mut App) -> Option<String> {
    let mut problem = None;
    let mut bindings = Vec::new();
    for entry in overrides {
        let action = cx
            .build_action(&entry.action, None)
            .map_err(|err| format!("unknown action {}: {err}", entry.action))
            // An unbind entry takes the key away from the action rather than running it.
            .map(|action| match entry.unbind {
                true => Box::new(Unbind(action.name().into())) as Box<dyn Action>,
                false => action,
            });
        let binding = action.and_then(|action| {
            let context = match &entry.context {
                Some(source) => Some(std::rc::Rc::new(
                    KeyBindingContextPredicate::parse(source)
                        .map_err(|err| format!("context {source}: {err}"))?,
                )),
                None => None,
            };
            KeyBinding::load(
                &entry.key,
                action,
                context,
                false,
                None,
                cx.keyboard_mapper().as_ref(),
            )
            .map_err(|err| format!("key {}: {err}", entry.key))
        });
        match binding {
            Ok(binding) => bindings.push(binding),
            Err(err) => {
                problem.get_or_insert(format!("settings.json keymap: {err}"));
            }
        }
    }
    cx.bind_keys(bindings);
    problem
}

/// One of subwy's own keys: the key, what it does, and where it works.
type Own = (&'static str, Box<dyn Action>, Option<&'static str>);

fn own(key: &'static str, action: impl Action, context: Option<&'static str>) -> Own {
    (key, Box::new(action), context)
}

/// Every key subwy binds itself, in binding order (a later binding wins a tie).
fn own_keys() -> Vec<Own> {
    let ws = Some(WORKSPACE);
    let panel = Some(PROJECT_PANEL);
    vec![
        own("cmd-q", Quit, None),
        // macOS's Settings key; in any window, the empty one too. Again, it closes the dialog.
        own("cmd-,", OpenSettings, None),
        own("escape", CloseSettings, Some(SETTINGS)),
        own("cmd-w", CloseSettings, Some(SETTINGS)),
        own("cmd-o", OpenFolder, ws),
        own("cmd-shift-n", NewWindow, ws),
        own("cmd-1", FocusTree, ws),
        own("cmd-shift-c", CopyPath, ws),
        own("cmd-alt-shift-c", CopyRelativePath, ws),
        own("cmd-s", Save, ws),
        own("cmd-w", CloseTab, ws),
        own("cmd-shift-[", PrevTab, ws),
        own("cmd-shift-]", NextTab, ws),
        own("cmd-shift-v", TogglePreview, ws),
        own("cmd-=", ZoomIn, ws),
        own("cmd-+", ZoomIn, ws),
        own("cmd--", ZoomOut, ws),
        own("cmd-0", ZoomReset, ws),
        own("up", SelectPrev, panel),
        own("down", SelectNext, panel),
        own("shift-up", ExtendPrev, panel),
        own("shift-down", ExtendNext, panel),
        own("right", Expand, panel),
        own("left", Collapse, panel),
        own("home", SelectFirst, panel),
        own("end", SelectLast, panel),
        own("pageup", PageUp, panel),
        own("pagedown", PageDown, panel),
        own("enter", Open, panel),
        own("space", PassThrough, panel),
        own("cmd-f", SpeedSearch, panel),
        own("escape", FocusEditor, panel),
        own("cmd-shift-o", ToggleFinder, ws),
        own("cmd-shift-f", ToggleSearch, ws),
        // gpui-kit binds cmd-shift-f to replace inside its editor ("Input"), which would win there; the
        // folder search takes it, and replace moves to cmd-r as in IntelliJ.
        own("cmd-shift-f", ToggleSearch, Some("Input")),
        own("cmd-r", gpui_kit::component::input::Replace, Some("Input")),
        own("cmd-alt-r", RevealInFinder, ws),
        own("cmd-n", NewFile, panel),
        own("shift-f6", Rename, panel),
        own("cmd-backspace", Trash, panel),
        own("cmd-x", CutItems, panel),
        own("cmd-c", CopyItems, panel),
        own("cmd-v", PasteItems, panel),
        own("f5", CopyTo, panel),
        own("f6", MoveTo, panel),
        // IntelliJ's Expand All and Collapse All. Cmd+Shift+= also reads as Cmd++, which zooms in
        // elsewhere; in the tree it expands, and Cmd+= still zooms.
        own("cmd-shift-=", ExpandAll, panel),
        own("cmd-+", ExpandAll, panel),
        own("cmd-shift--", CollapseAll, panel),
        own("cmd-_", CollapseAll, panel),
        // A name being typed has its own Cmd+Z: its input is deeper and wins.
        own("cmd-z", UndoChange, panel),
        // The recent folders' map on the empty window.
        own("up", SelectPrev, Some("Workspace && empty")),
        own("down", SelectNext, Some("Workspace && empty")),
        own("left", Collapse, Some("Workspace && empty")),
        own("right", Expand, Some("Workspace && empty")),
        own("enter", Open, Some("Workspace && empty")),
        // Markdown editing keys (Phase 2) go below this line, in their own block.
        // gpui-kit binds enter and tab on its editor element ("Input"), deeper than any container.
        // These match that same element, so they tie on depth and, added later, run first. When a
        // line is not a list item the handler propagates and GPUI moves on to gpui-kit's binding.
        own("enter", Newline, Some(MARKDOWN_EDITOR)),
        own("tab", IndentItem, Some(MARKDOWN_EDITOR)),
        own("shift-tab", OutdentItem, Some(MARKDOWN_EDITOR)),
        own("backspace", DeleteIndent, Some(MARKDOWN_EDITOR)),
        own("cmd-b", ToggleBold, Some(MARKDOWN_EDITOR)),
        own("cmd-i", ToggleItalic, Some(MARKDOWN_EDITOR)),
        // The terminal (IntelliJ's Alt+F12). Keys without a binding go to the shell.
        own("alt-f12", ToggleTerminal, ws),
        own("cmd-c", CopySelection, Some(TERMINAL)),
        own("cmd-v", Paste, Some(TERMINAL)),
        // gpui-kit's root moves focus with tab and shift-tab; in the terminal they are the shell's.
        own("tab", SendTab, Some(TERMINAL)),
        own("shift-tab", SendBackTab, Some(TERMINAL)),
        // More shells, as IntelliJ's terminal tabs: Cmd+T anywhere, the rest inside the terminal,
        // where they win over the editor's own Cmd+W and tab keys.
        own("cmd-t", NewTerminal, ws),
        own("cmd-w", CloseTerminal, Some(TERMINAL)),
        own("cmd-shift-[", PrevTerminal, Some(TERMINAL)),
        own("cmd-shift-]", NextTerminal, Some(TERMINAL)),
    ]
}

/// subwy's own keys as the Settings dialog's keymap lists them.
pub fn defaults() -> Vec<Binding> {
    own_keys()
        .into_iter()
        .map(|(key, action, context)| Binding {
            key: key.to_string(),
            action: action.name().to_string(),
            context: context.map(str::to_string),
        })
        .collect()
}

/// gpui-kit's own keys (its editor's), as they stood before subwy bound its own, so the keymap
/// can be built again from the start.
struct KitKeys(Vec<KeyBinding>);

impl Global for KitKeys {}

/// Binds every key: gpui-kit's, subwy's own, then the user's from the settings, which win. Called
/// once at start and again when the Settings dialog changes the keymap.
pub fn bind_all(cx: &mut App) -> Option<String> {
    let kit = match cx.try_global::<KitKeys>() {
        Some(kit) => kit.0.clone(),
        None => {
            let kit: Vec<KeyBinding> = cx.key_bindings().borrow().bindings().cloned().collect();
            cx.set_global(KitKeys(kit.clone()));
            kit
        }
    };
    cx.clear_key_bindings();
    cx.bind_keys(kit);
    init(cx);
    let overrides = crate::settings(cx).keymap.clone();
    bind_overrides(&overrides, cx)
}

fn init(cx: &mut App) {
    let bindings: Vec<KeyBinding> = own_keys()
        .into_iter()
        .map(|(key, action, context)| {
            let context = context.map(|source| {
                std::rc::Rc::new(
                    KeyBindingContextPredicate::parse(source).expect("subwy's own key context"),
                )
            });
            KeyBinding::load(key, action, context, false, None, &DummyKeyboardMapper)
                .expect("subwy's own key")
        })
        .collect();
    cx.bind_keys(bindings);
}
