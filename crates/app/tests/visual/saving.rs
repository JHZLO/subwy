//! Auto-save: edited files save when the window loses focus; a file changed on disk keeps its version.

use std::fs;

use crate::check::Check;

pub fn run(check: &mut Check) {
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
