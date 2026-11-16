//! Editor tabs: closing and opening again.

use std::{fs, path::PathBuf};

use gpui_kit::{AnyWindowHandle, Modifiers, MouseButton, Pixels, Point, point, px};

use crate::check::Check;

pub fn run(check: &mut Check) {
    reopens_closed_tabs(check);
    switches_with_arrows(check);
    splits_by_dropping(check);
}

/// Cmd+Shift+Left and Right in the editor go to the tab before and after, as in the terminal.
fn switches_with_arrows(check: &mut Check) {
    let dir = tempfile::tempdir().expect("temp dir");
    for name in ["a.md", "b.md"] {
        fs::write(dir.path().join(name), format!("# {name}\n")).expect("write");
    }
    let window = check.open(Some(dir.path()));
    // Rows: the folder, a.md, b.md. b.md opens last, in front.
    check.keys(window, "down down enter cmd-1 down enter");
    let active = |check: &mut Check| check.workspace(window, |w, cx| w.active_file(cx));
    check.wait_until(|check| active(check) == Some(PathBuf::from("b.md")));
    check.keys(window, "cmd-shift-left");
    let before = active(check);
    check.expect_eq(
        "tabs: cmd-shift-left goes to the tab before",
        before,
        Some(PathBuf::from("a.md")),
    );
    check.keys(window, "cmd-shift-right");
    let after = active(check);
    check.expect_eq(
        "tabs: cmd-shift-right goes to the tab after",
        after,
        Some(PathBuf::from("b.md")),
    );
    // A real Cmd+Shift+[ arrives as Cmd+{.
    check.keys(window, "cmd-{");
    let bracket = active(check);
    check.expect_eq(
        "tabs: cmd-shift-[ goes to the tab before",
        bracket,
        Some(PathBuf::from("a.md")),
    );
}

/// A file dragged from the tree onto the editor's right half opens beside the others; dropped on
/// the other side, a file moves over, and a side left without tabs goes.
fn splits_by_dropping(check: &mut Check) {
    let dir = tempfile::tempdir().expect("temp dir");
    for name in ["a.md", "b.md"] {
        fs::write(dir.path().join(name), format!("# {name}\n")).expect("write");
    }
    let window = check.open(Some(dir.path()));
    // Rows: the folder, a.md, b.md. Enter opens a.md, the editor not split.
    check.keys(window, "down down enter");
    let (width, _) = check.window_size(window);
    let right_half = point(px(width - 120.), px(300.));
    let sides = |check: &mut Check| check.workspace(window, |w, cx| w.split_files(cx));
    drag(check, window, point(px(100.), px(row_y(2))), right_half);
    let want = (vec![PathBuf::from("a.md")], vec![PathBuf::from("b.md")]);
    let split = check.wait_until(|check| sides(check) == want);
    check.shot(window, "tabs-split");
    check.expect_eq(
        "split: a file dropped on the right half opens beside",
        split,
        true,
    );

    // a.md dropped on the right side moves there; the left side, left empty, goes.
    drag(check, window, point(px(100.), px(row_y(1))), right_half);
    let want = (
        vec![PathBuf::from("b.md"), PathBuf::from("a.md")],
        Vec::new(),
    );
    let joined = check.wait_until(|check| sides(check) == want);
    check.expect_eq(
        "split: a side whose last file moves over closes",
        joined,
        true,
    );
}

/// The middle of the tree's row `index`, under the title bar and the tree's header.
fn row_y(index: usize) -> f32 {
    38. + 34. + 22. * index as f32 + 11.
}

/// Presses at `from`, moves past GPUI's drag threshold, then to `to`, and lets go.
fn drag(check: &mut Check, window: AnyWindowHandle, from: Point<Pixels>, to: Point<Pixels>) {
    let none = Modifiers::default();
    check.cx.simulate_mouse_move(window, from, None, none);
    check
        .cx
        .simulate_mouse_down(window, from, MouseButton::Left, none);
    let nudge = point(
        from.x + (to.x - from.x) * 0.1,
        from.y + (to.y - from.y) * 0.1,
    );
    check
        .cx
        .simulate_mouse_move(window, nudge, MouseButton::Left, none);
    check
        .cx
        .simulate_mouse_move(window, to, MouseButton::Left, none);
    check
        .cx
        .simulate_mouse_up(window, to, MouseButton::Left, none);
    check.cx.run_until_parked();
}

/// Cmd+Shift+T opens the tab closed last again, with its caret where it was.
fn reopens_closed_tabs(check: &mut Check) {
    let dir = tempfile::tempdir().expect("temp dir");
    fs::write(dir.path().join("a.md"), "one\ntwo\nthree\n").expect("write");
    let window = check.open(Some(dir.path()));
    check.keys(window, "down down enter down down cmd-w");
    let open = check.workspace(window, |w, cx| w.open_files(cx));
    check.expect_eq("tabs: cmd-w closes a.md", open, Vec::<PathBuf>::new());

    check.keys(window, "cmd-shift-t");
    check.wait_until(|check| {
        check.workspace(window, |w, cx| w.active_file(cx)) == Some(PathBuf::from("a.md"))
    });
    let active = check.workspace(window, |w, cx| w.active_file(cx));
    check.expect_eq(
        "tabs: cmd-shift-t opens the closed tab again",
        active,
        Some(PathBuf::from("a.md")),
    );
    check.keys(window, "cmd-alt-shift-c");
    check.expect_eq(
        "tabs: its caret is back on the line it was on",
        check.clipboard(),
        Some("a.md:3".to_string()),
    );
}
