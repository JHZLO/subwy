//! The one look of every modal (the file finder, the folder search, Settings), after a station
//! sign: the window dimmed behind, and on top a sheet whose head is the modal's line with its name
//! in a station's name box on it.

use gpui_kit::{assets::IconName, component::Icon, prelude::FluentBuilder as _, *};

use crate::theme::{self, c, z};

/// Each modal's line: an index into the palette's routes.
pub const FINDER_LINE: usize = 0;
pub const SEARCH_LINE: usize = 3;
pub const SETTINGS_LINE: usize = 1;

/// The sign at a sheet's top, and the line across its middle, as thick as the tree's routes.
pub const SIGN_HEIGHT: f32 = 58.;
pub const ROUTE: f32 = 3.;
/// Where a line that runs on into a list turns down: the list's station column.
pub const BEND_X: f32 = 26.;
const BEND_RADIUS: f32 = 16.;
/// The query field and the room around it.
pub const FIELD_HEIGHT: f32 = 40.;
pub const FIELD_SECTION: f32 = 2. + FIELD_HEIGHT + 12.;

fn dark(cx: &App) -> bool {
    theme::colors(cx).paper == theme::DARK.paper
}

/// The dimmed window behind a modal, over everything. A click on it, beside the sheet, runs
/// `on_outside`.
pub fn backdrop(
    on_outside: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Div {
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .occlude()
        .bg(hsla(0., 0., 0., if dark(cx) { 0.5 } else { 0.25 }))
        .on_mouse_down(MouseButton::Left, on_outside)
}

/// The sheet: paper in a 1px line, rounder than an island, lifted off the dimmed window by a
/// shadow (the one place subwy draws one: a modal floats).
pub fn sheet(cx: &App) -> Div {
    let p = theme::colors(cx);
    div()
        .relative()
        .flex()
        .flex_col()
        .overflow_hidden()
        .bg(c(p.paper))
        .border_1()
        .border_color(c(p.line))
        .rounded(z(12., cx))
        .shadow(vec![BoxShadow {
            color: hsla(0., 0., 0., if dark(cx) { 0.55 } else { 0.18 }),
            offset: point(px(0.), z(24., cx)),
            blur_radius: z(64., cx),
            spread_radius: px(0.),
            inset: false,
        }])
}

/// The station sign at a sheet's top: `title` in a name box on `line`, and room on the right for
/// [`note`]s and buttons. The line crosses it `straight`; otherwise the caller draws it turning
/// down at [`BEND_X`] into its list ([`bend`]) and the name box steps right of the turn.
pub fn sign(line: usize, icon: IconName, title: &'static str, straight: bool, cx: &App) -> Div {
    let p = theme::colors(cx);
    let color = c(p.route[line]);
    div()
        .relative()
        .flex_none()
        .h(z(SIGN_HEIGHT, cx))
        .flex()
        .items_center()
        .gap(z(8., cx))
        .pl(z(if straight { 20. } else { 44. }, cx))
        .pr(z(16., cx))
        .when(straight, |el| {
            el.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(z((SIGN_HEIGHT - ROUTE) / 2., cx))
                    .h(z(ROUTE, cx))
                    .bg(color),
            )
        })
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(z(8., cx))
                .py(z(5., cx))
                .pl(z(10., cx))
                .pr(z(16., cx))
                .bg(c(p.paper))
                .border(z(2.5, cx))
                .border_color(color)
                .rounded_full()
                .child(Icon::new(icon).size(z(15., cx)).text_color(color))
                .child(
                    div()
                        .text_size(z(15., cx))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(c(p.ink))
                        .child(title),
                ),
        )
        .child(div().flex_1())
}

/// The line of a [`sign`] that is not straight: across the sign from [`BEND_X`], then down to
/// `bottom` (from the sheet's top), rounding the turn. Drawn first, under the sign and the list.
pub fn bend(line: usize, bottom: f32, cx: &App) -> Div {
    let top = (SIGN_HEIGHT - ROUTE) / 2.;
    div()
        .absolute()
        .left(z(BEND_X - ROUTE / 2., cx))
        .top(z(top, cx))
        .right_0()
        .h(z((bottom - top).max(0.), cx))
        .border_t(z(ROUTE, cx))
        .border_l(z(ROUTE, cx))
        .border_color(c(theme::colors(cx).route[line]))
        .rounded_tl(z(BEND_RADIUS, cx))
}

/// A small box on a sign's line: a count, the folder searched.
pub fn note(text: impl Into<SharedString>, cx: &App) -> Div {
    let p = theme::colors(cx);
    div()
        .flex_none()
        .max_w(z(320., cx))
        .overflow_hidden()
        .text_ellipsis_start()
        .whitespace_nowrap()
        .py(z(3., cx))
        .px(z(12., cx))
        .bg(c(p.paper))
        .border(z(1.5, cx))
        .border_color(c(p.fold))
        .rounded_full()
        .text_size(z(12., cx))
        .text_color(c(p.sub))
        .child(text.into())
}

/// The query's field under the sign: a magnifier, then `input`, in a box lit by the line's color.
pub fn field(line: usize, input: impl IntoElement, cx: &App) -> Div {
    let p = theme::colors(cx);
    div()
        .h(z(FIELD_HEIGHT, cx))
        .flex()
        .items_center()
        .gap(z(8., cx))
        .pl(z(12., cx))
        .bg(c(p.side))
        .border_1()
        .border_color(c(p.fold))
        .rounded(z(10., cx))
        .shadow(vec![BoxShadow {
            color: c(p.route[line]).opacity(0.18),
            offset: point(px(0.), px(0.)),
            blur_radius: px(0.),
            spread_radius: z(3., cx),
            inset: false,
        }])
        .child(
            Icon::new(IconName::Search)
                .size(z(15., cx))
                .text_color(c(p.sub)),
        )
        .child(div().flex_1().min_w_0().child(input))
}
