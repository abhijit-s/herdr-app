//! Cursor effects (`cursor_trail` and `cursor_arrival` in settings.json): as
//! the focus moves from one pane of the shown tab to another, a trail from the
//! old pane's cursor into the new one's, in one of `cursor_trail_style`'s
//! looks, and a pulse at the new cursor, each over `cursor_trail_duration_ms`.
//! They are drawn above the tiled and floating panes, below dialogs; their
//! shapes are window/cursor_shapes.rs's, after sahaj-b/ghostty-cursor-shaders
//! (MIT). Every color is the theme's cursor color (or `cursor_trail_color`),
//! the comet's core paler.
//!
//! Each end is the terminal's `trail_end`: its cursor as drawn (a block's
//! cell, a beam's or an underline's strip), else its cursor cell when hidden,
//! else its middle cell. A pane that isn't a painted terminal (never drawn)
//! gets no effect. The new end follows the pane's cursor live while the effect
//! runs. A trail that starts while another is drawn starts from what that one
//! drew (its tail's corners, a comet's tail point, a block's old end), as
//! kitty's corners keep chasing a cursor that moves on.
//!
//! Nothing runs in the terminals' output paths, and their paint takes one
//! branch on `cursor_trail`; only `"all"` adds work there (cursor_jumps.rs: a
//! compare of the drawn cursor and at most one settle timer per terminal). With
//! both settings off, a window draw checks them; on, it compares the focused
//! pane with the one it saw last and keeps that pane's cursor anchor (a map
//! lookup and a cell read). Frames are asked for only while an effect is alive, and every
//! effect ends: its duration is clamped and its progress only grows.

use super::cursor_shapes;
use super::*;

/// How far the comet's core color is mixed toward white.
const CORE_WHITE: f32 = 0.45;

/// The effect being drawn at the terminal `pane` shows: a trail from `from`
/// when `style` is set, and an `arrival` pulse.
pub(super) struct Trail {
    pub(super) from: Option<cursor_shapes::Quad>,
    pub(super) pane: String,
    pub(super) start: Instant,
    duration: Duration,
    color: Hsla,
    pub(super) style: Option<settings::TrailStyle>,
    pub(super) arrival: settings::Arrival,
    /// The tail's corners as they chase the cursor (`TrailStyle::Tail`).
    chase: Option<cursor_shapes::Chase>,
    /// What the trail drew last, where one started meanwhile begins.
    pub(super) drawn: Option<cursor_shapes::Quad>,
}

impl Trail {
    fn new(from: Option<cursor_shapes::Quad>, pane: &str, s: &settings::Settings) -> Self {
        let style = from.map(|_| s.cursor_trail_style);
        Trail {
            from,
            pane: pane.to_owned(),
            start: Instant::now(),
            duration: s.trail_duration(),
            color: s.trail_color().unwrap_or_else(|| settings::color("cursor")),
            style,
            arrival: s.cursor_arrival,
            chase: from.filter(|_| style == Some(settings::TrailStyle::Tail)).map(cursor_shapes::Chase::new),
            drawn: None,
        }
    }

    /// The trail's shapes and the pulse's at `elapsed`, toward `end`, noting
    /// what the trail drew; None once both are over.
    fn shapes(&mut self, end: crate::terminal::TrailEnd, elapsed: Duration) -> Option<Vec<cursor_shapes::Layer>> {
        let t = elapsed.as_secs_f32() / self.duration.as_secs_f32();
        let h = f32::from(end.cell.size.height);
        let (mut layers, drawn) = match (&mut self.chase, self.style, self.from) {
            (Some(chase), _, _) => {
                let live = chase.step(end.cursor, end.shown, elapsed.as_secs_f32(), self.duration.as_secs_f32());
                if live { (chase.layers(end.cursor), Some(chase.corners)) } else { (Vec::new(), None) }
            }
            (None, Some(style), Some(from)) if t < 1.0 => cursor_shapes::trail(style, from, end.cursor, h, t),
            _ => (Vec::new(), None),
        };
        self.drawn = drawn;
        let pulse = cursor_shapes::arrival(self.arrival, end.cell, t);
        if layers.is_empty() && pulse.is_empty() {
            return None;
        }
        layers.extend(pulse);
        Some(layers)
    }
}

/// The shown tab's focused pane at the last draw, and where its trail would
/// start: kept each draw, so a pane that closes still has one.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Seen {
    pub(super) tab: String,
    pub(super) pane: String,
    anchor: Option<Bounds<Pixels>>,
}

/// Where a trail starts: from what the live one drew, else from `cursor`.
fn start_from(live: Option<&Trail>, cursor: Option<Bounds<Pixels>>) -> Option<cursor_shapes::Quad> {
    live.and_then(|t| t.drawn).or(cursor.map(cursor_shapes::quad))
}

/// How far along an effect is, 0 to 1, after `elapsed` of `duration`; None once it's over.
pub(super) fn progress(elapsed: Duration, duration: Duration) -> Option<f32> {
    (elapsed < duration).then(|| elapsed.as_secs_f32() / duration.as_secs_f32())
}

/// The core's color: the trail's mixed `CORE_WHITE` toward white.
fn paler(c: Hsla) -> Hsla {
    Hsla {
        l: c.l + (1.0 - c.l) * CORE_WHITE,
        ..c
    }
}

impl AppWindow {
    /// Starts an effect when the shown tab's focused pane changed since the
    /// last draw: a trail from where the pane it left last drew its cursor,
    /// and the arrival pulse, as the settings ask; any effect still running
    /// ends there. Both off, it forgets what it saw, so turning one on never
    /// starts from a stale pane.
    pub(super) fn watch_cursor_trail(&mut self, cx: &App) {
        let s = settings::get();
        if s.cursor_trail == settings::CursorTrail::Off
            && s.cursor_arrival == settings::Arrival::Off
        {
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
            .and_then(|term| term.view.read(cx).trail_end())
            .map(|end| end.cursor);
        match self.trail_seen.take() {
            Some(seen) if seen.tab == tab && seen.pane == pane => {
                self.trail_seen = Some(Seen {
                    anchor: anchor.or(seen.anchor),
                    ..seen
                });
                return;
            }
            Some(seen) => {
                let live = self.trail.take();
                let from = start_from(live.as_ref(), seen.anchor)
                    .filter(|_| s.cursor_trail != settings::CursorTrail::Off);
                if seen.tab == tab
                    && self.pane_term(&pane).is_some()
                    && (from.is_some() || s.cursor_arrival != settings::Arrival::Off)
                {
                    self.trail = Some(Trail::new(from, &pane, &s));
                }
            }
            None => {}
        }
        self.trail_seen = Some(Seen { tab, pane, anchor });
    }

    /// The focused pane's cursor settled after a jump (`cursor_trail: "all"`,
    /// cursor_jumps.rs): a trail from `from` (or from what a live trail
    /// drew) and the arrival pulse, or the pulse alone when only its shape
    /// changed (`from` None).
    pub(super) fn cursor_jump(
        &mut self,
        pane: &str,
        from: Option<Bounds<Pixels>>,
        cx: &mut Context<Self>,
    ) {
        let s = settings::get();
        let focused = self.model.active_tab().and_then(|t| t.focused_pane());
        if s.cursor_trail != settings::CursorTrail::All
            || focused.as_deref() != Some(pane)
            || (from.is_none() && s.cursor_arrival == settings::Arrival::Off)
        {
            return;
        }
        let from = from.and_then(|cursor| start_from(self.trail.as_ref(), Some(cursor)));
        self.trail = Some(Trail::new(from, pane, &s));
        self.trail_layer.update(cx, |_, cx| cx.notify());
    }

    /// The live effect's shapes now and their color, asking nothing of the
    /// window; None (and the effect dropped) once it's over or its pane is gone.
    pub(super) fn trail_frame(&mut self, cx: &App) -> Option<(Vec<cursor_shapes::Layer>, Hsla)> {
        let to = self.pane_term(&self.trail.as_ref()?.pane).map(|term| term.view.clone());
        let trail = self.trail.as_mut()?;
        let elapsed = trail.start.elapsed();
        let Some(to) = to else {
            self.trail = None;
            return None;
        };
        let Some(end) = to.read(cx).trail_end() else {
            // Not drawn yet: wait out its time, painting nothing.
            if progress(elapsed, trail.duration).is_none() {
                self.trail = None;
                return None;
            }
            return Some((Vec::new(), trail.color));
        };
        let color = trail.color;
        match trail.shapes(end, elapsed) {
            Some(layers) => Some((layers, color)),
            None => {
                self.trail = None;
                None
            }
        }
    }
}

/// The layer the live effect paints on, a view of its own between the
/// window's two drawings: the frames it asks for while the effect runs
/// notify it alone, so they draw it again and reuse the rest of the window.
pub(super) struct TrailLayer {
    pub(super) app: WeakEntity<AppWindow>,
}

impl Render for TrailLayer {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let app = self.app.clone();
        canvas(
            |_, _, _| {},
            move |_, _, window, cx| {
                let Ok((live, frame)) =
                    app.update(cx, |app, cx| (app.trail.is_some(), app.trail_frame(cx)))
                else {
                    return;
                };
                let Some((layers, color)) = frame else {
                    // Ended: the window draws once more in full, so anything that
                    // changed unnotified while trail frames reused it shows, as
                    // a full draw each frame used to show it.
                    if live {
                        let app = app.clone();
                        window.on_next_frame(move |_, cx| {
                            let _ = app.update(cx, |_, cx| cx.notify());
                        });
                    }
                    return;
                };
                crate::bench::count(&crate::bench::TRAIL);
                window.request_animation_frame();
                for layer in layers {
                    let mut path = match layer.stroke {
                        Some(w) => PathBuilder::stroke(px(w)),
                        None => PathBuilder::fill().with_style(PathStyle::Fill(FillOptions::default().with_fill_rule(FillRule::EvenOdd))),
                    };
                    path.add_polygon(&layer.outline, true);
                    // Filled even-odd: the hole, inside the outline, stays clear.
                    if let Some(hole) = &layer.hole {
                        path.add_polygon(hole, true);
                    }
                    if let Ok(path) = path.build() {
                        let tint = if layer.core { paler(color) } else { color };
                        window.paint_path(path, tint.opacity(layer.alpha));
                    }
                }
            },
        )
        .absolute()
        .size_full()
    }
}

#[cfg(test)]
mod tests {
    use super::{paler, progress};
    use std::time::Duration;

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
    fn the_core_is_the_cursor_color_paler() {
        let yellow = gpui::hsla(0.11, 1.0, 0.7, 1.0);
        let core = paler(yellow);
        assert_eq!((core.h, core.s, core.a), (yellow.h, yellow.s, yellow.a));
        assert!((core.l - (0.7 + 0.3 * 0.45)).abs() < 1e-6);
    }
}
