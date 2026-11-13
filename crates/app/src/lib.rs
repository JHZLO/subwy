//! subwy: a light, keyboard-first text and Markdown editor for macOS.
//! A library so the offscreen UI tests in tests/ can build the same window; `main.rs` only calls [`run`].

// The theme JSON in theme.rs is one large `json!` literal.
#![recursion_limit = "256"]

mod editor_tabs;
mod empty_state;
mod finder;
pub mod icons;
pub mod keymap;
mod keymap_view;
mod menus;
mod modal;
mod preview;
mod project_panel;
mod route;
mod search_panel;
mod settings_view;
mod status_bar;
mod terminal_panel;
mod terminal_tabs;
pub mod theme;
mod title_bar;
pub mod workspace;

use std::{path::PathBuf, time::Duration};

use gpui_kit::{App, AppContext as _, Global, Task};
use subwy_project::settings::{self, Settings};

/// The user's settings, read at startup and changed by the Settings dialog, and why the file could
/// not be used, if it could not.
pub struct UserSettings {
    pub settings: Settings,
    pub error: Option<String>,
    /// The file was read (or there was none). A file that could not be read is never written over:
    /// changes made in the Settings dialog then last until subwy quits.
    writable: bool,
    /// Writes the file a moment after the last change; a newer change replaces it.
    saving: Task<()>,
}

impl Global for UserSettings {}

pub fn settings(cx: &App) -> &Settings {
    &cx.global::<UserSettings>().settings
}

/// How long the file waits after a change, so dragging a number writes it once.
const SAVE_SETTINGS_AFTER: Duration = Duration::from_millis(300);

/// Changes the settings (the Settings dialog): every window shows the change at once, and the file
/// is written off the main thread a moment later.
pub fn update_settings(cx: &mut App, change: impl FnOnce(&mut Settings)) {
    let state = cx.global_mut::<UserSettings>();
    let keymap_before = state.settings.keymap.clone();
    change(&mut state.settings);
    let (settings, writable) = (state.settings.clone(), state.writable);
    if settings.keymap != keymap_before {
        // The menus show each action's key, so they are set again over the new keymap.
        if let Some(problem) = keymap::bind_all(cx) {
            eprintln!("subwy: {problem}");
        }
        menus::init(cx);
    }
    if writable {
        let saving = cx.spawn(async move |cx| {
            cx.background_executor().timer(SAVE_SETTINGS_AFTER).await;
            let path = settings::default_path();
            let saved = cx
                .background_spawn(async move { settings::save(&path, &settings) })
                .await;
            if let Err(err) = saved {
                eprintln!("subwy: settings not saved: {err}");
            }
        });
        cx.global_mut::<UserSettings>().saving = saving;
    }
    // After the update that called this: windows are updated one by one, the caller's too.
    cx.defer(workspace::settings_changed);
}

/// Registers subwy's settings, theme, keys and the shared session; what both the app and the UI
/// tests start from.
pub fn init(cx: &mut App) {
    let (settings, error) = settings::load(&settings::default_path());
    cx.set_global(UserSettings {
        settings,
        writable: error.is_none(),
        error,
        saving: Task::ready(()),
    });
    gpui_kit::init(cx);
    theme::init(cx);
    // The user's keys are bound after subwy's own, so they win.
    if let Some(problem) = keymap::bind_all(cx) {
        let state = cx.global_mut::<UserSettings>();
        state.error.get_or_insert(problem);
    }
    workspace::init(cx);
}

/// Runs the app. `arg` is a folder to open, or a file to open inside its folder.
pub fn run(arg: Option<PathBuf>) {
    // The first start after the rename from leaf brings leaf's settings and session along.
    let scripted = std::env::var_os("SUBWY_SETTINGS_PATH").is_some()
        || std::env::var_os("SUBWY_SESSION_PATH").is_some();
    if let Some(home) = std::env::var_os("HOME").filter(|_| !scripted)
        && let Err(err) = settings::adopt_leaf_files(std::path::Path::new(&home))
    {
        eprintln!("subwy: leaf's settings not brought over: {err}");
    }
    // A file of defaults the first time, so the settings are easy to find and edit.
    if let Err(err) = settings::write_default_if_missing(&settings::default_path()) {
        eprintln!("subwy: no settings file written: {err}");
    }
    // The default icon set, which gpui-kit's icons (chevrons, dialog buttons) draw from.
    let app = gpui_kit::application().with_assets(icons::SubwyAssets);
    // Paths from Finder's Open With, `open -a subwy` and the subwy command. When they start subwy,
    // macOS hands them over before launching finishes, so they wait in the channel until then.
    let (opened, requests) = smol::channel::unbounded::<Vec<String>>();
    app.on_open_urls(move |urls| {
        let _ = opened.try_send(urls);
    });
    app.run(move |cx| {
        init(cx);
        menus::init(cx);
        cx.on_action(|_: &keymap::Quit, cx| cx.quit());
        cx.on_window_closed(|cx, id| {
            if cx.windows().is_empty() {
                cx.quit();
            } else {
                workspace::window_closed(id, cx);
            }
        })
        .detach();
        let mut paths: Vec<PathBuf> = arg.into_iter().collect();
        while let Ok(urls) = requests.try_recv() {
            paths.extend(urls.iter().filter_map(|url| path_from_url(url)));
        }
        workspace::open_at_start(paths, cx);
        cx.spawn(async move |cx| {
            while let Ok(urls) = requests.recv().await {
                cx.update(|cx| {
                    for path in urls.iter().filter_map(|url| path_from_url(url)) {
                        workspace::open_path(&path, cx);
                    }
                });
            }
        })
        .detach();
        // Screenshot runs set SUBWY_BACKGROUND so the window never takes the keyboard from someone typing.
        if std::env::var_os("SUBWY_BACKGROUND").is_none() {
            cx.activate(true);
        }
    });
}

/// The path in a `file://` URL as macOS sends it (percent-encoded, folders end in a slash).
/// Other URLs are not subwy's.
fn path_from_url(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        return None;
    }
    let mut bytes = Vec::with_capacity(rest.len());
    let mut input = rest.bytes();
    while let Some(byte) = input.next() {
        if byte == b'%' {
            let hex = [input.next()?, input.next()?];
            let hex = std::str::from_utf8(&hex).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
        } else {
            bytes.push(byte);
        }
    }
    if bytes.len() > 1 && bytes.ends_with(b"/") {
        bytes.pop();
    }
    use std::os::unix::ffi::OsStringExt as _;
    Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}

#[cfg(test)]
mod tests {
    use super::path_from_url;
    use std::path::PathBuf;

    #[test]
    fn a_file_url_becomes_its_path() {
        assert_eq!(
            path_from_url("file:///Users/me/notes/"),
            Some(PathBuf::from("/Users/me/notes"))
        );
        assert_eq!(
            path_from_url("file:///Users/me/My%20Notes/todo.md"),
            Some(PathBuf::from("/Users/me/My Notes/todo.md"))
        );
        assert_eq!(
            path_from_url("file://localhost/tmp/a%25b"),
            Some(PathBuf::from("/tmp/a%b"))
        );
        // Korean names arrive as percent-encoded UTF-8.
        assert_eq!(
            path_from_url("file:///Users/me/%ED%95%9C%EA%B8%80.md"),
            Some(PathBuf::from("/Users/me/한글.md"))
        );
        assert_eq!(path_from_url("file:///"), Some(PathBuf::from("/")));
    }

    #[test]
    fn other_urls_are_not_paths() {
        assert_eq!(path_from_url("https://example.com/a.md"), None);
        assert_eq!(path_from_url("subwy://open"), None);
        assert_eq!(path_from_url("file://server/share"), None);
        assert_eq!(path_from_url("file:///bad%2"), None);
        assert_eq!(path_from_url("file:///bad%zz"), None);
    }
}
