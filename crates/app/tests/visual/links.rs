//! Following a Markdown link through gpui-kit's go to definition, which Cmd+click on it runs.

use std::{fs, path::PathBuf};

use gpui_kit::component::input::GoToDefinition;

use crate::check::Check;

pub fn run(check: &mut Check) {
    let dir = tempfile::tempdir().expect("temp dir");
    fs::write(
        dir.path().join("notes.md"),
        "See [the guide](docs/guide%20one.md#keys).\n",
    )
    .expect("write");
    fs::create_dir(dir.path().join("docs")).expect("mkdir");
    fs::write(dir.path().join("docs/guide one.md"), "# Keys\n").expect("write");
    let window = check.open(Some(dir.path()));
    // The tree lists docs, then notes.md; the caret goes inside the link.
    check.keys(
        window,
        "down down down enter right right right right right right",
    );
    let active = check.workspace(window, |w, cx| w.active_file(cx));
    check.expect_eq("notes.md is open", active, Some(PathBuf::from("notes.md")));
    check.cx.dispatch_action(window, GoToDefinition);
    let followed = check.wait_until(|check| {
        check.workspace(window, |w, cx| w.active_file(cx))
            == Some(PathBuf::from("docs/guide one.md"))
    });
    check.expect_eq(
        "a Markdown link opens its file (%20 decoded, #heading dropped)",
        followed,
        true,
    );
}
