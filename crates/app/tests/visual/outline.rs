//! The outline beside a Markdown file: its headings as stations, the one on screen marked, and a
//! click that takes the cursor to a heading.

use gpui_kit::{
    AnyWindowHandle, Modifiers, ScrollDelta, ScrollEvent, ScrollSource, TouchPhase, point, px,
    test::TestWindowExt as _,
};

use crate::check::{Check, fixture};

const TEXT: &str = "# Keys\n\nWalk the tree.\n\n## Tree\n\nArrows move.\n\n### Speed search\n\nType a name.\n\n## Editor\n\nThe editor is gpui-kit's.\n\n## Terminal\n\nAlt+F12.\n";

/// The outline's stations at 100% zoom: the column is 230 wide at the editor island's right (6 in
/// from the window's edge); the stations start under the title bar (38), the tab bar (34), the
/// route line above the editor (30) and the outline's own header (34), 30 apart.
fn station_x(check: &mut Check, window: AnyWindowHandle) -> f32 {
    check.window_size(window).0 - 6. - 230. + 60.
}

fn station_y(index: usize) -> f32 {
    38. + 34. + 30. + 34. + 30. * index as f32 + 15.
}

pub fn run(check: &mut Check) {
    let fixture = fixture();
    std::fs::write(fixture.path().join("docs/guide/keys.md"), TEXT).expect("write");
    let window = check.open(Some(fixture.path()));
    check.keys(window, "cmd-shift-o k e y s enter");
    check.wait_until(|check| {
        check.workspace(window, |w, cx| w.active_file(cx))
            == Some(std::path::PathBuf::from("docs/guide/keys.md"))
    });
    check.shot(window, "outline-1-stations");
    // The fourth station is Editor, on line 13.
    let x = station_x(check, window);
    click(check, window, x, station_y(3));
    check.keys(window, "cmd-alt-shift-c");
    let copied = check.clipboard();
    check.expect_eq(
        "a station takes the cursor to its heading",
        copied,
        Some("docs/guide/keys.md:13".into()),
    );
    check.shot(window, "outline-2-here");
    // Off in the settings, the editor takes the whole width again.
    check.update_workspace(window, |_, _, cx| {
        subwy::update_settings(cx, |s| s.outline = false)
    });
    check.update_workspace(window, |_, _, _| {});
    check.shot(window, "outline-3-off");
    check.update_workspace(window, |_, _, cx| {
        subwy::update_settings(cx, |s| s.outline = true)
    });
    follows_the_scroll(check);
}

/// In a file longer than the screen the marked station follows the scroll, not the cursor; with
/// the end of the file on screen it is the last; a click brings its heading up to be the marked one.
fn follows_the_scroll(check: &mut Check) {
    let dir = tempfile::tempdir().expect("temp dir");
    let body = "text\n".repeat(40);
    let text: String = (1..=6)
        .map(|n| format!("## Part {n}\n\n{body}\n"))
        .collect();
    std::fs::write(dir.path().join("long.md"), text).expect("write");
    let window = check.open(Some(dir.path()));
    // Rows: the folder, long.md.
    check.keys(window, "down down enter");
    settle(check, window);
    let section = |check: &mut Check| check.workspace(window, |w, cx| w.outline_section(cx));
    let top = section(check);
    check.expect_eq(
        "outline: the top of a long file marks the first station",
        top,
        Some(0),
    );

    let (width, _) = check.window_size(window);
    let editor = point(px(width - 6. - 230. - 150.), px(300.));
    for _ in 0..30 {
        check.cx.simulate_event(
            window,
            ScrollEvent {
                position: editor,
                delta: ScrollDelta::Pixels(point(px(0.), px(-40.))),
                modifiers: Modifiers::default(),
                touch_phase: TouchPhase::Moved,
                source: ScrollSource::Pointer,
            },
        );
    }
    settle(check, window);
    // The cursor is still on the first line.
    let scrolled = section(check);
    check.shot(window, "outline-4-scrolled");
    check.expect_eq(
        "outline: scrolling down moves the marked station on",
        scrolled.is_some_and(|at| at > 0),
        true,
    );

    check.keys(window, "cmd-down");
    settle(check, window);
    let end = section(check);
    check.expect_eq(
        "outline: the end of the file on screen marks the last station",
        end,
        Some(5),
    );

    let x = station_x(check, window);
    click(check, window, x, station_y(2));
    settle(check, window);
    let clicked = section(check);
    check.expect_eq(
        "outline: a clicked station is the marked one",
        clicked,
        Some(2),
    );
    check
        .cx
        .update_window(window, |_, window, _| window.remove_window())
        .expect("window");
}

/// The editor learns which lines it shows while it paints; draw a few frames.
fn settle(check: &mut Check, window: AnyWindowHandle) {
    for _ in 0..4 {
        check
            .cx
            .update_window(window, |_, window, cx| window.render_frame(cx))
            .expect("window");
        check.cx.run_until_parked();
    }
}

fn click(check: &mut Check, window: AnyWindowHandle, x: f32, y: f32) {
    check
        .cx
        .simulate_click(window, point(px(x), px(y)), Modifiers::default());
    check.cx.run_until_parked();
}
