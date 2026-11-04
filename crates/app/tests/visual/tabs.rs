//! Editor tabs: closing and opening again.

use std::{fs, path::PathBuf};

use crate::check::Check;

pub fn run(check: &mut Check) {
    reopens_closed_tabs(check);
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
