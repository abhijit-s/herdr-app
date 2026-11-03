//! Focus follows mouse (`focus_follows_mouse` in settings.json): the pointer
//! resting on a terminal pane for `focus_follows_mouse_delay_ms` focuses it
//! through `pane.focus`, as a click would. Every real move starts the wait over,
//! so a pane the pointer only sweeps across keeps out of it; at 0 ms the first
//! move onto a pane focuses it. Content moving under a still pointer, or a
//! jitter under 1 px, is not a move. After a key, or a focus change made some
//! other way, the pointer must first go more than 4 px from where it was. The
//! rules are `Follow`, kept free of the window so they are tested on their own.

use super::*;

/// Moves shorter than this (px) are the hand resting, not moving.
const JITTER: f32 = 1.0;
/// How far (px) the pointer must go after a key before it may move the focus.
const AFTER_KEYS: f32 = 4.0;

fn distance(a: Point<Pixels>, b: Point<Pixels>) -> f32 {
    let (dx, dy) = (f32::from(a.x - b.x), f32::from(a.y - b.y));
    (dx * dx + dy * dy).sqrt()
}

/// What must hold for the pointer to move the focus, read when it moves and
/// again when the wait ends.
#[derive(Clone, Copy, Debug)]
pub(super) struct Gate {
    pub enabled: bool,
    pub window_active: bool,
    pub button_held: bool,
    /// A dialog or popup is open (they hold the keyboard); toasts and windows don't count.
    pub modal_open: bool,
}

impl Gate {
    fn open(&self) -> bool {
        self.enabled && self.window_active && !self.button_held && !self.modal_open
    }
}

/// The pane to focus once the wait ends, and the focus it started from.
#[derive(Clone, Debug, PartialEq)]
struct Pending {
    id: u64,
    pane: String,
    focused: Option<String>,
}

/// What a pointer move asks of the window's timer.
#[derive(Debug, PartialEq)]
pub(super) enum Step {
    /// Leave the timer as it is.
    Keep,
    /// Drop the waiting timer.
    Cancel,
    /// Start a timer for this wait (replacing any other); `Follow::fire` with its id when it ends.
    Wait(u64),
}

#[derive(Default)]
pub(super) struct Follow {
    /// Where the pointer last really moved to.
    last: Option<Point<Pixels>>,
    /// Where it was at the last key or outside focus change, until it goes `AFTER_KEYS` from there.
    held: Option<Point<Pixels>>,
    /// The focused pane at the last move, to see it change some other way.
    seen: Option<Option<String>>,
    pending: Option<Pending>,
    next_id: u64,
}

impl Follow {
    /// Whether a move to `at` is too small to count (checked before looking
    /// for the pane under it).
    pub fn still(&self, at: Point<Pixels>) -> bool {
        self.last.is_some_and(|l| distance(l, at) < JITTER)
    }

    /// The pointer moved to `at`, over the terminal pane `under` (none: anything
    /// else), with `focused` the focused pane now.
    pub fn moved(&mut self, gate: Gate, at: Point<Pixels>, under: Option<&str>, focused: Option<&str>) -> Step {
        if self.still(at) {
            return Step::Keep;
        }
        let before = self.last.replace(at);
        // The focus moved some other way since (a key bound in the menu, a click, a mod):
        // like a key, the pointer must leave where it was first.
        if self.seen.as_ref().is_some_and(|s| s.as_deref() != focused) {
            self.held = self.held.or(before);
            self.seen = Some(focused.map(str::to_owned));
        } else if self.seen.is_none() {
            self.seen = Some(focused.map(str::to_owned));
        }
        if let Some(h) = self.held {
            if distance(h, at) <= AFTER_KEYS {
                return self.cancel();
            }
            self.held = None;
        }
        let Some(pane) = under.filter(|p| gate.open() && Some(*p) != focused) else {
            return self.cancel();
        };
        // Every move starts the wait over: only resting counts.
        self.next_id += 1;
        match &mut self.pending {
            Some(p) if p.pane == pane && p.focused.as_deref() == focused => p.id = self.next_id,
            _ => self.pending = Some(Pending { id: self.next_id, pane: pane.to_owned(), focused: focused.map(str::to_owned) }),
        }
        Step::Wait(self.next_id)
    }

    /// A key was pressed with the pointer at `at`: no wait until it moves more
    /// than `AFTER_KEYS` from there.
    pub fn keys(&mut self, at: Point<Pixels>) -> Step {
        self.held = Some(at);
        self.cancel()
    }

    /// The wait `id` ended: the pane to focus, unless the pointer left it, the
    /// focus moved some other way meanwhile, or the gate closed.
    pub fn fire(&mut self, id: u64, gate: Gate, under: Option<&str>, focused: Option<&str>) -> Option<String> {
        if self.pending.as_ref().is_none_or(|p| p.id != id) {
            return None;
        }
        let p = self.pending.take()?;
        let ok = gate.open() && under == Some(p.pane.as_str()) && focused == p.focused.as_deref() && focused != Some(p.pane.as_str());
        if ok {
            self.seen = Some(Some(p.pane.clone()));
        }
        ok.then_some(p.pane)
    }

    /// Forgets the wait, if any (a press, the setting turned off).
    pub fn cancel(&mut self) -> Step {
        if self.pending.take().is_some() { Step::Cancel } else { Step::Keep }
    }
}

impl AppWindow {
    /// Every pointer move over the window (its root's listener).
    pub(super) fn follow_mouse(&mut self, ev: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Off: no per-move work.
        if !self.person.focus_follows_mouse {
            return self.follow_mouse_cancel();
        }
        // The same place again (content moving under a still pointer) or a jitter: before any search.
        if self.follow.still(ev.position) {
            return;
        }
        let gate = self.follow_gate(ev.pressed_button.is_some(), window);
        let under = if gate.open() { self.terminal_pane_at(ev.position) } else { None };
        let focused = self.focused_pane.clone();
        match self.follow.moved(gate, ev.position, under.as_deref(), focused.as_deref()) {
            Step::Keep => {}
            Step::Cancel => self.follow_timer = None,
            Step::Wait(id) => {
                let delay = self.person.focus_follows_mouse_delay();
                if delay.is_zero() {
                    self.follow_timer = None;
                    return self.follow_mouse_fire(id, ev.position, window, cx);
                }
                self.follow_timer = Some(cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(delay).await;
                    let _ = this.update_in(cx, |this, window, cx| {
                        let at = window.mouse_position();
                        this.follow_mouse_fire(id, at, window, cx)
                    });
                }));
            }
        }
    }

    /// A press anywhere ends the wait.
    pub(super) fn follow_mouse_cancel(&mut self) {
        if self.follow.cancel() == Step::Cancel {
            self.follow_timer = None;
        }
    }

    /// A key down anywhere: the keyboard has the focus until the pointer really
    /// moves. Held while off too, so a key that turns it on holds as well.
    pub(super) fn follow_mouse_keys(&mut self, window: &Window) {
        if self.follow.keys(window.mouse_position()) == Step::Cancel {
            self.follow_timer = None;
        }
    }

    /// The wait `id` ended (or, at 0 ms, never began) with the pointer at `at`.
    fn follow_mouse_fire(&mut self, id: u64, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        self.follow_timer = None;
        // A press since the wait began already cancelled it (`follow_mouse_cancel`).
        let gate = self.follow_gate(false, window);
        let under = self.terminal_pane_at(at);
        let focused = self.focused_pane.clone();
        let Some(pane) = self.follow.fire(id, gate, under.as_deref(), focused.as_deref()) else { return };
        // The way a click or key focuses it: a tab's pane, or a layout's terminal block.
        let args = if self.model.tab_of(&pane).is_some() {
            json!({ "pane": pane })
        } else {
            match terminal_block(&self.chrome, Some(&pane)).and_then(Node::terminal) {
                Some(terminal) => json!({ "terminal": terminal }),
                None => return,
            }
        };
        self.run_as("pane.focus", args, false, window, cx);
    }

    pub(super) fn follow_gate(&self, button_held: bool, window: &Window) -> Gate {
        Gate {
            enabled: self.person.focus_follows_mouse,
            window_active: window.is_window_active(),
            button_held: button_held || self.drag.is_some() || self.pane_drag.is_some() || self.float_drag.is_some() || self.win_drag.is_some(),
            // Toasts only tell and windows stay open beside the work; a window's
            // own bounds are left out in `terminal_pane_at`.
            modal_open: self.overlays.iter().any(|o| o.kind.modal_keys()),
        }
    }

    /// The shown terminal pane (the active tab's, or a layout's terminal block)
    /// whose painted bounds hold `at`, unless a floating block or a window
    /// overlay is over it there. Only shown blocks count: bounds stay behind for
    /// hidden ones. Overlay and agent-view terminals are not panes.
    pub(super) fn terminal_pane_at(&self, at: Point<Pixels>) -> Option<String> {
        let bounds = self.bounds.borrow();
        let holds = |slot: &str| bounds.get(slot).is_some_and(|b| b.contains(&at));
        let tab = self.model.active_tab().filter(|_| self.shows_tab()).map(|t| t.shown());
        let pane = tab.into_iter().flat_map(|t| t.visible_blocks()).chain(self.chrome.visible_blocks()).filter(|n| n.terminal().is_some()).find(|n| holds(&n.id))?;
        let mut over = tab.into_iter().chain([&self.chrome]).flat_map(visible_nodes).flat_map(|n| n.floating.iter().filter(|f| !f.hidden));
        let mut windows = self.overlays.iter().filter(|o| o.kind == OverlayKind::Window);
        if over.any(|f| holds(&floats::slot(&f.id))) || windows.any(|o| holds(&o.slot)) {
            return None;
        }
        Some(pane.id.clone())
    }

    /// `focus.toggle_follows_mouse`: flips the setting in settings.json and says so.
    /// Unsaved, it warns, and a script or agent (`person: false`) gets an error.
    pub(super) fn toggle_follows_mouse(&mut self, person: bool, window: &mut Window, cx: &mut Context<Self>) {
        let saved = self.write_person(toggle_follows, window, cx);
        let on = self.person.focus_follows_mouse;
        if !on {
            self.follow_mouse_cancel();
        }
        let title = follows_mouse_toast(on, saved);
        let level = if saved == PersonSaved::Saved { "info" } else { "warning" };
        self.open_overlay(mods::new_overlay_id(), json!({ "kind": "toast", "notification": { "title": title, "level": level } }), window, cx);
        if saved != PersonSaved::Saved && !person {
            self.fail(title);
        }
    }
}

/// The toggle's change to settings.json.
fn toggle_follows(p: &mut settings::Person) {
    p.focus_follows_mouse = !p.focus_follows_mouse;
}

/// What the toggle says: the new state, and why it wasn't saved when it wasn't.
fn follows_mouse_toast(on: bool, saved: PersonSaved) -> String {
    let state = if on { "on" } else { "off" };
    match saved {
        PersonSaved::Saved => format!("Focus follows mouse: {state}"),
        PersonSaved::Broken => format!("Focus follows mouse: {state} (not saved: settings.json has errors)"),
        PersonSaved::WriteFailed => format!("Focus follows mouse: {state} (not saved: cannot write settings.json)"),
    }
}

#[cfg(test)]
mod tests {
    use super::super::PersonSaved;
    use super::{Follow, Gate, Step, follows_mouse_toast, toggle_follows};
    use gpui::{Pixels, Point, point, px};

    const OPEN: Gate = Gate { enabled: true, window_active: true, button_held: false, modal_open: false };

    fn at(x: f32) -> Point<Pixels> {
        point(px(x), px(10.0))
    }

    /// A wait for `pane` armed by a move to x, with `a` focused.
    fn armed(f: &mut Follow, x: f32, pane: &str) -> u64 {
        match f.moved(OPEN, at(x), Some(pane), Some("a")) {
            Step::Wait(id) => id,
            other => panic!("no wait for {pane}: {other:?}"),
        }
    }

    #[test]
    fn resting_on_another_pane_focuses_it_when_the_wait_ends() {
        let mut f = Follow::default();
        let id = armed(&mut f, 1.0, "b");
        assert_eq!(f.fire(id, OPEN, Some("b"), Some("a")), Some("b".into()));
        // Once: the same wait never fires twice.
        assert_eq!(f.fire(id, OPEN, Some("b"), Some("a")), None);
    }

    #[test]
    fn moving_within_the_pane_starts_the_wait_over() {
        let mut f = Follow::default();
        let first = armed(&mut f, 1.0, "b");
        let second = armed(&mut f, 3.0, "b");
        assert_ne!(first, second);
        assert_eq!(f.fire(first, OPEN, Some("b"), Some("a")), None, "the wait counted from entering the pane");
        assert_eq!(f.fire(second, OPEN, Some("b"), Some("a")), Some("b".into()));
    }

    #[test]
    fn sweeping_across_panes_focuses_none_of_them() {
        let mut f = Follow::default();
        // On the way from a, across b and c, back to a: every wait was overtaken.
        let b = armed(&mut f, 1.0, "b");
        let c = armed(&mut f, 5.0, "c");
        assert_eq!(f.moved(OPEN, at(9.0), Some("a"), Some("a")), Step::Cancel);
        assert_eq!(f.fire(b, OPEN, Some("a"), Some("a")), None);
        assert_eq!(f.fire(c, OPEN, Some("a"), Some("a")), None);
        // Swept across b to where it rests on c: c, once it rests.
        let b = armed(&mut f, 12.0, "b");
        let c = armed(&mut f, 16.0, "c");
        assert_eq!(f.fire(b, OPEN, Some("c"), Some("a")), None);
        assert_eq!(f.fire(c, OPEN, Some("c"), Some("a")), Some("c".into()));
    }

    #[test]
    fn a_jitter_under_a_pixel_neither_starts_nor_delays_a_wait() {
        let mut f = Follow::default();
        assert_eq!(f.moved(OPEN, at(1.0), Some("a"), Some("a")), Step::Keep);
        // A tremble onto b doesn't start one.
        assert!(f.still(at(1.6)));
        assert_eq!(f.moved(OPEN, at(1.6), Some("b"), Some("a")), Step::Keep);
        // Resting on b, a tremble doesn't put its wait off.
        let id = armed(&mut f, 3.0, "b");
        assert_eq!(f.moved(OPEN, at(3.5), Some("b"), Some("a")), Step::Keep);
        assert_eq!(f.moved(OPEN, point(px(3.0), px(10.7)), Some("b"), Some("a")), Step::Keep);
        assert_eq!(f.fire(id, OPEN, Some("b"), Some("a")), Some("b".into()));
    }

    #[test]
    fn after_a_key_the_pointer_must_go_more_than_4_px_first() {
        let mut f = Follow::default();
        let id = armed(&mut f, 1.0, "b");
        // A key with the pointer at 1: b's wait ends there.
        assert_eq!(f.keys(at(1.0)), Step::Cancel);
        assert_eq!(f.fire(id, OPEN, Some("b"), Some("a")), None);
        // Up to 4 px from there: still the keyboard's.
        assert_eq!(f.moved(OPEN, at(3.0), Some("b"), Some("a")), Step::Keep);
        assert_eq!(f.moved(OPEN, at(5.0), Some("b"), Some("a")), Step::Keep);
        assert_eq!(f.moved(OPEN, point(px(1.0), px(14.0)), Some("b"), Some("a")), Step::Keep);
        // Past it, in any direction: the pointer's again.
        let id = match f.moved(OPEN, at(5.5), Some("b"), Some("a")) {
            Step::Wait(id) => id,
            other => panic!("{other:?}"),
        };
        assert_eq!(f.fire(id, OPEN, Some("b"), Some("a")), Some("b".into()));
        // And stays so, coming back near where the key was.
        assert!(matches!(f.moved(OPEN, at(2.0), Some("c"), Some("b")), Step::Wait(_)));
    }

    #[test]
    fn a_focus_change_made_some_other_way_holds_the_pointer_like_a_key() {
        let mut f = Follow::default();
        assert_eq!(f.moved(OPEN, at(1.0), Some("b"), Some("b")), Step::Keep);
        // A key bound in the menu (no key event here) focuses c; the pointer twitches on b.
        assert_eq!(f.moved(OPEN, at(3.0), Some("b"), Some("c")), Step::Keep);
        assert_eq!(f.moved(OPEN, at(5.0), Some("b"), Some("c")), Step::Keep);
        assert!(matches!(f.moved(OPEN, at(6.0), Some("b"), Some("c")), Step::Wait(_)));
    }

    #[test]
    fn the_pane_already_focused_waits_for_nothing() {
        let mut f = Follow::default();
        assert_eq!(f.moved(OPEN, at(1.0), Some("a"), Some("a")), Step::Keep);
        // Back onto the focused pane before the wait ends: cancelled.
        armed(&mut f, 2.0, "b");
        assert_eq!(f.moved(OPEN, at(3.0), Some("a"), Some("a")), Step::Cancel);
    }

    #[test]
    fn leaving_for_something_that_is_not_a_terminal_pane_cancels() {
        let mut f = Follow::default();
        let id = armed(&mut f, 1.0, "b");
        assert_eq!(f.moved(OPEN, at(2.0), None, Some("a")), Step::Cancel);
        assert_eq!(f.fire(id, OPEN, Some("b"), Some("a")), None);
        // Nothing waiting: nothing to cancel.
        assert_eq!(f.moved(OPEN, at(3.0), None, Some("a")), Step::Keep);
    }

    #[test]
    fn moving_on_to_a_third_pane_replaces_the_wait() {
        let mut f = Follow::default();
        let first = armed(&mut f, 1.0, "b");
        let second = armed(&mut f, 2.0, "c");
        assert_ne!(first, second);
        assert_eq!(f.fire(first, OPEN, Some("c"), Some("a")), None, "a replaced timer that fires anyway does nothing");
        assert_eq!(f.fire(second, OPEN, Some("c"), Some("a")), Some("c".into()));
    }

    #[test]
    fn a_pointer_that_did_not_move_never_starts_a_wait() {
        let mut f = Follow::default();
        assert_eq!(f.moved(OPEN, at(1.0), Some("a"), Some("a")), Step::Keep);
        // A pane appears, the layout or a scroll moves b under the still pointer.
        assert_eq!(f.moved(OPEN, at(1.0), Some("b"), Some("a")), Step::Keep);
        assert!(matches!(f.moved(OPEN, at(2.5), Some("b"), Some("a")), Step::Wait(_)));
    }

    #[test]
    fn content_moving_away_under_a_still_pointer_ends_the_wait_unfired() {
        let mut f = Follow::default();
        let id = armed(&mut f, 1.0, "b");
        assert_eq!(f.moved(OPEN, at(1.0), Some("c"), Some("a")), Step::Keep);
        assert_eq!(f.fire(id, OPEN, Some("c"), Some("a")), None);
    }

    #[test]
    fn the_keyboard_wins_until_the_pointer_moves_again() {
        let mut f = Follow::default();
        // The keys move the focus to c during b's wait: b is not focused.
        let id = armed(&mut f, 1.0, "b");
        assert_eq!(f.fire(id, OPEN, Some("b"), Some("c")), None);
        // The pointer still on b: no move, no wait.
        assert_eq!(f.moved(OPEN, at(1.0), Some("b"), Some("c")), Step::Keep);
        // The next real move past the key's 4 px does.
        let id = match f.moved(OPEN, at(6.0), Some("b"), Some("c")) {
            Step::Wait(id) => id,
            other => panic!("{other:?}"),
        };
        assert_eq!(f.fire(id, OPEN, Some("b"), Some("c")), Some("b".into()));
    }

    #[test]
    fn each_closed_gate_holds_the_focus_and_cancels_the_wait() {
        let closed = [
            Gate { enabled: false, ..OPEN },
            Gate { window_active: false, ..OPEN },
            Gate { button_held: true, ..OPEN },
            Gate { modal_open: true, ..OPEN },
        ];
        for gate in closed {
            let mut f = Follow::default();
            assert_eq!(f.moved(gate, at(1.0), Some("b"), Some("a")), Step::Keep, "{gate:?}: started a wait");
            // Closing while waiting: the next move cancels it.
            let id = armed(&mut f, 2.0, "b");
            assert_eq!(f.moved(gate, at(3.0), Some("b"), Some("a")), Step::Cancel, "{gate:?}");
            assert_eq!(f.fire(id, OPEN, Some("b"), Some("a")), None, "{gate:?}");
            // Closing without a move: the wait ends unfired.
            let id = armed(&mut f, 4.0, "b");
            assert_eq!(f.fire(id, gate, Some("b"), Some("a")), None, "{gate:?}: fired");
        }
    }

    #[test]
    fn a_press_cancels_the_wait() {
        let mut f = Follow::default();
        let id = armed(&mut f, 1.0, "b");
        assert_eq!(f.cancel(), Step::Cancel);
        assert_eq!(f.cancel(), Step::Keep);
        assert_eq!(f.fire(id, OPEN, Some("b"), Some("a")), None);
    }

    #[test]
    fn the_pane_focused_some_other_way_meanwhile_is_left_alone() {
        let mut f = Follow::default();
        let id = armed(&mut f, 1.0, "b");
        // A click on b got there first.
        assert_eq!(f.fire(id, OPEN, Some("b"), Some("b")), None);
    }

    /// `focus.toggle_follows_mouse` without the window (its file watcher): the
    /// change it makes to the person's settings, saved or not, and what it says.
    #[test]
    fn the_toggle_flips_the_setting_and_says_whether_it_was_saved() {
        use crate::settings::Person;
        let mut p = Person { focus_follows_mouse: false, ..Person::default() };
        let mut written = None;
        let saved = super::super::change_person(&mut p, toggle_follows, false, |p| {
            written = Some(p.focus_follows_mouse);
            Ok(())
        });
        assert!(saved == PersonSaved::Saved && p.focus_follows_mouse);
        assert_eq!(written, Some(true));
        assert_eq!(follows_mouse_toast(p.focus_follows_mouse, saved), "Focus follows mouse: on");

        // settings.json mid-edit: on for now, left unwritten, and said so.
        let mut p = Person { focus_follows_mouse: false, ..Person::default() };
        let saved = super::super::change_person(&mut p, toggle_follows, true, |_| panic!("wrote a broken settings.json"));
        assert!(saved == PersonSaved::Broken && p.focus_follows_mouse);
        assert_eq!(follows_mouse_toast(p.focus_follows_mouse, saved), "Focus follows mouse: on (not saved: settings.json has errors)");

        // A failed write is not saved either, and says so; back off.
        let saved = super::super::change_person(&mut p, toggle_follows, false, |_| Err(std::io::Error::other("read-only")));
        assert!(saved == PersonSaved::WriteFailed && !p.focus_follows_mouse);
        assert_eq!(follows_mouse_toast(p.focus_follows_mouse, saved), "Focus follows mouse: off (not saved: cannot write settings.json)");
    }
}
