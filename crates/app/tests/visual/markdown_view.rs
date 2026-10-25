//! The Markdown view buttons (IntelliJ's Editor only / Editor and Preview / Preview only) and the
//! line between the editor and the preview, which drags.

use gpui_kit::{AnyWindowHandle, Modifiers, MouseButton, point, px, test::TestWindowExt as _};

use crate::check::{Check, fixture};

/// Where things sit at 100% zoom: the editor island starts right of the tree (6 + 300 + 6) and
/// ends 6 before the window's edge; its tab bar is the first 34 under the 38 title bar.
const EDITOR_LEFT: f32 = 312.;
const TAB_BAR_MIDDLE: f32 = 38. + 17.;

pub fn run(check: &mut Check) {
    let fixture = fixture();
    let window = check.open(Some(fixture.path()));
    let width = check
        .cx
        .update_window(window, |_, window, _| window.viewport_size().width.as_f32())
        .expect("window");
    let editor_right = width - 6.;
    // README.md is the last row.
    check.keys(window, "end enter");
    let button = |from_right: f32| editor_right - 8. - 12. - 26. * from_right;

    click(check, window, button(1.), TAB_BAR_MIDDLE);
    let panes = check.workspace(window, |w, cx| w.editor_panes(cx));
    check.expect_eq("editor and preview", (panes.0, panes.1), (true, true));

    // The line between them drags, here from the middle to about a third.
    let row = editor_right - EDITOR_LEFT;
    let grip = EDITOR_LEFT + row * 0.5 + 2.5;
    drag(check, window, grip, EDITOR_LEFT + row * 0.3 + 2.5);
    let split = check.workspace(window, |w, cx| w.editor_panes(cx).2);
    check.expect_eq(
        "dragging the line resizes the editor",
        (split * 100.).round(),
        30.,
    );
    check.shot(window, "markdown-1-split-dragged");

    let before = check.workspace(window, |w, cx| w.active_text(cx));
    click(check, window, button(0.), TAB_BAR_MIDDLE);
    let panes = check.workspace(window, |w, cx| w.editor_panes(cx));
    check.expect_eq("preview only", (panes.0, panes.1), (false, true));
    check.keys(window, "x y z");
    let after = check.workspace(window, |w, cx| w.active_text(cx));
    check.expect_eq("typing does not edit the hidden text", after, before);
    check.shot(window, "markdown-2-preview-only");

    click(check, window, button(2.), TAB_BAR_MIDDLE);
    let panes = check.workspace(window, |w, cx| w.editor_panes(cx));
    check.expect_eq("editor only", (panes.0, panes.1), (true, false));

    renamed_to_markdown(check);
}

/// A file renamed to `.md` is Markdown from then on: the preview shows it.
fn renamed_to_markdown(check: &mut Check) {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("notes"), "# Notes\n\ntext\n").expect("write");
    let window = check.open(Some(dir.path()));
    check.keys(window, "down down enter cmd-shift-v");
    let shown = page_shown(check, window);
    check.expect_eq("a file without an extension has no preview", shown, false);
    check.keys(window, "cmd-1 shift-f6 cmd-a n o t e s . m d enter");
    let shown = check.wait_until(|check| page_shown(check, window));
    check.expect_eq("renamed to .md, the preview shows it", shown, true);
    check.shot(window, "markdown-3-renamed");
}

/// Whether the preview shows a page, which it does only for a Markdown tab.
fn page_shown(check: &mut Check, window: AnyWindowHandle) -> bool {
    check
        .cx
        .update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.try_find("preview-page").is_some()
        })
        .expect("window")
}

fn click(check: &mut Check, window: AnyWindowHandle, x: f32, y: f32) {
    check
        .cx
        .simulate_click(window, point(px(x), px(y)), Modifiers::default());
    check.cx.run_until_parked();
}

fn drag(check: &mut Check, window: AnyWindowHandle, from: f32, to: f32) {
    let none = Modifiers::default();
    let y = px(300.);
    check
        .cx
        .simulate_mouse_move(window, point(px(from), y), None, none);
    check
        .cx
        .simulate_mouse_down(window, point(px(from), y), MouseButton::Left, none);
    let nudge = from + (to - from) * 0.1;
    check
        .cx
        .simulate_mouse_move(window, point(px(nudge), y), MouseButton::Left, none);
    check
        .cx
        .simulate_mouse_move(window, point(px(to), y), MouseButton::Left, none);
    check
        .cx
        .simulate_mouse_up(window, point(px(to), y), MouseButton::Left, none);
    check.cx.run_until_parked();
}
