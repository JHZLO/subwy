//! The file finder (Cmd+Shift+O).

use std::path::PathBuf;

use gpui_kit::{
    component::{Theme, ThemeMode},
    *,
};

use crate::check::{Check, fixture};

/// The center of the finder's result row `row` at 100% zoom, in a 1280-wide window: below the
/// title bar (38), the gap (8), the border (1), the query (34), its line (1) and the list padding (4).
fn row_center(row: usize) -> Point<Pixels> {
    point(
        px(640.),
        px(38. + 8. + 1. + 34. + 1. + 4. + 44. * row as f32 + 22.),
    )
}

pub fn run(check: &mut Check) {
    let fixture = fixture();
    let window = check.open(Some(fixture.path()));

    // An empty query lists the files in path order; gitignored target/ is left out.
    check.keys(window, "cmd-shift-o");
    check.shot(window, "finder-open");

    // A few letters of a nested file pick it; Enter opens it and the editor takes the keys.
    check.keys(window, "g p a t h");
    check.shot(window, "finder-query");
    check.keys(window, "enter");
    let active = check.workspace(window, |w, cx| w.active_file(cx));
    check.expect_eq(
        "finder opens docs/guide/paths.md",
        active,
        Some(PathBuf::from("docs/guide/paths.md")),
    );
    check.keys(window, "x");
    let text = check.workspace(window, |w, cx| w.active_text(cx));
    check.expect_eq(
        "after the finder the editor has focus",
        text.is_some_and(|t| t.starts_with("x# Copying paths")),
        true,
    );
    check.keys(window, "cmd-z");

    // Escape closes without opening anything and gives focus back to the editor.
    check.keys(window, "cmd-shift-o n o t e s escape x");
    let active = check.workspace(window, |w, cx| w.active_file(cx));
    check.expect_eq(
        "escape opens nothing",
        active,
        Some(PathBuf::from("docs/guide/paths.md")),
    );
    let text = check.workspace(window, |w, cx| w.active_text(cx));
    check.expect_eq(
        "escape gives focus back to the editor",
        text.is_some_and(|t| t.starts_with("x# Copying paths")),
        true,
    );
    check.keys(window, "cmd-z");

    // Opened from the tree, Cmd+Shift+O closes it again and the tree keeps walking. The tree
    // already follows the file the finder opened, so Down lands on the row after it.
    check.keys(window, "cmd-1 down");
    let before = check.workspace(window, |w, cx| w.tree_selection(cx));
    check.expect_eq(
        "the tree walks on from the opened file",
        before,
        Some(PathBuf::from("docs/notes.md")),
    );
    check.keys(window, "cmd-shift-o r e a d cmd-shift-o down");
    let selected = check.workspace(window, |w, cx| w.tree_selection(cx));
    check.expect_eq(
        "cmd-shift-o gives focus back to the tree",
        selected,
        Some(PathBuf::from("src")),
    );
    let active = check.workspace(window, |w, cx| w.active_file(cx));
    check.expect_eq(
        "closing with cmd-shift-o opens nothing",
        active,
        Some(PathBuf::from("docs/guide/paths.md")),
    );

    // A click on a result opens it: the second file in path order.
    check.keys(window, "cmd-shift-o");
    check
        .cx
        .simulate_click(window, row_center(1), Modifiers::default());
    let active = check.workspace(window, |w, cx| w.active_file(cx));
    check.expect_eq(
        "a click opens docs/guide/keys.md",
        active,
        Some(PathBuf::from("docs/guide/keys.md")),
    );

    // IntelliJ's double Shift opens it too.
    shift_tap(check, window, None);
    shift_tap(check, window, None);
    let open = check.workspace(window, |w, _| w.finder_open());
    check.expect_eq("double shift opens the finder", open, true);
    check.keys(window, "escape");

    // Shift held through a click (extending a selection) is no tap.
    let editor = point(px(800.), px(300.));
    shift_tap(check, window, Some(editor));
    shift_tap(check, window, Some(editor));
    let open = check.workspace(window, |w, _| w.finder_open());
    check.expect_eq("two shift-clicks leave the finder shut", open, false);

    // On light paper and zoomed: every size scales.
    check
        .cx
        .update(|cx| Theme::change(ThemeMode::Light, None, cx));
    check.keys(window, "cmd-= cmd-shift-o d m");
    check.shot(window, "finder-light-zoomed");
    check.keys(window, "escape cmd-0");
    check
        .cx
        .update(|cx| Theme::change(ThemeMode::Dark, None, cx));
}

/// Shift down and up with no other key, clicking at `click` while it is down.
fn shift_tap(check: &mut Check, window: AnyWindowHandle, click: Option<Point<Pixels>>) {
    let shift = Modifiers {
        shift: true,
        ..Modifiers::default()
    };
    check.cx.simulate_event(
        window,
        ModifiersChangedEvent {
            modifiers: shift,
            ..ModifiersChangedEvent::default()
        },
    );
    if let Some(at) = click {
        check.cx.simulate_click(window, at, shift);
    }
    check
        .cx
        .simulate_event(window, ModifiersChangedEvent::default());
    check.cx.run_until_parked();
}
