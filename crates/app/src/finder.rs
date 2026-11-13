//! The file finder (Cmd+Shift+O): type part of a path, pick a file, open it. Started with a colon
//! (Cmd+L), it asks for a line of the open file instead.

use std::{ops::Range, path::PathBuf, sync::Arc};

use gpui_kit::component::input::{Enter, Escape, Input, InputEvent, InputState, MoveDown, MoveUp};
use gpui_kit::{assets::IconName, prelude::FluentBuilder as _, *};
use subwy_project::{
    finder::{self, Route},
    folders::Folders,
    fuzzy,
    index::{self, FileIndex},
};

use crate::{
    modal,
    theme::{self, Palette, c, size, z},
};

/// Sizes at 100% zoom.
const WIDTH: f32 = 600.;
/// Below the title bar, over the top of the tab bar.
const TOP_GAP: f32 = 8.;
const INPUT_LINE: f32 = 20.;
const ROW_HEIGHT: f32 = 44.;
const PADDING_X: f32 = 14.;
const LIST_PADDING_Y: f32 = 4.;
const ROW_GAP: f32 = 14.;
const NAME_TEXT: f32 = 13.5;
const SMALL_TEXT: f32 = 11.5;
const TEXT_LINE: f32 = 17.;

/// The route drawn in front of each result, in its own box.
const ROUTE_WIDTH: f32 = 74.;
const ROUTE_HEIGHT: f32 = 16.;
/// The window folder's ring; the file's station lies `STATION_STEP` past the last ring.
const FIRST_RING: f32 = 6.;
const LAST_RING: f32 = 54.;
const STATION_STEP: f32 = 12.;
/// Rings no further apart than this, and no closer than touching: a deeper path loses its first
/// rings off the left, keeping the folders nearest the file (as its folder text does).
const RING_STEP_MOST: f32 = 24.;
const RING_STEP_LEAST: f32 = 10.;
const RING_RADIUS: f32 = 4.9;
const RING_BORDER: f32 = 1.8;
const STATION_RADIUS: f32 = 5.;
const LEG_WIDTH: f32 = 3.;

/// The most results shown.
const MAX_ROWS: usize = 12;

pub enum FinderEvent {
    /// Enter or a click picked this file, relative to the open folder.
    Open(PathBuf),
    /// Enter on `:line` or `:line:column`, both counted from one.
    GoToLine { line: u32, column: u32 },
    /// Escape: close and give focus back to where it was.
    Dismiss,
    /// Focus went elsewhere (a click outside, Cmd+1): close and leave it there.
    FocusLost,
}

pub struct Finder {
    input: Entity<InputState>,
    /// Empty until the walk of the folder, started when the finder opens, comes back.
    index: Option<Arc<FileIndex>>,
    found: fuzzy::Found,
    /// The name each route starts from; `None` when the paths start with their folder's name.
    window_folder: Option<String>,
    selected: usize,
    _load: Task<()>,
    /// The search for the latest query; replacing it drops the result of an older one.
    search: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<FinderEvent> for Finder {}

impl Focusable for Finder {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl Finder {
    pub fn new(folders: Folders, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let window_folder = (!folders.several()).then(|| folders.name().to_string());
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Go to file"));
        let subscriptions = vec![cx.subscribe_in(
            &input,
            window,
            |this, _, event: &InputEvent, _, cx| match event {
                InputEvent::Change => this.refilter(cx),
                InputEvent::Blur => cx.emit(FinderEvent::FocusLost),
                _ => {}
            },
        )];
        let load = cx.spawn(async move |this, cx| {
            let index = cx
                .background_spawn(async move { index::build_folders(&folders) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.index = Some(Arc::new(index));
                this.refilter(cx);
            });
        });
        Self {
            input,
            index: None,
            found: fuzzy::Found::default(),
            window_folder,
            selected: 0,
            _load: load,
            search: Task::ready(()),
            _subscriptions: subscriptions,
        }
    }

    /// Starts the finder on `query`, as Cmd+L does with a colon.
    pub fn set_query(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input
            .update(cx, |input, cx| input.set_value(query, window, cx));
        self.refilter(cx);
    }

    /// The query asks for a line, not a file.
    fn wants_line(&self, cx: &App) -> bool {
        self.input.read(cx).value().starts_with(':')
    }

    /// Searches off the main thread: a large folder takes a few milliseconds per keystroke.
    fn refilter(&mut self, cx: &mut Context<Self>) {
        if self.wants_line(cx) {
            self.search = Task::ready(());
            self.found = fuzzy::Found::default();
            cx.notify();
            return;
        }
        let Some(index) = self.index.clone() else {
            return;
        };
        let query = self.input.read(cx).value().to_string();
        let search =
            cx.background_spawn(async move { fuzzy::search(&query, &index.paths, MAX_ROWS) });
        self.search = cx.spawn(async move |this, cx| {
            let found = search.await;
            let _ = this.update(cx, |this, cx| {
                this.found = found;
                this.selected = 0;
                cx.notify();
            });
        });
    }

    /// Up and Down stop at the ends, as in the tree.
    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(last) = self.found.matches.len().checked_sub(1) else {
            return;
        };
        self.selected = self.selected.saturating_add_signed(delta).min(last);
        cx.notify();
    }

    fn open(&mut self, row: usize, cx: &mut Context<Self>) {
        if self.wants_line(cx) {
            let query = self.input.read(cx).value();
            if let Some((line, column)) = finder::line_query(&query) {
                cx.emit(FinderEvent::GoToLine { line, column });
            }
            return;
        }
        let (Some(index), Some(found)) = (&self.index, self.found.matches.get(row)) else {
            return;
        };
        cx.emit(FinderEvent::Open(PathBuf::from(&index.paths[found.index])));
    }

    fn render_row(&self, row: usize, route: Route, cx: &mut Context<Self>) -> Stateful<Div> {
        let p = theme::colors(cx);
        let selected = row == self.selected;
        let (ink, sub) = if selected {
            (p.sel_ink, p.sel_ink)
        } else {
            (p.ink, p.sub)
        };
        let styled = |text: String, marks: Vec<Range<usize>>, color: Option<u32>| {
            let matched = HighlightStyle {
                font_weight: Some(FontWeight::BOLD),
                color: color.map(c),
                ..Default::default()
            };
            StyledText::new(text).with_highlights(marks.into_iter().map(|range| (range, matched)))
        };
        let stops = match route.stops {
            1 => "1 stop".to_string(),
            n => format!("{n} stops"),
        };
        div()
            .id(("finder-row", row))
            .h(z(ROW_HEIGHT, cx))
            .px(z(PADDING_X, cx))
            .flex()
            .items_center()
            .gap(z(ROW_GAP, cx))
            .whitespace_nowrap()
            .overflow_hidden()
            .when(selected, |el| el.bg(c(p.sel_bg)))
            .when(!selected, |el| el.hover(|style| style.bg(c(p.hover))))
            .on_click(cx.listener(move |this, _, _, cx| this.open(row, cx)))
            .child(mini_route(route.stops, selected, p, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .line_height(z(TEXT_LINE, cx))
                    .child(
                        div()
                            .text_size(z(NAME_TEXT, cx))
                            .text_color(c(ink))
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(styled(route.name, route.name_marks, None)),
                    )
                    .child(
                        // A long way loses its start, keeping the folders nearest the file.
                        div()
                            .text_size(z(SMALL_TEXT, cx))
                            .text_color(c(sub))
                            .when(selected, |el| el.opacity(0.8))
                            .overflow_hidden()
                            .text_ellipsis_start()
                            .child(styled(route.via, route.via_marks, Some(ink))),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(z(SMALL_TEXT, cx))
                    .text_color(c(sub))
                    .child(stops),
            )
    }
}

/// The way to a file as a short line: a ring for each folder on the way, the window's own first,
/// each leg in its depth's route color, and the file's station in the color of the leg reaching it.
fn mini_route(stops: usize, selected: bool, p: &Palette, cx: &App) -> impl IntoElement {
    let unit = z(1., cx);
    let ring_ink = c(if selected { p.sel_ink } else { p.ink });
    let legs: Vec<Hsla> = (0..stops.max(1))
        .map(|depth| c(p.route[depth % p.route.len()]))
        .collect();
    let rings = ring_positions(stops);
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                let y = bounds.center().y;
                let at = |x: f32| point(bounds.left() + unit * x, y);
                let station = rings.last().map_or(FIRST_RING, |last| last + STATION_STEP);
                // Legs run between the rings' edges, so a ring's inside shows the row behind it.
                let ends: Vec<f32> = rings.iter().copied().chain([station]).collect();
                for (leg, pair) in ends.windows(2).enumerate() {
                    let to_station = leg + 2 == ends.len();
                    let mut path = PathBuilder::stroke(unit * LEG_WIDTH);
                    path.move_to(at(pair[0] + RING_RADIUS));
                    path.line_to(at(if to_station {
                        pair[1]
                    } else {
                        pair[1] - RING_RADIUS
                    }));
                    if let Ok(path) = path.build() {
                        window.paint_path(path, legs[leg]);
                    }
                }
                for &x in &rings {
                    let r = unit * RING_RADIUS;
                    window.paint_quad(quad(
                        Bounds::new(at(x) - point(r, r), size(r * 2., r * 2.)),
                        r,
                        transparent_black(),
                        unit * RING_BORDER,
                        ring_ink,
                        BorderStyle::Solid,
                    ));
                }
                let r = unit * STATION_RADIUS;
                let color = legs[ends.len().saturating_sub(2)];
                window.paint_quad(
                    fill(
                        Bounds::new(at(station) - point(r, r), size(r * 2., r * 2.)),
                        color,
                    )
                    .corner_radii(r),
                );
            });
        },
    )
    .flex_none()
    .w(z(ROUTE_WIDTH, cx))
    .h(z(ROUTE_HEIGHT, cx))
}

/// The x of each folder's ring in the route's box: spread over the box, at most
/// `RING_STEP_MOST` apart; past `RING_STEP_LEAST` the first ones fall off the left.
fn ring_positions(stops: usize) -> Vec<f32> {
    let gaps = stops.saturating_sub(1) as f32;
    let span = LAST_RING - FIRST_RING;
    let step = if gaps == 0. {
        0.
    } else {
        (span / gaps).min(RING_STEP_MOST)
    };
    if step >= RING_STEP_LEAST || gaps == 0. {
        (0..stops).map(|i| FIRST_RING + step * i as f32).collect()
    } else {
        (0..stops)
            .map(|i| LAST_RING - RING_STEP_LEAST * (gaps - i as f32))
            .collect()
    }
}

/// The finder at the top center of the dimmed window, over the tabs. A click beside it runs
/// `on_outside`.
pub fn overlay(
    finder: Entity<Finder>,
    on_outside: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    modal::backdrop(on_outside, cx)
        .flex()
        .justify_center()
        .items_start()
        .pt(z(size::TITLE_BAR + TOP_GAP, cx))
        .child(finder)
}

impl Render for Finder {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = theme::colors(cx);
        let rows: Vec<Stateful<Div>> = match self.index.clone() {
            Some(index) => self
                .found
                .matches
                .iter()
                .enumerate()
                .map(|(row, found)| {
                    let path = &index.paths[found.index];
                    let route =
                        finder::route(path, &found.positions, self.window_folder.as_deref());
                    self.render_row(row, route, cx)
                })
                .collect(),
            None => Vec::new(),
        };
        let wants_line = self.wants_line(cx);
        let stations = match (wants_line, self.found.total) {
            (true, _) => "line:column".to_string(),
            (false, 1) => "1 station".to_string(),
            (false, n) => format!("{n} stations"),
        };
        let input = Input::new(&self.input)
            .appearance(false)
            .pr(z(PADDING_X, cx))
            .text_size(z(size::UI_TEXT, cx))
            .line_height(z(INPUT_LINE, cx));
        // Input's own `h` sizes only multi-line inputs; the style's height sizes a single line.
        let input = Styled::h(input, z(INPUT_LINE + 4., cx));
        let title = match wants_line {
            true => "Go to Line",
            false => "Go to File",
        };
        modal::sheet(cx)
            .id("finder")
            // Clicks inside stay here: the dimmed window behind would close it.
            .occlude()
            .on_action(cx.listener(|this, _: &MoveUp, _, cx| this.move_selection(-1, cx)))
            .on_action(cx.listener(|this, _: &MoveDown, _, cx| this.move_selection(1, cx)))
            .on_action(cx.listener(|this, _: &Enter, _, cx| this.open(this.selected, cx)))
            .on_action(cx.listener(|_, _: &Escape, _, cx| cx.emit(FinderEvent::Dismiss)))
            .w(z(WIDTH, cx))
            .text_size(z(size::UI_TEXT, cx))
            .text_color(c(p.ink))
            .child(
                modal::sign(modal::FINDER_LINE, IconName::File, title, true, cx)
                    .when(self.index.is_some() || wants_line, |el| {
                        el.child(modal::note(stations, cx))
                    }),
            )
            .child(
                div()
                    .flex_none()
                    .pt(z(2., cx))
                    .pb(z(12., cx))
                    .px(z(16., cx))
                    .when(!rows.is_empty(), |el| {
                        el.border_b_1().border_color(c(p.line))
                    })
                    .child(modal::field(modal::FINDER_LINE, input, cx)),
            )
            .when(!rows.is_empty(), |el| {
                el.child(div().py(z(LIST_PADDING_Y, cx)).children(rows))
                    .child(crate::project_panel::key_hints(
                        &[("↑↓", "Choose"), ("↩", "Get off here"), ("Esc", "Stay")],
                        cx,
                    ))
            })
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: GPUI's glob export includes a `test` macro that would replace the built-in one.
    use super::ring_positions;

    #[test]
    fn rings_spread_over_the_box_up_to_a_step() {
        assert_eq!(ring_positions(1), [6.]);
        assert_eq!(ring_positions(2), [6., 30.]);
        assert_eq!(ring_positions(3), [6., 30., 54.]);
        assert_eq!(ring_positions(4), [6., 22., 38., 54.]);
    }

    #[test]
    fn a_deep_path_keeps_the_rings_nearest_the_file() {
        let rings = ring_positions(8);
        assert_eq!(rings.len(), 8);
        assert_eq!(rings[7], 54.);
        assert_eq!(rings[6], 44.);
        assert!(rings[0] < 0.);
    }
}
