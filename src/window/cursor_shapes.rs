//! The shapes the cursor effects draw (window/cursor_trail.rs runs them): a
//! trail from the old cursor cell to the new one in one of `TrailStyle`'s
//! looks, and an `Arrival` pulse at the new cursor. Each is a pure function of
//! the cells and the progress (0 to 1), returning the layers to paint back to
//! front, so they're tested without a window. Every layer is a closed outline:
//! filled ones convex, ringed ones stroked.
//!
//! The looks follow sahaj-b/ghostty-cursor-shaders (MIT), redrawn as GPUI
//! paths rather than ported from GLSL. GPUI has no blur, so a soft edge or a
//! glow is the same outline drawn wider and fainter behind.

use crate::settings::{Arrival, TrailStyle};
use gpui::{Bounds, Pixels, Point, point, px};

type P = (f32, f32);

/// One outline to paint: filled, or stroked `stroke` px wide; at `alpha` of
/// the effect's color, or of its paler core color.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Layer {
    pub(super) outline: Vec<Point<Pixels>>,
    pub(super) stroke: Option<f32>,
    pub(super) alpha: f32,
    pub(super) core: bool,
}

fn fill(outline: Vec<Point<Pixels>>, alpha: f32) -> Layer {
    Layer {
        outline,
        stroke: None,
        alpha,
        core: false,
    }
}

/// Below this many px, a trail has gone into the cursor.
const CAUGHT_UP: f32 = 0.5;

/// Exponential decay with time constant `tau` (a share of the duration): how
/// much of its way is left at progress `t`, scaled to be none at the end.
pub(super) fn settle(t: f32, tau: f32) -> f32 {
    let end = (-1.0 / tau).exp();
    ((-t.clamp(0.0, 1.0) / tau).exp() - end) / (1.0 - end)
}

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

fn pt((x, y): P) -> Point<Pixels> {
    point(px(x), px(y))
}

fn mid(b: Bounds<Pixels>) -> P {
    let c = b.center();
    (f32::from(c.x), f32::from(c.y))
}

/// A cell's corners, clockwise from the top left, grown `by` px all round.
fn corners(b: Bounds<Pixels>, by: f32) -> [P; 4] {
    let (x, y) = (f32::from(b.origin.x) - by, f32::from(b.origin.y) - by);
    let (w, h) = (
        f32::from(b.size.width) + 2.0 * by,
        f32::from(b.size.height) + 2.0 * by,
    );
    [(x, y), (x + w, y), (x + w, y + h), (x, y + h)]
}

/// `a` moved `k` of the way to `b`, size and place.
fn toward(a: Bounds<Pixels>, b: Bounds<Pixels>, k: f32) -> Bounds<Pixels> {
    let l = |p: Pixels, q: Pixels| px(f32::from(p) + (f32::from(q) - f32::from(p)) * k);
    Bounds::new(
        point(l(a.origin.x, b.origin.x), l(a.origin.y, b.origin.y)),
        gpui::size(
            l(a.size.width, b.size.width),
            l(a.size.height, b.size.height),
        ),
    )
}

/// The convex hull of `points` (Andrew's monotone chain), one way round.
fn hull(mut points: Vec<P>) -> Vec<Point<Pixels>> {
    points.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    points.dedup();
    let cross = |o: P, a: P, b: P| (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0);
    let mut lower: Vec<P> = Vec::new();
    for &q in &points {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], q) <= 0.0 {
            lower.pop();
        }
        lower.push(q);
    }
    let mut upper: Vec<P> = Vec::new();
    for &q in points.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], q) <= 0.0 {
            upper.pop();
        }
        upper.push(q);
    }
    lower.pop();
    upper.pop();
    lower.into_iter().chain(upper).map(pt).collect()
}

/// The hull of two cells, each grown `by` px.
fn span(a: Bounds<Pixels>, b: Bounds<Pixels>, by: f32) -> Vec<Point<Pixels>> {
    hull(corners(a, by).into_iter().chain(corners(b, by)).collect())
}

/// The trail of `style` at progress `t` from the cell `from` to the cell `to`:
/// nothing once it has gone into the cursor (or for the same cell).
pub(super) fn trail(
    style: TrailStyle,
    from: Bounds<Pixels>,
    to: Bounds<Pixels>,
    t: f32,
) -> Vec<Layer> {
    let (a, b) = (mid(from), mid(to));
    let len = (a.0 - b.0).hypot(a.1 - b.1);
    if len.is_nan() || len < CAUGHT_UP || t.is_nan() {
        return Vec::new();
    }
    match style {
        TrailStyle::Comet => comet(from, to, t),
        TrailStyle::Tail => tail(from, to, t),
        TrailStyle::Sweep => sweep(from, to, t),
        TrailStyle::Warp => warp(from, to, t),
    }
}

/// Comet: the tail's decay time against the duration, its layers back to
/// front (width against the cell's height, opacity, paler core), and how its
/// width falls off: a spindle that narrows to a needle point.
const COMET_TAIL: f32 = 1.0 / 3.0;
const COMET_LAYERS: [(f32, f32, bool); 4] = [
    (2.4, 0.06, false),
    (1.7, 0.15, false),
    (1.0, 0.35, false),
    (0.45, 1.0, true),
];
const COMET_TAPER: f32 = 0.6;
const COMET_SIDE: usize = 6;

/// A glowing streak, as Rex draws: its head a round nose a cell high on the
/// new cursor from the first frame, tapering to a point at the tail, which
/// decays into the head.
fn comet(from: Bounds<Pixels>, to: Bounds<Pixels>, t: f32) -> Vec<Layer> {
    let k = settle(t, COMET_TAIL);
    COMET_LAYERS
        .iter()
        .filter_map(|&(widen, alpha, core)| {
            streak(from, to, k, widen).map(|outline| Layer {
                outline,
                stroke: None,
                alpha: alpha * k,
                core,
            })
        })
        .collect()
}

/// The comet's outline with `k` of its tail's way left, `widen` cell heights
/// across at the head: from the tail point along one side, round the nose and
/// back along the other.
pub(super) fn streak(
    from: Bounds<Pixels>,
    to: Bounds<Pixels>,
    k: f32,
    widen: f32,
) -> Option<Vec<Point<Pixels>>> {
    let (a, head) = (mid(from), mid(to));
    let (dx, dy) = ((a.0 - head.0) * k, (a.1 - head.1) * k);
    let len = dx.hypot(dy);
    if len.is_nan() || len < CAUGHT_UP {
        return None;
    }
    // u points from the head back to the tail; n across it.
    let (u, n) = ((dx / len, dy / len), (-dy / len, dx / len));
    let half = f32::from(to.size.height) * widen / 2.0;
    let at = |along: f32, across: f32| {
        pt((
            head.0 + u.0 * along + n.0 * across,
            head.1 + u.1 * along + n.1 * across,
        ))
    };
    let width = |s: f32| half * (1.0 - s).powf(COMET_TAPER);
    let mut outline = Vec::with_capacity(2 * COMET_SIDE + 6);
    outline.push(at(len, 0.0));
    outline.extend(
        (1..COMET_SIDE)
            .rev()
            .map(|i| i as f32 / COMET_SIDE as f32)
            .map(|s| at(s * len, width(s))),
    );
    outline.extend(
        [0.0f32, 0.5, 0.85, 1.0, 0.85, 0.5, 0.0]
            .iter()
            .zip([1.0f32, 0.87, 0.5, 0.0, -0.5, -0.87, -1.0])
            .map(|(ahead, side)| at(-half * ahead, half * side)),
    );
    outline.extend(
        (1..COMET_SIDE)
            .map(|i| i as f32 / COMET_SIDE as f32)
            .map(|s| at(s * len, -width(s))),
    );
    Some(outline)
}

/// Tail: the slowest corner's decay time against the duration, and the
/// fastest's, kitty's `cursor_trail_decay 0.1 0.4` ratio.
const TAIL_SLOW: f32 = 1.0 / 3.0;
const TAIL_FAST: f32 = TAIL_SLOW / 4.0;

/// kitty's trail: a cursor-sized quad whose corners start on the old cell's
/// and decay to the new cell's; corners ahead of the move sit on the cursor
/// at once, one square to the move takes `TAIL_FAST`, one behind `TAIL_SLOW`.
fn tail(from: Bounds<Pixels>, to: Bounds<Pixels>, t: f32) -> Vec<Layer> {
    let (a, b) = (mid(from), mid(to));
    let len = (b.0 - a.0).hypot(b.1 - a.1);
    let (ux, uy) = ((b.0 - a.0) / len, (b.1 - a.1) / len);
    let mut left = 0.0f32;
    let quad: Vec<P> = corners(from, 0.0)
        .into_iter()
        .zip(corners(to, 0.0))
        .enumerate()
        .map(|(i, (s, e))| {
            let (cx, cy) = ([-1.0, 1.0, 1.0, -1.0][i], [-1.0, -1.0, 1.0, 1.0][i]);
            let facing = (cx * ux + cy * uy) / std::f32::consts::SQRT_2;
            let k = if facing > 1e-3 {
                0.0
            } else {
                settle(
                    t,
                    TAIL_SLOW + (TAIL_FAST - TAIL_SLOW) * (1.0 + facing.max(-1.0)),
                )
            };
            let q = (e.0 + (s.0 - e.0) * k, e.1 + (s.1 - e.1) * k);
            left = left.max((q.0 - e.0).hypot(q.1 - e.1));
            q
        })
        .collect();
    if left < CAUGHT_UP {
        return Vec::new();
    }
    vec![fill(hull(quad), settle(t, TAIL_SLOW))]
}

/// Sweep: a solid block from the old cell to the new, its old end catching up
/// (easing out) until only the cursor is left.
fn sweep(from: Bounds<Pixels>, to: Bounds<Pixels>, t: f32) -> Vec<Layer> {
    let k = ease_out(t);
    if k >= 1.0 {
        return Vec::new();
    }
    vec![fill(span(toward(from, to, k), to, 0.0), 0.8 * (1.0 - k))]
}

/// Warp: the decay time of its old end, and its soft edges as (grown by this
/// many cell heights, opacity), back to front.
const WARP_TAIL: f32 = 1.0 / 4.0;
const WARP_EDGES: [(f32, f32); 3] = [(0.5, 0.07), (0.25, 0.15), (0.0, 0.35)];

/// Neovide's stretched block: the block from its old end (decaying toward the
/// cursor) to the cursor with soft edges, and its half nearer the cursor drawn
/// again, so it fades toward the tail.
fn warp(from: Bounds<Pixels>, to: Bounds<Pixels>, t: f32) -> Vec<Layer> {
    let k = settle(t, WARP_TAIL);
    let end = toward(to, from, k);
    if (mid(end).0 - mid(to).0).hypot(mid(end).1 - mid(to).1) < CAUGHT_UP {
        return Vec::new();
    }
    let h = f32::from(to.size.height);
    let mut layers: Vec<Layer> = WARP_EDGES
        .iter()
        .map(|&(grow, alpha)| fill(span(end, to, grow * h), alpha * k))
        .collect();
    layers.push(fill(span(toward(end, to, 0.5), to, 0.0), 0.35 * k));
    layers
}

/// Arrival pulses: how far they spread, in cell heights from the cursor's
/// middle (rings) or edge (boxes), and the points round a circle.
const PULSE_REACH: f32 = 2.0;
const BOX_REACH: f32 = 1.2;
const CIRCLE: usize = 40;

/// The pulse of `kind` at the cursor cell `at` at progress `t`: nothing once
/// it's over.
pub(super) fn arrival(kind: Arrival, at: Bounds<Pixels>, t: f32) -> Vec<Layer> {
    if t.is_nan() || !(0.0..1.0).contains(&t) {
        return Vec::new();
    }
    let (c, h, e) = (mid(at), f32::from(at.size.height), ease_out(t));
    let fade = 1.0 - t;
    let ring = |r: f32| {
        (0..CIRCLE)
            .map(|i| i as f32 * std::f32::consts::TAU / CIRCLE as f32)
            .map(|a| pt((c.0 + r * a.cos(), c.1 + r * a.sin())))
            .collect::<Vec<_>>()
    };
    let line = 1.0 + 1.5 * fade;
    match kind {
        Arrival::Off => Vec::new(),
        Arrival::Ripple => vec![Layer {
            outline: ring(h * (0.5 + PULSE_REACH * e)),
            stroke: Some(line),
            alpha: fade,
            core: false,
        }],
        Arrival::Boom => vec![fill(ring(h * (0.25 + PULSE_REACH * e)), 0.45 * fade)],
        Arrival::RippleRect => vec![Layer {
            outline: corners(at, h * BOX_REACH * e).into_iter().map(pt).collect(),
            stroke: Some(line),
            alpha: fade,
            core: false,
        }],
        Arrival::BoomRect => vec![fill(
            corners(at, h * BOX_REACH * e).into_iter().map(pt).collect(),
            0.4 * fade,
        )],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(x: f32, y: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), gpui::size(px(8.0), px(16.0)))
    }

    fn xy(p: Point<Pixels>) -> P {
        (f32::from(p.x), f32::from(p.y))
    }

    fn dist(a: P, b: P) -> f32 {
        (a.0 - b.0).hypot(a.1 - b.1)
    }

    /// Each turn along the outline goes the same way: convex, never self-crossing.
    fn convex(o: &[Point<Pixels>]) -> bool {
        let turns: Vec<f32> = (0..o.len())
            .map(|i| {
                let (a, b, c) = (xy(o[i]), xy(o[(i + 1) % o.len()]), xy(o[(i + 2) % o.len()]));
                (b.0 - a.0) * (c.1 - b.1) - (b.1 - a.1) * (c.0 - b.0)
            })
            .collect();
        o.len() >= 3 && (turns.iter().all(|t| *t > -1e-3) || turns.iter().all(|t| *t < 1e-3))
    }

    const STYLES: [TrailStyle; 4] = [
        TrailStyle::Comet,
        TrailStyle::Tail,
        TrailStyle::Sweep,
        TrailStyle::Warp,
    ];
    const PULSES: [Arrival; 4] = [
        Arrival::Ripple,
        Arrival::Boom,
        Arrival::RippleRect,
        Arrival::BoomRect,
    ];
    const DIRS: [P; 4] = [
        (1.0, 0.0),
        (0.0, -1.0),
        (0.707, 0.707),
        (37.0 / 214.2, 211.0 / 214.2),
    ];

    #[test]
    fn every_trail_is_convex_finite_and_near_its_cells_while_it_runs() {
        for style in STYLES {
            for dir in DIRS {
                for len in [0.6f32, 1.0, 5.0, 10.0, 400.0] {
                    let (from, to) = (cell(0.0, 0.0), cell(dir.0 * len, dir.1 * len));
                    // Within the cells' box, grown by the widest glow (2.4 cell heights / 2).
                    let (lo, hi) = ((dir.0 * len).min(0.0) - 20.0, (dir.0 * len).max(0.0) + 28.0);
                    let (top, bottom) =
                        ((dir.1 * len).min(0.0) - 20.0, (dir.1 * len).max(0.0) + 36.0);
                    for t in [0.0, 0.1, 0.3, 0.6, 0.9] {
                        for layer in trail(style, from, to, t) {
                            assert!(
                                convex(&layer.outline),
                                "{style:?} {dir:?} {len} {t}: {:?}",
                                layer.outline
                            );
                            assert!(
                                layer.stroke.is_none() && (0.0..=1.0).contains(&layer.alpha),
                                "{style:?}"
                            );
                            for p in layer.outline.iter().map(|p| xy(*p)) {
                                assert!(
                                    p.0.is_finite()
                                        && p.1.is_finite()
                                        && (lo..=hi).contains(&p.0)
                                        && (top..=bottom).contains(&p.1),
                                    "{style:?} {dir:?} {len} {t}: {p:?}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn every_trail_starts_drawn_and_is_gone_at_its_end() {
        let (from, to) = (cell(0.0, 0.0), cell(300.0, 120.0));
        for style in STYLES {
            assert!(
                !trail(style, from, to, 0.05).is_empty(),
                "{style:?} draws as it starts"
            );
            assert!(
                trail(style, from, to, 1.0).is_empty(),
                "{style:?} is gone at the end"
            );
            assert!(
                trail(style, cell(5.0, 5.0), cell(5.0, 5.0), 0.2).is_empty(),
                "{style:?}: nothing for the same cell"
            );
            assert!(trail(style, from, to, f32::NAN).is_empty());
            // It fades: the strongest layer at 0.8 is fainter than at 0.1.
            let peak = |t: f32| {
                trail(style, from, to, t)
                    .iter()
                    .map(|l| l.alpha)
                    .fold(0.0f32, f32::max)
            };
            assert!(
                peak(0.8) < peak(0.1),
                "{style:?}: {} then {}",
                peak(0.1),
                peak(0.8)
            );
        }
    }

    #[test]
    fn the_comet_layers_are_the_glow_then_the_core() {
        let layers = trail(TrailStyle::Comet, cell(0.0, 0.0), cell(400.0, 0.0), 0.0);
        assert_eq!(
            layers.iter().map(|l| (l.alpha, l.core)).collect::<Vec<_>>(),
            vec![(0.06, false), (0.15, false), (0.35, false), (1.0, true)]
        );
    }

    #[test]
    fn the_tail_is_the_cursor_quad_its_head_on_the_cursor_from_the_first_frame() {
        let (from, to) = (cell(0.0, 0.0), cell(400.0, 0.0));
        let first = trail(TrailStyle::Tail, from, to, 0.0);
        let xs: Vec<f32> = first[0].outline.iter().map(|p| xy(*p).0).collect();
        assert!(
            xs.contains(&408.0) && xs.contains(&0.0),
            "from the old cursor's left to the new one's right: {xs:?}"
        );
        let ys: Vec<f32> = first[0].outline.iter().map(|p| xy(*p).1).collect();
        assert!(
            ys.iter().all(|y| *y == 0.0 || *y == 16.0),
            "a cell high: {ys:?}"
        );
        let left = |t: f32| {
            trail(TrailStyle::Tail, from, to, t)[0]
                .outline
                .iter()
                .map(|p| xy(*p).0)
                .fold(f32::MAX, f32::min)
        };
        assert!(left(0.1) < left(0.3) && left(0.3) < left(0.6) && left(0.6) < 400.0);
    }

    #[test]
    fn the_sweep_block_shrinks_from_the_old_cursor_into_the_new() {
        let (from, to) = (cell(0.0, 0.0), cell(400.0, 0.0));
        let width = |t: f32| {
            let o = &trail(TrailStyle::Sweep, from, to, t)[0].outline;
            let xs: Vec<f32> = o.iter().map(|p| xy(*p).0).collect();
            xs.iter().cloned().fold(f32::MIN, f32::max)
                - xs.iter().cloned().fold(f32::MAX, f32::min)
        };
        assert!((width(0.0) - 408.0).abs() < 0.01, "both cells at first");
        assert!(width(0.2) > width(0.5) && width(0.5) > width(0.9) && width(0.9) >= 8.0);
    }

    #[test]
    fn the_warp_is_strongest_at_the_cursor_with_soft_edges() {
        let (from, to) = (cell(0.0, 0.0), cell(400.0, 0.0));
        let layers = trail(TrailStyle::Warp, from, to, 0.1);
        assert_eq!(layers.len(), 4);
        // The soft edges reach past the block, the last layer (the half nearer the cursor) doesn't reach the old end.
        let reach_left = |l: &Layer| l.outline.iter().map(|p| xy(*p).0).fold(f32::MAX, f32::min);
        assert!(
            reach_left(&layers[0]) < reach_left(&layers[2])
                && reach_left(&layers[2]) < reach_left(&layers[3])
        );
        let top = |l: &Layer| l.outline.iter().map(|p| xy(*p).1).fold(f32::MAX, f32::min);
        assert!(
            (top(&layers[0]) + 8.0).abs() < 0.01,
            "grown half a cell height"
        );
    }

    #[test]
    fn every_pulse_grows_from_the_cursor_fades_and_ends() {
        let at = cell(100.0, 100.0);
        let c = (104.0, 108.0);
        for kind in PULSES {
            let size = |t: f32| trail_reach(&arrival(kind, at, t), c);
            assert!(
                size(0.0) < size(0.5) && size(0.5) < size(0.9),
                "{kind:?} grows"
            );
            assert!(
                size(0.99) <= 16.0 * (0.5 + 2.0) * 1.5 + 1.0,
                "{kind:?} stays within its reach"
            );
            assert!(
                arrival(kind, at, 0.5)[0].alpha > arrival(kind, at, 0.9)[0].alpha,
                "{kind:?} fades"
            );
            assert!(
                arrival(kind, at, 1.0).is_empty() && arrival(kind, at, f32::NAN).is_empty(),
                "{kind:?} ends"
            );
            for t in [0.0, 0.4, 0.95] {
                let l = &arrival(kind, at, t)[0];
                assert!(convex(&l.outline), "{kind:?} {t}");
                let rings = matches!(kind, Arrival::Ripple | Arrival::RippleRect);
                assert_eq!(
                    l.stroke.is_some(),
                    rings,
                    "{kind:?}: rings are stroked, booms filled"
                );
            }
        }
        assert!(arrival(Arrival::Off, at, 0.3).is_empty());
    }

    fn trail_reach(layers: &[Layer], c: P) -> f32 {
        layers
            .iter()
            .flat_map(|l| l.outline.iter())
            .map(|p| dist(xy(*p), c))
            .fold(0.0f32, f32::max)
    }

    #[test]
    fn the_hull_of_two_cells_is_one_way_round_without_repeats() {
        let o = span(cell(0.0, 0.0), cell(0.0, 0.0), 0.0);
        assert_eq!(o.len(), 4, "the same cell twice is its four corners");
        let o = span(cell(0.0, 0.0), cell(100.0, 50.0), 0.0);
        assert_eq!(o.len(), 6, "two cells on a diagonal: a hexagon");
        assert!(convex(&o));
    }

    /// The comet's outline and fade at progress `t`, as the trail draws them.
    fn streak_t(
        from: Bounds<Pixels>,
        to: Bounds<Pixels>,
        t: f32,
        widen: f32,
    ) -> Option<Vec<Point<Pixels>>> {
        streak(from, to, settle(t, COMET_TAIL), widen)
    }

    fn comet_fade(t: f32) -> f32 {
        settle(t, COMET_TAIL)
    }

    #[test]
    fn comet_the_trail_fades_as_it_shortens() {
        assert!((comet_fade(0.0) - 1.0).abs() < 1e-6);
        assert!(comet_fade(0.5) < comet_fade(0.25) && comet_fade(0.25) < comet_fade(0.1));
        assert!(comet_fade(0.1) < 0.8, "it eases out from the start");
        assert_eq!(comet_fade(1.0), 0.0);
        assert_eq!(comet_fade(2.0), 0.0);
    }

    #[test]
    fn comet_the_head_is_on_the_cursor_a_cell_high_and_the_tail_a_point() {
        let (from, to) = (cell(0.0, 0.0), cell(400.0, 0.0));
        let head = (404.0, 8.0);
        let first = streak_t(from, to, 0.0, 1.0).unwrap();
        assert_eq!(
            xy(first[0]),
            (4.0, 8.0),
            "the tail starts on the old cursor's middle, a single point"
        );
        // The widest pair of points straddles the head, a cell high (16 px) apart.
        let widest = first
            .iter()
            .map(|p| xy(*p))
            .filter(|p| (p.0 - head.0).abs() < 0.01)
            .map(|p| p.1)
            .fold((f32::MAX, f32::MIN), |(lo, hi), y| (lo.min(y), hi.max(y)));
        assert!((widest.1 - widest.0 - 16.0).abs() < 0.01, "{widest:?}");
        // Narrower toward the tail: the width at each sample falls off.
        let side: Vec<f32> = first[1..6].iter().map(|p| (xy(*p).1 - 8.0).abs()).collect();
        assert!(side.windows(2).all(|w| w[0] < w[1]), "{side:?}");
        // Nothing reaches past the head's nose (half a cell height ahead).
        assert!(first.iter().all(|p| xy(*p).0 <= head.0 + 8.0 + 0.01));
    }

    #[test]
    fn comet_the_tail_retracts_into_the_head() {
        let (from, to) = (cell(0.0, 0.0), cell(400.0, 0.0));
        let tail = |t: f32| xy(streak_t(from, to, t, 1.0).unwrap()[0]).0;
        let (a, b, c) = (tail(0.1), tail(0.2), tail(0.3));
        assert!(4.0 < a && a < b && b < c && c < 404.0, "{a} {b} {c}");
        assert!(b - a < a - 4.0, "fast at first, then slower: {a} {b}");
        let last = streak_t(from, to, 0.95, 1.0).unwrap();
        assert!(
            dist(xy(last[0]), (404.0, 8.0)) < 4.0,
            "the last frame is all but in the cursor: {last:?}"
        );
        assert_eq!(streak_t(from, to, 1.0, 1.0), None, "and gone at the end");
    }

    #[test]
    fn comet_the_glow_layers_share_the_tail_and_widen() {
        let (from, to) = (cell(0.0, 0.0), cell(-200.0, 150.0));
        let core = streak_t(from, to, 0.2, 0.45).unwrap();
        let glow = streak_t(from, to, 0.2, 2.4).unwrap();
        assert_eq!(core[0], glow[0], "one tail point");
        // How far the outline reaches from the line through head and tail.
        let (head, tail) = ((-196.0f32, 158.0f32), xy(core[0]));
        let len = dist(head, tail);
        let across = |o: &Vec<Point<Pixels>>| {
            o.iter()
                .map(|p| {
                    let (x, y) = xy(*p);
                    ((tail.0 - head.0) * (y - head.1) - (tail.1 - head.1) * (x - head.0)).abs()
                        / len
                })
                .fold(0.0f32, f32::max)
        };
        assert!(
            (across(&core) - 16.0 * 0.45 / 2.0).abs() < 0.01,
            "the core is 0.45 of a cell high"
        );
        assert!(
            (across(&glow) - 16.0 * 2.4 / 2.0).abs() < 0.01,
            "the outer glow 2.4"
        );
    }

    #[test]
    fn comet_the_streak_points_any_way_and_is_finite() {
        for to in [
            cell(0.0, -300.0),
            cell(-300.0, 0.0),
            cell(0.0, 300.0),
            cell(37.0, 211.0),
        ] {
            for t in [-1.0, 0.0, 0.01, 0.5, 0.99, f32::NAN] {
                if let Some(o) = streak_t(cell(0.0, 0.0), to, t, 2.4) {
                    assert!(
                        o.iter()
                            .all(|p| xy(*p).0.is_finite() && xy(*p).1.is_finite()),
                        "{to:?} {t}: {o:?}"
                    );
                    // The tail is on the way back to the old cursor, settle(t) of it.
                    let (head, from) = (xy(to.center()), (4.0, 8.0));
                    let k = settle(t, COMET_TAIL);
                    let want = (
                        head.0 + (from.0 - head.0) * k,
                        head.1 + (from.1 - head.1) * k,
                    );
                    assert!(
                        dist(xy(o[0]), want) < 0.01,
                        "{to:?} {t}: {:?} not {want:?}",
                        xy(o[0])
                    );
                    assert!(dist(xy(o[0]), head) >= CAUGHT_UP);
                }
            }
        }
    }

    #[test]
    fn comet_every_layer_is_a_convex_outline_at_any_length() {
        // Each turn along the outline goes the same way: convex, so never self-crossing.
        let convex = |o: &[Point<Pixels>]| {
            let turns: Vec<f32> = (0..o.len())
                .map(|i| {
                    let (a, b, c) = (xy(o[i]), xy(o[(i + 1) % o.len()]), xy(o[(i + 2) % o.len()]));
                    (b.0 - a.0) * (c.1 - b.1) - (b.1 - a.1) * (c.0 - b.0)
                })
                .collect();
            turns.iter().all(|t| *t > 0.0) || turns.iter().all(|t| *t < 0.0)
        };
        for dir in [
            (1.0f32, 0.0f32),
            (0.0, -1.0),
            (0.707, 0.707),
            (37.0 / 214.2, 211.0 / 214.2),
        ] {
            for len in [0.6f32, 1.0, 5.0, 10.0, 400.0] {
                let to = cell(dir.0 * len, dir.1 * len);
                for (widen, _, _) in COMET_LAYERS {
                    let o = streak_t(cell(0.0, 0.0), to, 0.0, widen).unwrap();
                    assert!(convex(&o), "{dir:?} {len} {widen}: {o:?}");
                }
            }
        }
    }

    #[test]
    fn comet_nothing_is_drawn_for_the_same_cell() {
        for t in [0.0, 0.3, 0.99] {
            assert_eq!(streak_t(cell(10.0, 10.0), cell(10.0, 10.0), t, 1.0), None);
        }
    }
}
