//! Focus follows mouse (`focus_follows_mouse` in settings.json): the pointer
//! resting on a terminal pane for `focus_follows_mouse_delay_ms` focuses it
//! through `pane.focus`, as a click would. Only a real pointer move starts the
//! wait, so content moving under a still pointer, or a keyboard focus change
//! made since, never moves the focus. The rules are `Follow`, kept free of the
//! window so they are tested on their own.

use super::*;

/// What must hold for the pointer to move the focus, read when it moves and
/// again when the wait ends.
#[derive(Clone, Copy, Debug)]
pub(super) struct Gate {
    pub enabled: bool,
    pub window_active: bool,
    pub button_held: bool,
    pub overlay_open: bool,
}

impl Gate {
    fn open(&self) -> bool {
        self.enabled && self.window_active && !self.button_held && !self.overlay_open
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
    last: Option<Point<Pixels>>,
    pending: Option<Pending>,
    next_id: u64,
}

impl Follow {
    /// The pointer moved to `at`, over the terminal pane `under` (none: anything
    /// else), with `focused` the focused pane now.
    pub fn moved(&mut self, gate: Gate, at: Point<Pixels>, under: Option<&str>, focused: Option<&str>) -> Step {
        // The same position again is the content moving, not the person.
        if self.last.replace(at) == Some(at) {
            return Step::Keep;
        }
        let Some(pane) = under.filter(|p| gate.open() && Some(*p) != focused) else {
            return self.cancel();
        };
        // Moving within the pane it waits for doesn't start the wait over.
        if self.pending.as_ref().is_some_and(|p| p.pane == pane) {
            return Step::Keep;
        }
        self.next_id += 1;
        self.pending = Some(Pending { id: self.next_id, pane: pane.to_owned(), focused: focused.map(str::to_owned) });
        Step::Wait(self.next_id)
    }

    /// The wait `id` ended: the pane to focus, unless the pointer left it, the
    /// focus moved some other way meanwhile, or the gate closed.
    pub fn fire(&mut self, id: u64, gate: Gate, under: Option<&str>, focused: Option<&str>) -> Option<String> {
        if self.pending.as_ref().is_none_or(|p| p.id != id) {
            return None;
        }
        let p = self.pending.take()?;
        (gate.open() && under == Some(p.pane.as_str()) && focused == p.focused.as_deref() && focused != Some(p.pane.as_str())).then_some(p.pane)
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
        let gate = self.follow_gate(ev.pressed_button.is_some(), window);
        let under = self.terminal_pane_under_pointer(cx);
        let focused = self.focused_pane.clone();
        match self.follow.moved(gate, ev.position, under.as_deref(), focused.as_deref()) {
            Step::Keep => {}
            Step::Cancel => self.follow_timer = None,
            Step::Wait(id) => {
                let delay = self.person.focus_follows_mouse_delay();
                self.follow_timer = Some(cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(delay).await;
                    let _ = this.update_in(cx, |this, window, cx| this.follow_mouse_fire(id, window, cx));
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

    fn follow_mouse_fire(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.follow_timer = None;
        // A press since the wait began already cancelled it (`follow_mouse_cancel`).
        let gate = self.follow_gate(false, window);
        let under = self.terminal_pane_under_pointer(cx);
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

    fn follow_gate(&self, button_held: bool, window: &Window) -> Gate {
        Gate {
            enabled: self.person.focus_follows_mouse,
            window_active: window.is_window_active(),
            button_held: button_held || self.drag.is_some() || self.pane_drag.is_some() || self.float_drag.is_some() || self.win_drag.is_some(),
            // Toasts only tell; everything else over the panes is being used.
            overlay_open: self.overlays.iter().any(|o| o.kind != OverlayKind::Toast),
        }
    }

    /// The shown terminal pane (a tab's, or a layout's terminal block) the
    /// pointer is over. Overlay and agent-view terminals are not panes.
    fn terminal_pane_under_pointer(&self, cx: &App) -> Option<String> {
        let tab = self.model.active_tab().map(|t| t.shown().visible_blocks()).unwrap_or_default();
        tab.into_iter()
            .chain(self.chrome.visible_blocks())
            .filter(|n| n.terminal().is_some())
            .map(|n| n.id.clone())
            .find(|id| self.pane_term(id).is_some_and(|t| t.view.read(cx).hovered()))
    }

    /// `focus.toggle_follows_mouse`: flips the setting in settings.json and says so.
    pub(super) fn toggle_follows_mouse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let on = !self.person.focus_follows_mouse;
        self.write_person(|p| p.focus_follows_mouse = on, window, cx);
        if !on {
            self.follow_mouse_cancel();
        }
        let title = format!("Focus follows mouse: {}", if on { "on" } else { "off" });
        self.open_overlay(mods::new_overlay_id(), json!({ "kind": "toast", "notification": { "title": title, "level": "info" } }), window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::{Follow, Gate, Step};
    use gpui::{Pixels, Point, point, px};

    const OPEN: Gate = Gate { enabled: true, window_active: true, button_held: false, overlay_open: false };

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
    fn moving_within_the_pane_keeps_the_wait_it_started() {
        let mut f = Follow::default();
        let id = armed(&mut f, 1.0, "b");
        assert_eq!(f.moved(OPEN, at(2.0), Some("b"), Some("a")), Step::Keep);
        assert_eq!(f.fire(id, OPEN, Some("b"), Some("a")), Some("b".into()));
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
        assert!(matches!(f.moved(OPEN, at(1.5), Some("b"), Some("a")), Step::Wait(_)));
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
        // The next real move does.
        let id = match f.moved(OPEN, at(1.5), Some("b"), Some("c")) {
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
            Gate { overlay_open: true, ..OPEN },
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
}
