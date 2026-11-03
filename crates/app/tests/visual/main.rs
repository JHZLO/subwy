//! Offscreen UI checks. Builds the real subwy window off screen, drives it with keystrokes, checks the
//! results and saves screenshots to target/visual/. It never shows a window, never takes the keyboard
//! and never touches the real session. Run with `cargo test -p subwy --test visual`.
//! Each feature keeps its scenarios in its own module.

mod check;
mod finder;
mod folders;
mod islands;
mod lines;
mod links;
mod markdown;
mod markdown_view;
mod outline;
mod phase1;
mod preview_sync;
mod saving;
mod search;
mod settings;
mod start;
mod terminal;
mod tree;
mod windows;

use std::{fs, path::PathBuf};

use gpui_kit::{platform::current_platform, *};

fn main() {
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/visual");
    fs::create_dir_all(&out).expect("out dir");
    let session = tempfile::NamedTempFile::new().expect("session file");
    // Settings the tests rely on: the defaults plus one extra key, to prove user bindings apply.
    let settings = tempfile::NamedTempFile::new().expect("settings file");
    fs::write(
        settings.path(),
        r#"{ "keymap": [ { "key": "cmd-p", "action": "workspace::ToggleFinder", "context": "Workspace" } ] }"#,
    )
    .expect("write settings");
    // Safe: set before anything else runs, on the only thread.
    unsafe {
        std::env::set_var("SUBWY_SESSION_PATH", session.path());
        std::env::set_var("SUBWY_SETTINGS_PATH", settings.path());
        std::env::set_var("SUBWY_SHELL", "/bin/sh");
    }

    // The real assets, so icons show in the screenshots.
    let mut cx = VisualTestAppContext::with_asset_source(
        current_platform(false),
        std::sync::Arc::new(subwy::icons::SubwyAssets),
    );
    cx.update(subwy::init);
    // Windows subwy opens by itself (not through Check::open) stay off screen as well, and no
    // file is handed to another app.
    cx.update(|cx| cx.set_global(subwy::workspace::OffscreenWindows));
    // An offscreen window draws only on input, so the tree's route is shot where it settles.
    cx.update(|cx| cx.set_reduce_motion(true));
    let mut check = check::Check {
        cx,
        out,
        failures: Vec::new(),
    };

    phase1::run(&mut check);
    finder::run(&mut check);
    tree::run(&mut check);
    markdown::run(&mut check);
    lines::run(&mut check);
    links::run(&mut check);
    search::run(&mut check);
    preview_sync::run(&mut check);
    windows::run(&mut check);
    terminal::run(&mut check);
    islands::run(&mut check);
    markdown_view::run(&mut check);
    folders::run(&mut check);
    settings::run(&mut check);
    outline::run(&mut check);
    start::run(&mut check);
    saving::run(&mut check);

    if check.failures.is_empty() {
        println!("visual checks passed; screenshots in target/visual/");
    } else {
        for failure in &check.failures {
            eprintln!("FAIL {failure}");
        }
        std::process::exit(1);
    }
}
