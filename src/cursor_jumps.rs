//! In-pane cursor jumps for `cursor_trail: "all"`: the focused terminal's
//! paint tells `Jumps` where its cursor is drawn, and one settle timer per
//! terminal asks it, once the cursor has been still for
//! `cursor_trail_settle_ms`, whether that was a jump worth an effect, as
//! kitty's cursor trail decides: at least `cursor_trail_min_cells` from the
//! last settled place (Chebyshev distance, so a diagonal step is one cell),
//! or the cursor's shape changed. Streaming output that moves the cursor every
//! frame never settles, so it draws nothing until it stops.
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

/// The focused terminal's cursor as paint saw it.
#[derive(Default)]
pub struct Jumps {
    settled: Option<Spot>,
    moved: Option<(Spot, Instant)>,
    timer: bool,
}

/// Cells between two cells, a diagonal step counting one.
pub fn chebyshev(a: Cell, b: Cell) -> u16 {
    a.0.abs_diff(b.0).max(a.1.abs_diff(b.1))
}

impl Jumps {
    /// Paint drew the cursor at `spot` (None: hidden) at `now`. True when a
    /// settle timer must be started: the cursor moved and none is pending.
    pub fn observe(&mut self, spot: Option<Spot>, now: Instant) -> bool {
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
            if chebyshev(b.cell, spot.cell) >= min_cells.max(1) {
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
        assert!(!j.observe(at(0, 0), t0), "the first sight only settles");
        assert!(
            j.observe(at(10, 3), ms(t0, 5)),
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
        j.observe(at(0, 0), t0);
        let mut timers = 0;
        // Streaming output: the cursor moves every 16 ms for a second.
        for i in 1..=60u64 {
            timers += j.observe(at(i as u16 % 40, (i / 40) as u16), ms(t0, i * 16)) as u32;
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
        j.observe(at(5, 5), t0);
        j.observe(at(6, 6), ms(t0, 1));
        assert_eq!(
            j.due(ms(t0, 100), SETTLE, 2),
            Due::Fire(None),
            "a diagonal step is one cell, under two"
        );
        j.observe(at(8, 6), ms(t0, 200));
        assert_eq!(
            j.due(ms(t0, 300), SETTLE, 2),
            Due::Fire(Some(Jump::Moved {
                from: (6, 6),
                to: (8, 6)
            })),
            "two from where it last settled"
        );
        j.observe(
            Some(Spot {
                cell: (8, 6),
                shape: CursorShape::Bar,
            }),
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
        j.observe(at(0, 0), t0);
        j.observe(None, ms(t0, 10));
        assert!(!j.knows());
        assert!(!j.observe(at(30, 10), ms(t0, 20)), "back: only settles");
        assert_eq!(j.due(ms(t0, 200), SETTLE, 2), Due::Fire(None));
    }

    #[test]
    fn a_settle_of_zero_fires_at_the_first_wake_and_one_cell_is_the_least_threshold() {
        let (mut j, t0) = (Jumps::default(), Instant::now());
        j.observe(at(0, 0), t0);
        j.observe(at(1, 0), t0);
        assert_eq!(
            j.due(t0, Duration::ZERO, 0),
            Due::Fire(Some(Jump::Moved {
                from: (0, 0),
                to: (1, 0)
            })),
            "0 cells counts as 1: staying put is never a jump"
        );
    }

    #[test]
    fn forgetting_keeps_a_pending_timer_so_there_is_never_a_second() {
        let (mut j, t0) = (Jumps::default(), Instant::now());
        j.observe(at(0, 0), t0);
        assert!(j.observe(at(9, 0), ms(t0, 1)));
        j.forget();
        j.observe(at(0, 0), ms(t0, 2));
        assert!(
            !j.observe(at(9, 9), ms(t0, 3)),
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
