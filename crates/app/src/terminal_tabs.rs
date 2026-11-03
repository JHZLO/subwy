//! The terminal island's tabs, as in IntelliJ's Terminal: several shells in the open folder, one
//! shown at a time. Cmd+T adds one (anywhere in the window), Cmd+W closes the one in front,
//! Cmd+Shift+[ and ] step between them, and a shell that exits takes its tab with it.

use std::path::{Path, PathBuf};

use gpui_kit::{prelude::FluentBuilder as _, *};

use crate::{
    keymap::{CloseTerminal, NextTerminal, PrevTerminal},
    terminal_panel::{TerminalEvent, TerminalPanel},
    theme::{self, c, size, z},
};

struct Tab {
    view: Entity<TerminalPanel>,
    /// Names the tab: 1 is "Local", n is "Local (n)". A closed tab's number is free again.
    number: usize,
    _subscription: Subscription,
}

impl Tab {
    fn title(&self) -> String {
        match self.number {
            1 => "Local".to_string(),
            n => format!("Local ({n})"),
        }
    }
}

pub enum TerminalTabsEvent {
    /// The last tab closed. `had_focus` says whether the terminal had the keyboard, so it can go
    /// to the editor.
    Empty { had_focus: bool },
    /// A tab's shell took the keyboard.
    Focused,
}

pub struct TerminalTabs {
    cwd: PathBuf,
    tabs: Vec<Tab>,
    active: usize,
}

impl EventEmitter<TerminalTabsEvent> for TerminalTabs {}

impl TerminalTabs {
    /// No shells yet: [`add`](Self::add) starts the first.
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            cwd,
            tabs: Vec::new(),
            active: 0,
        }
    }

    /// Starts a shell in a new tab after the others and brings it to the front. False when the
    /// shell could not start.
    pub fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let cwd = self.cwd.clone();
        self.add_in(&cwd, window, cx)
    }

    /// [`add`](Self::add), with the shell starting in `cwd` (the tree's Open in Terminal).
    pub fn add_in(&mut self, cwd: &Path, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let shell = crate::settings(cx).terminal_shell.trim().to_string();
        let started =
            match TerminalPanel::start(cwd, Some(shell.as_str()).filter(|s| !s.is_empty())) {
                Ok(started) => started,
                Err(err) => {
                    eprintln!("subwy: could not start the shell: {err}");
                    return false;
                }
            };
        let view = cx.new(|cx| TerminalPanel::new(started, window, cx));
        let subscription = cx.subscribe_in(
            &view,
            window,
            |this, view, event: &TerminalEvent, window, cx| match event {
                TerminalEvent::Exited => {
                    if let Some(index) = this.tabs.iter().position(|tab| &tab.view == view) {
                        this.close(index, window, cx);
                    }
                }
                TerminalEvent::Focused => cx.emit(TerminalTabsEvent::Focused),
            },
        );
        let number = (1..)
            .find(|n| self.tabs.iter().all(|tab| tab.number != *n))
            .unwrap_or(1);
        self.tabs.push(Tab {
            view,
            number,
            _subscription: subscription,
        });
        self.active = self.tabs.len() - 1;
        cx.notify();
        true
    }

    /// Starts a tab in each of `folders` (the session's), with `active` in front. False when no
    /// shell could start.
    pub fn restore(
        &mut self,
        folders: &[PathBuf],
        active: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        for folder in folders {
            self.add_in(folder, window, cx);
        }
        self.active = active.min(self.tabs.len().saturating_sub(1));
        !self.tabs.is_empty()
    }

    /// Each tab's folder now, in order, and the tab in front: what the session keeps.
    pub fn session(&self, cx: &App) -> (Vec<PathBuf>, usize) {
        let folders = self
            .tabs
            .iter()
            .map(|tab| tab.view.read(cx).cwd().unwrap_or_else(|| self.cwd.clone()))
            .collect();
        (folders, self.active)
    }

    /// Where the keyboard goes for the terminal: the tab in front.
    pub fn focus_handle(&self, cx: &App) -> Option<FocusHandle> {
        let tab = self.tabs.get(self.active)?;
        Some(tab.view.read(cx).focus_handle(cx))
    }

    pub fn contains_focused(&self, window: &Window, cx: &App) -> bool {
        self.tabs.iter().any(|tab| {
            tab.view
                .read(cx)
                .focus_handle(cx)
                .contains_focused(window, cx)
        })
    }

    /// Pastes `text` into the tab in front.
    pub fn paste_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get(self.active) {
            tab.view.update(cx, |panel, cx| panel.paste_text(text, cx));
        }
    }

    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    /// The text of the tab in front, one string per line; for the UI checks.
    pub fn visible_lines(&self, cx: &App) -> Vec<String> {
        self.tabs
            .get(self.active)
            .map(|tab| tab.view.read(cx).visible_lines(cx))
            .unwrap_or_default()
    }

    /// Closes a tab and its shell. The tab that takes its place comes to the front, with the
    /// keyboard if the closed one had it.
    fn close(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let had_focus = self.contains_focused(window, cx);
        self.tabs.remove(index);
        if self.tabs.is_empty() {
            cx.emit(TerminalTabsEvent::Empty { had_focus });
            return;
        }
        if index < self.active || self.active >= self.tabs.len() {
            self.active -= 1;
        }
        if had_focus {
            self.focus_active(window, cx);
        }
        cx.notify();
    }

    fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.active = index;
        self.focus_active(window, cx);
        cx.notify();
    }

    /// The next or previous tab, around the ends.
    fn step(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.tabs.len() as isize;
        if count > 1 {
            let index = (self.active as isize + delta).rem_euclid(count) as usize;
            self.activate(index, window, cx);
        }
    }

    fn focus_active(&self, window: &mut Window, cx: &mut App) {
        if let Some(handle) = self.focus_handle(cx) {
            handle.focus(window, cx);
        }
    }

    fn render_tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = theme::colors(cx);
        let tabs: Vec<_> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                let active = index == self.active;
                div()
                    .id(("terminal-tab", tab.number))
                    .relative()
                    .flex_none()
                    .h_full()
                    .flex()
                    .items_center()
                    .gap(z(6., cx))
                    .pl(z(12., cx))
                    .pr(z(6., cx))
                    .text_color(c(if active { p.ink } else { p.sub }))
                    .when(!active, |el| el.hover(|style| style.bg(c(p.hover))))
                    .child(tab.title())
                    .child(
                        div()
                            .id(("terminal-close", tab.number))
                            .size(z(16., cx))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(z(4., cx))
                            .text_color(c(p.sub))
                            .hover(|style| style.bg(c(p.guide)).text_color(c(p.ink)))
                            .child("×")
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                cx.stop_propagation();
                                if index < this.tabs.len() {
                                    this.close(index, window, cx);
                                }
                            })),
                    )
                    .when(active, |el| {
                        el.child(
                            div()
                                .absolute()
                                .left(z(12., cx))
                                .right(z(6., cx))
                                .bottom_0()
                                .h(z(2., cx))
                                .rounded(z(1., cx))
                                .bg(c(p.ink)),
                        )
                    })
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        if index < this.tabs.len() {
                            this.activate(index, window, cx);
                        }
                    }))
            })
            .collect();
        div()
            .h(z(size::TERMINAL_TABS, cx))
            .flex_none()
            .flex()
            .items_center()
            .pl(z(4., cx))
            .border_b_1()
            .border_color(c(p.line))
            .text_size(z(12.5, cx))
            .children(tabs)
            .child(
                div()
                    .id("terminal-new")
                    .ml(z(4., cx))
                    .size(z(22., cx))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(z(4., cx))
                    .text_color(c(p.sub))
                    .hover(|style| style.bg(c(p.hover)).text_color(c(p.ink)))
                    .child("+")
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        if this.add(window, cx) {
                            this.focus_active(window, cx);
                        }
                    })),
            )
    }
}

impl Render for TerminalTabs {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let front = self.tabs.get(self.active).map(|tab| tab.view.clone());
        div()
            .size_full()
            .flex()
            .flex_col()
            .on_action(cx.listener(|this, _: &CloseTerminal, window, cx| {
                this.close(this.active, window, cx)
            }))
            .on_action(cx.listener(|this, _: &PrevTerminal, window, cx| this.step(-1, window, cx)))
            .on_action(cx.listener(|this, _: &NextTerminal, window, cx| this.step(1, window, cx)))
            .child(self.render_tab_bar(cx))
            .children(front.map(|view| div().flex_1().min_h_0().child(view)))
    }
}
