//! Local History (Alt+Shift+H): the active file's earlier saves, newest first, as stations on a line
//! that comes down from the sign; beside them the chosen one compared with the file now. Enter puts
//! its text back in the tab.

use std::{ops::Range, path::PathBuf, time::SystemTime};

use gpui_kit::component::{
    ActiveTheme as _,
    input::{Enter, Escape, MoveDown, MoveUp},
};
use gpui_kit::{assets::IconName, prelude::FluentBuilder as _, *};
use subwy_project::history::{self, Change, Comparison, Row};

use crate::{
    keymap::LOCAL_HISTORY,
    modal,
    theme::{self, c, size, z},
};

/// Sizes at 100% zoom.
const WIDTH: f32 = 960.;
/// Below the title bar, over the top of the tab bar, as the folder search.
const TOP_GAP: f32 = 8.;
const SIDE_GAP: f32 = 24.;
const BOTTOM_GAP: f32 = 24.;
const LIST_WIDTH: f32 = 300.;
/// A save is a station on the line that comes down from the sign.
const ROW_HEIGHT: f32 = 46.;
const PADDING_X: f32 = 12.;
const STATION_COLUMN: f32 = 2. * (modal::BEND_X - PADDING_X);
const STATION: f32 = 9.;
const STOP: f32 = 14.;
/// The file now: a transfer station's ring, as the outline's top heading.
const NOW: f32 = 15.;
const TIME_TEXT: f32 = 13.5;
const SMALL_TEXT: f32 = 11.5;
const HEADER_HEIGHT: f32 = 40.;
const LINE_HEIGHT: f32 = 21.;
const NUMBER_WIDTH: f32 = 44.;
const MARK_WIDTH: f32 = 24.;

pub enum HistoryEvent {
    /// Put this text back in the tab.
    Restore(String),
    /// Escape: close and give focus back to where it was.
    Dismiss,
}

/// An earlier save as its row shows it.
struct Stop {
    when: String,
    text: String,
    comparison: Comparison,
}

pub struct LocalHistory {
    /// The file's name, on the sign.
    name: String,
    /// Read off the main thread; `None` until then. Saves that match the file now are left out.
    stops: Option<Vec<Stop>>,
    /// Which stop is chosen; the row under the file now is the first.
    selected: usize,
    list_scroll: UniformListScrollHandle,
    diff_scroll: UniformListScrollHandle,
    focus_handle: FocusHandle,
    _load: Task<()>,
}

impl EventEmitter<HistoryEvent> for LocalHistory {}

impl Focusable for LocalHistory {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl LocalHistory {
    /// The history of `file` (absolute), named `name`, beside `now`, the tab's text.
    pub fn new(file: PathBuf, name: String, now: String, cx: &mut Context<Self>) -> Self {
        let load = cx.background_executor().spawn(async move {
            let time = SystemTime::now();
            history::stations(&history::default_dir(), &file)
                .into_iter()
                .filter_map(|station| {
                    let text = station.read().ok().filter(|text| *text != now)?;
                    Some(Stop {
                        when: history::when(station.saved, time),
                        comparison: history::compare(&text, &now),
                        text,
                    })
                })
                .collect::<Vec<_>>()
        });
        let _load = cx.spawn(async move |this, cx| {
            let stops = load.await;
            let _ = this.update(cx, |this, cx| {
                this.stops = Some(stops);
                cx.notify();
            });
        });
        Self {
            name,
            stops: None,
            selected: 0,
            list_scroll: UniformListScrollHandle::new(),
            diff_scroll: UniformListScrollHandle::new(),
            focus_handle: cx.focus_handle(),
            _load,
        }
    }

    /// How many earlier saves it lists, once read; for the UI checks.
    pub fn stops(&self) -> Option<usize> {
        self.stops.as_ref().map(Vec::len)
    }

    fn select(&mut self, stop: usize, cx: &mut Context<Self>) {
        if stop >= self.stops().unwrap_or(0) {
            return;
        }
        self.selected = stop;
        // The rows under the file now's row.
        self.list_scroll
            .scroll_to_item(stop + 1, ScrollStrategy::Nearest);
        self.diff_scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    fn restore(&mut self, cx: &mut Context<Self>) {
        if let Some(stop) = self.stops.as_ref().and_then(|s| s.get(self.selected)) {
            cx.emit(HistoryEvent::Restore(stop.text.clone()));
        }
    }

    fn render_row(&self, row: usize, cx: &mut Context<Self>) -> AnyElement {
        let p = theme::colors(cx);
        let color = c(p.route[modal::HISTORY_LINE]);
        let Some(stop) = row.checked_sub(1).and_then(|i| self.stops.as_ref()?.get(i)) else {
            return self
                .row(
                    div()
                        .size(z(NOW, cx))
                        .rounded_full()
                        .bg(c(p.paper))
                        .border(z(2.5, cx))
                        .border_color(c(p.ink)),
                    "Now".to_string(),
                    "In the editor".to_string(),
                    false,
                    cx,
                )
                .id("history-now")
                .into_any_element();
        };
        let selected = row - 1 == self.selected;
        let station = match selected {
            true => div()
                .size(z(STOP, cx))
                .rounded_full()
                .bg(color)
                .shadow(vec![BoxShadow {
                    color: color.opacity(0.22),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: z(5., cx),
                    inset: false,
                }]),
            false => div()
                .size(z(STATION, cx))
                .rounded_full()
                .bg(c(p.paper))
                .border(z(2., cx))
                .border_color(color),
        };
        let changed = stop.comparison.gone + stop.comparison.added;
        let stop_index = row - 1;
        self.row(
            station,
            stop.when.clone(),
            lines(changed, "changed"),
            selected,
            cx,
        )
        .id(("history-row", row))
        .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
            this.select(stop_index, cx);
            if event.click_count() >= 2 {
                this.restore(cx);
            }
        }))
        .into_any_element()
    }

    /// A row of the list: its station on the line, then when and what changed.
    fn row(&self, station: Div, title: String, detail: String, selected: bool, cx: &App) -> Div {
        let p = theme::colors(cx);
        let ink = c(if selected { p.sel_ink } else { p.ink });
        div()
            .w_full()
            .h(z(ROW_HEIGHT, cx))
            .pl(z(PADDING_X, cx))
            .pr(z(PADDING_X - 4., cx))
            .flex()
            .items_center()
            .whitespace_nowrap()
            .child(
                div()
                    .flex_none()
                    .w(z(STATION_COLUMN, cx))
                    .flex()
                    .justify_center()
                    .child(station),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h(z(ROW_HEIGHT - 6., cx))
                    .flex()
                    .flex_col()
                    .justify_center()
                    .pl(z(10., cx))
                    .rounded(z(6., cx))
                    .when(selected, |el| el.bg(c(p.sel_bg)))
                    .child(
                        div()
                            .text_size(z(TIME_TEXT, cx))
                            .text_color(ink)
                            .when(selected, |el| el.font_weight(FontWeight::SEMIBOLD))
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(z(SMALL_TEXT, cx))
                            .text_color(if selected { ink } else { c(p.sub) })
                            .child(detail),
                    ),
            )
    }

    /// The chosen save beside the file now: what went and what came, with lines around them.
    fn render_comparison(&self, cx: &mut Context<Self>) -> Div {
        let p = theme::colors(cx);
        let pane = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(c(p.line));
        let Some(stops) = &self.stops else {
            return pane;
        };
        let Some(stop) = stops.get(self.selected) else {
            return pane.child(
                div()
                    .p(z(16., cx))
                    .text_color(c(p.sub))
                    .child("No earlier saves of this file yet. Each save keeps one."),
            );
        };
        let comparison = &stop.comparison;
        pane.child(
            div()
                .flex_none()
                .h(z(HEADER_HEIGHT, cx))
                .flex()
                .items_center()
                .gap(z(10., cx))
                .px(z(16., cx))
                .border_b_1()
                .border_color(c(p.line))
                .text_size(z(12., cx))
                .text_color(c(p.sub))
                .child(
                    div()
                        .text_color(c(p.ink))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(stop.when.clone()),
                )
                .child("compared with")
                .child(
                    div()
                        .text_color(c(p.ink))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Now"),
                )
                .child(div().flex_1())
                .when(comparison.gone > 0, |el| {
                    el.child(
                        div()
                            .text_color(c(p.route[GONE]))
                            .child(lines(comparison.gone, "gone")),
                    )
                })
                .when(comparison.added > 0, |el| {
                    el.child(
                        div()
                            .text_color(c(p.route[ADDED]))
                            .child(lines(comparison.added, "new")),
                    )
                }),
        )
        .child(
            div()
                .flex_1()
                .min_h_0()
                .py(z(6., cx))
                .font_family(cx.theme().mono_font_family.clone())
                .text_size(z(theme::editor_text(cx), cx))
                .child(
                    uniform_list(
                        "history-diff",
                        comparison.rows.len(),
                        cx.processor(|this, range: Range<usize>, _, cx| {
                            range.map(|row| this.render_line(row, cx)).collect()
                        }),
                    )
                    .track_scroll(&self.diff_scroll)
                    .size_full(),
                ),
        )
    }

    fn render_line(&self, row: usize, cx: &mut Context<Self>) -> AnyElement {
        let p = theme::colors(cx);
        let line = div()
            .h(z(LINE_HEIGHT, cx))
            .flex()
            .items_center()
            .whitespace_nowrap();
        let Some(row) = self
            .stops
            .as_ref()
            .and_then(|stops| stops.get(self.selected))
            .and_then(|stop| stop.comparison.rows.get(row))
        else {
            return line.into_any_element();
        };
        match row {
            Row::Line {
                change,
                line: number,
                text,
            } => {
                let (mark, color) = match change {
                    Change::Same => ("", None),
                    Change::Gone => ("-", Some(c(p.route[GONE]))),
                    Change::Added => ("+", Some(c(p.route[ADDED]))),
                };
                line.when_some(color, |el, color| el.bg(color.opacity(0.13)))
                    .child(
                        div()
                            .flex_none()
                            .w(z(NUMBER_WIDTH, cx))
                            .flex()
                            .justify_end()
                            .text_color(c(p.num))
                            .child(number.map(|n| n.to_string()).unwrap_or_default()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .w(z(MARK_WIDTH, cx))
                            .flex()
                            .justify_center()
                            .when_some(color, |el, color| el.text_color(color))
                            .child(mark),
                    )
                    .child(text.clone())
                    .into_any_element()
            }
            Row::Skipped(count) => line
                .pl(z(NUMBER_WIDTH + MARK_WIDTH, cx))
                .text_color(c(p.ignored))
                .child(format!("⋯ {}", lines(*count, "unchanged")))
                .into_any_element(),
        }
    }
}

/// The line colors that mark a change: what went (the clay line) and what came (the leaf line).
const GONE: usize = 2;
const ADDED: usize = 0;

/// "1 line changed", "6 lines gone".
fn lines(count: usize, what: &str) -> String {
    match count {
        1 => format!("1 line {what}"),
        _ => format!("{count} lines {what}"),
    }
}

/// Places the history at the top center of the dimmed window, like the folder search. A click
/// beside it runs `on_outside`.
pub fn overlay(
    view: Entity<LocalHistory>,
    on_outside: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    modal::backdrop(on_outside, cx)
        .flex()
        .justify_center()
        .items_start()
        .pt(z(size::TITLE_BAR + TOP_GAP, cx))
        .child(view)
}

impl Render for LocalHistory {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = theme::colors(cx);
        let zoom = theme::zoom(cx);
        let viewport = window.viewport_size();
        let width = (WIDTH * zoom).min(viewport.width.as_f32() - 2. * SIDE_GAP * zoom);
        let height = viewport.height.as_f32() / zoom - size::TITLE_BAR - TOP_GAP - BOTTOM_GAP;
        let list = height - modal::SIGN_HEIGHT - (size::KEY_HINTS + 1.);
        // The file now, then its earlier saves.
        let rows = 1 + self.stops().unwrap_or(0);
        let shown = rows.min((list / ROW_HEIGHT).max(1.) as usize);
        // The line comes down from the sign to the last station in view.
        let last_stop = modal::SIGN_HEIGHT + ROW_HEIGHT * (shown as f32 - 0.5);
        let note = match self.stops() {
            Some(count) => format!("{}, {}", self.name, stop_count(count)),
            None => self.name.clone(),
        };
        modal::sheet(cx)
            .id("local-history")
            .track_focus(&self.focus_handle)
            .key_context(LOCAL_HISTORY)
            // Clicks inside stay here: the dimmed window behind would close it.
            .occlude()
            .on_action(
                cx.listener(|this, _: &MoveUp, _, cx| {
                    this.select(this.selected.wrapping_sub(1), cx)
                }),
            )
            .on_action(cx.listener(|this, _: &MoveDown, _, cx| this.select(this.selected + 1, cx)))
            .on_action(cx.listener(|this, _: &Enter, _, cx| this.restore(cx)))
            .on_action(cx.listener(|_, _: &Escape, _, cx| cx.emit(HistoryEvent::Dismiss)))
            .w(px(width.max(320.)))
            .h(z(height, cx))
            .child(modal::bend(modal::HISTORY_LINE, last_stop, cx))
            .text_size(z(size::UI_TEXT, cx))
            .text_color(c(p.ink))
            .child(
                modal::sign(
                    modal::HISTORY_LINE,
                    IconName::Clock,
                    "Local History",
                    false,
                    cx,
                )
                .child(modal::note(note, cx)),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        div().flex_none().w(z(LIST_WIDTH, cx)).h_full().child(
                            uniform_list(
                                "history-rows",
                                rows,
                                cx.processor(|this, range: Range<usize>, _, cx| {
                                    range.map(|row| this.render_row(row, cx)).collect()
                                }),
                            )
                            .track_scroll(&self.list_scroll)
                            .size_full(),
                        ),
                    )
                    .child(self.render_comparison(cx)),
            )
            .child(crate::project_panel::key_hints(
                &[("↑↓", "Choose"), ("↩", "Restore"), ("Esc", "Stay")],
                cx,
            ))
    }
}

/// "1 stop", "6 stops".
fn stop_count(count: usize) -> String {
    match count {
        1 => "1 stop".to_string(),
        _ => format!("{count} stops"),
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: GPUI's glob export includes a `test` macro that would replace the built-in one.
    use super::{lines, stop_count};

    #[test]
    fn counts_read_as_english() {
        assert_eq!(lines(1, "gone"), "1 line gone");
        assert_eq!(lines(6, "changed"), "6 lines changed");
        assert_eq!(stop_count(1), "1 stop");
        assert_eq!(stop_count(0), "0 stops");
    }
}
