//! What a window shows before a folder is open (design doc 5.8): the recent folders as a network
//! map (5.17 F), or with none yet, a way to open one.

use std::path::PathBuf;

use gpui_kit::{TestSupportExt as _, prelude::FluentBuilder as _, *};
use subwy_project::{
    network::{self, Network},
    paths, session,
};

use crate::{
    keymap::OpenFolder,
    theme::{self, Palette, c, size, z},
};

/// The map's sizes at 100% zoom. Its stations sit `network`'s units apart, closer when the window
/// is too narrow for them.
const LABEL_WIDTH: f32 = 210.;
const LABEL_GAP: f32 = 20.;
/// Rows spread to fit the window between these.
const ROW_MOST: f32 = 130.;
const ROW_LEAST: f32 = 84.;
/// Room above the first line and below the last for the stations' names.
const MAP_TOP: f32 = 48.;
const MAP_BOTTOM: f32 = 64.;
/// Kept free around the map: the island's sides, and above and below it the title, the
/// greeting and the keys.
const MAP_SIDES: f32 = 96.;
const MAP_ABOVE_BELOW: f32 = 300.;
const LINE_WIDTH: f32 = 5.;
const STATION: f32 = 7.;
const CHOSEN: f32 = 9.;
const CHOSEN_HALO: f32 = 16.;
const INTERCHANGE: f32 = 10.;
const INTERCHANGE_BORDER: f32 = 3.;
/// From the line to a name below it (or above, at an interchange, whose line leaves downward).
const NAME_GAP: f32 = 16.;
const NAME_LINE: f32 = 18.;
const NAME_MOST: f32 = 150.;
/// A click on a station's dot as well as its name opens it.
const HIT: f32 = 26.;

/// The subwy mark (assets/brand/mark.svg): three route lines crossing at two interchanges, with
/// the station at the end of the first line, in the logo's bright colors.
pub fn subwy_mark(side: Pixels, cx: &App) -> impl IntoElement {
    let p = theme::colors(cx);
    let [first, second, third] = p.brand.map(c);
    let (ink, paper) = (c(p.ink), c(p.paper));
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let unit = bounds.size.width / 100.;
            let at = |x: f32, y: f32| point(bounds.origin.x + unit * x, bounds.origin.y + unit * y);
            let dot = |window: &mut Window, (x, y): (f32, f32), r: f32, color: Hsla| {
                let r = unit * r;
                let center = at(x, y);
                let corner = point(center.x - r, center.y - r);
                window.paint_quad(
                    fill(Bounds::new(corner, size(r * 2., r * 2.)), color).corner_radii(r),
                );
            };
            // Each line, with round ends like the SVG's.
            let lines: [(&[(f32, f32)], Hsla); 3] = [
                (&[(50., 12.), (50., 88.)], second),
                (&[(18., 76.), (82., 76.)], third),
                (&[(12., 62.), (36., 62.), (62., 36.), (82., 36.)], first),
            ];
            for (points, color) in lines {
                let mut path = PathBuilder::stroke(unit * 8.);
                path.move_to(at(points[0].0, points[0].1));
                for &(x, y) in &points[1..] {
                    path.line_to(at(x, y));
                }
                if let Ok(path) = path.build() {
                    window.paint_path(path, color);
                }
                for &end in [points[0], points[points.len() - 1]].iter() {
                    dot(window, end, 4., color);
                }
            }
            for ring in [(50., 48.), (50., 76.)] {
                let r = unit * 7.;
                let center = at(ring.0, ring.1);
                window.paint_quad(quad(
                    Bounds::new(point(center.x - r, center.y - r), size(r * 2., r * 2.)),
                    r,
                    paper,
                    unit * 3.,
                    ink,
                    BorderStyle::Solid,
                ));
            }
            dot(window, (82., 36.), 10.5, first.opacity(0.22));
            dot(window, (82., 36.), 6.5, first);
        },
    )
    .size(side)
}

fn keycap(key: &'static str, cx: &App) -> impl IntoElement {
    let p = theme::colors(cx);
    div()
        .px(z(5., cx))
        .rounded(px(4.))
        .border_1()
        .border_color(c(p.line))
        .bg(c(p.paper))
        .text_size(z(11., cx))
        .text_color(c(p.ink))
        .child(key)
}

/// How many recent folders the map shows: every one the session keeps.
pub const SHOWN_RECENT: usize = session::MAX_RECENT;

/// Where the map's parts are drawn, in pixels at the current zoom from the map's top left.
struct Fit {
    zoom: f32,
    /// How much of `network`'s spacing the window has room for.
    squeeze: f32,
    row: f32,
}

impl Fit {
    fn new(network: &Network, window: &Window, cx: &App) -> Self {
        let zoom = theme::zoom(cx);
        let viewport = window.viewport_size();
        let (width, height) = (
            f32::from(viewport.width) / zoom,
            f32::from(viewport.height) / zoom,
        );
        let room = width - MAP_SIDES - LABEL_WIDTH - LABEL_GAP;
        let squeeze = (room / network.width().max(1.)).clamp(0.5, 1.);
        let gaps = network.lines.len().saturating_sub(1).max(1) as f32;
        let room = height - MAP_ABOVE_BELOW - MAP_TOP - MAP_BOTTOM;
        let row = (room / gaps).clamp(ROW_LEAST, ROW_MOST);
        Self { zoom, squeeze, row }
    }

    fn x(&self, x: f32) -> f32 {
        (LABEL_WIDTH + LABEL_GAP + x * self.squeeze) * self.zoom
    }

    fn y(&self, line: usize) -> f32 {
        (MAP_TOP + self.row * line as f32) * self.zoom
    }

    fn px(&self, value: f32) -> Pixels {
        px(value * self.zoom)
    }

    fn size(&self, network: &Network) -> Size<Pixels> {
        let rows = network.lines.len().saturating_sub(1) as f32;
        size(
            px(self.x(network.width())),
            self.px(MAP_TOP + self.row * rows + MAP_BOTTOM),
        )
    }
}

/// The lines and their stations; the names are elements laid over it.
fn paint_map(network: Network, fit: Fit, selected: usize, p: &'static Palette) -> impl IntoElement {
    let color = move |line: usize| c(p.route[line % p.route.len()]);
    let (ink, paper) = (c(p.ink), c(p.paper));
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let at = |x: f32, y: f32| bounds.origin + point(px(x), px(y));
            let dot = |window: &mut Window, (x, y): (f32, f32), r: f32, color: Hsla| {
                let r = fit.px(r);
                window.paint_quad(
                    fill(
                        Bounds::new(at(x, y) - point(r, r), size(r * 2., r * 2.)),
                        color,
                    )
                    .corner_radii(r),
                );
            };
            for (i, line) in network.lines.iter().enumerate() {
                let y = fit.y(i);
                let end = (fit.x(line.end), y);
                let mut path = PathBuilder::stroke(fit.px(LINE_WIDTH));
                let start = match line.from {
                    // Down from the interchange, then right at 45 degrees, as the tree's route.
                    Some(from) => {
                        let x = fit.x(network.stations[from].x);
                        let bend = network::BEND * fit.zoom;
                        path.move_to(at(x, fit.y(network.stations[from].line)));
                        path.line_to(at(x, y - bend));
                        path.line_to(at(x + bend, y));
                        (x, fit.y(network.stations[from].line))
                    }
                    None => {
                        let x = fit.x(line.start);
                        path.move_to(at(x, y));
                        (x, y)
                    }
                };
                path.line_to(at(end.0, end.1));
                if let Ok(path) = path.build() {
                    window.paint_path(path, color(i));
                }
                // Round ends, as the logo's.
                for point in [start, end] {
                    dot(window, point, LINE_WIDTH / 2., color(i));
                }
            }
            for (i, station) in network.stations.iter().enumerate() {
                let center = (fit.x(station.x), fit.y(station.line));
                let line = color(station.line);
                if i == selected {
                    dot(window, center, CHOSEN_HALO, line.opacity(0.2));
                }
                if station.interchange {
                    let r = fit.px(INTERCHANGE);
                    window.paint_quad(quad(
                        Bounds::new(at(center.0, center.1) - point(r, r), size(r * 2., r * 2.)),
                        r,
                        paper,
                        fit.px(INTERCHANGE_BORDER),
                        ink,
                        BorderStyle::Solid,
                    ));
                } else {
                    let r = if i == selected { CHOSEN } else { STATION };
                    dot(window, center, r, line);
                }
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// The recent folders as a network map: a line for each folder they sit in, labeled on the left.
/// The arrows move along and across the lines; Enter or a click opens the chosen one.
fn map(
    recent: &[PathBuf],
    selected: usize,
    on_open: impl Fn(usize, &mut Window, &mut App) + Clone + 'static,
    window: &Window,
    cx: &App,
) -> impl IntoElement {
    let p = theme::colors(cx);
    let network = Network::new(recent);
    let fit = Fit::new(&network, window, cx);
    let name_width = (network::STEP * fit.squeeze - 16.).min(NAME_MOST) * fit.zoom;
    let labels = network.lines.iter().enumerate().map(|(i, line)| {
        div()
            .absolute()
            .left_0()
            .top(px(fit.y(i)) - fit.px(9.))
            .w(fit.px(LABEL_WIDTH))
            .flex()
            .justify_end()
            .text_size(fit.px(12.))
            .text_color(c(p.sub))
            .child(
                // A deep folder loses its start, keeping the folders nearest the line.
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis_start()
                    .child(paths::tilde(&line.parent)),
            )
    });
    let area = fit.size(&network);
    let stations = network
        .stations
        .iter()
        .enumerate()
        .flat_map(|(i, station)| {
            let chosen = i == selected;
            let (x, y) = (fit.x(station.x), fit.y(station.line));
            let name = recent[i]
                .file_name()
                .map(paths::display_name)
                .unwrap_or_else(|| paths::tilde(&recent[i]));
            let open_dot = on_open.clone();
            let open_name = on_open.clone();
            let hit = div()
                .id(("station-dot", i))
                .absolute()
                .left(px(x) - fit.px(HIT / 2.))
                .top(px(y) - fit.px(HIT / 2.))
                .size(fit.px(HIT))
                .rounded_full()
                .on_click(move |_, window, cx| open_dot(i, window, cx));
            let hint = div()
                .flex()
                .items_center()
                .gap(fit.px(4.))
                .text_size(fit.px(11.5))
                .text_color(c(p.sub))
                .child(keycap("↩", cx))
                .child("Open");
            let label = div()
                .px(fit.px(6.))
                .rounded(fit.px(4.))
                .max_w(px(name_width))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_color(c(p.ink))
                .when(chosen, |el| {
                    el.text_size(fit.px(14.)).font_weight(FontWeight::SEMIBOLD)
                })
                .when(!chosen && station.interchange, |el| {
                    el.font_weight(FontWeight::MEDIUM)
                })
                .hover(|style| style.bg(c(p.hover)))
                .child(name);
            // An interchange's line leaves downward, so its name and hint go above.
            let above = station.interchange;
            // Named by its folder, so the UI checks find it wherever the map lands.
            let name = div()
                .id(ElementId::Name(
                    format!("station {}", recent[i].display()).into(),
                ))
                .absolute()
                .left(px(x - name_width / 2.))
                .w(px(name_width))
                .when(!above, |el| el.top(px(y) + fit.px(NAME_GAP)))
                .when(above, |el| {
                    el.bottom(area.height - px(y) + fit.px(NAME_GAP))
                })
                .flex()
                .when(!above, |el| el.flex_col())
                .when(above, |el| el.flex_col_reverse())
                .items_center()
                .gap(fit.px(4.))
                .line_height(fit.px(NAME_LINE))
                .text_size(fit.px(13.))
                .child(label)
                .when(chosen, |el| el.child(hint))
                .on_click(move |_, window, cx| open_name(i, window, cx))
                .test_support();
            [hit.into_any_element(), name.into_any_element()]
        });
    let stations: Vec<AnyElement> = stations.collect();
    let labels: Vec<Div> = labels.collect();
    div()
        .relative()
        .flex_none()
        .w(area.width)
        .h(area.height)
        .child(paint_map(network, fit, selected, p))
        .children(labels)
        .children(stations)
}

/// `recent` are folders opened before (newest first); `selected` is the keyboard choice among them;
/// `on_open` opens the one at an index. With none, the window offers to open a folder.
pub fn render(
    recent: &[PathBuf],
    selected: usize,
    on_open: impl Fn(usize, &mut Window, &mut App) + Clone + 'static,
    window: &Window,
    cx: &App,
) -> AnyElement {
    if recent.is_empty() {
        return welcome(cx).into_any_element();
    }
    let p = theme::colors(cx);
    let hint = |key: &'static str, label: &'static str| {
        div()
            .flex()
            .items_center()
            .gap(z(5., cx))
            .child(keycap(key, cx))
            .child(label)
    };
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .bg(c(p.paper))
        .child(
            div()
                .pt(z(56., cx))
                .flex()
                .flex_col()
                .items_center()
                .gap(z(8., cx))
                .child(subwy_mark(z(40., cx), cx))
                .child(
                    div()
                        .text_size(z(22., cx))
                        .font_weight(FontWeight::MEDIUM)
                        .child("subwy"),
                )
                .child(
                    div()
                        .text_size(z(size::UI_TEXT, cx))
                        .text_color(c(p.sub))
                        .child("Pick a station to open its folder."),
                ),
        )
        .child(
            div()
                .flex_1()
                .w_full()
                .flex()
                .items_center()
                .justify_center()
                .overflow_hidden()
                .child(map(recent, selected, on_open, window, cx)),
        )
        .child(
            div()
                .pb(z(36., cx))
                .flex()
                .gap(z(18., cx))
                .text_size(z(11.5, cx))
                .text_color(c(p.sub))
                .child(hint("↑↓←→", "Move along the lines"))
                .child(hint("↩", "Open"))
                .child(hint("⌘O", "Another folder")),
        )
        .into_any_element()
}

/// Before any folder was opened: the mark, the Open folder button and a few keys.
fn welcome(cx: &App) -> impl IntoElement {
    let p = theme::colors(cx);
    let hint = |key: &'static str, label: &'static str| {
        div()
            .flex()
            .items_center()
            .gap(z(14., cx))
            .child(
                div()
                    .w(z(48., cx))
                    .flex()
                    .justify_end()
                    .child(keycap(key, cx)),
            )
            .child(label)
    };
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .bg(c(p.paper))
        .child(
            div()
                .w(z(380., cx))
                .flex()
                .flex_col()
                .items_center()
                .gap(z(12., cx))
                .child(subwy_mark(z(48., cx), cx))
                .child(
                    div()
                        .text_size(z(22., cx))
                        .font_weight(FontWeight::MEDIUM)
                        .child("subwy"),
                )
                .child(
                    div()
                        .text_size(z(13., cx))
                        .text_color(c(p.sub))
                        .child("Open a folder to walk its tree."),
                )
                .child(
                    div()
                        .id("open-folder")
                        .mt(z(10., cx))
                        .h(z(34., cx))
                        .pl(z(16., cx))
                        .pr(z(14., cx))
                        .flex()
                        .items_center()
                        .gap(z(10., cx))
                        .rounded(z(6., cx))
                        .border_1()
                        .border_color(c(p.line))
                        .bg(c(p.side))
                        .hover(|style| style.bg(c(p.hover)))
                        .text_size(z(13., cx))
                        .child("Open folder")
                        .child(keycap("⌘O", cx))
                        .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenFolder), cx)),
                )
                .child(
                    div()
                        .mt(z(22., cx))
                        .flex()
                        .flex_col()
                        .gap(z(8., cx))
                        .text_size(z(12., cx))
                        .text_color(c(p.sub))
                        .child(hint("⌘1", "Focus the tree"))
                        .child(hint("⌘⇧C", "Copy path"))
                        .child(hint("⌘=", "Zoom in")),
                ),
        )
}
