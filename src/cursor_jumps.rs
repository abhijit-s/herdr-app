//! In-pane cursor jumps for `cursor_trail: "all"`: the focused terminal's
//! paint tells `Jumps` where its cursor is drawn, and one settle timer per
//! terminal asks it, once the cursor has been still for
//! `cursor_trail_settle_ms`, whether that was a jump worth an effect, as
//! kitty's cursor trail decides (`kitty/cursor_trail.c`): more than
//! `cursor_trail_min_cells` from the last settled place in either axis
//! (Chebyshev distance, so a diagonal step is one cell; kitty's start
//! threshold, moves of that many cells or fewer are ignored), or the cursor's
//! shape changed. Stillness is measured from when the program last moved the
//! cursor (the output path calls `program_moved` after a chunk that moved it),
//! as kitty measures from `position_changed_by_client_at`, not from when a
//! frame drew it: frames come up to ~50 ms apart while output streams, so
//! paint time would find a streaming cursor "still" between frames. Typing
//! settles one cell at a time, so it doesn't draw; output draws nothing until
//! it pauses for the settle time.
//!
//! A hidden cursor forgets where it was, so it reappearing isn't a jump.

use crate::vt::CursorShape;
use std::time::{Duration, Instant};

pub type Cell = (u16, u16);

/// Where the cursor is drawn: its cell and shape.
#[derive(Clone, Copy, PartialEq)]
pub struct Spot {
    pub cell: Cell,
    pub shape: CursorShape,
}

/// What a settled cursor did since it last settled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Jump {
    /// It moved far enough: a trail from `from` to `to` (cells).
    Moved { from: Cell, to: Cell },
    /// It stayed (or moved too little) but changed shape: a pulse only.
    Reshaped,
}

/// What the settle timer does next.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Due {
    /// The cursor moved within the settle time: wait this much more.
    Wait(Duration),
    /// It settled (or there was nothing to settle): the jump, if it was one.
    Fire(Option<Jump>),
}

/// What a cell is measured against: how far the screen is scrolled back, its
/// size, and anything else whose change makes old cells meaningless.
pub type View = (u64, u16, u16, usize);

/// The focused terminal's cursor as paint saw it.
#[derive(Default)]
pub struct Jumps {
    view: Option<View>,
    settled: Option<Spot>,
    moved: Option<(Spot, Instant)>,
    /// When the program last moved the cursor, as the output path saw it.
    program: Option<Instant>,
    /// When paint last drew this terminal.
    seen: Option<Instant>,
    /// When the settle timer began waiting for a frame to draw the program's
    /// last move.
    catching: Option<Instant>,
    timer: bool,
}

/// How long the settle timer waits, once the cursor is still, for a frame to
/// draw the program's last move before settling on what was drawn; and how
/// often it looks meanwhile (never zero, so a settle time of 0 can't spin).
const CATCH_UP: Duration = Duration::from_millis(100);
const CATCH_UP_STEP: Duration = Duration::from_millis(8);
/// Not drawn for this long, the terminal isn't on screen (another tab, a
/// covered window): a pending timer gives up rather than wake on as its output
/// streams.
const LOST: Duration = Duration::from_millis(500);

/// Cells between two cells, a diagonal step counting one.
pub fn chebyshev(a: Cell, b: Cell) -> u16 {
    a.0.abs_diff(b.0).max(a.1.abs_diff(b.1))
}

impl Jumps {
    /// Program output moved the cursor at `at` (the output path, once per
    /// chunk that moved it).
    pub fn program_moved(&mut self, at: Instant) {
        self.program = Some(at);
    }

    /// Paint drew the cursor at `spot` (None: hidden) in `view` at `now`; a
    /// move counts from when the program made it, else from now.
    /// True when a settle timer must be started: the cursor moved and none is
    /// pending. A new view (scrolled, resized, settings changed) only settles,
    /// so scrolling back or a reflow is no jump.
    pub fn observe(&mut self, spot: Option<Spot>, view: View, now: Instant) -> bool {
        self.seen = Some(now);
        if self.view != Some(view) {
            self.forget();
            self.view = Some(view);
        }
        let Some(spot) = spot else {
            self.forget();
            return false;
        };
        let Some(settled) = self.settled else {
            self.settled = Some(spot);
            return false;
        };
        let latest = self.moved.map_or(settled, |(s, _)| s);
        if spot == latest {
            // Drawn where it was drawn last, as of the program's latest move.
            if let (Some((_, seen)), Some(p)) = (self.moved.as_mut(), self.program) {
                *seen = p.max(*seen);
            }
            return false;
        }
        // A new cell counts from the program's move; a shape change alone, which
        // the output path doesn't see, from now.
        let stamp = if spot.cell != latest.cell {
            self.program.map_or(now, |p| p.min(now))
        } else {
            now
        };
        self.moved = Some((spot, stamp));
        let start = !self.timer;
        self.timer = true;
        start
    }

    /// Forgets where the cursor was (hidden, or the terminal lost the focus);
    /// a pending timer stays pending, so there is still at most one.
    pub fn forget(&mut self) {
        (self.settled, self.moved, self.catching) = (None, None, None);
    }

    /// Whether there is anything to forget.
    pub fn knows(&self) -> bool {
        self.settled.is_some() || self.moved.is_some()
    }

    /// The settle timer fired at `now`: wait on while the cursor moved within
    /// `settle` (by paint's last sight or the program's last move, whichever is
    /// later, so a move not yet drawn still counts), else settle it and say
    /// whether it jumped.
    pub fn due(&mut self, now: Instant, settle: Duration, min_cells: u16) -> Due {
        let Some((spot, at)) = self.moved else {
            self.timer = false;
            return Due::Fire(None);
        };
        if self
            .seen
            .is_some_and(|seen| now.saturating_duration_since(seen) >= LOST)
        {
            self.forget();
            self.timer = false;
            return Due::Fire(None);
        }
        let still = now.saturating_duration_since(self.program.map_or(at, |p| p.max(at)));
        if still < settle {
            return Due::Wait(settle - still);
        }
        // The program moved the cursor after paint last drew it: wait for the
        // frame that shows where, so the trail ends there, not on a stale cell;
        // but never for long, in case no frame comes (a hidden window).
        if self.program.is_some_and(|p| p > at) {
            let waited = now.saturating_duration_since(*self.catching.get_or_insert(now));
            if waited < CATCH_UP {
                return Due::Wait(CATCH_UP_STEP.min(CATCH_UP - waited));
            }
        }
        self.timer = false;
        (self.moved, self.catching) = (None, None);
        let before = self.settled.replace(spot);
        Due::Fire(before.and_then(|b| {
            if chebyshev(b.cell, spot.cell) > min_cells {
                Some(Jump::Moved {
                    from: b.cell,
                    to: spot.cell,
                })
            } else {
                (b.shape != spot.shape).then_some(Jump::Reshaped)
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SETTLE: Duration = Duration::from_millis(80);
    const V: View = (0, 80, 24, 1);

    fn at(x: u16, y: u16) -> Option<Spot> {
        Some(Spot {
            cell: (x, y),
            shape: CursorShape::Block,
        })
    }

    fn ms(t0: Instant, n: u64) -> Instant {
        t0 + Duration::from_millis(n)
    }

    #[test]
    fn a_jump_fires_once_the_cursor_has_been_still_for_the_settle_time() {
        let (mut j, t0) = (Jumps::default(), Instant::now());
        assert!(!j.observe(at(0, 0), V, t0), "the first sight only settles");
        assert!(
            j.observe(at(10, 3), V, ms(t0, 5)),
            "a move starts the one timer"
        );
        assert_eq!(
            j.due(ms(t0, 50), SETTLE, 2),
            Due::Wait(Duration::from_millis(35)),
            "not yet still long enough"
        );
        assert_eq!(
            j.due(ms(t0, 85), SETTLE, 2),
            Due::Fire(Some(Jump::Moved {
                from: (0, 0),
                to: (10, 3)
            }))
        );
        assert_eq!(
            j.due(ms(t0, 200), SETTLE, 2),
            Due::Fire(None),
            "settled: nothing more"
        );
    }

    #[test]
    fn a_burst_never_fires_until_it_stops_and_then_once_from_where_it_started() {
        let (mut j, t0) = (Jumps::default(), Instant::now());
        j.observe(at(0, 0), V, t0);
        let mut timers = 0;
        // Streaming output: the cursor moves every 16 ms for a second.
        for i in 1..=60u64 {
            timers += j.observe(at(i as u16 % 40, (i / 40) as u16), V, ms(t0, i * 16)) as u32;
            // The timer, whenever it wakes during the burst, waits on.
            if i % 5 == 0 {
                assert!(
                    matches!(j.due(ms(t0, i * 16 + 1), SETTLE, 2), Due::Wait(_)),
                    "{i}"
                );
            }
        }
        assert_eq!(timers, 1, "one timer for the whole burst");
        assert_eq!(
            j.due(ms(t0, 960 + 80), SETTLE, 2),
            Due::Fire(Some(Jump::Moved {
                from: (0, 0),
                to: (20, 1)
            }))
        );
    }

    #[test]
    fn small_moves_settle_without_an_effect_and_shape_changes_pulse() {
        let (mut j, t0) = (Jumps::default(), Instant::now());
        j.observe(at(5, 5), V, t0);
        j.observe(at(6, 6), V, ms(t0, 1));
        assert_eq!(
            j.due(ms(t0, 100), SETTLE, 2),
            Due::Fire(None),
            "a diagonal step is one cell, under two"
        );
        j.observe(at(8, 6), V, ms(t0, 200));
        assert_eq!(
            j.due(ms(t0, 300), SETTLE, 2),
            Due::Fire(None),
            "two cells is the threshold itself: ignored, as kitty does"
        );
        j.observe(at(11, 6), V, ms(t0, 350));
        assert_eq!(
            j.due(ms(t0, 450), SETTLE, 2),
            Due::Fire(Some(Jump::Moved {
                from: (8, 6),
                to: (11, 6)
            })),
            "three from where it last settled: more than the threshold"
        );
        // Back to (8, 6), three cells, to try a shape change there.
        j.observe(at(8, 6), V, ms(t0, 460));
        assert_eq!(
            j.due(ms(t0, 560), SETTLE, 2),
            Due::Fire(Some(Jump::Moved {
                from: (11, 6),
                to: (8, 6)
            }))
        );
        j.observe(
            Some(Spot {
                cell: (8, 6),
                shape: CursorShape::Bar,
            }),
            V,
            ms(t0, 400),
        );
        assert_eq!(
            j.due(ms(t0, 500), SETTLE, 2),
            Due::Fire(Some(Jump::Reshaped))
        );
    }

    #[test]
    fn a_hidden_cursor_forgets_so_reappearing_is_no_jump() {
        let (mut j, t0) = (Jumps::default(), Instant::now());
        j.observe(at(0, 0), V, t0);
        j.observe(None, V, ms(t0, 10));
        assert!(!j.knows());
        assert!(!j.observe(at(30, 10), V, ms(t0, 20)), "back: only settles");
        assert_eq!(j.due(ms(t0, 200), SETTLE, 2), Due::Fire(None));
    }

    #[test]
    fn a_settle_of_zero_fires_at_the_first_wake_and_a_threshold_of_zero_draws_every_move() {
        let (mut j, t0) = (Jumps::default(), Instant::now());
        j.observe(at(0, 0), V, t0);
        j.observe(at(1, 0), V, t0);
        assert_eq!(
            j.due(t0, Duration::ZERO, 0),
            Due::Fire(Some(Jump::Moved {
                from: (0, 0),
                to: (1, 0)
            })),
            "one cell is more than 0"
        );
        j.observe(
            Some(Spot {
                cell: (1, 0),
                shape: CursorShape::Block,
            }),
            V,
            t0,
        );
        assert_eq!(
            j.due(t0, Duration::ZERO, 0),
            Due::Fire(None),
            "staying put is never a jump"
        );
    }

    #[test]
    fn forgetting_keeps_a_pending_timer_so_there_is_never_a_second() {
        let (mut j, t0) = (Jumps::default(), Instant::now());
        j.observe(at(0, 0), V, t0);
        assert!(j.observe(at(9, 0), V, ms(t0, 1)));
        j.forget();
        j.observe(at(0, 0), V, ms(t0, 2));
        assert!(
            !j.observe(at(9, 9), V, ms(t0, 3)),
            "the first timer still runs"
        );
        assert_eq!(
            j.due(ms(t0, 100), SETTLE, 2),
            Due::Fire(Some(Jump::Moved {
                from: (0, 0),
                to: (9, 9)
            }))
        );
    }

    #[test]
    fn a_new_view_settles_without_a_jump() {
        let (mut j, t0) = (Jumps::default(), Instant::now());
        j.observe(at(0, 20), V, t0);
        // Scrolled back five lines: the cursor's cell moves down five, but it didn't move.
        assert!(
            !j.observe(at(0, 25), (5, 80, 24, 1), ms(t0, 10)),
            "a new view only settles"
        );
        assert_eq!(j.due(ms(t0, 200), SETTLE, 2), Due::Fire(None));
        // Resized, or the settings changed (all off and on again): the same.
        assert!(!j.observe(at(9, 9), (5, 100, 30, 1), ms(t0, 300)));
        assert!(!j.observe(at(30, 2), (5, 100, 30, 2), ms(t0, 310)));
        assert!(
            j.observe(at(0, 0), (5, 100, 30, 2), ms(t0, 320)),
            "within one view a move is a move"
        );
    }

    /// The program moving the cursor to `(ms, column, row)`, drawn on frames
    /// every `frame` ms, through `Jumps` with its one settle timer as the output
    /// path, paint and `settle_jumps` drive it: the jumps that fire, each with
    /// when. The first move is where the cursor starts.
    fn run(
        moves: &[(u64, u16, u16)],
        frame: u64,
        settle_ms: u64,
        min_cells: u16,
    ) -> Vec<(u64, Jump)> {
        let (mut j, t0, settle) = (
            Jumps::default(),
            Instant::now(),
            Duration::from_millis(settle_ms),
        );
        let end = moves.last().map_or(0, |m| m.0) + 1000;
        let (mut at_cell, mut moved) = ((moves[0].1, moves[0].2), 0usize);
        let (mut wake, mut fired) = (None::<u64>, Vec::new());
        // Millisecond by millisecond: the program's moves first, then a frame, then the timer.
        for t in 0..=end {
            while moved < moves.len() && moves[moved].0 == t {
                let cell = (moves[moved].1, moves[moved].2);
                // As the output path: only a chunk that moved the cursor counts.
                if moved > 0 && cell != at_cell {
                    j.program_moved(ms(t0, t));
                }
                at_cell = cell;
                moved += 1;
            }
            if t % frame == 0 && j.observe(at(at_cell.0, at_cell.1), V, ms(t0, t)) {
                wake = Some(t + settle_ms);
            }
            if wake == Some(t) {
                wake = None;
                match j.due(ms(t0, t), settle, min_cells) {
                    Due::Wait(more) => wake = Some(t + more.as_millis() as u64),
                    Due::Fire(Some(jump)) => fired.push((t, jump)),
                    Due::Fire(None) => {}
                }
            }
        }
        fired
    }

    /// The default settle time.
    const S: u64 = 10;

    #[test]
    fn typing_on_a_line_never_draws() {
        // A key every 60 to 150 ms, a backspace, a pause, and three fast keys 30 ms apart.
        let mut moves = vec![(0, 10, 5)];
        let (mut t, mut x) = (0, 10);
        for (i, gap) in [60, 150, 90, 120, 75, 140, 60, 100, 500, 30, 30]
            .iter()
            .enumerate()
        {
            t += gap;
            x = if i == 4 { x - 1 } else { x + 1 };
            moves.push((t, x, 5));
        }
        for frame in [16, 50] {
            assert_eq!(
                run(&moves, frame, S, 2),
                vec![],
                "{frame} ms frames: keys settle a cell (two when a frame catches two keys) at a time"
            );
        }
    }

    #[test]
    fn real_jumps_still_draw_once_the_program_pauses() {
        for frame in [16, 50] {
            let jump =
                |from: Cell, to: Cell| run(&[(0, from.0, from.1), (100, to.0, to.1)], frame, S, 2);
            // Enter at the end of a command: the prompt on the next line; `clear`: home; ten cells along the row.
            assert_eq!(
                jump((30, 5), (0, 6))
                    .iter()
                    .map(|f| f.1)
                    .collect::<Vec<_>>(),
                vec![Jump::Moved {
                    from: (30, 5),
                    to: (0, 6)
                }]
            );
            assert_eq!(
                jump((40, 20), (0, 0))
                    .iter()
                    .map(|f| f.1)
                    .collect::<Vec<_>>(),
                vec![Jump::Moved {
                    from: (40, 20),
                    to: (0, 0)
                }]
            );
            assert_eq!(jump((5, 5), (15, 5)).len(), 1);
            // Drawn within a frame and the settle time of the move.
            assert!(
                jump((5, 5), (15, 5)).iter().all(|f| f.0 <= 100 + frame + S),
                "{frame}"
            );
        }
    }

    #[test]
    fn output_pushing_three_lines_draws_and_one_or_two_lines_dont() {
        for frame in [16, 50] {
            let push = |lines: u16| run(&[(0, 0, 5), (100, 0, 5 + lines)], frame, S, 2);
            assert_eq!(push(1), vec![], "{frame}");
            assert_eq!(push(2), vec![], "{frame}");
            assert_eq!(
                push(3).iter().map(|f| f.1).collect::<Vec<_>>(),
                vec![Jump::Moved {
                    from: (0, 5),
                    to: (0, 8)
                }],
                "{frame}"
            );
        }
    }

    #[test]
    fn a_settle_of_zero_waiting_for_a_frame_never_spins() {
        let (mut j, t0) = (Jumps::default(), Instant::now());
        j.observe(at(0, 0), V, t0);
        j.program_moved(t0);
        assert!(j.observe(at(9, 0), V, t0));
        j.program_moved(ms(t0, 2));
        let (mut t, mut wakes) = (2u64, 0);
        let fired = loop {
            wakes += 1;
            match j.due(ms(t0, t), Duration::ZERO, 2) {
                Due::Wait(more) => {
                    assert!(more > Duration::ZERO, "a zero wait would spin at {t}");
                    t += more.as_millis() as u64;
                }
                Due::Fire(jump) => break jump,
            }
        };
        assert!(
            t <= 2 + 100 + 8 && wakes <= 14,
            "gave up by {t} after {wakes} wakes"
        );
        assert_eq!(
            fired,
            Some(Jump::Moved {
                from: (0, 0),
                to: (9, 0)
            })
        );
    }

    #[test]
    fn a_terminal_not_drawn_for_a_while_gives_its_timer_up() {
        // Another tab streams: the program keeps moving the cursor, nothing draws it.
        let (mut j, t0) = (Jumps::default(), Instant::now());
        j.observe(at(0, 0), V, t0);
        j.program_moved(t0);
        assert!(j.observe(at(9, 0), V, t0));
        let mut t = 0;
        let fired = loop {
            t += 5;
            j.program_moved(ms(t0, t));
            if let Due::Fire(jump) = j.due(ms(t0, t + 1), Duration::from_millis(10), 2) {
                break jump;
            }
        };
        assert!((500..520).contains(&t), "gave up after half a second: {t}");
        assert_eq!(fired, None);
        assert!(!j.knows(), "and forgot, so its next sight only settles");
    }

    #[test]
    fn streaming_output_draws_nothing_until_it_pauses_whatever_the_frame_rate() {
        // The program moves the cursor every 2 ms (a chunk at a time) for 300 ms,
        // down a line every 20 ms, then leaves it for 300 ms; three times. Frames
        // come every 16 or every 50 ms, slower than the settle time: what keeps
        // it quiet is the program's moves, not the frames.
        for frame in [16u64, 50] {
            let mut moves = vec![(0, 0, 0)];
            for burst in 0..3u64 {
                let start = 1 + burst * 600;
                for i in 0..150u64 {
                    moves.push((
                        start + i * 2,
                        (i * 7 % 40) as u16,
                        (burst * 20 + i / 10) as u16,
                    ));
                }
            }
            let fired = run(&moves, frame, S, 2);
            assert_eq!(
                fired.len(),
                3,
                "{frame} ms frames: one per pause: {fired:?}"
            );
            assert!(
                fired
                    .iter()
                    .zip([0u64, 600, 1200])
                    .all(|((at, _), start)| *at >= start + 300 && *at < start + 600),
                "{frame} ms: {fired:?}"
            );
        }
    }

    #[test]
    fn streaming_with_small_moves_stays_silent() {
        // A spinner or a status line redrawn in place: the cursor steps back and
        // forth a cell or two, every 30 ms, for a second.
        let mut moves = vec![(0, 20, 10)];
        for i in 1..=33u64 {
            moves.push((i * 30, 20 + (i % 3) as u16, 10));
        }
        for frame in [16, 50] {
            assert_eq!(run(&moves, frame, S, 2), vec![], "{frame}");
        }
    }

    #[test]
    fn a_settled_move_not_yet_drawn_waits_for_its_frame_but_not_forever() {
        let (mut j, t0, settle) = (Jumps::default(), Instant::now(), Duration::from_millis(10));
        j.observe(at(0, 0), V, t0);
        j.program_moved(t0);
        assert!(j.observe(at(9, 0), V, t0));
        // The program moved on at 5 and stopped; no frame has drawn it yet.
        j.program_moved(ms(t0, 5));
        assert_eq!(
            j.due(ms(t0, 15), settle, 2),
            Due::Wait(CATCH_UP_STEP),
            "still long enough, but drawn stale"
        );
        // The frame comes: drawn at (20, 0).
        assert!(!j.observe(at(20, 0), V, ms(t0, 16)));
        assert_eq!(
            j.due(ms(t0, 25), settle, 2),
            Due::Fire(Some(Jump::Moved {
                from: (0, 0),
                to: (20, 0)
            }))
        );
        // No frame ever comes (a hidden window): it settles on what was drawn.
        j.program_moved(ms(t0, 30));
        assert!(j.observe(at(30, 0), V, ms(t0, 30)));
        j.program_moved(ms(t0, 35));
        let mut t = 45;
        let fired = loop {
            match j.due(ms(t0, t), settle, 2) {
                Due::Wait(more) => t += more.as_millis() as u64,
                Due::Fire(jump) => break jump,
            }
        };
        assert!(t <= 30 + 10 + 100 + 10, "gave up waiting by {t}");
        assert_eq!(
            fired,
            Some(Jump::Moved {
                from: (20, 0),
                to: (30, 0)
            })
        );
    }

    #[test]
    fn a_move_not_yet_drawn_keeps_the_timer_waiting() {
        // Paint saw the move at 0; the program moved again at 8, before the timer
        // woke at 10, and that frame hasn't come: the timer waits from 8.
        let (mut j, t0) = (Jumps::default(), Instant::now());
        j.observe(at(0, 0), V, t0);
        j.program_moved(t0);
        assert!(j.observe(at(9, 0), V, t0));
        j.program_moved(ms(t0, 8));
        assert_eq!(
            j.due(ms(t0, 10), Duration::from_millis(10), 2),
            Due::Wait(Duration::from_millis(8))
        );
    }

    #[test]
    fn chebyshev_counts_a_diagonal_step_as_one() {
        assert_eq!(
            (
                chebyshev((0, 0), (1, 1)),
                chebyshev((3, 0), (0, 2)),
                chebyshev((5, 5), (5, 5))
            ),
            (1, 3, 0)
        );
    }
}
