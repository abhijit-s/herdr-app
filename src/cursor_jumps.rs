//! In-pane cursor jumps for `cursor_trail: "all"`: the focused terminal's
//! paint tells `Jumps` where its cursor is drawn, and one settle timer per
//! terminal asks it, once the cursor has been still for
//! `cursor_trail_settle_ms`, whether that was a jump worth an effect, as
//! kitty's cursor trail decides (`kitty/cursor_trail.c`): more columns or
//! more rows from the last settled place than `cursor_trail_min_cells` allows
//! across or down (kitty's two-value start threshold: a move within both is
//! ignored, so a diagonal step is one cell), or the cursor's shape changed. Stillness is measured from when the program last moved the
//! cursor (the output path calls `program_moved` after a chunk that moved it),
//! as kitty measures from `position_changed_by_client_at`, not from when a
//! frame drew it: frames come up to ~50 ms apart while output streams, so
//! paint time would find a streaming cursor "still" between frames. Typing
//! settles one cell at a time, so it doesn't draw; output draws nothing until
//! it pauses for the settle time.
//!
//! A hidden cursor forgets where it was, so it reappearing isn't a jump.
//!
//! Only a program ready for input jumps. Until then the cursor rests wherever
//! the program's output pauses (a shell's startup output, an `ssh` banner),
//! which says nothing about where the user is going, so a rest there moves
//! the settled place without an effect. A program starts when the terminal's
//! screen begins (`started`: a new terminal, a respawned shell, a reattach)
//! and when the cursor leaves a prompt the shell marked (a command, `exec
//! zsh`, `ssh`); it is ready once the cursor rests on a marked prompt line
//! (OSC 133 A and its continuation lines), the terminal shows the alternate
//! screen (a full-screen program draws its UI at once), or `STARTUP` has
//! passed since its first output (a program that marks no prompts gives no
//! other sign). On a new screen the settle that lands on the first prompt within
//! `STARTUP` is quiet too: it comes from wherever the startup left the cursor. After a
//! command it is an ordinary one, so `ls` or `clear` draws as it lands. A
//! shell that marks only prompt starts (no `133;B`/`133;C`) leaves every line
//! marked, so a command from it never counts as a start.
//!
//! That is `cursor_trail_startup: "quiet"`. With `"prompt"` (the default) the
//! settle that makes the program ready at a prompt also draws one trail, from where the
//! cursor was as it started (the prompt it left, or on a new screen the first
//! spot seen, where the focus trail landed; a focus loss or a new view moves
//! that to the next spot seen) to the prompt, if it went down more
//! rows than `cursor_trail_min_cells` allows: however many pauses the startup
//! output has, one trail, and a bare prompt drawn on the line the cursor was
//! on draws none; ready without a prompt it draws as "quiet". With `"every"` there is no startup rule: every settle is an
//! ordinary jump.

use crate::settings::{MinCells, TrailStartup};
use crate::vt::CursorShape;
use std::time::{Duration, Instant};

pub type Cell = (u16, u16);

/// Where the cursor is drawn: its cell and shape.
#[derive(Clone, Copy, PartialEq)]
pub struct Spot {
    pub cell: Cell,
    pub shape: CursorShape,
    /// On a wide (two-cell) character, which a block covers whole.
    pub wide: bool,
}

/// What a settled cursor did since it last settled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Jump {
    /// It moved far enough: a trail from `from` (where it had `shape`, on a
    /// `wide` character or not) to `to` (cells).
    Moved {
        from: Cell,
        to: Cell,
        shape: CursorShape,
        wide: bool,
    },
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

/// What the cursor rests on when it settles, as the terminal reads it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Rest {
    /// A line the shell marked as its prompt: it waits for input.
    Prompt,
    /// The alternate screen: a full-screen program.
    FullScreen,
    /// Anything else: output, or a program that marks no prompts.
    Output,
}

/// Whether the terminal's program is ready for input.
#[derive(Clone, Copy)]
enum Phase {
    /// Started (on a `new_screen`, or from a prompt) and not yet ready; since
    /// its first output, once there is some; `from` where the cursor was as
    /// it started (the prompt it left, or the first spot seen since).
    Starting {
        since: Option<Instant>,
        new_screen: bool,
        from: Option<Spot>,
    },
    /// Ready, last seen resting on a marked prompt or not.
    Ready { at_prompt: bool },
}

impl Default for Phase {
    fn default() -> Self {
        Phase::Ready { at_prompt: false }
    }
}

/// How long a program that never shows a marked prompt is taken to be
/// starting, from its first output: longer than a shell with plugins or an
/// `ssh` login takes to reach its first prompt (about 2 s), and short enough
/// that a program without prompt marks (`sh`, a REPL) trails again soon.
pub const STARTUP: Duration = Duration::from_secs(3);

/// The focused terminal's cursor as paint saw it.
#[derive(Default)]
pub struct Jumps {
    view: Option<View>,
    settled: Option<Spot>,
    moved: Option<(Spot, Instant)>,
    phase: Phase,
    /// When the program last moved the cursor, as the output path saw it.
    program: Option<Instant>,
    /// When paint last drew this terminal.
    seen: Option<Instant>,
    /// When the settle timer began waiting for a frame to draw the program's
    /// last move.
    catching: Option<Instant>,
    timer: bool,
    /// What a starting program's settles draw (`cursor_trail_startup`).
    pub startup: TrailStartup,
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

impl Jumps {
    /// Program output moved the cursor at `at` (the output path, once per
    /// chunk that moved it).
    pub fn program_moved(&mut self, at: Instant) {
        self.program = Some(at);
        self.program_wrote(|| at);
    }

    /// Program output arrived at `now` (the output path without `"all"`, which
    /// doesn't follow the cursor): a starting program's startup counts from
    /// its first, so turning `"all"` on later finds it long started.
    pub fn program_wrote(&mut self, now: impl FnOnce() -> Instant) {
        if let Phase::Starting { since, .. } = &mut self.phase {
            since.get_or_insert_with(now);
        }
    }

    /// A program started on a new screen (a new terminal, a respawned shell, a
    /// reattach): forgets the old screen's cursor and takes the program as
    /// starting until it is ready; a pending timer stays pending.
    pub fn started(&mut self) {
        self.forget();
        self.phase = Phase::Starting {
            since: None,
            new_screen: true,
            from: None,
        };
    }

    /// Paint drew the cursor at `spot` (None: hidden) in `view` at `now`; a
    /// move counts from when the program made it, else from now.
    /// True when a settle timer must be started: the cursor moved and none is
    /// pending. A new view (scrolled, resized, settings changed) only settles,
    /// so scrolling back or a reflow is no jump.
    pub fn observe(&mut self, spot: Option<Spot>, view: View, now: Instant) -> bool {
        self.seen = Some(now);
        if self.view != Some(view) {
            self.lose_place();
            self.view = Some(view);
        }
        let Some(spot) = spot else {
            self.forget();
            return false;
        };
        let Some(settled) = self.settled else {
            self.settled = Some(spot);
            if let Phase::Starting { from: from @ None, .. } = &mut self.phase {
                *from = Some(spot);
            }
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

    /// The terminal lost the focus (or its cells changed meaning): forgets
    /// where the cursor was, and where a starting program started, so the
    /// next spot seen (where the focus trail lands) takes that place.
    pub fn lose_place(&mut self) {
        self.forget();
        if let Phase::Starting { from, .. } = &mut self.phase {
            *from = None;
        }
    }

    /// Whether there is anything to forget.
    pub fn knows(&self) -> bool {
        self.settled.is_some() || self.moved.is_some() || matches!(self.phase, Phase::Starting { from: Some(_), .. })
    }

    /// The settle timer fired at `now`: wait on while the cursor moved within
    /// `settle` (by paint's last sight or the program's last move, whichever is
    /// later, so a move not yet drawn still counts), else settle it and say
    /// whether it jumped: never while the program is starting, as told by
    /// `rest`, what the cursor rests on (read only when it settles).
    pub fn due(
        &mut self,
        now: Instant,
        settle: Duration,
        min_cells: impl Into<MinCells>,
        rest: impl FnOnce() -> Rest,
    ) -> Due {
        let min = min_cells.into();
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
        let Some((b, hop)) = self.drawn_from(rest(), now, before) else {
            return Due::Fire(None);
        };
        let down = b.cell.1.abs_diff(spot.cell.1) > min.y;
        Due::Fire(
            if down || (!hop && b.cell.0.abs_diff(spot.cell.0) > min.x) {
                Some(Jump::Moved {
                    from: b.cell,
                    to: spot.cell,
                    shape: b.shape,
                    wide: b.wide,
                })
            } else {
                (!hop && b.shape != spot.shape).then_some(Jump::Reshaped)
            },
        )
    }

    /// Where the cursor settling at `now` on `rest`, last settled at
    /// `before`, draws from, as `startup` asks; and whether that is the one
    /// hop of a startup (`TrailStartup::Prompt`): a move across rows of
    /// output, never one along the line it started on (a bare prompt drawn
    /// where the cursor was) nor a pulse alone.
    /// None while the program isn't ready ("quiet" and "prompt"), and for a
    /// new screen's first prompt with "quiet", reached from wherever its
    /// startup left the cursor.
    fn drawn_from(&mut self, rest: Rest, now: Instant, before: Option<Spot>) -> Option<(Spot, bool)> {
        let started = match self.phase {
            Phase::Starting { from, .. } => Some(from),
            Phase::Ready { .. } => None,
        };
        let quiet = self.gate(rest, now, before);
        let ready = matches!(self.phase, Phase::Ready { .. }).then_some(started).flatten();
        match (self.startup, ready) {
            (TrailStartup::Every, _) => before.map(|b| (b, false)),
            // Ready at a prompt. Ready otherwise (full screen, or the startup
            // time up) it draws as "quiet" does, never less.
            (TrailStartup::Prompt, Some(from)) if rest == Rest::Prompt => from.or(before).map(|f| (f, true)),
            _ if quiet => None,
            _ => before.map(|b| (b, false)),
        }
    }

    /// Moves the program on as the cursor settles at `now` on `rest`, from
    /// `before`; true when that settle is no jump because the program wasn't
    /// ready: it is still starting, or this is a new screen's first prompt.
    fn gate(&mut self, rest: Rest, now: Instant, before: Option<Spot>) -> bool {
        let in_startup = |since: Option<Instant>| {
            since.is_none_or(|s| now.saturating_duration_since(s) < STARTUP)
        };
        let at_prompt = Phase::Ready { at_prompt: true };
        let (phase, quiet) = match (rest, self.phase) {
            (
                Rest::Prompt,
                Phase::Starting {
                    since,
                    new_screen: true,
                    ..
                },
            ) => (at_prompt, in_startup(since)),
            (Rest::Prompt, _) => (at_prompt, false),
            (Rest::FullScreen, _) => (Phase::Ready { at_prompt: false }, false),
            // Off the prompt it waited at: a command started.
            (Rest::Output, Phase::Ready { at_prompt: true }) => (
                Phase::Starting {
                    since: None,
                    new_screen: false,
                    from: before,
                },
                true,
            ),
            (Rest::Output, Phase::Starting { since, .. }) if !in_startup(since) => {
                (Phase::Ready { at_prompt: false }, false)
            }
            (Rest::Output, starting @ Phase::Starting { .. }) => (starting, true),
            (Rest::Output, ready) => (ready, false),
        };
        self.phase = phase;
        quiet
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Rest::{FullScreen, Output, Prompt};

    const SETTLE: Duration = Duration::from_millis(80);
    const V: View = (0, 80, 24, 1);

    fn at(x: u16, y: u16) -> Option<Spot> {
        Some(Spot {
            cell: (x, y),
            shape: CursorShape::Block,
            wide: false,
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
            j.due(ms(t0, 50), SETTLE, 2, out),
            Due::Wait(Duration::from_millis(35)),
            "not yet still long enough"
        );
        assert_eq!(
            j.due(ms(t0, 85), SETTLE, 2, out),
            Due::Fire(Some(Jump::Moved {
                from: (0, 0),
                to: (10, 3),
                shape: CursorShape::Block,
                wide: false
            }))
        );
        assert_eq!(
            j.due(ms(t0, 200), SETTLE, 2, out),
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
                    matches!(j.due(ms(t0, i * 16 + 1), SETTLE, 2, out), Due::Wait(_)),
                    "{i}"
                );
            }
        }
        assert_eq!(timers, 1, "one timer for the whole burst");
        assert_eq!(
            j.due(ms(t0, 960 + 80), SETTLE, 2, out),
            Due::Fire(Some(Jump::Moved {
                from: (0, 0),
                to: (20, 1),
                shape: CursorShape::Block,
                wide: false
            }))
        );
    }

    #[test]
    fn small_moves_settle_without_an_effect_and_shape_changes_pulse() {
        let (mut j, t0) = (Jumps::default(), Instant::now());
        j.observe(at(5, 5), V, t0);
        j.observe(at(6, 6), V, ms(t0, 1));
        assert_eq!(
            j.due(ms(t0, 100), SETTLE, 2, out),
            Due::Fire(None),
            "a diagonal step is one cell, under two"
        );
        j.observe(at(8, 6), V, ms(t0, 200));
        assert_eq!(
            j.due(ms(t0, 300), SETTLE, 2, out),
            Due::Fire(None),
            "two cells is the threshold itself: ignored, as kitty does"
        );
        j.observe(at(11, 6), V, ms(t0, 350));
        assert_eq!(
            j.due(ms(t0, 450), SETTLE, 2, out),
            Due::Fire(Some(Jump::Moved {
                from: (8, 6),
                to: (11, 6),
                shape: CursorShape::Block,
                wide: false
            })),
            "three from where it last settled: more than the threshold"
        );
        // Back to (8, 6), three cells, to try a shape change there.
        j.observe(at(8, 6), V, ms(t0, 460));
        assert_eq!(
            j.due(ms(t0, 560), SETTLE, 2, out),
            Due::Fire(Some(Jump::Moved {
                from: (11, 6),
                to: (8, 6),
                shape: CursorShape::Block,
                wide: false
            }))
        );
        j.observe(
            Some(Spot {
                cell: (8, 6),
                shape: CursorShape::Bar,
                wide: false,
            }),
            V,
            ms(t0, 400),
        );
        assert_eq!(
            j.due(ms(t0, 500), SETTLE, 2, out),
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
        assert_eq!(j.due(ms(t0, 200), SETTLE, 2, out), Due::Fire(None));
    }

    #[test]
    fn a_settle_of_zero_fires_at_the_first_wake_and_a_threshold_of_zero_draws_every_move() {
        let (mut j, t0) = (Jumps::default(), Instant::now());
        j.observe(at(0, 0), V, t0);
        j.observe(at(1, 0), V, t0);
        assert_eq!(
            j.due(t0, Duration::ZERO, 0, out),
            Due::Fire(Some(Jump::Moved {
                from: (0, 0),
                to: (1, 0),
                shape: CursorShape::Block,
                wide: false
            })),
            "one cell is more than 0"
        );
        j.observe(
            Some(Spot {
                cell: (1, 0),
                shape: CursorShape::Block,
                wide: false,
            }),
            V,
            t0,
        );
        assert_eq!(
            j.due(t0, Duration::ZERO, 0, out),
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
            j.due(ms(t0, 100), SETTLE, 2, out),
            Due::Fire(Some(Jump::Moved {
                from: (0, 0),
                to: (9, 9),
                shape: CursorShape::Block,
                wide: false
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
        assert_eq!(j.due(ms(t0, 200), SETTLE, 2, out), Due::Fire(None));
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
        let moves: Vec<_> = moves.iter().map(|&(t, x, y)| (t, x, y, Output)).collect();
        simulate(&mut Jumps::default(), &moves, frame, settle_ms, min_cells)
    }

    /// What the terminal reads under a cursor in output.
    fn out() -> Rest {
        Output
    }

    /// `run` through `j`, each move also saying what the cursor rests on there.
    fn simulate(
        j: &mut Jumps,
        moves: &[(u64, u16, u16, Rest)],
        frame: u64,
        settle_ms: u64,
        min_cells: u16,
    ) -> Vec<(u64, Jump)> {
        let (t0, settle) = (Instant::now(), Duration::from_millis(settle_ms));
        let end = moves.last().map_or(0, |m| m.0) + 1000;
        let (mut at_cell, mut rest, mut moved) = ((moves[0].1, moves[0].2), moves[0].3, 0usize);
        let (mut wake, mut fired) = (None::<u64>, Vec::new());
        // Millisecond by millisecond: the program's moves first, then a frame, then the timer.
        for t in 0..=end {
            while moved < moves.len() && moves[moved].0 == t {
                let cell = (moves[moved].1, moves[moved].2);
                // As the output path: only a chunk that moved the cursor counts.
                if moved > 0 && cell != at_cell {
                    j.program_moved(ms(t0, t));
                }
                (at_cell, rest) = (cell, moves[moved].3);
                moved += 1;
            }
            if t % frame == 0 && j.observe(at(at_cell.0, at_cell.1), V, ms(t0, t)) {
                wake = Some(t + settle_ms);
            }
            if wake == Some(t) {
                wake = None;
                match j.due(ms(t0, t), settle, min_cells, || rest) {
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
                    to: (0, 6),
                    shape: CursorShape::Block,
                    wide: false
                }]
            );
            assert_eq!(
                jump((40, 20), (0, 0))
                    .iter()
                    .map(|f| f.1)
                    .collect::<Vec<_>>(),
                vec![Jump::Moved {
                    from: (40, 20),
                    to: (0, 0),
                    shape: CursorShape::Block,
                    wide: false
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
                    to: (0, 8),
                    shape: CursorShape::Block,
                    wide: false
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
            match j.due(ms(t0, t), Duration::ZERO, 2, out) {
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
                to: (9, 0),
                shape: CursorShape::Block,
                wide: false
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
            if let Due::Fire(jump) = j.due(ms(t0, t + 1), Duration::from_millis(10), 2, out) {
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
            j.due(ms(t0, 15), settle, 2, out),
            Due::Wait(CATCH_UP_STEP),
            "still long enough, but drawn stale"
        );
        // The frame comes: drawn at (20, 0).
        assert!(!j.observe(at(20, 0), V, ms(t0, 16)));
        assert_eq!(
            j.due(ms(t0, 25), settle, 2, out),
            Due::Fire(Some(Jump::Moved {
                from: (0, 0),
                to: (20, 0),
                shape: CursorShape::Block,
                wide: false
            }))
        );
        // No frame ever comes (a hidden window): it settles on what was drawn.
        j.program_moved(ms(t0, 30));
        assert!(j.observe(at(30, 0), V, ms(t0, 30)));
        j.program_moved(ms(t0, 35));
        let mut t = 45;
        let fired = loop {
            match j.due(ms(t0, t), settle, 2, out) {
                Due::Wait(more) => t += more.as_millis() as u64,
                Due::Fire(jump) => break jump,
            }
        };
        assert!(t <= 30 + 10 + 100 + 10, "gave up waiting by {t}");
        assert_eq!(
            fired,
            Some(Jump::Moved {
                from: (20, 0),
                to: (30, 0),
                shape: CursorShape::Block,
                wide: false
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
            j.due(ms(t0, 10), Duration::from_millis(10), 2, out),
            Due::Wait(Duration::from_millis(8))
        );
    }

    #[test]
    fn a_move_counts_when_it_passes_either_threshold_and_one_number_is_both() {
        let jump = |to: Cell, min: MinCells| {
            let (mut j, t0) = (Jumps::default(), Instant::now());
            j.observe(at(10, 10), V, t0);
            j.observe(at(to.0, to.1), V, ms(t0, 1));
            matches!(
                j.due(ms(t0, 100), SETTLE, min, out),
                Due::Fire(Some(Jump::Moved { .. }))
            )
        };
        let (across, down) = (MinCells { x: 6, y: 2 }, MinCells { x: 2, y: 6 });
        assert!(
            !jump((16, 12), across),
            "six across and two down: within both"
        );
        assert!(jump((17, 10), across), "seven across: past the columns'");
        assert!(jump((10, 13), across), "three down: past the rows'");
        assert!(!jump((12, 16), down) && jump((13, 10), down));
        assert!(
            !jump((11, 11), MinCells::from(2)) && jump((13, 11), MinCells::from(2)),
            "one number for both, a diagonal step is one cell"
        );
    }

    fn moved(from: Cell, to: Cell) -> Jump {
        Jump::Moved {
            from,
            to,
            shape: CursorShape::Block,
            wide: false,
        }
    }

    /// `cursor_trail_startup: "quiet"`, what the startup tests below pin.
    fn quiet() -> Jumps {
        in_mode(TrailStartup::Quiet)
    }

    fn in_mode(startup: TrailStartup) -> Jumps {
        Jumps { startup, ..Jumps::default() }
    }

    fn started() -> Jumps {
        let mut j = quiet();
        j.started();
        j
    }

    fn started_in(startup: TrailStartup) -> Jumps {
        let mut j = in_mode(startup);
        j.started();
        j
    }

    /// A new screen's startup with banners and pauses past the settle time,
    /// then its marked prompt (each after the cursor stood at the top left).
    const BANNERS: [(u64, u16, u16, Rest); 5] = [
        (0, 0, 0, Output),
        (1000, 0, 10, Output),
        (1300, 0, 18, Output),
        (1380, 0, 19, Output),
        (1540, 2, 20, Prompt),
    ];

    #[test]
    fn each_startup_mode_draws_a_banner_and_prompt_startup_its_own_way() {
        let drawn = |mode| trails(&mut started_in(mode), &BANNERS).into_iter().map(|f| f.1).collect::<Vec<_>>();
        assert_eq!(drawn(TrailStartup::Quiet), vec![], "quiet: nothing");
        assert_eq!(drawn(TrailStartup::Prompt), vec![moved((0, 0), (2, 20))], "prompt: one, from where it started to the prompt");
        assert_eq!(
            drawn(TrailStartup::Every),
            vec![moved((0, 0), (0, 10)), moved((0, 10), (0, 18))],
            "every: each settle that went far enough (not the last row's, nor the prompt one row on)"
        );
        // However the output pauses, the prompt mode draws once: all at once too.
        let burst = [(0, 0, 0, Output), (1000, 2, 20, Prompt)];
        assert_eq!(trails(&mut started_in(TrailStartup::Prompt), &burst).iter().map(|f| f.1).collect::<Vec<_>>(), vec![moved((0, 0), (2, 20))]);
        // And jumps after it draw as ever.
        let mut moves = BANNERS.to_vec();
        moves.push((3000, 2, 1, Prompt));
        let after = trails(&mut started_in(TrailStartup::Prompt), &moves);
        assert_eq!(after.iter().map(|f| f.1).collect::<Vec<_>>(), vec![moved((0, 0), (2, 20)), moved((2, 20), (2, 1))]);
    }

    #[test]
    fn ready_without_a_prompt_the_prompt_mode_draws_as_quiet_does() {
        // `vim` from a new screen (far across, a shape change), and a program
        // that marks no prompts past the startup time.
        let vim = [(0, 0, 0, Output), (100, 40, 1, FullScreen), (400, 5, 3, FullScreen)];
        let unmarked = [(0, 0, 0, Output), (100, 2, 0, Output), (1500, 2, 10, Output), (3200, 30, 10, Output)];
        for moves in [&vim[..], &unmarked[..]] {
            let fired = trails(&mut started_in(TrailStartup::Prompt), moves);
            assert!(!fired.is_empty());
            assert_eq!(fired, trails(&mut started(), moves));
        }
        let (mut j, t0) = (started_in(TrailStartup::Prompt), Instant::now());
        j.observe(at(0, 0), V, t0);
        j.program_moved(ms(t0, 10));
        j.observe(Some(Spot { cell: (1, 1), shape: CursorShape::Bar, wide: false }), V, ms(t0, 10));
        assert_eq!(j.due(ms(t0, 100), SETTLE, 6, || FullScreen), Due::Fire(Some(Jump::Reshaped)), "a full-screen program's shape change pulses");
    }

    #[test]
    fn the_prompt_mode_draws_nothing_extra_for_a_bare_first_prompt() {
        // A prompt drawn where the cursor was, a long one included, and a
        // shape change at it: nothing, as quiet; the next jump draws.
        for prompt in [(2, 0), (40, 0), (2, 1)] {
            let moves = [(0, 0, 0, Output), (50, prompt.0, prompt.1, Prompt), (600, 2, 12, Prompt)];
            let fired = trails(&mut started_in(TrailStartup::Prompt), &moves);
            assert_eq!(fired, trails(&mut started(), &moves), "{prompt:?}");
            assert_eq!(fired.iter().map(|f| f.1).collect::<Vec<_>>(), vec![moved(prompt, (2, 12))]);
        }
        let (mut j, t0) = (started_in(TrailStartup::Prompt), Instant::now());
        j.observe(at(0, 0), V, t0);
        j.program_moved(ms(t0, 10));
        let beam = Spot { cell: (2, 0), shape: CursorShape::Bar, wide: false };
        j.observe(Some(beam), V, ms(t0, 10));
        assert_eq!(j.due(ms(t0, 100), SETTLE, 6, || Prompt), Due::Fire(None), "no pulse for the prompt's cursor shape");
    }

    #[test]
    fn the_prompt_mode_hops_once_from_the_prompt_a_command_left() {
        // `exec zsh` from the prompt at row 5: its output, then its prompt.
        let mut moves = AT_THE_PROMPT.to_vec();
        moves.extend([(2000, 0, 16, Output), (2300, 2, 18, Prompt), (3000, 2, 1, Prompt)]);
        let drawn = |j: &mut Jumps| trails(j, &moves).into_iter().map(|f| f.1).collect::<Vec<_>>();
        assert_eq!(drawn(&mut in_mode(TrailStartup::Prompt)), vec![moved((4, 5), (2, 18)), moved((2, 18), (2, 1))]);
        assert_eq!(drawn(&mut quiet()), vec![moved((2, 18), (2, 1))]);
        // Hiding the cursor while it runs keeps where it started.
        let (mut j, t0) = (in_mode(TrailStartup::Prompt), Instant::now());
        for (t, x, y, rest) in AT_THE_PROMPT {
            j.program_moved(ms(t0, t));
            j.observe(at(x, y), V, ms(t0, t));
            j.due(ms(t0, t + 100), SETTLE, 6, || rest);
        }
        j.observe(None, V, ms(t0, 1150));
        j.observe(at(0, 15), V, ms(t0, 1200));
        j.program_moved(ms(t0, 1300));
        j.observe(at(2, 20), V, ms(t0, 1300));
        assert_eq!(j.due(ms(t0, 1400), SETTLE, 6, || Prompt), Due::Fire(Some(moved((4, 5), (2, 20)))));
    }

    #[test]
    fn a_focus_loss_moves_where_the_prompt_mode_hops_from() {
        let (mut j, t0) = (started_in(TrailStartup::Prompt), Instant::now());
        j.observe(at(0, 0), V, t0);
        j.program_moved(ms(t0, 10));
        j.observe(at(0, 10), V, ms(t0, 10));
        assert_eq!(j.due(ms(t0, 100), SETTLE, 6, out), Due::Fire(None));
        // Away and back (the focus trail lands at row 10): from there.
        j.lose_place();
        j.observe(at(0, 10), V, ms(t0, 200));
        j.program_moved(ms(t0, 300));
        j.observe(at(2, 20), V, ms(t0, 300));
        assert_eq!(j.due(ms(t0, 400), SETTLE, 6, || Prompt), Due::Fire(Some(moved((0, 10), (2, 20)))));
    }

    /// A trail's settings: 16 ms frames, a 20 ms settle, six cells ignored.
    fn trails(j: &mut Jumps, moves: &[(u64, u16, u16, Rest)]) -> Vec<(u64, Jump)> {
        simulate(j, moves, 16, 20, 6)
    }

    #[test]
    fn a_new_shells_output_before_its_first_prompt_draws_nothing_and_jumps_after_it_do() {
        // A new terminal (a split, a tab, the first at start), the cursor at
        // its top left: a block of output 10 rows down and a pause, 8 more rows
        // and a pause, a line, then the shell's prompt (marked), then `clear`.
        // However long the screen takes to show any output (a remote machine
        // still dialing), the startup counts from it.
        for first in [1000, 6000] {
            let fired = trails(
                &mut started(),
                &[
                    (0, 0, 0, Output),
                    (first, 0, 10, Output),
                    (first + 300, 0, 18, Output),
                    (first + 380, 0, 19, Output),
                    (first + 540, 2, 20, Prompt),
                    (first + 1500, 2, 1, Prompt),
                ],
            );
            assert_eq!(
                fired.iter().map(|f| f.1).collect::<Vec<_>>(),
                vec![moved((2, 20), (2, 1))],
                "only the jump at the prompt, from it ({first})"
            );
            assert!(
                fired[0].0 <= first + 1500 + 16 + 20,
                "and on time: {fired:?}"
            );
        }
    }

    #[test]
    fn a_bare_first_prompt_and_a_jump_after_it_draw_as_without_a_startup() {
        // No output before the prompt: the next jump draws, just as soon.
        let moves = [(0, 0, 0, Output), (50, 2, 0, Prompt), (600, 2, 12, Prompt)];
        let fired = trails(&mut started(), &moves);
        assert_eq!(fired, trails(&mut quiet(), &moves));
        assert_eq!(
            fired.iter().map(|f| f.1).collect::<Vec<_>>(),
            vec![moved((2, 0), (2, 12))]
        );
    }

    #[test]
    fn without_prompt_marks_jumps_draw_from_the_startup_time_after_the_first_output() {
        // `sh`, a REPL: nothing marks a prompt.
        let fired = trails(
            &mut started(),
            &[
                (0, 0, 0, Output),
                (100, 2, 0, Output),
                (1500, 2, 10, Output),
                (100 + 3100, 30, 10, Output),
            ],
        );
        assert_eq!(
            fired.iter().map(|f| f.1).collect::<Vec<_>>(),
            vec![moved((2, 10), (30, 10))]
        );
    }

    #[test]
    fn a_full_screen_program_draws_its_jumps_from_the_start() {
        // `htop` or `vim` as the pane's program: the alternate screen.
        let moves = [
            (0, 0, 0, Output),
            (100, 40, 20, FullScreen),
            (400, 5, 3, FullScreen),
        ];
        let fired = trails(&mut started(), &moves);
        assert_eq!(fired, trails(&mut quiet(), &moves));
        assert_eq!(fired.len(), 2);
    }

    /// At a marked prompt, two keys typed and Enter at 1000.
    const AT_THE_PROMPT: [(u64, u16, u16, Rest); 4] = [
        (0, 2, 5, Prompt),
        (100, 3, 5, Prompt),
        (200, 4, 5, Prompt),
        (1000, 0, 6, Output),
    ];

    #[test]
    fn a_program_run_from_the_prompt_starts_too() {
        // `exec zsh`: the new shell's output, then its marked prompt, then `clear`.
        let mut moves = AT_THE_PROMPT.to_vec();
        moves.extend([
            (2000, 0, 16, Output),
            (2300, 2, 18, Prompt),
            (3000, 2, 1, Prompt),
        ]);
        assert_eq!(
            trails(&mut quiet(), &moves)
                .iter()
                .map(|f| f.1)
                .collect::<Vec<_>>(),
            vec![moved((2, 18), (2, 1))]
        );
        // `ssh` to a host that marks no prompts, its banner 4 s after Enter (a
        // slow login): the banner and the remote prompt draw nothing; output
        // from the startup time after the banner does.
        let mut moves = AT_THE_PROMPT.to_vec();
        moves.extend([
            (5000, 0, 14, Output),
            (5500, 2, 15, Output),
            (7500, 2, 23, Output),
            (8500, 2, 5, Output),
        ]);
        assert_eq!(
            trails(&mut quiet(), &moves)
                .iter()
                .map(|f| f.1)
                .collect::<Vec<_>>(),
            vec![moved((2, 23), (2, 5))]
        );
    }

    #[test]
    fn a_command_lands_on_its_prompt_with_a_trail() {
        // `ls`: its output and the next prompt at once, or after a pause.
        for pause in [false, true] {
            let mut moves = AT_THE_PROMPT[..3].to_vec();
            if pause {
                moves.extend([(1000, 0, 15, Output), (1200, 2, 22, Prompt)]);
            } else {
                moves.push((1000, 2, 22, Prompt));
            }
            assert_eq!(
                trails(&mut quiet(), &moves).len(),
                1,
                "pause: {pause}"
            );
        }
    }

    #[test]
    fn a_new_screen_starts_again() {
        // A respawned shell or a reattach in the same terminal.
        let mut j = quiet();
        assert_eq!(
            trails(&mut j, &[(0, 0, 0, Prompt), (100, 0, 10, Prompt)]).len(),
            1
        );
        j.started();
        assert!(!j.knows(), "the old screen's cursor is forgotten");
        let fired = trails(
            &mut j,
            &[
                (0, 0, 0, Output),
                (500, 0, 12, Output),
                (1000, 2, 14, Prompt),
                (1500, 2, 1, Prompt),
            ],
        );
        assert_eq!(
            fired.iter().map(|f| f.1).collect::<Vec<_>>(),
            vec![moved((2, 14), (2, 1))]
        );
    }

    #[test]
    fn a_new_screens_first_prompt_comes_quietly_from_where_its_startup_left_the_cursor() {
        // Output and the prompt in one burst, never pausing: from the top
        // left to the prompt is no jump. `clear` after it is.
        let fired = trails(
            &mut started(),
            &[
                (0, 0, 0, Output),
                (1000, 0, 18, Output),
                (1005, 2, 20, Prompt),
                (2000, 2, 1, Prompt),
            ],
        );
        assert_eq!(
            fired.iter().map(|f| f.1).collect::<Vec<_>>(),
            vec![moved((2, 20), (2, 1))]
        );
        // A prompt that changes the cursor's shape: no pulse as it lands.
        let (mut j, t0) = (started(), Instant::now());
        j.observe(at(0, 0), V, t0);
        j.program_moved(ms(t0, 10));
        j.observe(at(0, 10), V, ms(t0, 10));
        assert_eq!(
            j.due(ms(t0, 100), SETTLE, 6, out),
            Due::Fire(None),
            "output: no jump"
        );
        let beam = Spot {
            cell: (2, 12),
            shape: CursorShape::Bar,
            wide: false,
        };
        j.program_moved(ms(t0, 200));
        j.observe(Some(beam), V, ms(t0, 200));
        assert_eq!(j.due(ms(t0, 300), SETTLE, 6, || Prompt), Due::Fire(None));
    }

    #[test]
    fn a_program_that_wrote_while_all_was_off_is_long_started_when_it_comes_on() {
        // Its startup counted from its first output, four seconds ago.
        let mut j = started();
        j.program_wrote(|| Instant::now() - Duration::from_secs(4));
        let fired = trails(&mut j, &[(0, 0, 0, Output), (100, 0, 10, Output)]);
        assert_eq!(
            fired.iter().map(|f| f.1).collect::<Vec<_>>(),
            vec![moved((0, 0), (0, 10))]
        );
    }
}
