//! What every offscreen scenario shares: a fixture folder, the window, keys, screenshots, checks.

use std::{
    fs,
    path::{Path, PathBuf},
};

use gpui_kit::{test::TestWindowExt as _, *};
use subwy::workspace::Workspace;

pub const FILES: &[(&str, &str)] = &[
    (
        "docs/guide/keys.md",
        "# Keys\n\nArrow keys walk the tree.\n",
    ),
    (
        "docs/guide/paths.md",
        "# Copying paths\n\nleaf copies a path in one keystroke,\nfrom the tree or from the editor.\n\n\
         | Keys | You get |\n|---|---|\n| Cmd+Shift+C | the absolute path |\n| Cmd+Alt+Shift+C | docs/guide/paths.md:12 |\n\n\
         ## Why the line number\n\nPaste it into a PR or a chat and the reader lands on `docs/guide/paths.md:12`.\n\n\
         - Folders copy without a trailing slash\n- 한글 파일명은 NFC 로 복사된다\n- [ ] Same keys in the tree and the editor\n\n\
         > Copying never needs the mouse.\n",
    ),
    ("docs/notes.md", "notes\n"),
    (
        "src/main.rs",
        "use std::path::Path;\n\n// Prints where leaf runs.\nfn main() {\n    let here = Path::new(\".\");\n    println!(\"leaf in {}\", here.display());\n}\n",
    ),
    ("README.md", "# leaf\n"),
    ("target/debug/leaf", "binary\n"),
    (".gitignore", "target/\n"),
];

pub fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp dir");
    for (path, text) in FILES {
        let path = dir.path().join(path);
        fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
        fs::write(path, text).expect("write");
    }
    dir
}

pub struct Check {
    pub cx: VisualTestAppContext,
    pub out: PathBuf,
    pub failures: Vec<String>,
}

impl Check {
    pub fn open(&mut self, root: Option<&Path>) -> AnyWindowHandle {
        let root = root.map(|r| r.canonicalize().expect("root"));
        let window = self
            .cx
            .open_offscreen_window_default(move |window, cx| {
                let workspace = cx.new(|cx| {
                    let mut workspace = Workspace::new(window, cx);
                    if let Some(root) = root {
                        workspace.set_root(root, window, cx);
                    }
                    workspace
                });
                cx.new(|cx| gpui_kit::base::Root::new(workspace, window, cx))
            })
            .expect("offscreen window");
        self.cx.run_until_parked();
        window.into()
    }

    /// One key at a time, letting folder reads and file opens finish in between, as a person types.
    pub fn keys(&mut self, window: AnyWindowHandle, keys: &str) {
        for key in keys.split_whitespace() {
            self.cx.simulate_keystrokes(window, key);
            self.cx.run_until_parked();
        }
    }

    pub fn shot(&mut self, window: AnyWindowHandle, name: &str) {
        self.cx.run_until_parked();
        // An offscreen window draws only on input; a theme or zoom change needs a fresh frame.
        self.cx
            .update_window(window, |_, window, cx| window.render_frame(cx))
            .expect("window");
        let path = self.out.join(format!("{name}.png"));
        match self.cx.capture_screenshot(window) {
            Ok(image) => {
                image.save(&path).expect("save png");
                println!("screenshot {}", path.display());
            }
            Err(err) => self.failures.push(format!("{name}: no screenshot ({err})")),
        }
    }

    pub fn workspace<R>(
        &mut self,
        window: AnyWindowHandle,
        read: impl FnOnce(&Workspace, &App) -> R,
    ) -> R {
        self.cx
            .update_window(window, |root, _, cx| {
                let root = root.downcast::<gpui_kit::base::Root>().expect("root view");
                let workspace = root
                    .read(cx)
                    .view()
                    .clone()
                    .downcast::<Workspace>()
                    .expect("workspace");
                read(workspace.read(cx), cx)
            })
            .expect("window")
    }

    /// Changes the window's workspace, as a menu or a dialog would (the checks never click a
    /// native dialog).
    pub fn update_workspace(
        &mut self,
        window: AnyWindowHandle,
        change: impl FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>),
    ) {
        self.cx
            .update_window(window, |root, window, cx| {
                let root = root.downcast::<gpui_kit::base::Root>().expect("root view");
                let workspace = root
                    .read(cx)
                    .view()
                    .clone()
                    .downcast::<Workspace>()
                    .expect("workspace");
                workspace.update(cx, |workspace, cx| change(workspace, window, cx));
            })
            .expect("window");
        self.cx.run_until_parked();
    }

    pub fn expect_eq<T: PartialEq + std::fmt::Debug>(&mut self, what: &str, got: T, want: T) {
        if got == want {
            println!("ok   {what}");
        } else {
            self.failures
                .push(format!("{what}: got {got:?}, want {want:?}"));
        }
    }

    /// Waits up to two seconds for `done`: file operations and reads run off the main thread and
    /// land a moment after the keys that start them.
    pub fn wait_until(&mut self, mut done: impl FnMut(&mut Self) -> bool) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            self.cx.run_until_parked();
            if done(self) {
                return true;
            }
            if std::time::Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// The window's size in points. The screen bounds it, so on a small one (a CI machine's) it is
    /// less than the 1280 by 800 asked for.
    pub fn window_size(&mut self, window: AnyWindowHandle) -> (f32, f32) {
        self.cx
            .update_window(window, |_, window, _| {
                let size = window.viewport_size();
                (size.width.as_f32(), size.height.as_f32())
            })
            .expect("window")
    }

    pub fn clipboard(&self) -> Option<String> {
        self.cx.read_from_clipboard().and_then(|item| item.text())
    }
}
