//! Cursor trail (`cursor_trail` in settings.json): as the focus moves from one
//! pane of the shown tab to another, a cursor-sized quad stretches from the old
//! pane's cursor to the new one's and its tail chases the cursor, as kitty's
//! `cursor_trail` does. The head is on the new cursor from the first frame;
//! the tail's corners decay toward it exponentially, those facing the move
//! faster than those behind, and the whole trail is gone within
//! `cursor_trail_duration_ms`. It is drawn above the tiled and floating panes,
//! below dialogs.
//!
//! Each end is the terminal's `trail_anchor`: its cursor cell, shown or
//! hidden, else its middle cell. A pane that isn't a painted terminal (never
//! drawn) gets no trail. The head follows the new pane's cursor live while the
//! trail runs.
//!
//! Nothing runs in the terminals' output or paint paths. With the setting off,
//! a window draw checks one setting; on, it compares the focused pane with the
//! one it saw last. Frames are asked for only while a trail is alive, and every
//! trail ends: its duration is clamped and its progress only grows.

use super::*;

/// The slowest corner's decay time constant, as a share of the trail's
/// duration (`settle` lands it exactly at the end).
const SLOW: f32 = 1.0 / 3.0;
/// A corner square to the move decays this much faster, as kitty's default
/// `cursor_trail_decay 0.1 0.4`.
const FAST: f32 = SLOW / 4.0;
/// The trail's opacity as it starts, on top of the cursor color's own: the
/// cursor's; it eases out to nothing at its end.
const OPACITY: f32 = 1.0;
/// Below this many px left to go, the tail has reached the cursor.
const CAUGHT_UP: f32 = 0.5;

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

/// A cell's corners, clockwise from the top left.
fn corners(cell: Bounds<Pixels>) -> [(f32, f32); 4] {
    let (x, y) = (f32::from(cell.origin.x), f32::from(cell.origin.y));
    let (w, h) = (f32::from(cell.size.width), f32::from(cell.size.height));
    [(x, y), (x + w, y), (x + w, y + h), (x, y + h)]
}

/// A corner's decay time constant, as a share of the trail's duration, from
/// how far it faces the move (`facing`, -1 behind to 1 ahead): those ahead of
/// the cell's middle are the head and sit on the cursor at once (0), one
/// square to the move takes `FAST`, the one straight behind `SLOW`.
fn decay(facing: f32) -> f32 {
    if facing > 1e-3 {
        0.0
    } else {
        SLOW + (FAST - SLOW) * (1.0 + facing.max(-1.0))
    }
}

/// How much of its way a corner with decay time `tau` has left at progress
/// `t`: exponential decay, scaled so it is exactly none at the end.
fn settle(t: f32, tau: f32) -> f32 {
    let end = (-1.0 / tau).exp();
    ((-t / tau).exp() - end) / (1.0 - end)
}

/// The trail at progress `t` from the cell `from` to the cell `to`: each of
/// the quad's corners starts at `from`'s and decays toward `to`'s, so it is
/// cursor-sized at both ends and stretched between. None once every corner is
/// within `CAUGHT_UP` of the cursor (or for the same cell).
pub(super) fn comet(
    from: Bounds<Pixels>,
    to: Bounds<Pixels>,
    t: f32,
) -> Option<[Point<Pixels>; 4]> {
    let t = t.clamp(0.0, 1.0);
    let (a, b) = (from.center(), to.center());
    let (dx, dy) = (
        f32::from(b.x) - f32::from(a.x),
        f32::from(b.y) - f32::from(a.y),
    );
    let len = dx.hypot(dy);
    if len.is_nan() || len < CAUGHT_UP {
        return None;
    }
    let (ux, uy) = (dx / len, dy / len);
    let (start, end) = (corners(from), corners(to));
    let mut left = 0.0f32;
    let mut quad = [point(px(0.0), px(0.0)); 4];
    for (i, (s, e)) in start.into_iter().zip(end).enumerate() {
        // The corner's direction from the cell's middle, unit length.
        let (cx, cy) = ([-1.0, 1.0, 1.0, -1.0][i], [-1.0, -1.0, 1.0, 1.0][i]);
        let tau = decay((cx * ux + cy * uy) / std::f32::consts::SQRT_2);
        let k = if tau == 0.0 { 0.0 } else { settle(t, tau) };
        let (x, y) = (e.0 + (s.0 - e.0) * k, e.1 + (s.1 - e.1) * k);
        left = left.max((x - e.0).hypot(y - e.1));
        quad[i] = point(px(x), px(y));
    }
    (left >= CAUGHT_UP).then_some(quad)
}

/// The trail's opacity at progress `t`: full as it starts, easing out with
/// the slowest corner's distance, to nothing at the end.
pub(super) fn opacity(t: f32) -> f32 {
    OPACITY * settle(t.clamp(0.0, 1.0), SLOW).clamp(0.0, 1.0)
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
                    let Some(quad) = to
                        .read(cx)
                        .trail_anchor()
                        .and_then(|head| comet(from, head, t))
                    else {
                        return;
                    };
                    let mut path = PathBuilder::fill();
                    path.add_polygon(&quad, true);
                    if let Ok(path) = path.build() {
                        window.paint_path(path, color.opacity(opacity(t)));
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
    use super::{comet, opacity, progress};
    use gpui::{Bounds, Pixels, point, px, size};
    use std::time::Duration;

    fn cell(x: f32, y: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(8.0), px(16.0)))
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
    fn the_trail_fades_out() {
        assert!((opacity(0.0) - 1.0).abs() < 1e-6);
        assert!(opacity(0.5) < opacity(0.25) && opacity(0.25) < opacity(0.1));
        assert!(opacity(0.1) < 0.8, "it eases out from the start");
        assert_eq!(opacity(1.0), 0.0);
        assert_eq!(opacity(2.0), 0.0);
    }

    fn x(p: gpui::Point<Pixels>) -> f32 {
        f32::from(p.x)
    }

    fn y(p: gpui::Point<Pixels>) -> f32 {
        f32::from(p.y)
    }

    #[test]
    fn the_head_is_on_the_cursor_at_once_and_the_tail_chases_it() {
        let (from, to) = (cell(0.0, 0.0), cell(400.0, 0.0));
        // Corners: top left, top right, bottom right, bottom left.
        let first = comet(from, to, 0.0).unwrap();
        assert_eq!(
            (x(first[1]), x(first[2])),
            (408.0, 408.0),
            "the head is on the cursor's far side from the first frame"
        );
        assert_eq!(
            (x(first[0]), x(first[3])),
            (0.0, 0.0),
            "the tail starts on the old cursor"
        );
        // The tail decays toward the cursor: quickly at first, slower as it nears.
        let tail = |t: f32| x(comet(from, to, t).unwrap()[0]);
        let (a, b, c) = (tail(0.1), tail(0.2), tail(0.3));
        assert!(0.0 < a && a < b && b < c && c < 400.0, "{a} {b} {c}");
        assert!(b - a < a, "it slows: {a} {b}");
        // The trail is the cursor's height all along.
        for t in [0.0, 0.1, 0.4] {
            let q = comet(from, to, t).unwrap();
            assert_eq!((y(q[2]) - y(q[1]), y(q[3]) - y(q[0])), (16.0, 16.0), "{t}");
        }
    }

    #[test]
    fn corners_square_to_the_move_catch_up_faster_than_the_one_behind() {
        // Down and to the right: the bottom right corner leads, the top left trails.
        let (from, to) = (cell(0.0, 0.0), cell(300.0, 300.0));
        let q = comet(from, to, 0.15).unwrap();
        let end = [
            (300.0, 300.0),
            (308.0, 300.0),
            (308.0, 316.0),
            (300.0, 316.0),
        ];
        let left: Vec<f32> = q
            .iter()
            .zip(end)
            .map(|(p, (ex, ey))| (x(*p) - ex).hypot(y(*p) - ey))
            .collect();
        assert_eq!(left[2], 0.0, "the leading corner is on the cursor");
        assert!(
            left[1] < left[0] && left[3] < left[0],
            "the side corners lead the one behind: {left:?}"
        );
    }

    #[test]
    fn the_trail_ends_on_the_cursor() {
        let (from, to) = (cell(0.0, 0.0), cell(0.0, 600.0));
        assert!(comet(from, to, 0.5).is_some());
        assert_eq!(
            comet(from, to, 1.0),
            None,
            "within half a px by its end, even across 600 px"
        );
    }

    #[test]
    fn the_trail_points_any_way() {
        for to in [
            cell(0.0, -300.0),
            cell(-300.0, 0.0),
            cell(0.0, 300.0),
            cell(-200.0, 150.0),
        ] {
            let q = comet(cell(0.0, 0.0), to, 0.2).unwrap();
            let end = super::corners(to);
            let on = q
                .iter()
                .zip(end)
                .filter(|(p, e)| x(**p) == e.0 && y(**p) == e.1)
                .count();
            assert!(
                (1..=2).contains(&on),
                "{to:?}: the leading corners are on the cursor: {q:?}"
            );
        }
    }

    #[test]
    fn nothing_is_drawn_for_the_same_cell() {
        for t in [0.0, 0.3, 0.99] {
            assert_eq!(comet(cell(10.0, 10.0), cell(10.0, 10.0), t), None);
        }
    }

    #[test]
    fn the_quad_is_always_finite() {
        for t in [-1.0, 0.0, 0.01, 0.5, 1.0, 5.0, f32::NAN] {
            if let Some(q) = comet(cell(0.0, 0.0), cell(37.0, 211.0), t) {
                assert!(
                    q.iter()
                        .all(|p| f32::from(p.x).is_finite() && f32::from(p.y).is_finite()),
                    "{t}: {q:?}"
                );
            }
        }
    }
}
