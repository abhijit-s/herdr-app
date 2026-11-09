//! Cursor trail (`cursor_trail` in settings.json): as the focus moves from one
//! pane of the shown tab to another, a glowing streak runs from the new pane's
//! cursor back toward the old one's and retracts into the cursor, as Rex's
//! and kitty's cursor trails do. Its head is on the new cursor from the first
//! frame, about a cell high; it tapers to a point at its tail, which decays
//! toward the head and is gone within `cursor_trail_duration_ms`. It is drawn
//! above the tiled and floating panes, below dialogs.
//!
//! GPUI has no blur, so the glow is the same tapered outline drawn wider and
//! fainter behind a paler core: `LAYERS`, four paths a frame while it runs.
//! Every color is the theme's cursor color (or `cursor_trail_color`).
//!
//! Each end is the terminal's `trail_anchor`: its cursor cell, shown or
//! hidden, else its middle cell. A pane that isn't a painted terminal (never
//! drawn) gets no trail. The head follows the new pane's cursor live while the
//! trail runs.
//!
//! Nothing runs in the terminals' output or paint paths. With the setting off,
//! a window draw checks one setting; on, it compares the focused pane with the
//! one it saw last and keeps that pane's cursor anchor (a map lookup and a
//! cell read). Frames are asked for only while a trail is alive, and every
//! trail ends: its duration is clamped and its progress only grows.

use super::*;

/// The tail's decay time constant, as a share of the trail's duration
/// (`settle` lands it exactly on the head at the end).
const TAIL: f32 = 1.0 / 3.0;
/// Below this many px long, the streak has gone into the cursor.
const CAUGHT_UP: f32 = 0.5;
/// Samples along each side of the streak, head to tail.
const SIDE: usize = 6;
/// How fast the width falls off toward the tail: under 1, a spindle that
/// stays full near the head and narrows to a needle point.
const TAPER: f32 = 0.6;
/// The layers drawn, back to front: (width against the cell's height, opacity,
/// paler core). The wide faint ones are the glow.
const LAYERS: [(f32, f32, bool); 4] = [
    (2.4, 0.06, false),
    (1.7, 0.15, false),
    (1.0, 0.35, false),
    (0.45, 1.0, true),
];
/// How far the core's color is mixed toward white.
const CORE_WHITE: f32 = 0.45;

/// A trail being drawn, toward the terminal `pane` shows.
pub(super) struct Trail {
    pub(super) from: Bounds<Pixels>,
    pub(super) pane: String,
    pub(super) start: Instant,
    duration: Duration,
    color: Hsla,
}

/// The shown tab's focused pane at the last draw, and where its trail would
/// start: kept each draw, so a pane that closes still has one.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Seen {
    pub(super) tab: String,
    pub(super) pane: String,
    anchor: Option<Bounds<Pixels>>,
}

/// How far along a trail is, 0 to 1, after `elapsed` of `duration`; None once it's over.
pub(super) fn progress(elapsed: Duration, duration: Duration) -> Option<f32> {
    (elapsed < duration).then(|| elapsed.as_secs_f32() / duration.as_secs_f32())
}

/// How much of its way the tail has left at progress `t`: exponential decay,
/// scaled so it is exactly none at the end.
fn settle(t: f32) -> f32 {
    let end = (-1.0 / TAIL).exp();
    ((-t / TAIL).exp() - end) / (1.0 - end)
}

/// The streak at progress `t` from the cell `from` to the cell `to`, `widen`
/// times the cell's height across at the head: a closed outline from the tail
/// point along one side to the head, round the head's nose and back along the
/// other side. The head is `to`'s middle throughout; the tail starts at
/// `from`'s and decays toward it. None once it is shorter than `CAUGHT_UP`
/// (or for the same cell).
pub(super) fn streak(
    from: Bounds<Pixels>,
    to: Bounds<Pixels>,
    t: f32,
    widen: f32,
) -> Option<Vec<Point<Pixels>>> {
    let (a, b) = (from.center(), to.center());
    let head = (f32::from(b.x), f32::from(b.y));
    let k = settle(t.clamp(0.0, 1.0));
    let (dx, dy) = ((f32::from(a.x) - head.0) * k, (f32::from(a.y) - head.1) * k);
    let len = dx.hypot(dy);
    if len.is_nan() || len < CAUGHT_UP {
        return None;
    }
    // u points from the head back to the tail; n across it.
    let (u, n) = ((dx / len, dy / len), (-dy / len, dx / len));
    let half = f32::from(to.size.height) * widen / 2.0;
    let at = |along: f32, across: f32| {
        point(
            px(head.0 + u.0 * along + n.0 * across),
            px(head.1 + u.1 * along + n.1 * across),
        )
    };
    let width = |s: f32| half * (1.0 - s).powf(TAPER);
    let mut outline = Vec::with_capacity(2 * SIDE + 4);
    outline.push(at(len, 0.0));
    outline.extend(
        (1..SIDE)
            .rev()
            .map(|i| i as f32 / SIDE as f32)
            .map(|s| at(s * len, width(s))),
    );
    // The nose: a half circle's worth of points ahead of the head.
    outline.extend(
        [0.0f32, 0.5, 0.85, 1.0, 0.85, 0.5, 0.0]
            .iter()
            .zip([1.0f32, 0.87, 0.5, 0.0, -0.5, -0.87, -1.0])
            .map(|(ahead, side)| at(-half * ahead, half * side)),
    );
    outline.extend(
        (1..SIDE)
            .map(|i| i as f32 / SIDE as f32)
            .map(|s| at(s * len, -width(s))),
    );
    Some(outline)
}

/// The trail's opacity at progress `t`: full as it starts, easing out as the
/// streak shortens, to nothing at the end.
pub(super) fn opacity(t: f32) -> f32 {
    settle(t.clamp(0.0, 1.0)).clamp(0.0, 1.0)
}

/// The core's color: the trail's mixed `CORE_WHITE` toward white.
fn paler(c: Hsla) -> Hsla {
    Hsla {
        l: c.l + (1.0 - c.l) * CORE_WHITE,
        ..c
    }
}

impl AppWindow {
    /// Starts a trail when the shown tab's focused pane changed since the last
    /// draw, from where the pane it left last drew its cursor; any trail still
    /// running ends there. Off, it forgets what it saw, so turning it on never
    /// starts one from a stale pane.
    pub(super) fn watch_cursor_trail(&mut self, cx: &App) {
        let s = settings::get();
        if s.cursor_trail == settings::CursorTrail::Off {
            (self.trail_seen, self.trail) = (None, None);
            return;
        }
        let Some((tab, pane)) = self
            .model
            .active_tab()
            .and_then(|t| Some((t.id.clone(), t.focused_pane()?)))
        else {
            (self.trail_seen, self.trail) = (None, None);
            return;
        };
        let anchor = self
            .pane_term(&pane)
            .and_then(|term| term.view.read(cx).trail_anchor());
        match self.trail_seen.take() {
            Some(seen) if seen.tab == tab && seen.pane == pane => {
                self.trail_seen = Some(Seen {
                    anchor: anchor.or(seen.anchor),
                    ..seen
                });
                return;
            }
            Some(seen) => {
                self.trail = None;
                if let Some(from) = seen
                    .anchor
                    .filter(|_| seen.tab == tab && self.pane_term(&pane).is_some())
                {
                    let color = s.trail_color().unwrap_or_else(|| settings::color("cursor"));
                    self.trail = Some(Trail {
                        from,
                        pane: pane.clone(),
                        start: Instant::now(),
                        duration: s.trail_duration(),
                        color,
                    });
                }
            }
            None => {}
        }
        self.trail_seen = Some(Seen { tab, pane, anchor });
    }

    /// The layer the live trail paints on, asking for the next frame while it
    /// runs; None (and the trail dropped) once it's over or its pane is gone.
    pub(super) fn cursor_trail_layer(&mut self) -> Option<AnyElement> {
        let trail = self.trail.as_ref()?;
        let view = self.pane_term(&trail.pane).map(|term| term.view.clone());
        let (Some(to), Some(_)) = (view, progress(trail.start.elapsed(), trail.duration)) else {
            self.trail = None;
            return None;
        };
        let (from, start, duration, color) = (trail.from, trail.start, trail.duration, trail.color);
        Some(
            canvas(
                |_, _, _| {},
                move |_, _, window, cx| {
                    let Some(t) = progress(start.elapsed(), duration) else {
                        return;
                    };
                    window.request_animation_frame();
                    let Some(head) = to.read(cx).trail_anchor() else {
                        return;
                    };
                    let fade = opacity(t);
                    for (widen, alpha, core) in LAYERS {
                        let Some(outline) = streak(from, head, t, widen) else {
                            return;
                        };
                        let mut path = PathBuilder::fill();
                        path.add_polygon(&outline, true);
                        if let Ok(path) = path.build() {
                            let tint = if core { paler(color) } else { color };
                            window.paint_path(path, tint.opacity(alpha * fade));
                        }
                    }
                },
            )
            .absolute()
            .size_full()
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{CAUGHT_UP, opacity, paler, progress, streak};
    use gpui::{Bounds, Pixels, Point, point, px, size};
    use std::time::Duration;

    fn cell(x: f32, y: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(8.0), px(16.0)))
    }

    fn xy(p: Point<Pixels>) -> (f32, f32) {
        (f32::from(p.x), f32::from(p.y))
    }

    fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
        (a.0 - b.0).hypot(a.1 - b.1)
    }

    #[test]
    fn a_trail_runs_its_duration_then_ends() {
        let d = Duration::from_millis(200);
        assert_eq!(progress(Duration::ZERO, d), Some(0.0));
        assert_eq!(progress(Duration::from_millis(100), d), Some(0.5));
        assert_eq!(
            progress(Duration::from_millis(200), d),
            None,
            "over at its duration"
        );
        assert_eq!(progress(Duration::from_secs(60), d), None, "and after");
        // Progress only grows, so a trail asking for frames always reaches its end.
        let steps: Vec<f32> = (0..200)
            .filter_map(|ms| progress(Duration::from_millis(ms), d))
            .collect();
        assert!(steps.windows(2).all(|w| w[0] < w[1]) && steps.len() == 200);
    }

    #[test]
    fn the_trail_fades_as_it_shortens() {
        assert!((opacity(0.0) - 1.0).abs() < 1e-6);
        assert!(opacity(0.5) < opacity(0.25) && opacity(0.25) < opacity(0.1));
        assert!(opacity(0.1) < 0.8, "it eases out from the start");
        assert_eq!(opacity(1.0), 0.0);
        assert_eq!(opacity(2.0), 0.0);
    }

    #[test]
    fn the_head_is_on_the_cursor_a_cell_high_and_the_tail_a_point() {
        let (from, to) = (cell(0.0, 0.0), cell(400.0, 0.0));
        let head = (404.0, 8.0);
        let first = streak(from, to, 0.0, 1.0).unwrap();
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
    fn the_tail_retracts_into_the_head() {
        let (from, to) = (cell(0.0, 0.0), cell(400.0, 0.0));
        let tail = |t: f32| xy(streak(from, to, t, 1.0).unwrap()[0]).0;
        let (a, b, c) = (tail(0.1), tail(0.2), tail(0.3));
        assert!(4.0 < a && a < b && b < c && c < 404.0, "{a} {b} {c}");
        assert!(b - a < a - 4.0, "fast at first, then slower: {a} {b}");
        let last = streak(from, to, 0.99, 1.0).unwrap();
        assert!(
            dist(xy(last[0]), (404.0, 8.0)) < 2.0,
            "the last frame is all but in the cursor: {last:?}"
        );
        assert_eq!(streak(from, to, 1.0, 1.0), None, "and gone at the end");
    }

    #[test]
    fn the_glow_layers_share_the_tail_and_widen() {
        let (from, to) = (cell(0.0, 0.0), cell(-200.0, 150.0));
        let core = streak(from, to, 0.2, 0.45).unwrap();
        let glow = streak(from, to, 0.2, 2.4).unwrap();
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
    fn the_streak_points_any_way_and_is_finite() {
        for to in [
            cell(0.0, -300.0),
            cell(-300.0, 0.0),
            cell(0.0, 300.0),
            cell(37.0, 211.0),
        ] {
            for t in [-1.0, 0.0, 0.01, 0.5, 0.99, f32::NAN] {
                if let Some(o) = streak(cell(0.0, 0.0), to, t, 2.4) {
                    assert!(
                        o.iter()
                            .all(|p| xy(*p).0.is_finite() && xy(*p).1.is_finite()),
                        "{to:?} {t}: {o:?}"
                    );
                    assert!(dist(xy(o[0]), xy(to.center())) >= CAUGHT_UP);
                }
            }
        }
    }

    #[test]
    fn nothing_is_drawn_for_the_same_cell() {
        for t in [0.0, 0.3, 0.99] {
            assert_eq!(streak(cell(10.0, 10.0), cell(10.0, 10.0), t, 1.0), None);
        }
    }

    #[test]
    fn the_core_is_the_cursor_color_paler() {
        let yellow = gpui::hsla(0.11, 1.0, 0.7, 1.0);
        let core = paler(yellow);
        assert_eq!((core.h, core.s, core.a), (yellow.h, yellow.s, yellow.a));
        assert!((core.l - (0.7 + 0.3 * 0.45)).abs() < 1e-6);
    }
}
