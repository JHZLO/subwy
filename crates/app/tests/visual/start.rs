//! The empty window's network map of recent folders.

use std::path::Path;

use gpui_kit::{
    component::{Theme, ThemeMode},
    test::TestWindowExt as _,
    *,
};

use crate::check::Check;

/// The folders of the design mockup, opened oldest first so `leaf` is the newest.
const FOLDERS: [&str; 8] = [
    "Desktop/tripstore-web/amber",
    "Documents/papers",
    "Desktop/screenshots",
    "Desktop/tripstore-web/resin",
    "Desktop/tripstore-web",
    "Documents/blog",
    "Desktop/memo",
    "Desktop/tripstore-web/leaf",
];

pub fn run(check: &mut Check) {
    let base = tempfile::tempdir().expect("temp dir");
    let base = base.path().canonicalize().expect("base");
    for folder in FOLDERS {
        std::fs::create_dir_all(base.join(folder)).expect("folder");
    }
    // Opening each folder makes it recent. The window then closes, as a window showing a folder
    // would take Enter on its station.
    let window = check.open(Some(&base.join(FOLDERS[0])));
    for folder in &FOLDERS[1..] {
        let root = base.join(folder);
        check.update_workspace(window, move |w, window, cx| w.set_root(root, window, cx));
    }
    close(check, window);

    // The map starts on the newest folder; Left moves along its line.
    let empty = check.open(None);
    check.shot(empty, "start-1-map");
    check.keys(empty, "left");
    check.shot(empty, "start-2-left");
    opens(
        check,
        empty,
        "left moves along the line",
        &base,
        "Desktop/tripstore-web/amber",
    );

    // Up crosses to the nearest station on the line above: the interchange the line leaves.
    let empty = check.open(None);
    check.keys(empty, "up");
    opens(
        check,
        empty,
        "up takes the nearest station above",
        &base,
        "Desktop/tripstore-web",
    );

    // Down from an interchange takes its own line, then the next line down.
    let empty = check.open(None);
    check.keys(empty, "down down");
    check.shot(empty, "start-3-down");
    opens(
        check,
        empty,
        "down goes line by line",
        &base,
        "Documents/papers",
    );

    // Right stops at the end of a line.
    let empty = check.open(None);
    check.keys(empty, "right right right");
    opens(
        check,
        empty,
        "right stops at the last station",
        &base,
        "Documents/papers",
    );

    // A click on a station opens it.
    let empty = check.open(None);
    let blog = base.join("Documents/blog");
    check
        .cx
        .update_window(empty, |_, window, cx| {
            window.render_frame(cx);
            window.click(
                ElementId::Name(format!("station {}", blog.display()).into()),
                cx,
            );
        })
        .expect("window");
    check.cx.run_until_parked();
    let root = check.workspace(empty, |w, cx| w.root(cx));
    check.expect_eq(
        "a click on a station opens it",
        root,
        Some(base.join("Documents/blog")),
    );
    close(check, empty);

    // On light paper and zoomed: the lines move closer to fit, every size scales.
    // After the window opens: a new window takes the system's appearance.
    let empty = check.open(None);
    check
        .cx
        .update(|cx| Theme::change(ThemeMode::Light, None, cx));
    check.keys(empty, "cmd-= cmd-= left");
    check.shot(empty, "start-4-light-zoomed");
    check.keys(empty, "cmd-0");
    close(check, empty);
    check
        .cx
        .update(|cx| Theme::change(ThemeMode::Dark, None, cx));
}

/// Enter opens the chosen station's folder in the empty window; the window then closes.
fn opens(check: &mut Check, window: AnyWindowHandle, what: &str, base: &Path, folder: &str) {
    check.keys(window, "enter");
    let root = check.workspace(window, |w, cx| w.root(cx));
    check.expect_eq(what, root, Some(base.join(folder)));
    close(check, window);
}

fn close(check: &mut Check, window: AnyWindowHandle) {
    check
        .cx
        .update_window(window, |_, window, _| window.remove_window())
        .expect("window");
}
