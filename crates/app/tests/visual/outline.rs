//! The outline beside a Markdown file: its headings as stations, the one holding the cursor marked,
//! and a click that takes the cursor to a heading.

use gpui_kit::{AnyWindowHandle, Modifiers, point, px};

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
}

fn click(check: &mut Check, window: AnyWindowHandle, x: f32, y: f32) {
    check
        .cx
        .simulate_click(window, point(px(x), px(y)), Modifiers::default());
    check.cx.run_until_parked();
}
