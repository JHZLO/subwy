//! The one look of every modal (the file finder, the folder search, Settings), after a station
//! sign: the window dimmed behind, and on top a sheet of paper whose top edge is a band in the
//! color of its line.

use gpui_kit::*;

use crate::theme::{self, c, z};

/// Each modal's line: an index into the palette's routes.
pub const FINDER_LINE: usize = 0;
pub const SEARCH_LINE: usize = 3;
pub const SETTINGS_LINE: usize = 1;

/// The band across the sheet's top: as thick as the tree's routes.
pub const BAND: f32 = 3.;

/// The dimmed window behind a modal, over everything. A click on it, beside the sheet, runs
/// `on_outside`.
pub fn backdrop(
    on_outside: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Div {
    let p = theme::colors(cx);
    let dark = p.paper == theme::DARK.paper;
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .occlude()
        .bg(hsla(0., 0., 0., if dark { 0.5 } else { 0.25 }))
        .on_mouse_down(MouseButton::Left, on_outside)
}

/// The sheet: paper in a 1px line, rounded as the islands, its top a band of route `line`. No
/// shadow; the dimmed window sets it apart.
pub fn sheet(line: usize, cx: &App) -> Div {
    let p = theme::colors(cx);
    div()
        .flex()
        .flex_col()
        .overflow_hidden()
        .bg(c(p.paper))
        .border_1()
        .border_color(c(p.line))
        .rounded(z(8., cx))
        .child(div().flex_none().h(z(BAND, cx)).bg(c(p.route[line])))
}
