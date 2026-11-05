//! The Settings dialog's Keymap page: every subwy action with the keys it answers to, a search, and a
//! recorder. Changing a key writes overrides into the settings (subwy's own key is taken away, the
//! new one given) and the whole keymap is bound again at once.

use gpui_kit::{
    component::{
        input::{Input, InputEvent, InputState},
        kbd::Kbd,
    },
    prelude::FluentBuilder as _,
    *,
};
use subwy_project::settings as user;

use crate::{
    keymap, settings,
    theme::{self, c, size, z},
    update_settings,
};

/// The namespaces of subwy's actions, as the page groups them: (section, where a key given to an
/// action without one works).
fn section(action: &str) -> Option<(usize, &'static str, &'static str)> {
    match action.split_once("::")?.0 {
        "workspace" => Some((0, "Anywhere", keymap::WORKSPACE)),
        "project_panel" => Some((1, "Tree", keymap::PROJECT_PANEL)),
        "editor" => Some((2, "Editor", "Editor > Input")),
        "markdown" => Some((3, "Markdown", "Markdown > Input")),
        "terminal" => Some((4, "Terminal", keymap::TERMINAL)),
        _ => None,
    }
}

/// `project_panel::CopyRelativePath` as "Copy Relative Path".
fn label(action: &str) -> String {
    let name = action.rsplit_once("::").map_or(action, |(_, name)| name);
    let mut label = String::new();
    for ch in name.chars() {
        if ch.is_uppercase() && !label.is_empty() {
            label.push(' ');
        }
        label.push(ch);
    }
    label
}

/// Keys GPUI reports when a modifier is pressed and let go alone; they are not shortcuts here.
const LONE_MODIFIERS: &[&str] = &[
    "shift", "control", "alt", "platform", "function", "cmd", "ctrl",
];

struct Row {
    action: String,
    label: String,
    section: &'static str,
    keys: Vec<String>,
    changed: bool,
    clashes: Vec<String>,
}

pub struct KeymapView {
    search: Entity<InputState>,
    /// The action waiting for its new key, and what takes the next keystroke for it.
    recording: Option<(String, Subscription)>,
    _subscription: Subscription,
}

impl KeymapView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search actions and keys"));
        let subscription = cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        Self {
            search,
            recording: None,
            _subscription: subscription,
        }
    }

    /// Every subwy action, by section and name, that the search matches.
    fn rows(&self, cx: &App) -> Vec<Row> {
        let defaults = keymap::defaults();
        let overrides = &settings(cx).keymap;
        let query = self.search.read(cx).value().to_lowercase();
        let mut rows: Vec<(usize, Row)> = cx
            .all_action_names()
            .iter()
            .filter_map(|&action| {
                let (order, section, _) = section(action)?;
                let keys = user::keys_for(&defaults, overrides, action);
                let clashes = keys
                    .iter()
                    .flat_map(|key| user::clashes(&defaults, overrides, action, key))
                    .map(|other| label(&other))
                    .collect();
                let row = Row {
                    action: action.to_string(),
                    label: label(action),
                    section,
                    changed: overrides.iter().any(|o| o.action == action),
                    keys,
                    clashes,
                };
                Some((order, row))
            })
            .filter(|(_, row)| {
                query.is_empty()
                    || row.label.to_lowercase().contains(&query)
                    || row.section.to_lowercase().contains(&query)
                    || row.keys.iter().any(|key| {
                        key.contains(&query) || key_text(key, cx).to_lowercase().contains(&query)
                    })
            })
            .collect();
        rows.sort_by(|(a, row_a), (b, row_b)| a.cmp(b).then_with(|| row_a.label.cmp(&row_b.label)));
        rows.into_iter().map(|(_, row)| row).collect()
    }

    /// Waits for the next keystroke, anywhere, and makes it `action`'s key. It is taken before any
    /// binding runs, so Cmd+W can be recorded without closing the tab.
    fn start_recording(&mut self, action: String, cx: &mut Context<Self>) {
        let this = cx.weak_entity();
        let interceptor = cx.intercept_keystrokes(move |event, _, cx| {
            if LONE_MODIFIERS.contains(&event.keystroke.key.as_str()) {
                return;
            }
            let keystroke = event.keystroke.clone();
            let _ = this.update(cx, |this, cx| this.recorded(&keystroke, cx));
            cx.stop_propagation();
        });
        self.recording = Some((action, interceptor));
        cx.notify();
    }

    /// The key for the action being recorded; Escape alone cancels.
    fn recorded(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) {
        let Some((action, interceptor)) = self.recording.take() else {
            return;
        };
        // Not dropped inside its own callback.
        cx.defer(move |_| drop(interceptor));
        let modifiers = keystroke.modifiers;
        let bare = !(modifiers.platform || modifiers.control || modifiers.alt || modifiers.shift);
        if keystroke.key == "escape" && bare {
            cx.notify();
            return;
        }
        let key = keystroke.unparse(cx.platform_info().os);
        let fallback = section(&action).map(|(_, _, context)| context);
        let defaults = keymap::defaults();
        update_settings(cx, |s| {
            user::set_key(&mut s.keymap, &defaults, &action, fallback, &key)
        });
        cx.notify();
    }

    fn remove(&mut self, action: &str, cx: &mut Context<Self>) {
        let defaults = keymap::defaults();
        update_settings(cx, |s| user::clear_keys(&mut s.keymap, &defaults, action));
        cx.notify();
    }

    fn reset(&mut self, action: &str, cx: &mut Context<Self>) {
        update_settings(cx, |s| user::reset_keys(&mut s.keymap, action));
        cx.notify();
    }

    fn render_row(&self, index: usize, row: Row, cx: &mut Context<Self>) -> impl IntoElement {
        let p = theme::colors(cx);
        let recording = self
            .recording
            .as_ref()
            .is_some_and(|(action, _)| *action == row.action);
        let action = row.action.clone();
        let keys = div()
            .id(("keymap-keys", index))
            .flex_none()
            .min_w(z(160., cx))
            .h(z(26., cx))
            .px(z(8., cx))
            .flex()
            .items_center()
            .justify_end()
            .gap(z(6., cx))
            .rounded(z(4., cx))
            .cursor_pointer()
            .when(recording, |el| el.border_1().border_color(c(p.accent)))
            .when(!recording, |el| el.hover(|style| style.bg(c(p.hover))))
            .map(|el| {
                if recording {
                    el.text_color(c(p.accent_text))
                        .child("Press a key, Esc cancels")
                } else if row.keys.is_empty() {
                    el.text_color(c(p.ignored)).child("No key")
                } else {
                    el.children(row.keys.iter().filter_map(|key| {
                        Keystroke::parse_with_os(key, cx.platform_info().os)
                            .ok()
                            .map(Kbd::new)
                    }))
                }
            })
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.start_recording(action.clone(), cx)
            }));
        let remove = {
            let action = row.action.clone();
            text_button(("keymap-remove", index), "Remove", cx)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.remove(&action, cx)))
        };
        let reset = row.changed.then(|| {
            let action = row.action.clone();
            text_button(("keymap-reset", index), "Reset", cx)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.reset(&action, cx)))
        });
        div()
            .id(("keymap-row", index))
            .w_full()
            .min_h(z(34., cx))
            .py(z(4., cx))
            .flex()
            .items_center()
            .gap(z(12., cx))
            .border_b_1()
            .border_color(c(p.line))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(z(8., cx))
                            .child(div().text_color(c(p.ink)).child(row.label))
                            .child(
                                div()
                                    .text_size(z(11.5, cx))
                                    .text_color(c(p.sub))
                                    .child(row.section),
                            ),
                    )
                    .when(!row.clashes.is_empty(), |el| {
                        el.child(
                            div()
                                .text_size(z(11.5, cx))
                                .text_color(c(p.ansi[1]))
                                .child(format!("Same key as {}", row.clashes.join(", "))),
                        )
                    }),
            )
            .child(keys)
            .child(remove)
            .children(reset)
    }
}

/// A key as the page shows it: `cmd-shift-f` as ⇧⌘F.
fn key_text(key: &str, cx: &App) -> String {
    Keystroke::parse_with_os(key, cx.platform_info().os)
        .map(|stroke| Kbd::format(&stroke))
        .unwrap_or_else(|_| key.to_string())
}

fn text_button(id: impl Into<ElementId>, label: &'static str, cx: &App) -> Stateful<Div> {
    let p = theme::colors(cx);
    div()
        .id(id)
        .flex_none()
        .px(z(6., cx))
        .h(z(24., cx))
        .flex()
        .items_center()
        .rounded(z(4., cx))
        .text_size(z(12., cx))
        .text_color(c(p.sub))
        .cursor_pointer()
        .hover(|style| style.bg(c(p.hover)).text_color(c(p.ink)))
        .child(label)
}

impl Render for KeymapView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = theme::colors(cx);
        let rows = self.rows(cx);
        let rows: Vec<_> = rows
            .into_iter()
            .enumerate()
            .map(|(index, row)| self.render_row(index, row, cx).into_any_element())
            .collect();
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(z(8., cx))
            .text_size(z(size::UI_TEXT, cx))
            .child(
                div()
                    .h(z(32., cx))
                    .rounded(z(6., cx))
                    .border_1()
                    .border_color(c(p.line))
                    .child(
                        Input::new(&self.search)
                            .appearance(false)
                            .size_full()
                            .px(z(8., cx))
                            .text_size(z(size::UI_TEXT, cx)),
                    ),
            )
            .child(
                div()
                    .text_size(z(11.5, cx))
                    .text_color(c(p.sub))
                    .child("Click a key to change it, then press the new one."),
            )
            .child(div().flex().flex_col().children(rows))
    }
}

#[cfg(test)]
mod tests {
    use super::label;

    #[test]
    fn action_names_read_as_words() {
        assert_eq!(
            label("project_panel::CopyRelativePath"),
            "Copy Relative Path"
        );
        assert_eq!(label("workspace::Quit"), "Quit");
    }
}
