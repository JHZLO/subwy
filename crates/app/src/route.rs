//! The route in the tree: a line from the top of the tree down to the selection, drawn like a
//! metro map. Each folder on the way runs its own colored line down its indent column and bends
//! 45 degrees into the next folder's column; folders where two lines meet are rings, and the
//! selection is the station. A move glides: the station slides to its row, a new leg draws itself
//! (down its column, then the bend) and a leg no longer needed folds back the same way.

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use gpui_kit::*;

/// How long a move glides.
const GLIDE: Duration = Duration::from_millis(200);
/// The ring that leaves the station once it has arrived: when it starts, and how long it spreads.
const PING_AFTER: Duration = Duration::from_millis(160);
const PING: Duration = Duration::from_millis(450);

/// One leg of the route: from the row of the folder at its top down to the row of the next folder
/// on the way, or of the selection. Rows are list positions, fractional mid-glide. `drawn` runs
/// from 0 to 2: up to 1 is the run down the column, past 1 the bend.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Leg {
    pub top: f32,
    pub end: f32,
    pub drawn: f32,
}

/// The route at one moment: `legs[d]` runs from depth `d` to depth `d + 1`, and the station sits
/// at (depth, row).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shape {
    pub legs: Vec<Leg>,
    pub station: Option<(f32, f32)>,
}

impl Shape {
    /// The settled route through `stops`: the row of each depth's folder, the selection last.
    pub fn through(stops: &[usize]) -> Self {
        Self {
            legs: stops
                .windows(2)
                .map(|pair| Leg {
                    top: pair[0] as f32,
                    end: pair[1] as f32,
                    drawn: 2.,
                })
                .collect(),
            station: stops
                .last()
                .map(|&row| ((stops.len() - 1) as f32, row as f32)),
        }
    }
}

/// The route over time.
#[derive(Default)]
pub struct Motion {
    /// The selection the route leads to, and the shape it settles in there.
    target: Option<PathBuf>,
    goal: Shape,
    /// The glide under way, from `from` to `to` (the goal, with the legs only one end has folded).
    from: Shape,
    to: Shape,
    since: Option<Instant>,
}

impl Motion {
    /// Points the route at `stops`, the way to `selection`. A new selection glides there unless
    /// `glide` is off (the system asks for less motion). Rows that shift under the same selection
    /// (a folder read, a name typed above it) snap, as the rows themselves do.
    pub fn aim(&mut self, selection: Option<&Path>, stops: &[usize], now: Instant, glide: bool) {
        let goal = Shape::through(stops);
        let moved = self.target.as_deref() != selection;
        if !moved && goal == self.goal {
            return;
        }
        let from = self.at(now);
        if moved && glide && goal != self.goal && from.station.is_some() && goal.station.is_some() {
            (self.from, self.to) = align(from, &goal);
            self.since = Some(now);
        } else {
            self.from = goal.clone();
            self.to = goal.clone();
            self.since = None;
        }
        self.target = selection.map(Path::to_path_buf);
        self.goal = goal;
    }

    /// The route as it stands at `now`.
    pub fn at(&self, now: Instant) -> Shape {
        let Some(since) = self.since else {
            return self.to.clone();
        };
        let t = (now.saturating_duration_since(since).as_secs_f32() / GLIDE.as_secs_f32()).min(1.);
        let (slide, draw) = (ease_out_quint()(t), ease_in_out(t));
        Shape {
            legs: self
                .from
                .legs
                .iter()
                .zip(&self.to.legs)
                .map(|(a, b)| Leg {
                    top: lerp(a.top, b.top, slide),
                    end: lerp(a.end, b.end, slide),
                    drawn: lerp(a.drawn, b.drawn, draw),
                })
                .collect(),
            station: match (self.from.station, self.to.station) {
                (Some(a), Some(b)) => Some((lerp(a.0, b.0, slide), lerp(a.1, b.1, slide))),
                (_, b) => b,
            },
        }
    }

    /// Whether the route still moves at `now`, so another frame is needed.
    pub fn moving(&self, now: Instant) -> bool {
        self.since
            .is_some_and(|since| now.saturating_duration_since(since) < PING_AFTER + PING)
    }

    /// How far the ring leaving the station has spread at `now`, 0 to 1, while it does.
    pub fn ping(&self, now: Instant) -> Option<f32> {
        let after = now
            .saturating_duration_since(self.since?)
            .checked_sub(PING_AFTER)?;
        (after < PING).then(|| after.as_secs_f32() / PING.as_secs_f32())
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Gives both ends of a glide the same legs: a leg only the goal has grows where it will stand,
/// and a leg only `from` has folds where it stands.
fn align(mut from: Shape, goal: &Shape) -> (Shape, Shape) {
    let mut to = goal.clone();
    let count = from.legs.len().max(to.legs.len());
    let folded = Leg {
        top: 0.,
        end: 0.,
        drawn: 0.,
    };
    from.legs.resize(count, folded);
    to.legs.resize(count, folded);
    for (a, b) in from.legs.iter_mut().zip(to.legs.iter_mut()) {
        if a.drawn <= 0. {
            *a = Leg { drawn: 0., ..*b };
        } else if b.drawn <= 0. {
            *b = Leg { drawn: 0., ..*a };
        }
    }
    (from, to)
}

/// Where and how the route is drawn, in window pixels at the current zoom.
pub struct Look {
    /// The top left of the list's first row, scrolled.
    pub origin: Point<Pixels>,
    pub row: Pixels,
    /// The x of depth 0's column from `origin`, and the step to the next column, which is also
    /// the height of a bend (45 degrees).
    pub column: Pixels,
    pub indent: Pixels,
    /// Where the selection's and the hovered row's backgrounds end on the right.
    pub right: Pixels,
    pub zoom: f32,
    /// The line leaving each depth's folder, the open folder's first.
    pub lines: [Hsla; 6],
    pub ring: Hsla,
    pub paper: Hsla,
    pub selection: Hsla,
    pub hover: Hsla,
}

impl Look {
    fn x(&self, depth: f32) -> Pixels {
        self.origin.x + self.column + self.indent * depth
    }

    fn y(&self, row: f32) -> Pixels {
        self.origin.y + self.row * (row + 0.5)
    }

    fn px(&self, value: f32) -> Pixels {
        px(value * self.zoom)
    }

    fn line(&self, depth: usize) -> Hsla {
        self.lines[depth % self.lines.len()]
    }
}

/// Paints the route under the rows, inside `bounds`: the hovered row's and the selection's
/// backgrounds, the dashed way to the hovered row where it leaves the route, the legs, the rings
/// and the station. `stops` is the settled route, `hovered` the way to the row under the pointer.
pub fn paint(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    look: &Look,
    shape: &Shape,
    ping: Option<f32>,
    stops: &[usize],
    hovered: &[usize],
) {
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        if let Some(&row) = hovered.last()
            && Some(&row) != stops.last()
        {
            band(
                window,
                look,
                (hovered.len() - 1) as f32,
                row as f32,
                look.hover,
            );
        }
        if let Some((depth, row)) = shape.station {
            band(window, look, depth, row, look.selection);
        }
        // The hovered row's way shares the route as far as their folders agree.
        let shared = stops
            .iter()
            .zip(hovered)
            .take_while(|(a, b)| a == b)
            .count();
        let ghost = shared.max(1)..hovered.len();
        for depth in ghost.clone() {
            let leg = Leg {
                top: hovered[depth - 1] as f32,
                end: hovered[depth] as f32,
                drawn: 2.,
            };
            stroke(window, look, depth - 1, &leg, true);
        }
        for (depth, leg) in shape.legs.iter().enumerate() {
            stroke(window, look, depth, leg, false);
        }
        // A folder's ring comes and goes with the leg leaving it.
        for (depth, leg) in shape.legs.iter().enumerate() {
            let shown = (leg.drawn * 2.).clamp(0., 1.);
            if shown > 0. {
                ring(
                    window,
                    look,
                    (depth as f32, leg.top),
                    5. * (0.3 + 0.7 * shown),
                    2.,
                    (look.ring.opacity(shown), look.paper.opacity(shown)),
                );
            }
        }
        for depth in ghost {
            let ink = look.line(depth - 1).opacity(0.85);
            ring(
                window,
                look,
                (depth as f32, hovered[depth] as f32),
                5.,
                2.,
                (ink, look.paper),
            );
        }
        if let Some((depth, row)) = shape.station {
            // The station takes the color of the line that reaches it.
            let color = look.line((depth.round() as usize).saturating_sub(1));
            if let Some(spread) = ping {
                dot(
                    window,
                    look,
                    (depth, row),
                    5. * (1. + 2. * spread),
                    color.opacity(0.4 * (1. - spread)),
                );
            }
            dot(window, look, (depth, row), 9., color.opacity(0.18));
            dot(window, look, (depth, row), 5., color);
        }
    });
}

/// The selection's background on the other selected rows, given as (depth, line), each with a bar
/// of the lead's line at its left end, which the hovered row's background lacks. No route runs to
/// them: the route belongs to the lead, whose station sits at depth `lead`.
pub fn paint_marked(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    look: &Look,
    marked: &[(usize, usize)],
    lead: Option<f32>,
) {
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        for &(depth, row) in marked {
            band(window, look, depth as f32, row as f32, look.selection);
            // The color of the lead's station; without the lead in view, the row's own line.
            let reach = lead.map_or(depth, |lead| lead.round() as usize);
            let left = look.x(depth as f32) - look.px(9.);
            let height = look.row - look.px(10.);
            window.paint_quad(
                fill(
                    Bounds::new(
                        point(left, look.y(row as f32) - height / 2.),
                        size(look.px(3.), height),
                    ),
                    look.line(reach.saturating_sub(1)),
                )
                .corner_radii(look.px(1.5)),
            );
        }
    });
}

/// A row's background, from just left of its chevron to the right end, with soft corners.
fn band(window: &mut Window, look: &Look, depth: f32, row: f32, color: Hsla) {
    let left = look.x(depth) - look.px(11.);
    let height = look.row - look.px(2.);
    let top = look.y(row) - height / 2.;
    window.paint_quad(
        fill(
            Bounds::new(point(left, top), size(look.right - left, height)),
            color,
        )
        .corner_radii(look.px(6.)),
    );
}

/// The leg from depth `depth` down to the next: as much of its run and its bend as is drawn.
fn stroke(window: &mut Window, look: &Look, depth: usize, leg: &Leg, dashed: bool) {
    let run = leg.drawn.clamp(0., 1.);
    if run <= 0. {
        return;
    }
    let turn = (leg.drawn - 1.).clamp(0., 1.);
    let (from_x, to_x) = (look.x(depth as f32), look.x(depth as f32 + 1.));
    let (top, end) = (look.y(leg.top), look.y(leg.end));
    let bend = end - look.indent;
    let mut path = if dashed {
        PathBuilder::stroke(look.px(2.)).dash_array(&[look.px(3.), look.px(3.)])
    } else {
        PathBuilder::stroke(look.px(2.5))
    };
    path.move_to(point(from_x, top));
    path.line_to(point(from_x, top + (bend - top) * run));
    if turn > 0. {
        path.line_to(point(
            from_x + (to_x - from_x) * turn,
            bend + (end - bend) * turn,
        ));
    }
    let color = look.line(depth);
    if let Ok(path) = path.build() {
        window.paint_path(path, if dashed { color.opacity(0.8) } else { color });
    }
}

/// A ring centered on (depth, row): `ink` for its border, `fill` inside.
fn ring(
    window: &mut Window,
    look: &Look,
    (depth, row): (f32, f32),
    radius: f32,
    border: f32,
    (ink, fill): (Hsla, Hsla),
) {
    let r = look.px(radius);
    let (x, y) = (look.x(depth), look.y(row));
    window.paint_quad(quad(
        Bounds::new(point(x - r, y - r), size(r * 2., r * 2.)),
        r,
        fill,
        look.px(border),
        ink,
        BorderStyle::Solid,
    ));
}

fn dot(window: &mut Window, look: &Look, (depth, row): (f32, f32), radius: f32, color: Hsla) {
    let r = look.px(radius);
    let (x, y) = (look.x(depth), look.y(row));
    window.paint_quad(
        fill(
            Bounds::new(point(x - r, y - r), size(r * 2., r * 2.)),
            color,
        )
        .corner_radii(r),
    );
}

#[cfg(test)]
mod tests {
    use std::{
        path::Path,
        time::{Duration, Instant},
    };

    // Not `super::*`: that brings gpui's `test` attribute, which shadows the standard one.
    use super::{GLIDE, Leg, Motion, PING, PING_AFTER, Shape};

    fn leg(top: f32, end: f32, drawn: f32) -> Leg {
        Leg { top, end, drawn }
    }

    /// A route settled on `stops` for `selection` at `now`.
    fn settled(selection: &str, stops: &[usize], now: Instant) -> Motion {
        let mut motion = Motion::default();
        motion.aim(Some(Path::new(selection)), stops, now, true);
        motion
    }

    #[test]
    fn a_settled_route_has_a_leg_between_each_pair_of_folders() {
        let shape = Shape::through(&[0, 1, 4]);
        assert_eq!(shape.legs, [leg(0., 1., 2.), leg(1., 4., 2.)]);
        assert_eq!(shape.station, Some((2., 4.)));
        assert_eq!(Shape::through(&[]), Shape::default());
    }

    #[test]
    fn the_first_route_appears_without_a_glide() {
        let now = Instant::now();
        let motion = settled("a/b", &[0, 1, 2], now);
        assert_eq!(motion.at(now), Shape::through(&[0, 1, 2]));
        assert!(!motion.moving(now));
    }

    #[test]
    fn a_new_selection_glides_from_the_old_route_to_the_new() {
        let now = Instant::now();
        let mut motion = settled("a/b", &[0, 1, 2], now);
        motion.aim(Some(Path::new("a/c")), &[0, 1, 5], now, true);
        assert_eq!(motion.at(now), Shape::through(&[0, 1, 2]));
        let halfway = motion.at(now + GLIDE / 2);
        let (_, row) = halfway.station.unwrap();
        assert!(row > 2. && row < 5.);
        assert_eq!(motion.at(now + GLIDE), Shape::through(&[0, 1, 5]));
        assert!(motion.moving(now + GLIDE));
    }

    #[test]
    fn a_new_leg_draws_itself_where_it_will_stand() {
        let now = Instant::now();
        let mut motion = settled("a", &[0, 1], now);
        motion.aim(Some(Path::new("a/b")), &[0, 1, 2], now, true);
        assert_eq!(motion.at(now).legs[1], leg(1., 2., 0.));
        let halfway = motion.at(now + GLIDE / 2).legs[1];
        assert_eq!((halfway.top, halfway.end), (1., 2.));
        assert!(halfway.drawn > 0. && halfway.drawn < 2.);
        assert_eq!(motion.at(now + GLIDE).legs[1], leg(1., 2., 2.));
    }

    #[test]
    fn a_dropped_leg_folds_where_it_stood() {
        let now = Instant::now();
        let mut motion = settled("a/b", &[0, 1, 2], now);
        motion.aim(Some(Path::new("a")), &[0, 1], now, true);
        assert_eq!(motion.at(now).legs[1], leg(1., 2., 2.));
        assert_eq!(motion.at(now + GLIDE).legs[1], leg(1., 2., 0.));
        assert_eq!(motion.at(now + GLIDE).station, Some((1., 1.)));
    }

    #[test]
    fn a_move_mid_glide_starts_from_where_the_route_is() {
        let now = Instant::now();
        let mut motion = settled("a/b", &[0, 1, 2], now);
        motion.aim(Some(Path::new("a/c")), &[0, 1, 6], now, true);
        let later = now + GLIDE / 4;
        let there = motion.at(later);
        motion.aim(Some(Path::new("a/d")), &[0, 1, 3], later, true);
        assert_eq!(motion.at(later), there);
        assert_eq!(motion.at(later + GLIDE), Shape::through(&[0, 1, 3]));
    }

    #[test]
    fn rows_that_shift_under_the_same_selection_snap() {
        let now = Instant::now();
        let mut motion = settled("a/b", &[0, 1, 2], now);
        motion.aim(Some(Path::new("a/b")), &[0, 3, 4], now, true);
        assert_eq!(motion.at(now), Shape::through(&[0, 3, 4]));
        assert!(!motion.moving(now));
    }

    #[test]
    fn less_motion_snaps_to_the_new_route() {
        let now = Instant::now();
        let mut motion = settled("a/b", &[0, 1, 2], now);
        motion.aim(Some(Path::new("a/c")), &[0, 1, 5], now, false);
        assert_eq!(motion.at(now), Shape::through(&[0, 1, 5]));
        assert!(!motion.moving(now));
        assert_eq!(motion.ping(now + PING_AFTER), None);
    }

    #[test]
    fn a_ring_spreads_once_the_station_arrives() {
        let now = Instant::now();
        let mut motion = settled("a/b", &[0, 1, 2], now);
        motion.aim(Some(Path::new("a/c")), &[0, 1, 5], now, true);
        assert_eq!(motion.ping(now), None);
        assert_eq!(motion.ping(now + PING_AFTER), Some(0.));
        let spread = motion.ping(now + PING_AFTER + PING / 2).unwrap();
        assert!((spread - 0.5).abs() < 0.01);
        assert_eq!(motion.ping(now + PING_AFTER + PING), None);
        assert!(!motion.moving(now + PING_AFTER + PING + Duration::from_millis(1)));
    }
}
