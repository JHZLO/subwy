//! Local History (Alt+Shift+H): each save keeps the file's text; the modal lists the earlier ones
//! beside the file now, and Enter puts one back as an edit Cmd+Z takes back.

use std::path::PathBuf;

use subwy_project::history;

use crate::check::Check;

pub fn run(check: &mut Check) {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path().canonicalize().expect("root");
    let file = root.join("a.md");
    std::fs::write(&file, "one\ntwo\nthree\n").expect("write");
    let window = check.open(Some(&root));
    // Rows: the folder, a.md.
    check.keys(window, "down down enter");
    let active = |check: &mut Check| check.workspace(window, |w, cx| w.active_file(cx));
    check.wait_until(|check| active(check) == Some(PathBuf::from("a.md")));

    // The first save keeps the text it writes over, then its own.
    check.keys(window, "x cmd-s");
    let kept = check.wait_until(|_| history::stations(&history::default_dir(), &file).len() == 2);
    check.expect_eq("a save keeps the text before it", kept, true);

    check.keys(window, "alt-shift-h");
    let stops = |check: &mut Check| check.workspace(window, |w, cx| w.history_stops(cx));
    check.wait_until(|check| stops(check).is_some());
    let listed = stops(check);
    check.expect_eq(
        "the history lists the save before, not the file now",
        listed,
        Some(1),
    );
    check.shot(window, "history-modal");

    check.keys(window, "enter");
    let closed = stops(check);
    check.expect_eq("enter closes the history", closed, None);
    let text = check.workspace(window, |w, cx| w.active_text(cx));
    check.expect_eq(
        "enter puts the earlier text back",
        text.as_deref(),
        Some("one\ntwo\nthree\n"),
    );
    check.keys(window, "cmd-z");
    let text = check.workspace(window, |w, cx| w.active_text(cx));
    check.expect_eq(
        "cmd-z takes the restore back",
        text.as_deref(),
        Some("xone\ntwo\nthree\n"),
    );

    check.keys(window, "alt-shift-h escape");
    let closed = stops(check);
    check.expect_eq("escape closes the history", closed, None);
    check
        .cx
        .update_window(window, |_, window, _| window.remove_window())
        .expect("window");
}
