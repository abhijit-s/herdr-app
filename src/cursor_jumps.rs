//! In-pane cursor jumps for `cursor_trail: "all"`: the focused terminal's
//! paint tells `Jumps` where its cursor is drawn, and one settle timer per
//! terminal asks it, once the cursor has been still for
//! `cursor_trail_settle_ms`, whether that was a jump worth an effect, as
//! kitty's cursor trail decides (`kitty/cursor_trail.c`): more than
//! `cursor_trail_min_cells` from the last settled place in either axis
//! (Chebyshev distance, so a diagonal step is one cell; kitty's start
//! threshold, moves of that many cells or fewer are ignored), or the cursor's
//! shape changed. Typing settles one cell at a time, so it never draws; output
//! that moves the cursor faster than the settle time draws nothing until it
//! pauses.
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
    timer: bool,
}

/// Cells between two cells, a diagonal step counting one.
pub fn chebyshev(a: Cell, b: Cell) -> u16 {
    a.0.abs_diff(b.0).max(a.1.abs_diff(b.1))
}

impl Jumps {
    /// Paint drew the cursor at `spot` (None: hidden) in `view` at `now`.
    /// True when a settle timer must be started: the cursor moved and none is
    /// pending. A new view (scrolled, resized, settings changed) only settles,
    /// so scrolling back or a reflow is no jump.
    pub fn observe(&mut self, spot: Option<Spot>, view: View, now: Instant) -> bool {
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
            return false;
        }
        self.moved = Some((spot, now));
        let start = !self.timer;
        self.timer = true;
        start
    }

    /// Forgets where the cursor was (hidden, or the terminal lost the focus);
    /// a pending timer stays pending, so there is still at most one.
    pub fn forget(&mut self) {
        (self.settled, self.moved) = (None, None);
    }

    /// Whether there is anything to forget.
    pub fn knows(&self) -> bool {
        self.settled.is_some() || self.moved.is_some()
    }

    /// The settle timer fired at `now`: wait on while the cursor moved within
    /// `settle`, else settle it and say whether it jumped.
    pub fn due(&mut self, now: Instant, settle: Duration, min_cells: u16) -> Due {
        let Some((spot, at)) = self.moved else {
            self.timer = false;
            return Due::Fire(None);
        };
        let still = now.saturating_duration_since(at);
        if still < settle {
            return Due::Wait(settle - still);
        }
        self.timer = false;
        self.moved = None;
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
        j.observe(at(8, 6), V, ms(t0, 460));
        j.due(ms(t0, 560), SETTLE, 2);
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

    /// Paint observations `(ms, column, row)` run through `Jumps` with its one
    /// settle timer, as `settle_jumps` drives it: the jumps that fire, each
    /// with when.
    fn run(moves: &[(u64, u16, u16)], settle_ms: u64, min_cells: u16) -> Vec<(u64, Jump)> {
        let (mut j, t0, settle) = (
            Jumps::default(),
            Instant::now(),
            Duration::from_millis(settle_ms),
        );
        let (mut wake, mut fired) = (None::<u64>, Vec::new());
        let mut events = moves.iter().copied().peekable();
        loop {
            let next_move = events.peek().map(|m| m.0);
            match (next_move, wake) {
                (Some(m), Some(w)) if w < m => {}
                (Some(_), _) => {
                    let (t, x, y) = events.next().unwrap();
                    if j.observe(at(x, y), V, ms(t0, t)) {
                        wake = Some(t + settle_ms);
                    }
                    continue;
                }
                (None, None) => return fired,
                (None, Some(_)) => {}
            }
            let w = wake.take().unwrap();
            match j.due(ms(t0, w), settle, min_cells) {
                Due::Wait(more) => wake = Some(w + more.as_millis() as u64),
                Due::Fire(Some(jump)) => fired.push((w, jump)),
                Due::Fire(None) => {}
            }
        }
    }

    #[test]
    fn typing_on_a_line_never_draws() {
        // A key every 60 to 150 ms, a backspace, a pause of half a second, more keys.
        let mut moves = vec![(0, 10, 5)];
        let (mut t, mut x) = (0, 10);
        for (i, gap) in [60, 150, 90, 120, 75, 140, 60, 100].iter().enumerate() {
            t += gap;
            x = if i == 4 { x - 1 } else { x + 1 };
            moves.push((t, x, 5));
        }
        t += 500;
        for gap in [80, 110, 65] {
            t += gap;
            x += 1;
            moves.push((t, x, 5));
        }
        assert_eq!(
            run(&moves, 10, 2),
            vec![],
            "each key settles alone as a one-cell move"
        );
    }

    #[test]
    fn real_jumps_still_draw_after_the_settle_time() {
        let jump = |from: Cell, to: Cell| run(&[(0, from.0, from.1), (100, to.0, to.1)], 10, 2);
        // Enter at the end of a command: the prompt on the next line.
        assert_eq!(
            jump((30, 5), (0, 6)),
            vec![(
                110,
                Jump::Moved {
                    from: (30, 5),
                    to: (0, 6)
                }
            )]
        );
        // `clear`: home.
        assert_eq!(
            jump((40, 20), (0, 0)),
            vec![(
                110,
                Jump::Moved {
                    from: (40, 20),
                    to: (0, 0)
                }
            )]
        );
        // Ten cells along the row (a word jump, an editor motion).
        assert_eq!(jump((5, 5), (15, 5)).len(), 1);
    }

    #[test]
    fn output_pushing_three_lines_draws_and_one_or_two_lines_dont() {
        let push = |lines: u16| run(&[(0, 0, 5), (100, 0, 5 + lines)], 10, 2);
        assert_eq!(push(1), vec![]);
        assert_eq!(push(2), vec![]);
        assert_eq!(
            push(3),
            vec![(
                110,
                Jump::Moved {
                    from: (0, 5),
                    to: (0, 8)
                }
            )]
        );
    }

    #[test]
    fn a_stream_faster_than_the_settle_time_draws_at_most_once_per_pause() {
        // Three bursts: the cursor moving every 5 ms for 300 ms, then still for 200 ms.
        let mut moves = vec![(0, 0, 0)];
        for burst in 0..3u64 {
            let start = 1 + burst * 500;
            for i in 0..60u64 {
                moves.push((start + i * 5, (i % 40) as u16, (burst * 3 + i / 20) as u16));
            }
        }
        let fired = run(&moves, 10, 2);
        assert_eq!(fired.len(), 3, "one per pause: {fired:?}");
        assert!(
            fired
                .iter()
                .zip([0u64, 500, 1000])
                .all(|((at, _), start)| *at > start + 295 && *at < start + 500),
            "{fired:?}"
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
