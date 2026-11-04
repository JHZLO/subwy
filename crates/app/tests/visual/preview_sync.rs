//! The preview follows the editor; local images show.

use std::{fs, path::PathBuf};

use gpui_kit::{test::TestWindowExt as _, *};

use crate::check::Check;

pub fn run(check: &mut Check) {
    follows_the_editor(check);
    shows_local_images(check);
}

/// Draws a few frames: the editor scrolls in one, the preview asks where the line is in the next,
/// and moves in the one after.
fn settle(check: &mut Check, window: AnyWindowHandle) {
    for _ in 0..4 {
        check
            .cx
            .update_window(window, |_, window, cx| window.render_frame(cx))
            .expect("window");
        check.cx.run_until_parked();
    }
}

/// Where the top of the preview's page is, in window pixels; it moves up as the preview scrolls.
fn page_top(check: &mut Check, window: AnyWindowHandle) -> f32 {
    check
        .cx
        .update_window(window, |_, window, _| {
            window
                .try_find("preview-page")
                .map_or(f32::NAN, |page| f32::from(page.bounds().top()))
        })
        .expect("window")
}

fn follows_the_editor(check: &mut Check) {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut text = String::from("# A long read\n\nSections to scroll through.\n");
    for i in 1..=80 {
        text.push_str(&format!(
            "\n## Section {i}\n\nParagraph {i} has a few words in it, enough for a line.\n\n\
             - item {i}a\n- item {i}b\n"
        ));
    }
    fs::write(dir.path().join("long.md"), text).expect("write");
    let window = check.open(Some(dir.path()));

    check.keys(window, "down down enter cmd-shift-v");
    let active = check.workspace(window, |w, cx| w.active_file(cx));
    check.expect_eq(
        "preview sync: enter opens long.md",
        active,
        Some(PathBuf::from("long.md")),
    );
    settle(check, window);
    let top = page_top(check, window);

    check.keys(window, "pagedown pagedown pagedown");
    settle(check, window);
    let middle = top - page_top(check, window);
    check.shot(window, "preview-sync-middle");
    check.expect_eq(
        "preview sync: page down scrolls the preview",
        middle > 0.,
        true,
    );

    check.keys(window, "cmd-down");
    settle(check, window);
    let end = top - page_top(check, window);
    check.shot(window, "preview-sync-end");
    check.expect_eq(
        "preview sync: the end of the file scrolls it further",
        end > middle,
        true,
    );

    // The editor scrolls on past its last line, where the preview has reached its end.
    let editor = check
        .cx
        .update_window(window, |_, window, _| {
            window
                .try_find("preview-page")
                .map(|page| point(page.bounds().left() - px(150.), px(200.)))
        })
        .expect("window")
        .expect("preview page");
    for _ in 0..10 {
        check.cx.simulate_event(
            window,
            ScrollWheelEvent {
                position: editor,
                delta: ScrollDelta::Pixels(point(px(0.), px(-40.))),
                modifiers: Modifiers::default(),
                touch_phase: TouchPhase::Moved,
            },
        );
        settle(check, window);
    }
    let past = top - page_top(check, window);
    check.expect_eq(
        "preview sync: scrolling past the editor's last line keeps it at the end",
        past >= end,
        true,
    );

    check.keys(window, "cmd-up");
    settle(check, window);
    let back = top - page_top(check, window);
    check.expect_eq("preview sync: the start scrolls it back", back, 0.);
}

fn shows_local_images(check: &mut Check) {
    let dir = tempfile::tempdir().expect("temp dir");
    let picture = dir.path().join("img/block.png");
    fs::create_dir_all(picture.parent().expect("parent")).expect("dirs");
    // Magenta appears nowhere else on screen, so its pixels can only be the picture.
    image::RgbaImage::from_pixel(96, 48, image::Rgba([255, 0, 255, 255]))
        .save(&picture)
        .expect("png");
    let picture = picture.canonicalize().expect("picture");
    fs::write(
        dir.path().join("pictures.md"),
        format!(
            "# Pictures\n\n![A magenta block]({})\n\n![The same block, relative](img/block.png)\n\n![A picture that is not there](img/gone.png)\n\nThe end.\n",
            picture.display()
        ),
    )
    .expect("write");
    let window = check.open(Some(dir.path()));

    // The img folder comes first in the tree.
    check.keys(window, "down down down enter cmd-shift-v");
    let active = check.workspace(window, |w, cx| w.active_file(cx));
    check.expect_eq(
        "preview image: enter opens pictures.md",
        active,
        Some(PathBuf::from("pictures.md")),
    );
    settle(check, window);
    check.shot(window, "preview-image");
    let magenta = check
        .cx
        .capture_screenshot(window)
        .expect("screenshot")
        .pixels()
        .filter(|p| p[0] > 230 && p[1] < 40 && p[2] > 230)
        .count();
    // Two 96 x 48 point blocks (absolute and relative path); a Retina capture has four pixels to a point.
    check.expect_eq(
        "preview image: images on disk show, by absolute and relative path",
        magenta >= 2 * 96 * 48,
        true,
    );
}
