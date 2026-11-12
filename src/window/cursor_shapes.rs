//! The shapes the cursor effects draw (window/cursor_trail.rs runs them): a
//! trail from where the cursor was (or from what a running trail last drew)
//! to the cursor as it is drawn, in one of `TrailStyle`'s looks, and an
//! `Arrival` pulse at the new cursor. Comet, sweep and warp are pure functions
//! of the ends and the progress (0 to 1); the tail is kitty's chase, stepped
//! frame by frame (`Chase`). Each returns the layers to paint back to front,
//! so they're tested without a window. Every outline is convex, filled or
//! stroked; a filled one may have a convex hole cut out of it.
//!
//! The looks follow sahaj-b/ghostty-cursor-shaders (MIT) and the rules of
//! kitty's `cursor_trail.c`, redrawn as GPUI paths rather than ported from
//! GLSL or C. GPUI has no blur, so a soft edge or a glow is the same outline
//! drawn wider and fainter behind.

use crate::settings::{Arrival, TrailStyle};
use gpui::{Bounds, Pixels, Point, point, px};

pub(super) type P = (f32, f32);

/// Four corners, clockwise from the top left: a cell, a cursor, or a tail's
/// corners partway to them.
pub(super) type Quad = [P; 4];

/// One outline to paint: filled, or stroked `stroke` px wide, less `hole`; at
/// `alpha` of the effect's color, or of its paler core color.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Layer {
    pub(super) outline: Vec<Point<Pixels>>,
    pub(super) hole: Option<Vec<Point<Pixels>>>,
    pub(super) stroke: Option<f32>,
    pub(super) alpha: f32,
    pub(super) core: bool,
}

fn fill(outline: Vec<Point<Pixels>>, alpha: f32) -> Layer {
    Layer {
        outline,
        hole: None,
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

/// A rect's corners, clockwise from the top left, grown `by` px all round.
fn corners(b: Bounds<Pixels>, by: f32) -> Quad {
    let (x, y) = (f32::from(b.origin.x) - by, f32::from(b.origin.y) - by);
    let (w, h) = (
        f32::from(b.size.width) + 2.0 * by,
        f32::from(b.size.height) + 2.0 * by,
    );
    [(x, y), (x + w, y), (x + w, y + h), (x, y + h)]
}

/// A rect as the corners a trail starts from.
pub(super) fn quad(b: Bounds<Pixels>) -> Quad {
    corners(b, 0.0)
}

/// The smallest rect holding `q`.
fn enclosing(q: Quad) -> Bounds<Pixels> {
    let xs = q.iter().map(|p| p.0);
    let (x0, x1) = xs.clone().fold((f32::MAX, f32::MIN), |(lo, hi), x| (lo.min(x), hi.max(x)));
    let ys = q.iter().map(|p| p.1);
    let (y0, y1) = ys.fold((f32::MAX, f32::MIN), |(lo, hi), y| (lo.min(y), hi.max(y)));
    Bounds::new(point(px(x0), px(y0)), gpui::size(px(x1 - x0), px(y1 - y0)))
}

fn centroid(q: &Quad) -> P {
    (
        q.iter().map(|p| p.0).sum::<f32>() / 4.0,
        q.iter().map(|p| p.1).sum::<f32>() / 4.0,
    )
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

fn cross(o: P, a: P, b: P) -> f32 {
    (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
}

/// The convex hull of `points` (Andrew's monotone chain), with a positive
/// area (`area`).
fn hull_points(mut points: Vec<P>) -> Vec<P> {
    points.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    points.dedup();
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
    lower.into_iter().chain(upper).collect()
}

fn hull(points: Vec<P>) -> Vec<Point<Pixels>> {
    hull_points(points).into_iter().map(pt).collect()
}

/// The hull of two rects, each grown `by` px.
fn span(a: Bounds<Pixels>, b: Bounds<Pixels>, by: f32) -> Vec<Point<Pixels>> {
    hull(corners(a, by).into_iter().chain(corners(b, by)).collect())
}

/// Twice the signed area of `o`: positive one way round, negative the other.
fn area(o: &[P]) -> f32 {
    (0..o.len())
        .map(|i| cross((0.0, 0.0), o[i], o[(i + 1) % o.len()]))
        .sum::<f32>()
}

/// The part of the convex `subject` inside the convex `clip`, whose area is
/// positive: Sutherland-Hodgman.
fn clipped(subject: &[P], clip: &[P]) -> Vec<P> {
    let mut out = subject.to_vec();
    for i in 0..clip.len() {
        let (a, b) = (clip[i], clip[(i + 1) % clip.len()]);
        let inside = |p: P| cross(a, b, p) >= 0.0;
        let meet = |p: P, q: P| {
            let (cp, cq) = (cross(a, b, p), cross(a, b, q));
            let k = cp / (cp - cq);
            (p.0 + (q.0 - p.0) * k, p.1 + (q.1 - p.1) * k)
        };
        let input = std::mem::take(&mut out);
        for j in 0..input.len() {
            let (p, q) = (input[j], input[(j + 1) % input.len()]);
            match (inside(p), inside(q)) {
                (true, true) => out.push(q),
                (true, false) => out.push(meet(p, q)),
                (false, true) => {
                    out.push(meet(p, q));
                    out.push(q);
                }
                (false, false) => {}
            }
        }
        if out.is_empty() {
            break;
        }
    }
    out
}

/// The trail of `style` at progress `t` from `from` (corners: a cursor, or
/// what a trail last drew) to the cursor `to` as drawn, sized against the
/// cell height `h`; and the back end it draws, where a trail started now
/// begins. Nothing once it has gone into the cursor. The tail is `Chase`'s.
pub(super) fn trail(
    style: TrailStyle,
    from: Quad,
    to: Bounds<Pixels>,
    h: f32,
    t: f32,
) -> (Vec<Layer>, Option<Quad>) {
    let (a, b) = (centroid(&from), mid(to));
    let len = (a.0 - b.0).hypot(a.1 - b.1);
    if len.is_nan() || t.is_nan() || (len < CAUGHT_UP && enclosing(from) == to) {
        return (Vec::new(), None);
    }
    match style {
        TrailStyle::Comet => comet(a, to, h, t),
        TrailStyle::Sweep => sweep(enclosing(from), to, t),
        TrailStyle::Warp => warp(enclosing(from), to, h, t),
        TrailStyle::Tail => (Vec::new(), None),
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
/// decays into the head from `from`.
fn comet(from: P, to: Bounds<Pixels>, h: f32, t: f32) -> (Vec<Layer>, Option<Quad>) {
    let k = settle(t, COMET_TAIL);
    let head = mid(to);
    let tail = (head.0 + (from.0 - head.0) * k, head.1 + (from.1 - head.1) * k);
    let layers: Vec<Layer> = COMET_LAYERS
        .iter()
        .filter_map(|&(widen, alpha, core)| {
            streak(from, head, h, k, widen).map(|outline| Layer {
                outline,
                hole: None,
                stroke: None,
                alpha: alpha * k,
                core,
            })
        })
        .collect();
    let back = (!layers.is_empty()).then_some([tail; 4]);
    (layers, back)
}

/// The comet's outline from `from` to `head` with `k` of its tail's way
/// left, `widen` cell heights (`h`) across at the head: from the tail point
/// along one side, round the nose and back along the other.
pub(super) fn streak(from: P, head: P, h: f32, k: f32, widen: f32) -> Option<Vec<Point<Pixels>>> {
    let (dx, dy) = ((from.0 - head.0) * k, (from.1 - head.1) * k);
    let len = dx.hypot(dy);
    if len.is_nan() || len < CAUGHT_UP {
        return None;
    }
    // u points from the head back to the tail; n across it.
    let (u, n) = ((dx / len, dy / len), (-dy / len, dx / len));
    let half = h * widen / 2.0;
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

/// Tail: kitty's `cursor_trail_decay 0.1 0.4` ratio, the fastest corner's
/// decay against the slowest's. The slowest corner's decay is the effect's
/// duration: in that time a corner closes all but 1/1024 of its way (kitty's
/// step), so a `cursor_trail_duration_ms` of 400 is kitty's default. A tail
/// runs until every corner is within half a px of the cursor's (kitty's
/// `needs_render`), and never past `TAIL_CAP` durations.
const TAIL_FAST: f32 = 0.25;
const TAIL_CAP: f32 = 3.0;

/// kitty's trail: a quad whose corners chase the cursor's, each at a speed
/// set by how far ahead of the move it faces (those ahead fastest), drawn in
/// the cursor's color at full strength, less the cursor itself; it fades only
/// while the cursor is hidden, and back while it shows.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Chase {
    pub(super) corners: Quad,
    opacity: f32,
    /// When it last stepped, in s from the trail's start.
    at: f32,
}

impl Chase {
    pub(super) fn new(from: Quad) -> Self {
        Chase {
            corners: from,
            opacity: 1.0,
            at: 0.0,
        }
    }

    /// Moves the corners toward `to`'s for the time since the last step (the
    /// slowest over `decay` s) and the opacity toward `shown`'s; whether it is
    /// still to be drawn at `now` (s from the start).
    pub(super) fn step(&mut self, to: Bounds<Pixels>, shown: bool, now: f32, decay: f32) -> bool {
        let dt = (now - self.at).max(0.0);
        self.at = now;
        let (target, center) = (quad(to), mid(to));
        let diag = (f32::from(to.size.width).hypot(f32::from(to.size.height)) / 2.0).max(1e-3);
        let gap: Vec<P> = (0..4)
            .map(|i| (target[i].0 - self.corners[i].0, target[i].1 - self.corners[i].1))
            .collect();
        // How far each corner's move points the way its target faces from the
        // cursor's middle: the corners ahead of the move score highest.
        let facing: Vec<f32> = (0..4)
            .map(|i| {
                let len = gap[i].0.hypot(gap[i].1);
                if len < 1e-6 {
                    return 0.0;
                }
                let out = (target[i].0 - center.0, target[i].1 - center.1);
                (gap[i].0 * out.0 + gap[i].1 * out.1) / diag / len
            })
            .collect();
        let (lo, hi) = facing.iter().fold((f32::MAX, f32::MIN), |(lo, hi), f| (lo.min(*f), hi.max(*f)));
        let fast = decay * TAIL_FAST;
        for i in 0..4 {
            if gap[i].0.hypot(gap[i].1) < 1e-6 {
                continue;
            }
            let k = if hi > lo { decay + (fast - decay) * (facing[i] - lo) / (hi - lo) } else { decay };
            let step = 1.0 - (-10.0 * dt / k.max(1e-6)).exp2();
            self.corners[i].0 += gap[i].0 * step;
            self.corners[i].1 += gap[i].1 * step;
        }
        let fade = dt / decay.max(1e-6);
        self.opacity = if shown { (self.opacity + fade).min(1.0) } else { (self.opacity - fade).max(0.0) };
        let far = (0..4).any(|i| {
            (target[i].0 - self.corners[i].0).abs() >= CAUGHT_UP || (target[i].1 - self.corners[i].1).abs() >= CAUGHT_UP
        });
        far && self.opacity > 0.0 && now < decay * TAIL_CAP
    }

    /// The quad as it stands, less the cursor `to` (kitty draws the trail
    /// only outside the cursor's rect).
    pub(super) fn layers(&self, to: Bounds<Pixels>) -> Vec<Layer> {
        let outer = hull_points(self.corners.to_vec());
        if outer.len() < 3 {
            return Vec::new();
        }
        let cut = clipped(&quad(to), &outer);
        let hole = (cut.len() >= 3 && area(&cut).abs() > 1e-3).then(|| cut.into_iter().map(pt).collect());
        vec![Layer {
            outline: outer.into_iter().map(pt).collect(),
            hole,
            stroke: None,
            alpha: self.opacity,
            core: false,
        }]
    }
}

/// Sweep: a solid block from the old cursor to the new, its old end catching up
/// (easing out) until only the cursor is left.
fn sweep(from: Bounds<Pixels>, to: Bounds<Pixels>, t: f32) -> (Vec<Layer>, Option<Quad>) {
    let k = ease_out(t);
    if k >= 1.0 {
        return (Vec::new(), None);
    }
    let end = toward(from, to, k);
    (vec![fill(span(end, to, 0.0), 0.8 * (1.0 - k))], Some(quad(end)))
}

/// Warp: the decay time of its old end, and its soft edges as (grown by this
/// many cell heights, opacity), back to front.
const WARP_TAIL: f32 = 1.0 / 4.0;
const WARP_EDGES: [(f32, f32); 3] = [(0.5, 0.07), (0.25, 0.15), (0.0, 0.35)];

/// Neovide's stretched block: the block from its old end (decaying toward the
/// cursor) to the cursor with soft edges, and its half nearer the cursor drawn
/// again, so it fades toward the tail.
fn warp(from: Bounds<Pixels>, to: Bounds<Pixels>, h: f32, t: f32) -> (Vec<Layer>, Option<Quad>) {
    let k = settle(t, WARP_TAIL);
    let end = toward(to, from, k);
    if (mid(end).0 - mid(to).0).hypot(mid(end).1 - mid(to).1) < CAUGHT_UP {
        return (Vec::new(), None);
    }
    let mut layers: Vec<Layer> = WARP_EDGES
        .iter()
        .map(|&(grow, alpha)| fill(span(end, to, grow * h), alpha * k))
        .collect();
    layers.push(fill(span(toward(end, to, 0.5), to, 0.0), 0.35 * k));
    (layers, Some(quad(end)))
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
    let stroked = |outline| Layer {
        outline,
        hole: None,
        stroke: Some(1.0 + 1.5 * fade),
        alpha: fade,
        core: false,
    };
    match kind {
        Arrival::Off => Vec::new(),
        Arrival::Ripple => vec![stroked(ring(h * (0.5 + PULSE_REACH * e)))],
        Arrival::Boom => vec![fill(ring(h * (0.25 + PULSE_REACH * e)), 0.45 * fade)],
        Arrival::RippleRect => vec![stroked(corners(at, h * BOX_REACH * e).into_iter().map(pt).collect())],
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

    /// A beam (2 px wide) and an underline (2 px high) in the cell at (x, y),
    /// as the terminal paints them.
    fn beam(x: f32, y: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), gpui::size(px(2.0), px(16.0)))
    }

    fn underline(x: f32, y: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y + 14.0)), gpui::size(px(8.0), px(2.0)))
    }

    const H: f32 = 16.0;

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

    /// Whether `p` is inside the convex outline `o` (or on its edge).
    fn within(o: &[Point<Pixels>], p: P) -> bool {
        let pts: Vec<P> = o.iter().map(|q| xy(*q)).collect();
        let sides: Vec<f32> = (0..pts.len()).map(|i| cross(pts[i], pts[(i + 1) % pts.len()], p)).collect();
        sides.iter().all(|s| *s >= -1e-3) || sides.iter().all(|s| *s <= 1e-3)
    }

    /// Whether `layer` paints `p`: inside its outline and not in its hole (the
    /// even-odd fill the window paints it with).
    fn paints(layer: &Layer, p: P) -> bool {
        within(&layer.outline, p) && !layer.hole.as_ref().is_some_and(|h| within(h, p))
    }

    const STYLES: [TrailStyle; 3] = [TrailStyle::Comet, TrailStyle::Sweep, TrailStyle::Warp];
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

    /// The tail run as the window runs it, frames `frame` s apart, from
    /// `from` to `to`: each frame's layers while it lasts.
    fn chase(from: Bounds<Pixels>, to: Bounds<Pixels>, decay: f32, frame: f32) -> Vec<Vec<Layer>> {
        let mut c = Chase::new(quad(from));
        let (mut frames, mut now) = (Vec::new(), 0.0);
        while c.step(to, true, now, decay) {
            frames.push(c.layers(to));
            now += frame;
            assert!(frames.len() < 10_000, "a tail always ends");
        }
        frames
    }

    #[test]
    fn every_trail_is_convex_finite_and_near_its_cells_while_it_runs() {
        for style in STYLES {
            for dir in DIRS {
                for len in [0.6f32, 1.0, 5.0, 10.0, 400.0] {
                    let (from, to) = (cell(0.0, 0.0), cell(dir.0 * len, dir.1 * len));
                    // Within the cells' box, grown by the widest glow (2.4 cell heights / 2).
                    let (lo, hi) = ((dir.0 * len).min(0.0) - 20.0, (dir.0 * len).max(0.0) + 28.0);
                    let (top, bottom) = ((dir.1 * len).min(0.0) - 20.0, (dir.1 * len).max(0.0) + 36.0);
                    for t in [0.0, 0.1, 0.3, 0.6, 0.9] {
                        for layer in trail(style, quad(from), to, H, t).0 {
                            assert!(convex(&layer.outline) && layer.hole.is_none(), "{style:?} {dir:?} {len} {t}: {:?}", layer.outline);
                            assert!(layer.stroke.is_none() && (0.0..=1.0).contains(&layer.alpha), "{style:?}");
                            for p in layer.outline.iter().map(|p| xy(*p)) {
                                assert!(
                                    p.0.is_finite() && p.1.is_finite() && (lo..=hi).contains(&p.0) && (top..=bottom).contains(&p.1),
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
            assert!(!trail(style, quad(from), to, H, 0.05).0.is_empty(), "{style:?} draws as it starts");
            assert_eq!(trail(style, quad(from), to, H, 1.0), (Vec::new(), None), "{style:?} is gone at the end");
            assert!(trail(style, quad(cell(5.0, 5.0)), cell(5.0, 5.0), H, 0.2).0.is_empty(), "{style:?}: nothing for the same cell");
            assert!(trail(style, quad(from), to, H, f32::NAN).0.is_empty());
            // It fades: the strongest layer at 0.8 is fainter than at 0.1.
            let peak = |t: f32| trail(style, quad(from), to, H, t).0.iter().map(|l| l.alpha).fold(0.0f32, f32::max);
            assert!(peak(0.8) < peak(0.1), "{style:?}: {} then {}", peak(0.1), peak(0.8));
        }
    }

    #[test]
    fn every_trail_says_what_it_draws_so_a_new_one_starts_there() {
        let (from, to) = (cell(0.0, 0.0), cell(400.0, 0.0));
        // The comet's back end is its tail point, as `streak` draws it.
        let (layers, back) = trail(TrailStyle::Comet, quad(from), to, H, 0.2);
        assert!(dist(back.unwrap()[0], xy(layers[0].outline[0])) < 1e-3, "{back:?}");
        // The sweep's and the warp's are their old ends, between the cells, moving on.
        for style in [TrailStyle::Sweep, TrailStyle::Warp] {
            let x = centroid(&trail(style, quad(from), to, H, 0.2).1.unwrap()).0;
            assert!(4.0 < x && x < 404.0, "{style:?}: {x}");
            let later = centroid(&trail(style, quad(from), to, H, 0.5).1.unwrap()).0;
            assert!(later > x, "{style:?}: it moves on toward the cursor");
        }
        // A trail from a comet's tail point (a quad of one point) draws too.
        for style in STYLES {
            assert!(!trail(style, [(200.0, 8.0); 4], cell(0.0, 100.0), H, 0.1).0.is_empty(), "{style:?}");
        }
    }

    #[test]
    fn the_comet_layers_are_the_glow_then_the_core() {
        let layers = trail(TrailStyle::Comet, quad(cell(0.0, 0.0)), cell(400.0, 0.0), H, 0.0).0;
        assert_eq!(
            layers.iter().map(|l| (l.alpha, l.core)).collect::<Vec<_>>(),
            vec![(0.06, false), (0.15, false), (0.35, false), (1.0, true)]
        );
    }

    #[test]
    fn every_trail_ends_on_the_cursor_as_drawn_not_its_cell() {
        let from = cell(0.0, 0.0);
        // A beam in the cell at (400, 0): the comet's nose rounds the beam's middle.
        let comet = trail(TrailStyle::Comet, quad(from), beam(400.0, 0.0), H, 0.0).0;
        let nose = comet[3].outline.iter().map(|p| xy(*p).0).fold(f32::MIN, f32::max);
        assert!((nose - (401.0 + 16.0 * 0.45 / 2.0)).abs() < 0.01, "{nose}");
        // The sweep's block narrows into the beam, or into an underline's 2 px.
        let sweep = |to: Bounds<Pixels>| trail(TrailStyle::Sweep, quad(from), to, H, 0.99).0[0].clone();
        let right = sweep(beam(400.0, 0.0)).outline.iter().map(|p| xy(*p).0).fold(f32::MIN, f32::max);
        assert!(right <= 402.01, "{right}");
        let top = sweep(underline(400.0, 0.0)).outline.iter().map(|p| xy(*p).1).fold(f32::MAX, f32::min);
        assert!(top > 13.0, "into the underline: {top}");
        // The warp ends on the beam, its soft edges a share of the cell's height.
        let warp = trail(TrailStyle::Warp, quad(from), beam(400.0, 0.0), H, 0.1).0;
        let top = warp[0].outline.iter().map(|p| xy(*p).1).fold(f32::MAX, f32::min);
        assert!((top + 8.0).abs() < 0.01, "grown half a cell height: {top}");
        // The tail's corners settle on the beam's.
        let frames = chase(from, beam(400.0, 0.0), 0.12, 1.0 / 60.0);
        let last = &frames.last().unwrap()[0];
        assert!(last.outline.iter().all(|p| (397.0..402.6).contains(&xy(*p).0)), "{:?}", last.outline);
    }

    #[test]
    fn the_tail_is_solid_and_cut_out_where_the_cursor_is() {
        let (from, to) = (cell(0.0, 0.0), cell(400.0, 0.0));
        let frames = chase(from, to, 0.4, 1.0 / 60.0);
        assert!(frames.len() > 3);
        for (i, f) in frames.iter().enumerate() {
            let l = &f[0];
            assert_eq!(l.alpha, 1.0, "frame {i}: full strength while the cursor shows");
            assert!(convex(&l.outline), "frame {i}: {:?}", l.outline);
            for p in [(404.0, 8.0), (401.0, 1.0), (407.0, 15.0)] {
                assert!(!paints(l, p), "frame {i}: the cursor's own cell is cut out, {p:?}");
            }
            if let Some(hole) = &l.hole {
                assert!(convex(hole) && hole.iter().all(|p| within(&l.outline, xy(*p))), "frame {i}: a hole inside the quad");
            }
        }
        assert!(paints(&frames[1][0], (200.0, 8.0)), "the stretch between the cells is drawn");
        // Hidden, it fades over the slowest decay; shown again, it comes back.
        let mut c = Chase::new(quad(from));
        c.step(to, true, 0.0, 0.4);
        c.step(to, false, 0.1, 0.4);
        assert!((c.layers(to)[0].alpha - 0.75).abs() < 1e-4);
        c.step(to, true, 0.15, 0.4);
        assert!((c.layers(to)[0].alpha - 0.875).abs() < 1e-4);
        let mut c = Chase::new(quad(from));
        assert!(!c.step(to, false, 0.5, 0.4), "hidden long enough, it's gone");
    }

    #[test]
    fn the_tail_chases_with_its_leading_corners_fastest_and_ends_within_half_a_px() {
        let (from, to) = (cell(0.0, 0.0), cell(400.0, 0.0));
        let mut c = Chase::new(quad(from));
        assert!(c.step(to, true, 0.0, 0.4), "no time yet: still to draw");
        assert_eq!(c.corners, quad(from), "and nothing moved");
        assert!(c.step(to, true, 1.0 / 60.0, 0.4));
        // The right-hand corners lead the move: they close their way at the
        // fast decay (a quarter of the slowest), eased rather than snapped.
        let (lead, back) = (c.corners[1].0, c.corners[0].0);
        let want = 8.0 + 400.0 * (1.0 - (-10.0f32 / 60.0 / 0.1).exp2());
        assert!((lead - want).abs() < 0.01 && lead < 400.0, "{lead} vs {want}");
        let want = 400.0 * (1.0 - (-10.0f32 / 60.0 / 0.4).exp2());
        assert!((back - want).abs() < 0.01, "the trailing corners at the slow decay: {back} vs {want}");
        // It runs until every corner is within half a px of the cursor's.
        let frames = chase(from, to, 0.4, 1.0 / 60.0);
        let secs = frames.len() as f32 / 60.0;
        assert!((0.3..0.45).contains(&secs), "about the slowest decay for a long jump: {secs}");
        let mut c = Chase::new(quad(from));
        let mut now = 0.0;
        while c.step(to, true, now, 0.4) {
            now += 1.0 / 60.0;
        }
        assert!(c.corners.iter().zip(quad(to)).all(|(a, b)| (a.0 - b.0).abs() < 0.5 && (a.1 - b.1).abs() < 0.5), "{:?}", c.corners);
        // At 30 frames a second too: time, not frames, sets its pace.
        let slow = chase(from, to, 0.4, 1.0 / 30.0).len() as f32 / 30.0;
        assert!((slow - secs).abs() < 0.05, "{slow} vs {secs}");
        // A shorter duration is a faster tail; none runs past three durations.
        assert!(chase(from, to, 0.14, 1.0 / 60.0).len() < frames.len() / 2);
        let mut c = Chase::new(quad(cell(0.0, 0.0)));
        assert!(!c.step(cell(1e9, 0.0), true, 0.14 * 3.0, 0.14), "capped");
    }

    #[test]
    fn a_tail_steered_mid_run_starts_from_its_corners() {
        let mut c = Chase::new(quad(cell(0.0, 0.0)));
        c.step(cell(400.0, 0.0), true, 0.0, 0.4);
        c.step(cell(400.0, 0.0), true, 0.05, 0.4);
        let mut next = Chase::new(c.corners);
        assert!(next.step(cell(0.0, 300.0), true, 0.0, 0.4));
        assert_eq!(next.corners, c.corners, "it starts exactly where the last one was");
        assert_eq!(next.layers(cell(0.0, 300.0))[0].outline, c.layers(cell(400.0, 0.0))[0].outline);
    }

    #[test]
    fn the_sweep_block_shrinks_from_the_old_cursor_into_the_new() {
        let (from, to) = (cell(0.0, 0.0), cell(400.0, 0.0));
        let width = |t: f32| {
            let o = &trail(TrailStyle::Sweep, quad(from), to, H, t).0[0].outline;
            let xs: Vec<f32> = o.iter().map(|p| xy(*p).0).collect();
            xs.iter().cloned().fold(f32::MIN, f32::max) - xs.iter().cloned().fold(f32::MAX, f32::min)
        };
        assert!((width(0.0) - 408.0).abs() < 0.01, "both cells at first");
        assert!(width(0.2) > width(0.5) && width(0.5) > width(0.9) && width(0.9) >= 8.0);
    }

    #[test]
    fn the_warp_is_strongest_at_the_cursor_with_soft_edges() {
        let (from, to) = (cell(0.0, 0.0), cell(400.0, 0.0));
        let layers = trail(TrailStyle::Warp, quad(from), to, H, 0.1).0;
        assert_eq!(layers.len(), 4);
        // The soft edges reach past the block, the last layer (the half nearer the cursor) doesn't reach the old end.
        let reach_left = |l: &Layer| l.outline.iter().map(|p| xy(*p).0).fold(f32::MAX, f32::min);
        assert!(reach_left(&layers[0]) < reach_left(&layers[2]) && reach_left(&layers[2]) < reach_left(&layers[3]));
        let top = |l: &Layer| l.outline.iter().map(|p| xy(*p).1).fold(f32::MAX, f32::min);
        assert!((top(&layers[0]) + 8.0).abs() < 0.01, "grown half a cell height");
    }

    #[test]
    fn every_pulse_grows_from_the_cursor_fades_and_ends() {
        let at = cell(100.0, 100.0);
        let c = (104.0, 108.0);
        for kind in PULSES {
            let size = |t: f32| trail_reach(&arrival(kind, at, t), c);
            assert!(size(0.0) < size(0.5) && size(0.5) < size(0.9), "{kind:?} grows");
            assert!(size(0.99) <= 16.0 * (0.5 + 2.0) * 1.5 + 1.0, "{kind:?} stays within its reach");
            assert!(arrival(kind, at, 0.5)[0].alpha > arrival(kind, at, 0.9)[0].alpha, "{kind:?} fades");
            assert!(arrival(kind, at, 1.0).is_empty() && arrival(kind, at, f32::NAN).is_empty(), "{kind:?} ends");
            for t in [0.0, 0.4, 0.95] {
                let l = &arrival(kind, at, t)[0];
                assert!(convex(&l.outline), "{kind:?} {t}");
                let rings = matches!(kind, Arrival::Ripple | Arrival::RippleRect);
                assert_eq!(l.stroke.is_some(), rings, "{kind:?}: rings are stroked, booms filled");
            }
        }
        assert!(arrival(Arrival::Off, at, 0.3).is_empty());
    }

    fn trail_reach(layers: &[Layer], c: P) -> f32 {
        layers.iter().flat_map(|l| l.outline.iter()).map(|p| dist(xy(*p), c)).fold(0.0f32, f32::max)
    }

    #[test]
    fn the_hull_of_two_cells_is_one_way_round_without_repeats() {
        let o = span(cell(0.0, 0.0), cell(0.0, 0.0), 0.0);
        assert_eq!(o.len(), 4, "the same cell twice is its four corners");
        let o = span(cell(0.0, 0.0), cell(100.0, 50.0), 0.0);
        assert_eq!(o.len(), 6, "two cells on a diagonal: a hexagon");
        assert!(convex(&o));
        assert!(area(&hull_points(corners(cell(0.0, 0.0), 0.0).to_vec())) > 0.0);
    }

    #[test]
    fn clipping_keeps_the_part_inside() {
        let square = hull_points(vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]);
        let half = clipped(&[(5.0, -5.0), (20.0, -5.0), (20.0, 20.0), (5.0, 20.0)], &square);
        assert!((area(&half).abs() - 100.0).abs() < 1e-3, "{half:?}");
        assert!(clipped(&[(20.0, 20.0), (30.0, 20.0), (30.0, 30.0)], &square).is_empty(), "outside: nothing");
    }

    /// The comet's outline at progress `t`, as the trail draws it.
    fn streak_t(from: Bounds<Pixels>, to: Bounds<Pixels>, t: f32, widen: f32) -> Option<Vec<Point<Pixels>>> {
        streak(mid(from), mid(to), f32::from(to.size.height), settle(t, COMET_TAIL), widen)
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
        assert_eq!(xy(first[0]), (4.0, 8.0), "the tail starts on the old cursor's middle, a single point");
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
        assert!(dist(xy(last[0]), (404.0, 8.0)) < 4.0, "the last frame is all but in the cursor: {last:?}");
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
                    ((tail.0 - head.0) * (y - head.1) - (tail.1 - head.1) * (x - head.0)).abs() / len
                })
                .fold(0.0f32, f32::max)
        };
        assert!((across(&core) - 16.0 * 0.45 / 2.0).abs() < 0.01, "the core is 0.45 of a cell high");
        assert!((across(&glow) - 16.0 * 2.4 / 2.0).abs() < 0.01, "the outer glow 2.4");
    }

    #[test]
    fn comet_the_streak_points_any_way_and_is_finite() {
        for to in [cell(0.0, -300.0), cell(-300.0, 0.0), cell(0.0, 300.0), cell(37.0, 211.0)] {
            for t in [-1.0, 0.0, 0.01, 0.5, 0.99, f32::NAN] {
                if let Some(o) = streak_t(cell(0.0, 0.0), to, t, 2.4) {
                    assert!(o.iter().all(|p| xy(*p).0.is_finite() && xy(*p).1.is_finite()), "{to:?} {t}: {o:?}");
                    // The tail is on the way back to the old cursor, settle(t) of it.
                    let (head, from) = (xy(to.center()), (4.0, 8.0));
                    let k = settle(t, COMET_TAIL);
                    let want = (head.0 + (from.0 - head.0) * k, head.1 + (from.1 - head.1) * k);
                    assert!(dist(xy(o[0]), want) < 0.01, "{to:?} {t}: {:?} not {want:?}", xy(o[0]));
                    assert!(dist(xy(o[0]), head) >= CAUGHT_UP);
                }
            }
        }
    }

    #[test]
    fn comet_every_layer_is_a_convex_outline_at_any_length() {
        for dir in DIRS {
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
