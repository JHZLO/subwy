//! The user's settings file. Read once at startup; subwy writes a file of defaults the first time so it is
//! easy to find and edit. A file that does not parse is reported and the defaults are used instead.

use std::{
    io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// The editor's font family. "SF Mono" means the system's monospaced font.
    pub editor_font: String,
    pub editor_font_size: f32,
    /// Spaces a Tab inserts in code.
    pub tab_size: usize,
    /// Tab inserts a tab character instead of spaces.
    pub hard_tabs: bool,
    pub soft_wrap: SoftWrap,
    /// Line numbers in the editor's gutter.
    pub line_numbers: bool,
    /// Spaces and tabs drawn as faint marks.
    pub show_whitespace: bool,
    /// Faint lines down each indent level.
    pub indent_guides: bool,
    /// Code blocks fold from the gutter.
    pub folding: bool,
    /// A Markdown file's headings as stations on a line beside the text.
    pub outline: bool,
    /// Edited files save when subwy loses focus, and before a window closes or subwy quits.
    pub auto_save: bool,
    /// Light, dark, or as macOS is set.
    pub appearance: Appearance,
    /// The key hint line under the tree.
    pub key_hints: bool,
    /// Gitignored items in the tree, dimmed; `false` leaves them out.
    pub show_ignored: bool,
    /// The tree selects the file in the active tab (IntelliJ's Always Select Opened File).
    pub follow_active_file: bool,
    /// A file opens on the first click in the tree, not the second.
    pub single_click_open: bool,
    /// A window opening a folder starts a shell under the editor.
    pub terminal_on_start: bool,
    /// The program new terminal tabs run; empty is the login shell.
    pub terminal_shell: String,
    /// The terminal's text size at 100% zoom; none is the editor's.
    pub terminal_font_size: Option<f32>,
    /// subwy starts with the windows it had open when it quit.
    pub reopen_windows: bool,
    /// Extra key bindings, applied after subwy's own (so they win).
    pub keymap: Vec<KeyOverride>,
}

/// Light or dark paper; `System` follows macOS, as subwy always did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

/// Which files wrap long lines. Wrapping costs time on large files, so Markdown only by default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SoftWrap {
    #[default]
    Markdown,
    All,
    None,
}

/// One extra binding, e.g. `{ "key": "cmd-p", "action": "workspace::ToggleFinder", "context": "Workspace" }`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyOverride {
    pub key: String,
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// Takes `key` away from `action` instead of giving it: how the Settings dialog replaces or
    /// removes one of subwy's own keys.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unbind: bool,
}

/// A key binding as the keymap page reasons about it: one of subwy's own, or one that works now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub key: String,
    pub action: String,
    pub context: Option<String>,
}

/// The keys that work now, given subwy's own (`defaults`) and the user's `overrides`: the defaults
/// not taken away, then the ones the overrides give.
pub fn effective_keys(defaults: &[Binding], overrides: &[KeyOverride]) -> Vec<Binding> {
    let taken = |binding: &Binding| {
        overrides.iter().any(|o| {
            o.unbind
                && o.key == binding.key
                && o.action == binding.action
                && o.context == binding.context
        })
    };
    let mut keys: Vec<Binding> = defaults.iter().filter(|b| !taken(b)).cloned().collect();
    keys.extend(overrides.iter().filter(|o| !o.unbind).map(|o| Binding {
        key: o.key.clone(),
        action: o.action.clone(),
        context: o.context.clone(),
    }));
    keys
}

/// The keys `action` answers to now, each once.
pub fn keys_for(defaults: &[Binding], overrides: &[KeyOverride], action: &str) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for binding in effective_keys(defaults, overrides) {
        if binding.action == action && !keys.contains(&binding.key) {
            keys.push(binding.key);
        }
    }
    keys
}

/// Other actions that `key` also runs where `action` has it: a clash the keymap page points out.
pub fn clashes(
    defaults: &[Binding],
    overrides: &[KeyOverride],
    action: &str,
    key: &str,
) -> Vec<String> {
    let keys = effective_keys(defaults, overrides);
    let contexts: Vec<&Option<String>> = keys
        .iter()
        .filter(|b| b.action == action && b.key == key)
        .map(|b| &b.context)
        .collect();
    let mut others: Vec<String> = Vec::new();
    for binding in &keys {
        if binding.key == key
            && binding.action != action
            && contexts.contains(&&binding.context)
            && !others.contains(&binding.action)
        {
            others.push(binding.action.clone());
        }
    }
    others
}

/// Gives `action` the one key `key`, in every context its own keys work in (`fallback` when it
/// has none): its own keys are taken away and earlier changes to it dropped.
pub fn set_key(
    overrides: &mut Vec<KeyOverride>,
    defaults: &[Binding],
    action: &str,
    fallback: Option<&str>,
    key: &str,
) {
    clear_keys(overrides, defaults, action);
    let mut contexts: Vec<Option<String>> = Vec::new();
    for binding in defaults.iter().filter(|b| b.action == action) {
        if !contexts.contains(&binding.context) {
            contexts.push(binding.context.clone());
        }
    }
    if contexts.is_empty() {
        contexts.push(fallback.map(str::to_string));
    }
    overrides.extend(contexts.into_iter().map(|context| KeyOverride {
        key: key.to_string(),
        action: action.to_string(),
        context,
        unbind: false,
    }));
}

/// Leaves `action` without a key.
pub fn clear_keys(overrides: &mut Vec<KeyOverride>, defaults: &[Binding], action: &str) {
    reset_keys(overrides, action);
    overrides.extend(
        defaults
            .iter()
            .filter(|b| b.action == action)
            .map(|b| KeyOverride {
                key: b.key.clone(),
                action: action.to_string(),
                context: b.context.clone(),
                unbind: true,
            }),
    );
}

/// Back to subwy's own keys for `action`.
pub fn reset_keys(overrides: &mut Vec<KeyOverride>, action: &str) {
    overrides.retain(|o| o.action != action);
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            editor_font: "SF Mono".into(),
            editor_font_size: 13.,
            tab_size: 4,
            hard_tabs: false,
            soft_wrap: SoftWrap::Markdown,
            line_numbers: true,
            show_whitespace: false,
            indent_guides: true,
            folding: true,
            outline: true,
            auto_save: true,
            appearance: Appearance::System,
            key_hints: true,
            show_ignored: true,
            follow_active_file: true,
            single_click_open: false,
            terminal_on_start: true,
            terminal_shell: String::new(),
            terminal_font_size: None,
            reopen_windows: true,
            keymap: Vec::new(),
        }
    }
}

impl Settings {
    /// Whether a file in `language` wraps long lines.
    pub fn wraps(&self, markdown: bool) -> bool {
        match self.soft_wrap {
            SoftWrap::Markdown => markdown,
            SoftWrap::All => true,
            SoftWrap::None => false,
        }
    }
}

/// `~/Library/Application Support/subwy/settings.json`, or `SUBWY_SETTINGS_PATH` for tests.
pub fn default_path() -> PathBuf {
    if let Some(path) = std::env::var_os("SUBWY_SETTINGS_PATH") {
        return PathBuf::from(path);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join("Library/Application Support/subwy/settings.json")
}

/// The settings, or the defaults and why the file could not be used. A missing file is not an error.
pub fn load(path: &Path) -> (Settings, Option<String>) {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return (Settings::default(), None),
        Err(err) => return (Settings::default(), Some(format!("settings.json: {err}"))),
    };
    match serde_json::from_slice::<Settings>(&bytes) {
        Ok(settings) => match validate(&settings) {
            Ok(()) => (settings, None),
            Err(err) => (Settings::default(), Some(format!("settings.json: {err}"))),
        },
        Err(err) => (Settings::default(), Some(format!("settings.json: {err}"))),
    }
}

fn validate(settings: &Settings) -> Result<(), String> {
    if !(6. ..=72.).contains(&settings.editor_font_size) {
        return Err("editor_font_size must be between 6 and 72".into());
    }
    if !(1..=16).contains(&settings.tab_size) {
        return Err("tab_size must be between 1 and 16".into());
    }
    if settings
        .terminal_font_size
        .is_some_and(|size| !(6. ..=72.).contains(&size))
    {
        return Err("terminal_font_size must be between 6 and 72".into());
    }
    Ok(())
}

/// Writes the settings the Settings dialog changed: to a file beside it first, then in its place, so
/// a crash in between leaves the old file whole.
pub fn save(path: &Path, settings: &Settings) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_vec_pretty(settings).map_err(io::Error::other)?;
    let temp = path.with_extension("json.saving");
    std::fs::write(&temp, json)?;
    std::fs::rename(&temp, path)
}

/// Writes the defaults when there is no settings file yet, so there is something to edit.
pub fn write_default_if_missing(path: &Path) -> io::Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_vec_pretty(&Settings::default()).map_err(io::Error::other)?;
    std::fs::write(path, json)
}

/// Copies the settings and session subwy kept while it was called leaf into its own folder, the
/// first time it starts, so the rename keeps the keys, windows and recent folders. leaf's files stay.
pub fn adopt_leaf_files(home: &Path) -> io::Result<()> {
    let support = home.join("Library/Application Support");
    let (old, new) = (support.join("leaf"), support.join("subwy"));
    if new.exists() || !old.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(&new)?;
    for name in ["settings.json", "session.json"] {
        let from = old.join(name);
        if from.is_file() {
            std::fs::copy(&from, new.join(name))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_gives_the_defaults_without_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            load(&dir.path().join("settings.json")),
            (Settings::default(), None)
        );
    }

    #[test]
    fn fields_left_out_keep_their_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{ "tab_size": 2, "soft_wrap": "all" }"#).unwrap();
        let (settings, error) = load(&path);
        assert_eq!(error, None);
        assert_eq!(settings.tab_size, 2);
        assert_eq!(settings.soft_wrap, SoftWrap::All);
        assert_eq!(settings.editor_font, "SF Mono");
    }

    #[test]
    fn a_broken_or_unknown_field_falls_back_and_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{ "tab_sise": 2 }"#).unwrap();
        let (settings, error) = load(&path);
        assert_eq!(settings, Settings::default());
        assert!(error.unwrap().contains("tab_sise"));
        std::fs::write(&path, r#"{ "tab_size": 0 }"#).unwrap();
        assert!(load(&path).1.unwrap().contains("tab_size"));
    }

    #[test]
    fn key_overrides_read_with_and_without_a_context() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{ "keymap": [ { "key": "cmd-p", "action": "workspace::ToggleFinder", "context": "Workspace" },
                             { "key": "f2", "action": "project_panel::Rename" } ] }"#,
        )
        .unwrap();
        let (settings, error) = load(&path);
        assert_eq!(error, None);
        assert_eq!(settings.keymap.len(), 2);
        assert_eq!(settings.keymap[1].context, None);
    }

    #[test]
    fn the_default_file_is_written_once_and_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("subwy/settings.json");
        write_default_if_missing(&path).unwrap();
        std::fs::write(&path, r#"{ "tab_size": 8 }"#).unwrap();
        write_default_if_missing(&path).unwrap();
        assert_eq!(load(&path).0.tab_size, 8);
    }

    #[test]
    fn leaf_files_come_over_once() {
        let home = tempfile::tempdir().unwrap();
        let support = home.path().join("Library/Application Support");
        std::fs::create_dir_all(support.join("leaf")).unwrap();
        std::fs::write(support.join("leaf/settings.json"), r#"{ "tab_size": 8 }"#).unwrap();
        std::fs::write(support.join("leaf/session.json"), "{}").unwrap();
        adopt_leaf_files(home.path()).unwrap();
        assert_eq!(load(&support.join("subwy/settings.json")).0.tab_size, 8);
        assert!(support.join("subwy/session.json").is_file());
        assert!(support.join("leaf/settings.json").is_file());
        // Once subwy has its own folder, leaf's files are left alone.
        std::fs::write(support.join("leaf/settings.json"), r#"{ "tab_size": 2 }"#).unwrap();
        adopt_leaf_files(home.path()).unwrap();
        assert_eq!(load(&support.join("subwy/settings.json")).0.tab_size, 8);
    }

    #[test]
    fn nothing_to_bring_over_without_leaf() {
        let home = tempfile::tempdir().unwrap();
        adopt_leaf_files(home.path()).unwrap();
        assert!(
            !home
                .path()
                .join("Library/Application Support/subwy")
                .exists()
        );
    }

    #[test]
    fn soft_wrap_choices() {
        let mut settings = Settings::default();
        assert!(settings.wraps(true) && !settings.wraps(false));
        settings.soft_wrap = SoftWrap::All;
        assert!(settings.wraps(false));
        settings.soft_wrap = SoftWrap::None;
        assert!(!settings.wraps(true));
    }

    #[test]
    fn saved_settings_read_back_and_leave_no_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("subwy/settings.json");
        let settings = Settings {
            tab_size: 2,
            key_hints: false,
            ..Settings::default()
        };
        save(&path, &settings).unwrap();
        assert_eq!(load(&path), (settings, None));
        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["settings.json"]);
    }

    fn binding(key: &str, action: &str, context: Option<&str>) -> Binding {
        Binding {
            key: key.into(),
            action: action.into(),
            context: context.map(Into::into),
        }
    }

    fn defaults() -> Vec<Binding> {
        vec![
            binding("cmd-shift-f", "workspace::ToggleSearch", Some("Workspace")),
            binding("cmd-shift-f", "workspace::ToggleSearch", Some("Input")),
            binding("cmd-n", "project_panel::NewFile", Some("ProjectPanel")),
            binding("cmd-c", "project_panel::CopyItems", Some("ProjectPanel")),
        ]
    }

    #[test]
    fn a_new_key_replaces_the_own_ones_in_every_context() {
        let mut overrides = Vec::new();
        set_key(
            &mut overrides,
            &defaults(),
            "workspace::ToggleSearch",
            Some("Workspace"),
            "cmd-alt-f",
        );
        assert_eq!(
            keys_for(&defaults(), &overrides, "workspace::ToggleSearch"),
            ["cmd-alt-f"]
        );
        // Taken away and given in both contexts the default had.
        assert_eq!(overrides.len(), 4);
        assert_eq!(overrides.iter().filter(|o| o.unbind).count(), 2);
        // Another change replaces the first one rather than piling up.
        set_key(
            &mut overrides,
            &defaults(),
            "workspace::ToggleSearch",
            Some("Workspace"),
            "f3",
        );
        assert_eq!(
            keys_for(&defaults(), &overrides, "workspace::ToggleSearch"),
            ["f3"]
        );
        assert_eq!(overrides.len(), 4);
    }

    #[test]
    fn an_action_without_a_key_gets_one_in_its_fallback_context() {
        let mut overrides = Vec::new();
        set_key(
            &mut overrides,
            &defaults(),
            "project_panel::CopyTo",
            Some("ProjectPanel"),
            "f5",
        );
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0].context.as_deref(), Some("ProjectPanel"));
    }

    #[test]
    fn keys_clear_and_reset() {
        let mut overrides = Vec::new();
        clear_keys(&mut overrides, &defaults(), "project_panel::NewFile");
        assert!(keys_for(&defaults(), &overrides, "project_panel::NewFile").is_empty());
        reset_keys(&mut overrides, "project_panel::NewFile");
        assert_eq!(
            keys_for(&defaults(), &overrides, "project_panel::NewFile"),
            ["cmd-n"]
        );
        assert!(overrides.is_empty());
    }

    #[test]
    fn a_clash_is_the_same_key_in_the_same_context() {
        let mut overrides = Vec::new();
        set_key(
            &mut overrides,
            &defaults(),
            "project_panel::NewFile",
            Some("ProjectPanel"),
            "cmd-c",
        );
        assert_eq!(
            clashes(&defaults(), &overrides, "project_panel::NewFile", "cmd-c"),
            ["project_panel::CopyItems"]
        );
        // The same key in another context is no clash.
        assert!(
            clashes(
                &defaults(),
                &overrides,
                "workspace::ToggleSearch",
                "cmd-shift-f"
            )
            .is_empty()
        );
    }

    #[test]
    fn an_unbind_entry_reads_and_writes_only_when_set() {
        let entry = KeyOverride {
            key: "cmd-n".into(),
            action: "project_panel::NewFile".into(),
            context: None,
            unbind: false,
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(!json.contains("unbind"));
        let read: KeyOverride =
            serde_json::from_str(r#"{ "key": "cmd-n", "action": "a::B", "unbind": true }"#)
                .unwrap();
        assert!(read.unbind);
    }
}
