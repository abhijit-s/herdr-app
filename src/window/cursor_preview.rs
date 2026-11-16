//! The Settings window's cursor effect preview (`CursorPreview` from the
//! "herdr" mod module): a small stand-in terminal, a grid of cells with a few
//! faint lines of placeholder output and a block cursor. A click moves the
//! cursor to the clicked cell and plays the effect the settings ask for at
//! that moment, drawn by the panes' own `Trail` and `paint_layers`, so it is
//! the effect a pane draws, not a copy of it. It asks for frames only while an
//! effect runs. Each such frame also builds the Settings page's elements
//! again from the script's last description (GPUI draws a notified view's
//! ancestors); the script itself doesn't run. The main window's trail layer
//! avoids even that with its cached views, which a mod's window doesn't have.

use super::cursor_shapes;
use super::cursor_trail::{Trail, start_from};
use super::trail_paint::paint_layers;
use super::*;

/// The stand-in terminal's size in cells, and its placeholder lines: each a
/// (first column, length in cells) on its row.
const COLS: usize = 44;
const ROWS: usize = 7;
const LINES: [(usize, usize); 5] = [(0, 18), (2, 31), (2, 24), (0, 12), (0, 2)];

/// The preview's view: where the cursor is, and the effect running.
pub(crate) struct CursorPreview {
    cursor: (usize, usize),
    trail: Option<Trail>,
    /// The grid's origin, cell size and columns at the last paint, which a
    /// click reads to find its cell.
    grid: Option<(Point<Pixels>, Size<Pixels>, usize)>,
    /// The effect frames it has painted.
    frames: u64,
}

/// What keeps the preview's view across renders. Element state is watched by
/// the view it's rendered in, which a notify of it would run again; holding
/// the preview rather than being it, a frame notifies only the preview (whose
/// ancestors GPUI then draws again, without their scripts running).
struct Slot(Entity<CursorPreview>);

/// The preview as a mod element, kept across renders under `id`.
pub(crate) fn element(id: &str, window: &mut Window, cx: &mut App) -> AnyElement {
    AnyView::from(view(id, window, cx)).into_any_element()
}

/// The preview's view under `id`, made at its first render.
fn view(id: &str, window: &mut Window, cx: &mut App) -> Entity<CursorPreview> {
    let slot = window.use_keyed_state(SharedString::from(format!("cursor-preview-{id}")), cx, |_, cx| {
        Slot(cx.new(|_| CursorPreview::new()))
    });
    slot.read(cx).0.clone()
}

impl CursorPreview {
    fn new() -> Self {
        CursorPreview {
            cursor: (2, LINES.len() - 1),
            trail: None,
            grid: None,
            frames: 0,
        }
    }

    /// The cell at window point `at`, held within the grid.
    fn cell_at(&self, at: Point<Pixels>) -> Option<(usize, usize)> {
        let (origin, cell, cols) = self.grid?;
        let col = f32::from(at.x - origin.x) / f32::from(cell.width);
        let row = f32::from(at.y - origin.y) / f32::from(cell.height);
        Some((
            (col.max(0.0) as usize).min(cols - 1),
            (row.max(0.0) as usize).min(ROWS - 1),
        ))
    }

    /// The rect of the cell at `(col, row)` in the last paint's grid.
    fn rect(&self, (col, row): (usize, usize)) -> Option<Bounds<Pixels>> {
        let (origin, cell, cols) = self.grid?;
        let col = col.min(cols - 1);
        Some(Bounds::new(
            point(
                origin.x + cell.width * col as f32,
                origin.y + cell.height * row as f32,
            ),
            cell,
        ))
    }

    /// Moves the cursor to `to` and starts the effect `s` asks for, as a
    /// pane does: a trail from the cursor (or from what a running one drew)
    /// unless `cursor_trail` is off, and the arrival pulse; none while the
    /// system asks for reduced motion. Whether an effect runs.
    fn moved(&mut self, to: (usize, usize), s: &settings::Settings, reduce_motion: bool) -> bool {
        let from = self.rect(self.cursor);
        let cols = self.grid.map_or(COLS, |(_, _, cols)| cols);
        self.cursor = (to.0.min(cols - 1), to.1.min(ROWS - 1));
        let live = self.trail.take();
        let trail = start_from(live.as_ref(), from)
            .filter(|_| s.cursor_trail != settings::CursorTrail::Off);
        if !reduce_motion && (trail.is_some() || s.cursor_arrival != settings::Arrival::Off) {
            self.trail = Some(Trail::new(trail, live.as_ref(), "preview", s));
        }
        self.trail.is_some()
    }

    /// The effect's layers now and its color; None (and the effect dropped)
    /// once it's over.
    fn frame(&mut self) -> Option<(Vec<cursor_shapes::Layer>, Hsla)> {
        let at = self.rect(self.cursor)?;
        let trail = self.trail.as_mut()?;
        let end = crate::terminal::TrailEnd {
            cell: at,
            cursor: at,
            shown: true,
        };
        let elapsed = trail.start.elapsed();
        let frame = trail.shapes(end, elapsed).map(|l| (l, trail.color()));
        if frame.is_none() {
            self.trail = None;
        }
        frame
    }
}

impl Render for CursorPreview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cell = crate::terminal::cell_size(window);
        let view = cx.entity().downgrade();
        div()
            .id("cursor-preview")
            .w_full()
            .h(cell.height * (ROWS + 2) as f32)
            .rounded(px(6.0))
            .border_1()
            .border_color(settings::color("border"))
            .bg(settings::color("background"))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &MouseDownEvent, _, cx| {
                    let Some(to) = this.cell_at(e.position) else { return };
                    this.moved(to, &settings::get(), cx.reduce_motion());
                    cx.notify();
                }),
            )
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        let (origin, cell, cols) = fit(bounds, crate::terminal::cell_size(window));
                        let reduce_motion = cx.reduce_motion();
                        let _ = view.update(cx, |this, _| {
                            this.grid = Some((origin, cell, cols));
                            // As in panes: reduced motion ends one running.
                            if reduce_motion {
                                this.trail = None;
                            }
                            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                                paint_grid(this, window);
                                if let Some((layers, color)) = this.frame() {
                                    this.frames += 1;
                                    window.request_animation_frame();
                                    paint_layers(&layers, color, window);
                                }
                            });
                        });
                    },
                )
                .size_full(),
            )
    }
}

/// The grid in the box `bounds` for cells of `cell`: as many columns as fit,
/// up to `COLS`, centred, and its origin, cell and columns.
fn fit(bounds: Bounds<Pixels>, cell: Size<Pixels>) -> (Point<Pixels>, Size<Pixels>, usize) {
    let cols = ((bounds.size.width / cell.width).floor() as usize).clamp(1, COLS);
    let grid = size(cell.width * cols as f32, cell.height * ROWS as f32);
    let origin = point(
        bounds.origin.x + ((bounds.size.width - grid.width) / 2.0).max(px(0.0)),
        bounds.origin.y + ((bounds.size.height - grid.height) / 2.0).max(px(0.0)),
    );
    (origin, cell, cols)
}

/// The stand-in terminal: its placeholder lines in the terminals' ink, faint,
/// and the cursor block.
fn paint_grid(preview: &CursorPreview, window: &mut Window) {
    let Some((origin, cell, cols)) = preview.grid else { return };
    let text = settings::color("terminal_foreground").opacity(0.18);
    for (row, &(col, len)) in LINES.iter().enumerate() {
        let len = len.min(cols.saturating_sub(col));
        if len == 0 {
            continue;
        }
        let bar = Bounds::new(
            point(
                origin.x + cell.width * col as f32,
                origin.y + cell.height * (row as f32 + 0.3),
            ),
            size(cell.width * len as f32, cell.height * 0.4),
        );
        window.paint_quad(fill(bar, text).corner_radii(cell.height * 0.2));
    }
    if let Some(at) = preview.rect(preview.cursor) {
        window.paint_quad(fill(at, settings::color("cursor")));
    }
}

#[cfg(test)]
mod tests {
    use super::{COLS, CursorPreview, ROWS, cursor_shapes};
    use crate::settings;
    use gpui::{point, px, size};

    fn preview() -> CursorPreview {
        CursorPreview {
            grid: Some((point(px(10.0), px(20.0)), size(px(8.0), px(16.0)), COLS)),
            ..CursorPreview::new()
        }
    }

    fn on(style: settings::TrailStyle, arrival: settings::Arrival) -> settings::Settings {
        settings::Settings {
            cursor_trail: settings::CursorTrail::Panes,
            cursor_trail_style: style,
            cursor_arrival: arrival,
            cursor_trail_duration_ms: 200,
            ..settings::Settings::default()
        }
    }

    #[test]
    fn a_click_finds_its_cell_held_within_the_grid() {
        let p = preview();
        assert_eq!(p.cell_at(point(px(10.0 + 8.0 * 5.5), px(20.0 + 16.0 * 2.2))), Some((5, 2)));
        assert_eq!(p.cell_at(point(px(0.0), px(0.0))), Some((0, 0)));
        assert_eq!(p.cell_at(point(px(9999.0), px(9999.0))), Some((COLS - 1, ROWS - 1)));
        assert_eq!(CursorPreview::new().cell_at(point(px(0.0), px(0.0))), None, "not painted yet");
    }

    /// It plays what the settings say at the click, with the panes' own
    /// shapes: the same layers `cursor_shapes::trail` draws from the old cell
    /// to the new.
    #[test]
    fn a_click_plays_the_chosen_effect_with_the_panes_shapes() {
        use settings::{Arrival, TrailStyle};
        for style in [TrailStyle::Comet, TrailStyle::Sweep, TrailStyle::Warp, TrailStyle::Tail] {
            let mut p = preview();
            let from = p.rect(p.cursor).unwrap();
            assert!(p.moved((30, 1), &on(style, Arrival::Off), false));
            assert_eq!(p.trail.as_ref().unwrap().style, Some(style));
            assert_eq!(p.trail.as_ref().unwrap().from, Some(cursor_shapes::quad(from)));
            let (layers, _) = p.frame().expect("it draws as it starts");
            assert!(!layers.is_empty(), "{style:?}");
            if style != TrailStyle::Tail {
                // The panes' layers for the same ends, at about the same time:
                // the same outlines, within a frame's worth of motion.
                let to = p.rect((30, 1)).unwrap();
                let t = p.trail.as_ref().unwrap().start.elapsed().as_secs_f32() / 0.2;
                let want = cursor_shapes::trail(style, cursor_shapes::quad(from), to, 16.0, t).0;
                assert_eq!(layers.len(), want.len(), "{style:?}: the panes' layers");
                for (got, want) in layers.iter().zip(&want) {
                    assert_eq!(got.outline.len(), want.outline.len(), "{style:?}");
                    // The same points, wherever each outline starts.
                    for a in &got.outline {
                        let near = want.outline.iter().any(|b| {
                            (f32::from(a.x - b.x)).hypot(f32::from(a.y - b.y)) < 2.0
                        });
                        assert!(near, "{style:?}: {a:?} not in {:?}", want.outline);
                    }
                }
            }
        }
        // A setting changed between clicks shows on the next one.
        let mut p = preview();
        p.moved((30, 1), &on(TrailStyle::Comet, Arrival::Off), false);
        p.moved((3, 3), &on(TrailStyle::Sweep, Arrival::Ripple), false);
        let t = p.trail.as_ref().unwrap();
        assert_eq!((t.style, t.arrival), (Some(TrailStyle::Sweep), Arrival::Ripple));
    }

    /// A tail chases into the clicked cell and ends there, as in a pane.
    #[test]
    fn a_tail_chases_into_the_clicked_cell_and_ends() {
        use settings::{Arrival, TrailStyle};
        let mut p = preview();
        p.moved((40, 0), &on(TrailStyle::Tail, Arrival::Off), false);
        let to = p.rect((40, 0)).unwrap();
        let mut gaps = Vec::new();
        for _ in 0..100 {
            let Some((layers, _)) = p.frame() else { break };
            let x = layers[0].outline.iter().map(|q| f32::from(q.x)).fold(f32::MAX, f32::min);
            gaps.push(f32::from(to.origin.x) - x);
            p.trail.as_mut().unwrap().age(std::time::Duration::from_millis(16));
        }
        assert!(p.trail.is_none(), "it ends: {gaps:?}");
        assert!(gaps.len() >= 3 && gaps[0] > 200.0, "{gaps:?}");
        assert!(gaps.windows(2).all(|w| w[1] <= w[0] + 1e-3), "closing in: {gaps:?}");
    }

    #[test]
    fn with_the_trail_off_only_the_pulse_plays_and_with_both_off_nothing() {
        use settings::{Arrival, CursorTrail, TrailStyle};
        let mut p = preview();
        let pulse = settings::Settings { cursor_trail: CursorTrail::Off, ..on(TrailStyle::Comet, Arrival::Boom) };
        assert!(p.moved((9, 2), &pulse, false));
        assert_eq!(p.trail.as_ref().unwrap().from, None, "the pulse alone");
        let off = settings::Settings { cursor_trail: CursorTrail::Off, ..on(TrailStyle::Comet, Arrival::Off) };
        assert!(!p.moved((1, 1), &off, false), "nothing runs, so no frames");
        assert_eq!(p.cursor, (1, 1), "the cursor still moves");
        assert!(p.frame().is_none());
        assert!(!p.moved((5, 5), &on(TrailStyle::Comet, Arrival::Ripple), true), "reduce motion: none");
    }

    /// In a window: a click in the box plays the effect, one frame asked for
    /// at a time while it runs, and none once it ends.
    #[gpui::test]
    fn in_a_window_a_click_draws_frames_only_while_the_effect_runs(cx: &mut gpui::TestAppContext) {
        use gpui::{IntoElement, ParentElement as _, Render, Styled as _, div};
        use std::{cell::Cell, rc::Rc};
        type Seen = Rc<std::cell::RefCell<Option<gpui::Entity<CursorPreview>>>>;
        struct Host(Rc<Cell<u32>>, Seen);
        impl Render for Host {
            fn render(&mut self, window: &mut gpui::Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
                self.0.set(self.0.get() + 1);
                let view = super::view("t", window, cx);
                *self.1.borrow_mut() = Some(view.clone());
                div().size_full().child(gpui::AnyView::from(view))
            }
        }
        cx.update(|_| {
            settings::set(settings::Settings {
                cursor_trail_duration_ms: 50,
                ..on(settings::TrailStyle::Sweep, settings::Arrival::Ripple)
            })
        });
        let renders = Rc::new(Cell::new(0));
        let seen: Seen = Default::default();
        let (host, shown) = (renders.clone(), seen.clone());
        let (_, cx) = cx.add_window_view(|_, _| Host(host, shown));
        cx.run_until_parked();
        let preview = seen.borrow().clone().expect("rendered");
        let (grid, frames) = preview.read_with(cx, |p, _| (p.grid, p.frames));
        let (origin, cell, _) = grid.expect("painted");
        assert_eq!(frames, 0, "nothing runs at rest");
        let before = renders.get();
        let at = origin + point(cell.width * 20.5, cell.height * 1.5);
        cx.simulate_mouse_down(at, gpui::MouseButton::Left, gpui::Modifiers::default());
        let start = std::time::Instant::now();
        cx.run_until_parked();
        while preview.read_with(cx, |p, _| p.trail.is_some()) {
            assert!(start.elapsed() < std::time::Duration::from_secs(2), "it ends");
            std::thread::sleep(std::time::Duration::from_millis(5));
            let asked = cx.update(|window, cx| window.simulate_next_frame(cx));
            assert_eq!(asked, 1, "one frame asked for while it runs");
            cx.run_until_parked();
        }
        let ran = preview.read_with(cx, |p, _| (p.cursor, p.frames));
        assert_eq!(ran.0, (20, 1), "the cursor moved to the clicked cell");
        assert!(ran.1 >= 2, "it drew the effect over several frames: {}", ran.1);
        // GPUI draws a notified view's ancestors again, so the view it sits in
        // is drawn with each frame, and no more often.
        assert!(renders.get() - before <= ran.1 as u32 + 1, "{} renders for {} frames", renders.get() - before, ran.1);
        assert_eq!(cx.update(|window, cx| window.simulate_next_frame(cx)), 0, "no frame asked for once it's over");
        cx.run_until_parked();
        assert_eq!(preview.read_with(cx, |p, _| p.frames), ran.1);
    }

    /// Frames are asked for only while the effect runs: once it's over,
    /// `frame` says so and drops it, so the canvas stops asking.
    #[test]
    fn it_stops_asking_for_frames_once_the_effect_ends() {
        use settings::{Arrival, TrailStyle};
        let mut p = preview();
        p.moved((30, 1), &on(TrailStyle::Sweep, Arrival::Ripple), false);
        p.trail.as_mut().unwrap().age(std::time::Duration::from_millis(250));
        assert!(p.frame().is_none(), "over after its duration");
        assert!(p.trail.is_none(), "and dropped");
        assert!(p.frame().is_none(), "with nothing running, no frame");
    }
}
