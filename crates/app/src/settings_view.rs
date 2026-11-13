//! The Settings dialog (Cmd+,): the settings file's choices as switches, numbers and lists, applied
//! to every window at once and written back to the file. Built on gpui-kit's settings page: a sidebar
//! of pages with search, and a reset to the default beside each changed item.

use gpui_kit::{
    assets::IconName,
    component::{
        Icon,
        setting::{
            NumberFieldOptions, SettingField, SettingGroup, SettingItem, SettingPage,
            Settings as Pages,
        },
    },
    *,
};
use subwy_project::settings::{Appearance, Settings, SoftWrap};

use crate::{
    keymap::{CloseSettings, SETTINGS},
    keymap_view::KeymapView,
    modal, settings, theme,
    theme::{c, z},
    update_settings,
};

pub struct SettingsView {
    focus_handle: FocusHandle,
    keymap: Entity<KeymapView>,
}

impl SettingsView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            keymap: cx.new(|cx| KeymapView::new(window, cx)),
        }
    }
}

impl EventEmitter<CloseSettings> for SettingsView {}

/// The dialog's size at 100% zoom; a smaller window leaves a margin around it.
const DIALOG_WIDTH: f32 = 960.;
const DIALOG_HEIGHT: f32 = 680.;
const DIALOG_MARGIN: f32 = 32.;

/// The Settings dialog over the window: the window dimmed behind, the settings on a sheet of paper
/// in the middle. `on_outside` runs for a click beside the sheet.
pub fn dialog(
    view: Entity<SettingsView>,
    on_outside: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    window: &Window,
    cx: &App,
) -> impl IntoElement {
    let zoom = theme::zoom(cx);
    let viewport = window.viewport_size();
    let width = (DIALOG_WIDTH * zoom).min(viewport.width.as_f32() - 2. * DIALOG_MARGIN);
    let height = (DIALOG_HEIGHT * zoom).min(viewport.height.as_f32() - 2. * DIALOG_MARGIN);
    modal::backdrop(on_outside, cx)
        .flex()
        .items_center()
        .justify_center()
        .child(
            modal::sheet(cx)
                .w(px(width.max(320.)))
                .h(px(height.max(240.)))
                // A click on the sheet stays there.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(div().flex_1().min_h_0().child(view)),
        )
}

impl Focusable for SettingsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// Monospaced fonts macOS has or people often install; the list shows the ones on this Mac.
const FONTS: &[&str] = &[
    "SF Mono",
    "Menlo",
    "Monaco",
    "Courier New",
    "JetBrains Mono",
    "Fira Code",
    "Source Code Pro",
    "IBM Plex Mono",
    "Cascadia Code",
    "Hack",
    "D2Coding",
];

/// The editor fonts to offer: the installed ones of [`FONTS`], and the one in use if it is not
/// among them (typed into the file by hand).
fn font_choices(cx: &App) -> Vec<(SharedString, SharedString)> {
    let installed = cx.text_system().all_font_names();
    let current = settings(cx).editor_font.clone();
    let mut fonts: Vec<String> = FONTS
        .iter()
        .filter(|font| **font == "SF Mono" || installed.iter().any(|name| name == *font))
        .map(|font| font.to_string())
        .collect();
    if !fonts.contains(&current) {
        fonts.push(current);
    }
    fonts
        .into_iter()
        .map(|font| (font.clone().into(), font.into()))
        .collect()
}

fn soft_wrap_value(wrap: SoftWrap) -> SharedString {
    match wrap {
        SoftWrap::Markdown => "markdown",
        SoftWrap::All => "all",
        SoftWrap::None => "none",
    }
    .into()
}

/// A switch that reads and writes one setting.
fn switch(
    title: &'static str,
    description: &'static str,
    get: fn(&Settings) -> bool,
    set: fn(&mut Settings, bool),
) -> SettingItem {
    let default = get(&Settings::default());
    SettingItem::new(
        title,
        SettingField::switch(
            move |cx| get(settings(cx)),
            move |value, cx| update_settings(cx, |s| set(s, value)),
        )
        .default_value(default),
    )
    .description(description)
}

fn editor_page(cx: &App) -> SettingPage {
    let defaults = Settings::default();
    SettingPage::new("Editor").groups([
        SettingGroup::new().title("Font").items([
            SettingItem::new(
                "Font",
                SettingField::scrollable_dropdown(
                    font_choices(cx),
                    |cx| settings(cx).editor_font.clone().into(),
                    |value, cx| update_settings(cx, |s| s.editor_font = value.to_string()),
                )
                .default_value(defaults.editor_font.clone()),
            )
            .description("The editor's monospaced font; SF Mono is the system's."),
            SettingItem::new(
                "Font size",
                SettingField::number_input(
                    NumberFieldOptions {
                        min: 6.,
                        max: 72.,
                        step: 1.,
                    },
                    |cx| settings(cx).editor_font_size as f64,
                    |value, cx| update_settings(cx, |s| s.editor_font_size = value as f32),
                )
                .default_value(defaults.editor_font_size as f64),
            )
            .description("Points at 100% zoom (Cmd+= and Cmd+- zoom the whole window)."),
        ]),
        SettingGroup::new().title("Text").items([
            SettingItem::new(
                "Soft wrap",
                SettingField::dropdown(
                    vec![
                        ("markdown".into(), "Markdown files".into()),
                        ("all".into(), "All files".into()),
                        ("none".into(), "None".into()),
                    ],
                    |cx| soft_wrap_value(settings(cx).soft_wrap),
                    |value, cx| {
                        let wrap = match value.as_ref() {
                            "all" => SoftWrap::All,
                            "none" => SoftWrap::None,
                            _ => SoftWrap::Markdown,
                        };
                        update_settings(cx, |s| s.soft_wrap = wrap)
                    },
                )
                .default_value(soft_wrap_value(defaults.soft_wrap)),
            )
            .description("Which files wrap long lines at the window's edge."),
            SettingItem::new(
                "Tab size",
                SettingField::number_input(
                    NumberFieldOptions {
                        min: 1.,
                        max: 16.,
                        step: 1.,
                    },
                    |cx| settings(cx).tab_size as f64,
                    |value, cx| update_settings(cx, |s| s.tab_size = value as usize),
                )
                .default_value(defaults.tab_size as f64),
            )
            .description("How wide a tab is, and how many spaces Tab puts in code."),
            switch(
                "Indent with tabs",
                "Tab puts a tab character instead of spaces.",
                |s| s.hard_tabs,
                |s, on| s.hard_tabs = on,
            ),
        ]),
        SettingGroup::new().title("Saving").items([switch(
            "Save automatically",
            "Edited files save when subwy loses focus, and before a window closes or subwy quits.",
            |s| s.auto_save,
            |s, on| s.auto_save = on,
        )]),
        SettingGroup::new().title("Display").items([
            switch(
                "Line numbers",
                "Numbers in the gutter left of the text.",
                |s| s.line_numbers,
                |s, on| s.line_numbers = on,
            ),
            switch(
                "Show whitespace",
                "Spaces and tabs as faint marks.",
                |s| s.show_whitespace,
                |s, on| s.show_whitespace = on,
            ),
            switch(
                "Indent guides",
                "Faint lines down each indent level.",
                |s| s.indent_guides,
                |s, on| s.indent_guides = on,
            ),
            switch(
                "Code folding",
                "Blocks fold and unfold from the gutter.",
                |s| s.folding,
                |s, on| s.folding = on,
            ),
            switch(
                "Outline",
                "A Markdown file's headings as stations on a line beside the text.",
                |s| s.outline,
                |s, on| s.outline = on,
            ),
        ]),
    ])
}

fn appearance_value(appearance: Appearance) -> SharedString {
    match appearance {
        Appearance::System => "system",
        Appearance::Light => "light",
        Appearance::Dark => "dark",
    }
    .into()
}

fn appearance_page() -> SettingPage {
    SettingPage::new("Appearance").group(
        SettingGroup::new().items([
            SettingItem::new(
                "Appearance",
                SettingField::dropdown(
                    vec![
                        ("system".into(), "As macOS".into()),
                        ("light".into(), "Light".into()),
                        ("dark".into(), "Dark".into()),
                    ],
                    |cx| appearance_value(settings(cx).appearance),
                    |value, cx| {
                        let appearance = match value.as_ref() {
                            "light" => Appearance::Light,
                            "dark" => Appearance::Dark,
                            _ => Appearance::System,
                        };
                        update_settings(cx, |s| s.appearance = appearance)
                    },
                )
                .default_value(appearance_value(Appearance::System)),
            )
            .description("Light or dark paper; As macOS follows the system as it changes."),
            SettingItem::new(
                "Zoom",
                SettingField::dropdown(
                    theme::ZOOM_STEPS
                        .iter()
                        .map(|step| (step.to_string().into(), format!("{step}%").into()))
                        .collect(),
                    |cx| {
                        let zoom = cx.try_global::<theme::Zoom>().map_or(100, |zoom| zoom.0);
                        zoom.to_string().into()
                    },
                    |value, cx| {
                        if let Ok(percent) = value.parse() {
                            theme::set_zoom(percent, cx);
                            cx.defer(crate::workspace::zoom_changed);
                        }
                    },
                )
                .default_value(SharedString::from("100")),
            )
            .description(
                "The whole window, as Cmd+= and Cmd+- step it; kept with the open windows.",
            ),
        ]),
    )
}

fn tree_page() -> SettingPage {
    SettingPage::new("Tree").group(SettingGroup::new().items([
        switch(
            "Key hints",
            "The line under the tree with the keys you can press now.",
            |s| s.key_hints,
            |s, on| s.key_hints = on,
        ),
        switch(
            "Show ignored files",
            "Gitignored items, dimmed. Off leaves them out of the tree.",
            |s| s.show_ignored,
            |s, on| s.show_ignored = on,
        ),
        switch(
            "Always select opened file",
            "Switching tabs selects the file in the tree, opening its folders.",
            |s| s.follow_active_file,
            |s, on| s.follow_active_file = on,
        ),
        switch(
            "Open files with a single click",
            "One click opens a file; otherwise it takes two.",
            |s| s.single_click_open,
            |s, on| s.single_click_open = on,
        ),
    ]))
}

fn terminal_page() -> SettingPage {
    SettingPage::new("Terminal").group(SettingGroup::new().items([
        switch(
            "Open with the folder",
            "A window that opens a folder starts a shell under the editor; off, Alt+F12 starts it.",
            |s| s.terminal_on_start,
            |s, on| s.terminal_on_start = on,
        ),
        SettingItem::new(
            "Shell",
            SettingField::input(
                |cx| settings(cx).terminal_shell.clone().into(),
                |value, cx| update_settings(cx, |s| s.terminal_shell = value.to_string()),
            )
            .default_value(SharedString::default()),
        )
        .description("The program new terminal tabs run, such as /bin/zsh; empty is your login shell."),
        SettingItem::new(
            "Font size",
            SettingField::number_input(
                NumberFieldOptions {
                    min: 6.,
                    max: 72.,
                    step: 1.,
                },
                |cx| theme::terminal_text(cx) as f64,
                |value, cx| update_settings(cx, |s| s.terminal_font_size = Some(value as f32)),
            )
            // Back to the editor's size, whatever it is then.
            .on_reset(
                |cx| settings(cx).terminal_font_size.is_some(),
                |_, cx| update_settings(cx, |s| s.terminal_font_size = None),
            ),
        )
        .description("Points at 100% zoom; the editor's size until set."),
    ]))
}

fn startup_page() -> SettingPage {
    SettingPage::new("Startup").group(SettingGroup::new().item(switch(
        "Reopen windows",
        "subwy starts with the folders it had open when it quit.",
        |s| s.reopen_windows,
        |s, on| s.reopen_windows = on,
    )))
}

fn keymap_page(view: Entity<KeymapView>) -> SettingPage {
    SettingPage::new("Keymap").group(
        SettingGroup::new().item(
            SettingItem::render(move |_, _, _| view.clone())
                .keywords(["key", "shortcut", "binding", "keymap"]),
        ),
    )
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = theme::colors(cx);
        div()
            .key_context(SETTINGS)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_, _: &CloseSettings, _, cx| cx.emit(CloseSettings)))
            .size_full()
            .flex()
            .flex_col()
            .bg(c(p.paper))
            .text_size(z(theme::size::UI_TEXT, cx))
            .child(
                modal::sign(
                    modal::SETTINGS_LINE,
                    IconName::Settings,
                    "Settings",
                    true,
                    cx,
                )
                .child(
                    div()
                        .id("settings-close")
                        .size(z(22., cx))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(z(4., cx))
                        // It sits on the sign's line, as the name box does.
                        .bg(c(p.paper))
                        .text_color(c(p.sub))
                        .hover(|style| style.bg(c(p.hover)).text_color(c(p.ink)))
                        .child(Icon::new(IconName::X).size(z(14., cx)))
                        .on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(CloseSettings))),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(Pages::new("subwy-settings").pages([
                        editor_page(cx),
                        appearance_page(),
                        tree_page(),
                        terminal_page(),
                        startup_page(),
                        keymap_page(self.keymap.clone()),
                    ])),
            )
    }
}
