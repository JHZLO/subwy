//! Auto-save: edited files save when the window loses focus; a file changed on disk keeps its version.

use std::fs;

use gpui_kit::ClipboardItem;

use crate::check::Check;

pub fn run(check: &mut Check) {
    saves_after_focus_loss(check);
    pasted_returns_save_as_the_files_line_ending(check);
}

fn saves_after_focus_loss(check: &mut Check) {
    let dir = tempfile::tempdir().expect("temp dir");
    let (a, b) = (dir.path().join("a.md"), dir.path().join("b.md"));
    fs::write(&a, "alpha\n").expect("write");
    fs::write(&b, "beta\n").expect("write");
    let window = check.open(Some(dir.path()));

    check.keys(window, "down down enter");
    check.cx.simulate_input(window, "x");
    // What the window does when it loses focus.
    check.update_workspace(window, |w, window, cx| w.save_edited(window, cx));
    let saved = check.wait_until(|_| fs::read_to_string(&a).is_ok_and(|text| text == "xalpha\n"));
    check.expect_eq(
        "an edited file saves when the window loses focus",
        saved,
        true,
    );

    check.keys(window, "cmd-1 down enter");
    check.cx.simulate_input(window, "y");
    fs::write(&b, "changed\n").expect("write");
    check.update_workspace(window, |w, window, cx| w.save_edited(window, cx));
    for _ in 0..10 {
        check.cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    check.expect_eq(
        "a file changed on disk is not overwritten",
        fs::read_to_string(&b).ok(),
        Some("changed\n".to_string()),
    );
    let text = check.workspace(window, |w, cx| w.active_text(cx));
    check.expect_eq(
        "its edits stay in the tab",
        text,
        Some("ybeta\n".to_string()),
    );
}

/// Text copied from elsewhere may break its lines with `\r\n` or `\r`; pasted, it saves with the
/// file's own line ending, so the file never turns mixed (and read-only).
fn pasted_returns_save_as_the_files_line_ending(check: &mut Check) {
    let dir = tempfile::tempdir().expect("temp dir");
    let (crlf, lf) = (dir.path().join("crlf.txt"), dir.path().join("lf.txt"));
    fs::write(&crlf, "end\r\n").expect("write");
    fs::write(&lf, "end\n").expect("write");
    let window = check.open(Some(dir.path()));
    check
        .cx
        .write_to_clipboard(ClipboardItem::new_string("one\r\ntwo\rthree\n".to_string()));

    check.keys(window, "down down enter cmd-v");
    check.update_workspace(window, |w, window, cx| w.save_edited(window, cx));
    let saved = check.wait_until(|_| {
        fs::read(&crlf).is_ok_and(|bytes| bytes == b"one\r\ntwo\r\nthree\r\nend\r\n")
    });
    check.expect_eq("a paste saves with a CRLF file's line ending", saved, true);

    check.keys(window, "cmd-1 down enter cmd-v");
    check.update_workspace(window, |w, window, cx| w.save_edited(window, cx));
    let saved =
        check.wait_until(|_| fs::read(&lf).is_ok_and(|bytes| bytes == b"one\ntwo\nthree\nend\n"));
    check.expect_eq("a paste saves with an LF file's line ending", saved, true);
}
