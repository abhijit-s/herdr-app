//! Cursor effects (`cursor_trail` and `cursor_arrival` in settings.json): as
//! the keyboard focus moves from one caret of the window to another (a pane of
//! the shown tab, or a text input: the palette, find, a rename), a trail from
//! the old caret into the new one, in one of `cursor_trail_style`'s looks, and
//! a pulse at the new caret, each over `cursor_trail_duration_ms`. With
//! `"all"`, also as the focused caret jumps (cursor_jumps.rs for terminals,
//! `watch_input_caret` for inputs). They are drawn above everything else in
//! the window, dialogs too, so a trail into a dialog's input shows; their
//! shapes are window/cursor_shapes.rs's, after sahaj-b/ghostty-cursor-shaders
//! (MIT). Every color is the theme's cursor color (or `cursor_trail_color`),
//! the comet's core paler; the blaze has its own flame unless
//! `cursor_trail_color` is set.
//!
//! A pane's end is the terminal's `trail_end`: its cursor as drawn (a block's
//! cell, a beam's or an underline's strip), else its cursor cell when hidden,
//! else its middle cell. A pane that isn't a painted terminal (never drawn)
//! gets no effect. An input's end is its caret where it last painted
//! (gpui-kit tells `input_caret_painted`, a `HERDR:` hook in its input
//! element), shown or mid-blink; a trail toward an input that loses the
//! focus or stops painting ends. Both ends are where the layout puts the cursor, never where
//! a pane slide (pane_slide.rs) draws it mid-slide, so a sliding pane doesn't
//! drag the trail along. The new end follows the pane's cursor live while the
//! effect runs; when the layout moves its pane again (another split, a resize,
//! a zoom) the trail re-aims from what it drew. A trail that starts while
//! another is drawn starts from what that one
//! drew (its tail's corners, a comet's tail point, a block's old end), as
//! kitty's corners keep chasing a cursor that moves on.
//!
//! While the system asks for reduced motion (macOS: Accessibility > Display >
//! Reduce motion, followed as it changes; GPUI's `App::reduce_motion`), no
//! effect starts and one running ends.
//!
//! Nothing runs in the terminals' output paths, and their paint takes one
//! branch on `cursor_trail`; only `"all"` adds work there (cursor_jumps.rs: a
//! compare of the drawn cursor and at most one settle timer per terminal). With
//! both settings off, a window draw checks them; on, it compares the focused
//! caret with the one it saw last and keeps that caret's anchor (a map lookup
//! and a cell read, or the input's last caret). A focused input's paint calls
//! the hook, which with both off reads one setting and returns, else compares
//! the caret with the last it painted. Frames are asked for only while an
//! effect is alive, and every effect ends: its duration is clamped and its
//! progress only grows.

use super::cursor_particles;
use super::cursor_shapes;
use super::trail_paint::{self, paint_layers};
use super::*;

/// A caret a trail runs from or to: the cursor of the terminal a pane shows,
/// or the caret of the text input holding the window's keyboard focus.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Caret {
    Pane(String),
    Input(WeakFocusHandle),
}

#[cfg(test)]
impl Caret {
    /// The pane, or `"input"` for a text input's caret.
    pub(super) fn pane(&self) -> String {
        match self {
            Caret::Pane(pane) => pane.clone(),
            Caret::Input(_) => "input".into(),
        }
    }
}

/// The focused text input's caret where it last painted, in window
/// coordinates, with the terminals' cell size, which measures its jumps
/// across and sizes its arrival pulse.
pub(super) struct InputCaret {
    pub(super) focus: WeakFocusHandle,
    pub(super) caret: Bounds<Pixels>,
    pub(super) cell: Size<Pixels>,
}

impl InputCaret {
    /// Where a trail meets it: the caret (a beam) at the left of a cell, as a
    /// terminal's beam cursor is.
    fn end(&self) -> crate::terminal::TrailEnd {
        crate::terminal::TrailEnd {
            cell: Bounds::new(self.caret.origin, size(self.cell.width, self.caret.size.height)),
            cursor: self.caret,
            shown: true,
        }
    }
}

/// Whether an input's caret moved from `was` to `now` past `min` (kitty's
/// threshold, as for terminals): across in the terminals' cell widths, since
/// an input's text has no cells, and down in the caret's own height, its
/// line. Whole cells, rounded, so typing (a character's width) never passes 1.
pub(super) fn input_jump(was: Bounds<Pixels>, now: Bounds<Pixels>, cell: Size<Pixels>, min: settings::MinCells) -> bool {
    let cells = |d: Pixels, unit: Pixels| (f32::from(d).abs() / f32::from(unit).max(1.0)).round();
    cells(now.origin.x - was.origin.x, cell.width) > f32::from(min.x)
        || cells(now.origin.y - was.origin.y, now.size.height) > f32::from(min.y)
}

/// gpui-kit's hook: a focused text input painted its caret at `caret`. In
/// the main window, the window watches it (`watch_input_caret`); a trail it
/// starts draws from the next frame.
pub(crate) fn input_caret_painted(focus: &FocusHandle, caret: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    let s = settings::get();
    if s.cursor_trail == settings::CursorTrail::Off && s.pulse() == settings::Arrival::Off {
        return;
    }
    let Some(Some(app)) = window.root::<AppWindow>() else {
        return;
    };
    let cell = crate::terminal::cell_size(window);
    let started = app.update(cx, |app, cx| {
        let reduced = cx.reduce_motion();
        app.watch_input_caret(InputCaret { focus: focus.downgrade(), caret, cell }, reduced)
            .then(|| app.trail_layer.clone())
    });
    // Painting now, so the trail's layer is notified for the next frame.
    if let Some(layer) = started {
        window.on_next_frame(move |_, cx| layer.update(cx, |_, cx| cx.notify()));
    }
}

/// The effect being drawn toward the caret `to`: a trail from `from` when
/// `style` is set, and an `arrival` pulse.
pub(super) struct Trail {
    pub(super) from: Option<cursor_shapes::Quad>,
    pub(super) to: Caret,
    pub(super) start: Instant,
    duration: Duration,
    color: trail_paint::Tint,
    pub(super) style: Option<settings::TrailStyle>,
    pub(super) arrival: settings::Arrival,
    /// The pulse's `cursor_arrival_size` and `cursor_arrival_intensity`.
    arrival_scale: (f32, f32),
    /// When the pulse started: the trail's start, or a later shape change.
    pulse: Instant,
    /// The tail's corners as they chase the cursor (`TrailStyle::Tail`),
    /// None again once they have caught up.
    chase: Option<cursor_shapes::Chase>,
    /// What the trail drew last, where one started meanwhile begins.
    pub(super) drawn: Option<cursor_shapes::Quad>,
    /// The tail's motion blur (`cursor_trail_motion_blur`).
    blur: bool,
    /// Pixie dust still to shed along it (`cursor_trail_particles`), at its
    /// first frame, once the new cursor is known.
    dust: bool,
    /// Where the layout put its pane at its last frame: a new place (another
    /// split, a resize, a zoom) re-aims it.
    slot: Option<Bounds<Pixels>>,
}

impl Trail {
    /// The colors it is drawn in.
    pub(super) fn color(&self) -> trail_paint::Tint {
        self.color
    }

    /// Its pixie dust, once, from where it starts to the cursor `to` (whose
    /// cell is `cell`), if the settings asked for it.
    pub(super) fn take_dust(
        &mut self,
        to: Bounds<Pixels>,
        cell: Bounds<Pixels>,
    ) -> Option<cursor_particles::Burst> {
        let from = self.from.filter(|_| std::mem::take(&mut self.dust))?;
        let size = f32::from(cell.size.width).max(f32::from(cell.size.height));
        let mid = |b: Bounds<Pixels>| (f32::from(b.center().x), f32::from(b.center().y));
        Some(cursor_particles::Burst { from: cursor_shapes::centroid(&from), to: mid(to), size })
    }

    /// The same effect, started `by` earlier: trail and pulse alike.
    #[cfg(test)]
    pub(super) fn age(&mut self, by: Duration) {
        (self.start, self.pulse) = (self.start - by, self.pulse - by);
    }

    /// The same effect, held at its first frame for `by`: a test's draws take
    /// real time, and its effects run on the real clock.
    #[cfg(test)]
    pub(super) fn hold(&mut self, by: Duration) {
        (self.start, self.pulse) = (Instant::now() + by, Instant::now() + by);
    }

    /// A trail from `from` (None: the pulse alone); a tail steered from a
    /// tail (`live`) keeps its fade, as kitty's one opacity does.
    pub(super) fn new(
        from: Option<cursor_shapes::Quad>,
        live: Option<&Trail>,
        to: Caret,
        s: &settings::Settings,
    ) -> Self {
        let style = from.map(|_| s.cursor_trail_style);
        let opacity = live
            .and_then(|t| t.chase.as_ref())
            .map_or(1.0, |c| c.opacity);
        let now = Instant::now();
        Trail {
            from,
            to,
            start: now,
            duration: s.trail_duration(),
            color: trail_paint::Tint::new(
                s.trail_color().unwrap_or_else(|| settings::color("cursor")),
                s.trail_color().is_some(),
            ),
            style,
            arrival: s.pulse(),
            arrival_scale: (s.cursor_arrival_size, s.cursor_arrival_intensity),
            pulse: now,
            chase: from
                .filter(|_| style == Some(settings::TrailStyle::Tail))
                .map(|from| cursor_shapes::Chase::new(from).faded(opacity)),
            drawn: None,
            blur: s.cursor_trail_motion_blur,
            dust: from.is_some() && s.cursor_trail_particles == settings::Particles::Pixiedust,
            slot: None,
        }
    }

    /// Its pane laid out at `slot` this frame: moved since its last frame by
    /// more than a pixel (landing may settle one device pixel), the trail
    /// starts again from what it drew toward the pane's new place, its pulse
    /// running on and its dust already shed. A slide never moves it: the
    /// layout has its pane where it lands from the slide's first frame. A
    /// tail needs none: its corners chase the cursor wherever it goes.
    fn follow_layout(&mut self, slot: Option<Bounds<Pixels>>, s: &settings::Settings) {
        let moved = |a: Bounds<Pixels>, b: Bounds<Pixels>| {
            [a.left() - b.left(), a.top() - b.top(), a.right() - b.right(), a.bottom() - b.bottom()]
                .iter()
                .any(|d| d.abs() > px(1.0))
        };
        if let (Some(was), Some(now), Some(drawn), None) = (self.slot, slot, self.drawn, &self.chase)
            && moved(was, now)
        {
            let (arrival, pulse) = (self.arrival, self.pulse);
            *self = Trail { dust: false, arrival, pulse, ..Trail::new(Some(drawn), Some(self), self.to.clone(), s) };
        }
        self.slot = slot;
    }

    /// The trail's shapes and the pulse's at `elapsed`, toward `end`, noting
    /// what the trail drew; None once both are over.
    pub(super) fn shapes(
        &mut self,
        end: crate::terminal::TrailEnd,
        elapsed: Duration,
    ) -> Option<Vec<cursor_shapes::Layer>> {
        let t = elapsed.as_secs_f32() / self.duration.as_secs_f32();
        let h = f32::from(end.cell.size.height);
        let tail = self.style == Some(settings::TrailStyle::Tail);
        let (mut layers, drawn, chasing) = match (&mut self.chase, self.style, self.from) {
            (Some(chase), _, _) => {
                let live = chase.step(
                    end.cursor,
                    end.shown,
                    elapsed.as_secs_f32(),
                    self.duration.as_secs_f32(),
                );
                if live {
                    let layers = if self.blur {
                        chase.blurred(end.cursor)
                    } else {
                        chase.layers(end.cursor)
                    };
                    (layers, Some(chase.corners), true)
                } else {
                    (Vec::new(), None, false)
                }
            }
            (None, Some(style), Some(from)) if t < 1.0 && !tail => {
                let (layers, drawn) = cursor_shapes::trail(style, from, end.cursor, h, t);
                (layers, drawn, false)
            }
            _ => (Vec::new(), None, false),
        };
        // Caught up, a tail stays ended, however the cursor moves on.
        if !chasing {
            self.chase = None;
        }
        self.drawn = drawn;
        let pulse_t = (self.start + elapsed).saturating_duration_since(self.pulse).as_secs_f32() / self.duration.as_secs_f32();
        let pulse = cursor_shapes::arrival(self.arrival, end.cell, pulse_t, self.arrival_scale.0, self.arrival_scale.1);
        if layers.is_empty() && pulse.is_empty() && !chasing {
            return None;
        }
        layers.extend(pulse);
        Some(layers)
    }
}

/// The shown tab and the caret focused at the last draw (or input paint), and
/// where its trail would start: kept each time, so a pane that closes or an
/// input that goes still has one.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Seen {
    pub(super) tab: String,
    pub(super) caret: Caret,
    anchor: Option<Bounds<Pixels>>,
}

/// Where a trail starts: from what the live one drew, else from `cursor`.
pub(super) fn start_from(live: Option<&Trail>, cursor: Option<Bounds<Pixels>>) -> Option<cursor_shapes::Quad> {
    live.and_then(|t| t.drawn)
        .or(cursor.map(cursor_shapes::quad))
}

/// How far along an effect is, 0 to 1, after `elapsed` of `duration`; None once it's over.
pub(super) fn progress(elapsed: Duration, duration: Duration) -> Option<f32> {
    (elapsed < duration).then(|| elapsed.as_secs_f32() / duration.as_secs_f32())
}

impl AppWindow {
    /// Starts an effect when the focus moved to the shown tab's focused pane
    /// since the last draw, from another pane of the tab or from a text input:
    /// a trail from where the caret it left was last drawn, and the arrival
    /// pulse, as the settings ask; any effect still running ends there. While
    /// a painted input has the focus, its paint (`watch_input_caret`) does
    /// this instead; focus that leaves an input for something with no caret
    /// of its own (a list, a button, an input not painted yet) leaves what it
    /// saw as it was. Both off, it forgets what it saw, so turning one on
    /// never starts from a stale caret.
    pub(super) fn watch_cursor_trail(&mut self, window: &Window, cx: &App) {
        let s = settings::get();
        if s.cursor_trail == settings::CursorTrail::Off
            && s.pulse() == settings::Arrival::Off
        {
            (self.trail_seen, self.trail, self.input_caret) = (None, None, None);
            self.dust = cursor_particles::Dust::default();
            return;
        }
        let Some((tab, pane)) = self
            .model
            .active_tab()
            .and_then(|t| Some((t.id.clone(), t.focused_pane()?)))
        else {
            (self.trail_seen, self.trail, self.input_caret) = (None, None, None);
            self.dust = cursor_particles::Dust::default();
            return;
        };
        let focus = window.focused(cx);
        let on = |handle: &WeakFocusHandle| focus.as_ref().is_some_and(|f| f == handle);
        if self.input_caret.as_ref().is_some_and(|input| on(&input.focus)) {
            return;
        }
        let in_input = self.trail_seen.as_ref().is_some_and(|seen| matches!(seen.caret, Caret::Input(_)));
        let to_pane = focus.as_ref().is_none_or(|f| *f == self.focus || self.terms.values().any(|t| t.focus == *f));
        if in_input && !to_pane {
            return;
        }
        self.input_caret = None;
        let anchor = self
            .pane_term(&pane)
            .and_then(|term| term.view.read(cx).trail_end())
            .map(|end| end.cursor);
        let caret = Caret::Pane(pane.clone());
        match self.trail_seen.take() {
            Some(seen) if seen.tab == tab && seen.caret == caret => {
                self.trail_seen = Some(Seen {
                    anchor: anchor.or(seen.anchor),
                    ..seen
                });
                return;
            }
            Some(seen) => {
                // Dust is in the window's px: a tab change leaves it behind.
                if seen.tab != tab {
                    self.dust = cursor_particles::Dust::default();
                }
                let live = self.trail.take();
                let from = start_from(live.as_ref(), seen.anchor)
                    .filter(|_| s.cursor_trail != settings::CursorTrail::Off);
                // An input (the palette) may have switched the tab it returns to.
                let from_input = matches!(seen.caret, Caret::Input(_));
                if (seen.tab == tab || from_input)
                    && !cx.reduce_motion()
                    && self.pane_term(&pane).is_some()
                    && (from.is_some() || s.pulse() != settings::Arrival::Off)
                {
                    self.trail = Some(Trail::new(from, live.as_ref(), caret.clone(), &s));
                }
            }
            None => {}
        }
        self.trail_seen = Some(Seen { tab, caret, anchor });
    }

    /// A focused text input painted its caret at `now`: when the focus moved
    /// to it from another caret, a trail from where that one was last drawn
    /// and the pulse; with `"all"`, a jump of its own caret past
    /// `cursor_trail_min_cells` (`input_jump`) draws the same. The caret is
    /// drawn where it settles, so there is no settle wait: a click elsewhere
    /// in the text, Home or End, a word jump or a cleared query draws at
    /// once, and typing, a character at a time, never does. True when an
    /// effect started.
    pub(super) fn watch_input_caret(&mut self, now: InputCaret, reduced: bool) -> bool {
        if self.input_caret.as_ref().is_some_and(|was| was.focus == now.focus && was.caret == now.caret) {
            return false;
        }
        let s = settings::get();
        let (caret, anchor, cell) = (Caret::Input(now.focus.clone()), now.caret, now.cell);
        self.input_caret = Some(now);
        let seen = self.trail_seen.take();
        let tab = seen.as_ref().map(|seen| seen.tab.clone())
            .or_else(|| self.model.active_tab().map(|t| t.id.clone()))
            .unwrap_or_default();
        let moved = seen.as_ref().is_some_and(|seen| match seen.anchor {
            _ if seen.caret != caret => true,
            Some(was) => s.cursor_trail == settings::CursorTrail::All && input_jump(was, anchor, cell, s.trail_min_cells()),
            None => false,
        });
        let mut started = false;
        if moved && !reduced {
            let from = start_from(self.trail.as_ref(), seen.as_ref().and_then(|seen| seen.anchor))
                .filter(|_| s.cursor_trail != settings::CursorTrail::Off);
            if from.is_some() || s.pulse() != settings::Arrival::Off {
                self.trail = Some(Trail::new(from, self.trail.as_ref(), caret.clone(), &s));
                started = true;
            }
        }
        self.trail_seen = Some(Seen { tab, caret, anchor: Some(anchor) });
        started
    }

    /// The focused pane's cursor settled after a jump (`cursor_trail: "all"`,
    /// cursor_jumps.rs): a trail from `from` (or from what a live trail
    /// drew) and the arrival pulse, or the pulse alone when only its shape
    /// changed (`from` None). Not while a text input (its find field) holds
    /// the focus: then the input's caret is the one that jumps.
    pub(super) fn cursor_jump(
        &mut self,
        pane: &str,
        from: Option<Bounds<Pixels>>,
        cx: &mut Context<Self>,
    ) {
        let s = settings::get();
        let focused = self.model.active_tab().and_then(|t| t.focused_pane());
        let caret = Caret::Pane(pane.to_owned());
        if s.cursor_trail != settings::CursorTrail::All
            || cx.reduce_motion()
            || focused.as_deref() != Some(pane)
            || self.trail_seen.as_ref().is_some_and(|seen| seen.caret != caret)
            || (from.is_none() && s.pulse() == settings::Arrival::Off)
        {
            return;
        }
        let reshaped = from.is_none() && self.trail.as_ref().is_some_and(|t| t.to == caret);
        if let Some(live) = self.trail.as_mut().filter(|_| reshaped) {
            // A shape change while a trail runs: it runs on, the pulse anew.
            (live.arrival, live.pulse) = (s.pulse(), Instant::now());
        } else {
            let start = from.and_then(|cursor| start_from(self.trail.as_ref(), Some(cursor)));
            self.trail = Some(Trail::new(start, self.trail.as_ref(), caret, &s));
        }
        self.trail_layer.update(cx, |_, cx| cx.notify());
    }

    /// The live effects' shapes now, the pixie dust under the trail, and
    /// their color, asking nothing of the window; None once both are over
    /// (the trail dropped once it's over or its pane is gone, the dust once
    /// its last particle has faded).
    pub(super) fn trail_frame(&mut self, cx: &App) -> Option<(Vec<cursor_shapes::Layer>, trail_paint::Tint)> {
        if cx.reduce_motion() {
            (self.trail, self.dust) = (None, cursor_particles::Dust::default());
            return None;
        }
        let trail = self.trail_alone(cx);
        if self.dust.is_empty() {
            return trail;
        }
        // Kept off the focused caret as it is now, which may have moved on
        // since the trail ended.
        let focused = match self.trail_seen.as_ref().map(|seen| &seen.caret) {
            Some(Caret::Input(focus)) => Some(Caret::Input(focus.clone())),
            _ => self.model.active_tab().and_then(|t| t.focused_pane()).map(Caret::Pane),
        };
        let cursor = focused
            .and_then(|caret| self.caret_end(&caret, cx))
            .map(|end| end.cursor);
        let dust = self.dust.layers(Instant::now(), cursor);
        match trail {
            Some((layers, tint)) => Some((dust.into_iter().chain(layers).collect(), tint)),
            None => (!self.dust.is_empty()).then_some((dust, self.dust_tint)),
        }
    }

    /// The trail's own shapes now (`trail_frame`), shedding its pixie dust at
    /// its first frame.
    fn trail_alone(&mut self, cx: &App) -> Option<(Vec<cursor_shapes::Layer>, trail_paint::Tint)> {
        let to = self.trail.as_ref()?.to.clone();
        let there = match &to {
            Caret::Pane(pane) => self.pane_term(pane).is_some(),
            Caret::Input(focus) => self.input_caret.as_ref().is_some_and(|input| input.focus == *focus),
        };
        if !there {
            self.trail = None;
            return None;
        }
        let slot = match &to {
            Caret::Pane(pane) => self.bounds.borrow().get(pane).copied(),
            Caret::Input(_) => None,
        };
        let end = self.caret_end(&to, cx);
        let trail = self.trail.as_mut()?;
        if end.is_some() {
            trail.follow_layout(slot, &settings::get());
        }
        let elapsed = trail.start.elapsed();
        let Some(end) = end else {
            // Not drawn yet: wait out its time, painting nothing.
            if progress(elapsed, trail.duration).is_none() {
                self.trail = None;
                return None;
            }
            return Some((Vec::new(), trail.color));
        };
        let color = trail.color;
        let dust = trail.take_dust(end.cursor, end.cell);
        let shapes = trail.shapes(end, elapsed);
        self.dust_tint = color;
        // From the frame it's shed: a pane drawn late doesn't lose its dust's start.
        if let Some(burst) = dust {
            self.dust.add(burst, Instant::now());
        }
        if shapes.is_none() {
            self.trail = None;
        }
        shapes.map(|layers| (layers, color))
    }

    /// Where a trail meets `caret` now: its terminal's `trail_end`, or the
    /// input's caret while it is the one last painted; None before either is
    /// drawn.
    fn caret_end(&self, caret: &Caret, cx: &App) -> Option<crate::terminal::TrailEnd> {
        match caret {
            Caret::Pane(pane) => self.pane_term(pane)?.view.read(cx).trail_end(),
            Caret::Input(focus) => self.input_caret.as_ref().filter(|input| input.focus == *focus).map(InputCaret::end),
        }
    }
}

/// The layer the live effect paints on, a view of its own over the window's
/// two drawings: the frames it asks for while the effect runs notify it
/// alone, so they draw it again and reuse the rest of the window.
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
                    app.update(cx, |app, cx| (app.trail.is_some() || !app.dust.is_empty(), app.trail_frame(cx)))
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
                paint_layers(&layers, &color, window);
            },
        )
        .absolute()
        .size_full()
    }
}

#[cfg(test)]
mod tests {
    use super::super::trail_paint::paler;
    use super::progress;
    use std::time::Duration;

    /// A tail that has caught up stays ended while its pulse runs on, though
    /// the cursor moves on (typing must never draw one).
    #[test]
    fn a_caught_up_tail_stays_ended_while_the_pulse_runs() {
        use crate::settings::{Arrival, Settings, TrailStyle};
        use gpui::{Bounds, point, px, size};
        let cell = |x: f32| Bounds::new(point(px(x), px(0.0)), size(px(8.0), px(16.0)));
        let end = |x: f32| crate::terminal::TrailEnd {
            cell: cell(x),
            cursor: cell(x),
            shown: true,
        };
        let s = Settings {
            cursor_trail_style: TrailStyle::Tail,
            cursor_arrival: Arrival::Ripple,
            cursor_trail_duration_ms: 400,
            ..Settings::default()
        };
        let mut trail =
            super::Trail::new(Some(super::cursor_shapes::quad(cell(0.0))), None, super::Caret::Pane("a".into()), &s);
        let filled = |layers: &[super::cursor_shapes::Layer]| {
            layers.iter().filter(|l| l.stroke.is_none()).count()
        };
        let mut ms = 0;
        while trail.chase.is_some() {
            let layers = trail
                .shapes(end(16.0), std::time::Duration::from_millis(ms))
                .expect("the tail or the pulse runs");
            assert!(
                ms > 0 || filled(&layers) == 1,
                "the tail draws as it starts"
            );
            ms += 16;
            assert!(ms < 400, "a short tail ends before the pulse does");
        }
        let later = trail
            .shapes(end(400.0), std::time::Duration::from_millis(ms))
            .expect("the pulse runs on");
        assert_eq!(
            filled(&later),
            0,
            "no tail after the cursor moved on: {later:?}"
        );
        assert!(
            trail.drawn.is_none(),
            "and nothing for a next trail to steer from"
        );
    }

    /// With `cursor_trail_motion_blur`, a moving tail draws its blur under
    /// it; without, the tail alone.
    #[test]
    fn a_tail_draws_its_motion_blur_when_asked() {
        use crate::settings::{Arrival, Settings, TrailStyle};
        use gpui::{Bounds, point, px, size};
        let cell = |x: f32| Bounds::new(point(px(x), px(0.0)), size(px(8.0), px(16.0)));
        let end = crate::terminal::TrailEnd { cell: cell(400.0), cursor: cell(400.0), shown: true };
        let filled = |blur: bool| {
            let s = Settings {
                cursor_trail_style: TrailStyle::Tail,
                cursor_arrival: Arrival::Off,
                cursor_trail_duration_ms: 400,
                cursor_trail_motion_blur: blur,
                ..Settings::default()
            };
            let mut trail = super::Trail::new(Some(super::cursor_shapes::quad(cell(0.0))), None, super::Caret::Pane("a".into()), &s);
            trail.shapes(end, Duration::ZERO);
            trail.shapes(end, Duration::from_millis(33)).unwrap().len()
        };
        assert_eq!((filled(false), filled(true)), (1, 2));
    }

    /// An input's caret jumps past the threshold in terminal cell widths
    /// across and its own height down, rounded to whole cells: a character
    /// (typing) never passes 1, Home along a word does, a line down passes 0.
    #[test]
    fn an_input_caret_jumps_in_whole_cells_across_and_lines_down() {
        use crate::settings::MinCells;
        use gpui::{Bounds, point, px, size};
        let caret = |x: f32, y: f32| Bounds::new(point(px(x), px(y)), size(px(2.0), px(18.0)));
        let cell = size(px(8.0), px(16.0));
        let jump = |from, to, min: MinCells| super::input_jump(from, to, cell, min);
        assert!(!jump(caret(100.0, 0.0), caret(109.0, 0.0), 2.into()), "a wide character is one cell");
        assert!(!jump(caret(100.0, 0.0), caret(119.0, 0.0), 2.into()), "two, rounded, is within 2");
        assert!(jump(caret(100.0, 0.0), caret(60.0, 0.0), 2.into()), "five back is a jump");
        assert!(!jump(caret(100.0, 0.0), caret(100.0, 18.0), 2.into()), "a line down is within 2 rows");
        assert!(jump(caret(100.0, 0.0), caret(100.0, 18.0), MinCells { x: 8, y: 0 }), "and past 0");
        assert!(jump(caret(100.0, 0.0), caret(107.0, 0.0), 0.into()), "0 draws every move of a character");
        assert!(!jump(caret(100.0, 0.0), caret(100.0, 0.0), 0.into()), "but not none");
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
    fn the_core_is_the_cursor_color_paler() {
        let yellow = gpui::hsla(0.11, 1.0, 0.7, 1.0);
        let core = paler(yellow);
        assert_eq!((core.h, core.s, core.a), (yellow.h, yellow.s, yellow.a));
        assert!((core.l - (0.7 + 0.3 * 0.25)).abs() < 1e-6);
    }
}
