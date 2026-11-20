//! Back and Forward (Cmd+[ and Cmd+]): the caret goes back to where it jumped from, in this file
//! or another, and ahead again.

use std::path::PathBuf;

use gpui_kit::AnyWindowHandle;

use crate::check::Check;

pub fn run(check: &mut Check) {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path().canonicalize().expect("root");
    let lines: String = (1..=40).map(|n| format!("line {n}\n")).collect();
    std::fs::write(root.join("a.md"), lines).expect("write");
    std::fs::write(root.join("b.md"), "# B\n").expect("write");
    let window = check.open(Some(&root));
    // Rows: the folder, a.md, b.md.
    check.keys(window, "down down enter");
    let active = |check: &mut Check| check.workspace(window, |w, cx| w.active_file(cx));
    check.wait_until(|check| active(check) == Some(PathBuf::from("a.md")));
    check.keys(window, "cmd-l 2 0 enter");
    expect_caret(check, window, "go to line jumps", "a.md:20");
    check.keys(window, "cmd-1 down enter");
    check.wait_until(|check| active(check) == Some(PathBuf::from("b.md")));

    check.keys(window, "cmd-[");
    expect_caret(check, window, "back to the other file", "a.md:20");
    check.keys(window, "cmd-[");
    expect_caret(check, window, "back past the jump in the file", "a.md:1");
    check.keys(window, "cmd-[");
    expect_caret(check, window, "nothing further back", "a.md:1");
    check.keys(window, "cmd-]");
    expect_caret(check, window, "forward to the jump", "a.md:20");
    check.keys(window, "cmd-]");
    expect_caret(check, window, "forward to the other file", "b.md:1");
    check.shot(window, "places-forward");

    // Back reopens a tab closed since.
    check.keys(window, "cmd-w");
    expect_caret(check, window, "closing shows the tab before", "a.md:20");
    check.keys(window, "cmd-[");
    check.wait_until(|check| active(check) == Some(PathBuf::from("b.md")));
    expect_caret(check, window, "back reopens the closed tab", "b.md:1");
    check.keys(window, "cmd-]");
    expect_caret(check, window, "and forward comes back", "a.md:20");
    check
        .cx
        .update_window(window, |_, window, _| window.remove_window())
        .expect("window");
}

/// The active file and its caret line as Cmd+Alt+Shift+C copies them (`path:line`, one-based).
fn expect_caret(check: &mut Check, window: AnyWindowHandle, what: &str, want: &str) {
    check.keys(window, "cmd-alt-shift-c");
    let copied = check.clipboard();
    check.expect_eq(what, copied.as_deref(), Some(want));
}
