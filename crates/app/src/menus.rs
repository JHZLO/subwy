//! The macOS menu bar. The menus show each action's key binding from keymap.rs.

use gpui_kit::component::input::{Copy, Cut, Paste, Redo, SelectAll, Undo};
use gpui_kit::*;

use crate::keymap::{
    CloseTab, CopyFileName, CopyPath, CopyRelativePath, FocusTree, NewWindow, NextTab, OpenFolder,
    OpenSettings, PrevTab, Quit, RevealInFinder, Save, ShowLocalHistory, TogglePreview, ZoomIn,
    ZoomOut, ZoomReset,
};

fn menu(name: &str, items: Vec<MenuItem>) -> Menu {
    Menu {
        name: name.to_string().into(),
        items,
        disabled: false,
    }
}

pub fn init(cx: &mut App) {
    cx.set_menus(vec![
        menu(
            "subwy",
            vec![
                MenuItem::action("Settings…", OpenSettings),
                MenuItem::separator(),
                MenuItem::action("Quit subwy", Quit),
            ],
        ),
        menu(
            "File",
            vec![
                MenuItem::action("New Window", NewWindow),
                MenuItem::action("Open Folder…", OpenFolder),
                MenuItem::separator(),
                MenuItem::action("Save", Save),
                MenuItem::action("Close Tab", CloseTab),
                MenuItem::action("Local History…", ShowLocalHistory),
                MenuItem::separator(),
                MenuItem::action("Reveal in Finder", RevealInFinder),
            ],
        ),
        menu(
            "Edit",
            vec![
                MenuItem::os_action("Undo", Undo, OsAction::Undo),
                MenuItem::os_action("Redo", Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action("Cut", Cut, OsAction::Cut),
                MenuItem::os_action("Copy", Copy, OsAction::Copy),
                MenuItem::os_action("Paste", Paste, OsAction::Paste),
                MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
                MenuItem::separator(),
                MenuItem::action("Copy Path", CopyPath),
                MenuItem::action("Copy Relative Path", CopyRelativePath),
                MenuItem::action("Copy File Name", CopyFileName),
            ],
        ),
        menu(
            "View",
            vec![
                MenuItem::action("Focus Tree", FocusTree),
                MenuItem::action("Toggle Preview", TogglePreview),
                MenuItem::separator(),
                MenuItem::action("Previous Tab", PrevTab),
                MenuItem::action("Next Tab", NextTab),
                MenuItem::separator(),
                MenuItem::action("Zoom In", ZoomIn),
                MenuItem::action("Zoom Out", ZoomOut),
                MenuItem::action("Actual Size", ZoomReset),
            ],
        ),
        // macOS lists the open windows in the menu named Window.
        menu("Window", vec![]),
    ]);
}
