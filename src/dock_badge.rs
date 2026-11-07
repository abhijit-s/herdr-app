//! The Dock badge (settings.json `dock_badge`): how many agents wait on the
//! person, the way Mail counts unread mail. It counts the rows the agents list
//! (defaults/mods/agents) shows as needing you or finished and not yet seen,
//! with that list's rules for sub-agents (`count_rows`). The window counts on
//! every publish; the Dock is told only when the label changes, so the badge
//! never costs a redraw.

/// Whether a status from `Model::status` waits on the person.
pub fn is_waiting(status: &str) -> bool {
    matches!(status, "blocked" | "done")
}

/// An agent from `agents()`: a row of the agents list, unless a tree nests it.
pub struct Listed<'a> {
    pub address: &'a str,
    pub status: &'a str,
    pub pinned: bool,
}

/// A live terminal from `terminals()`: who started it and its own word.
pub struct Live<'a> {
    pub address: &'a str,
    pub parent: Option<&'a str>,
    pub word: Option<&'a str>,
}

/// How many rows of the agents list wait on the person, by its rules: a
/// sub-agent (in `agents()` when it has a pane) gets no row of its own under
/// a listed agent unless pinned, and a row waits when its agent does or when
/// a live sub-agent below it is blocked or hit an error. A sub-agent's
/// finish isn't news: it reports to its parent (`kidStatus` in the mod).
pub fn count_rows(listed: &[Listed], live: &[Live]) -> usize {
    let parent = |addr: &str| live.iter().find(|t| t.address == addr).and_then(|t| t.parent);
    let below = |kid: &str, top: &str| {
        let mut cur = kid;
        for _ in 0..32 {
            match parent(cur) {
                Some(p) if p == top => return true,
                Some(p) => cur = p,
                None => return false,
            }
        }
        false
    };
    let status_of = |addr: &str| listed.iter().find(|a| a.address == addr).map(|a| a.status);
    let kid_blocked = |t: &Live| matches!(status_of(t.address).or(t.word), Some("blocked" | "error"));
    listed
        .iter()
        .filter(|a| a.pinned || !listed.iter().any(|top| top.address != a.address && below(a.address, top.address)))
        .filter(|a| is_waiting(a.status) || live.iter().any(|t| below(t.address, a.address) && kid_blocked(t)))
        .count()
}

/// The Dock's label: the count, or empty (no badge) for none or when off.
pub fn label(waiting: usize, enabled: bool) -> String {
    if enabled && waiting > 0 { waiting.to_string() } else { String::new() }
}

/// What the Dock shows, so it is told only about changes.
#[derive(Default)]
pub struct Badge {
    waiting: usize,
    shown: String,
}

/// What an update asks of the Dock.
#[derive(Debug, Default, PartialEq)]
pub struct Change {
    /// The label to show, when it differs from the one shown.
    pub label: Option<String>,
    /// Something waits now where nothing did (`dock_bounce_on_wait`).
    pub rose: bool,
}

impl Badge {
    pub fn waiting(&self) -> usize {
        self.waiting
    }

    /// The label on the Dock now; empty: no badge.
    #[cfg(test)]
    pub fn shown(&self) -> &str {
        &self.shown
    }

    pub fn update(&mut self, waiting: usize, enabled: bool) -> Change {
        let rose = self.waiting == 0 && waiting > 0;
        self.waiting = waiting;
        let label = label(waiting, enabled);
        if label == self.shown {
            return Change { label: None, rose };
        }
        self.shown = label.clone();
        Change { label: Some(label), rose }
    }
}

/// Puts the label on the Dock icon; empty takes the badge off. Only on the
/// main thread of a macOS build; anywhere else (tests) this does nothing.
#[cfg(all(target_os = "macos", not(test)))]
pub fn show(label: &str) {
    let Some(mtm) = objc2::MainThreadMarker::new() else { return };
    let text = (!label.is_empty()).then(|| objc2_foundation::NSString::from_str(label));
    let tile = objc2_app_kit::NSApplication::sharedApplication(mtm).dockTile();
    tile.setBadgeLabel(text.as_deref());
    tracing::debug!(badge = %tile.badgeLabel().map(|l| l.to_string()).unwrap_or_default(), "dock badge");
}

#[cfg(not(all(target_os = "macos", not(test))))]
pub fn show(_label: &str) {}

/// One informational Dock bounce. AppKit ignores it while Herdr is the app in front.
#[cfg(all(target_os = "macos", not(test)))]
pub fn bounce() {
    let Some(mtm) = objc2::MainThreadMarker::new() else { return };
    objc2_app_kit::NSApplication::sharedApplication(mtm).requestUserAttention(objc2_app_kit::NSRequestUserAttentionType::InformationalRequest);
}

#[cfg(not(all(target_os = "macos", not(test))))]
pub fn bounce() {}

#[cfg(test)]
mod tests {
    use super::*;

    fn count<'a>(statuses: impl IntoIterator<Item = &'a str>) -> usize {
        statuses.into_iter().filter(|s| is_waiting(s)).count()
    }

    #[test]
    fn counts_agents_that_need_you_or_finished_unseen() {
        // Sync with `Model::status`: every word it can return is classified here.
        for (status, waits) in [("blocked", true), ("done", true), ("working", false), ("idle", false)] {
            assert_eq!(is_waiting(status), waits, "{status}");
        }
    }

    #[test]
    fn counts_what_the_model_shows() {
        let mut m = crate::model::Model::default();
        m.set_word("t1", Some("blocked".into()), None, false);
        m.set_word("t2", Some("working".into()), None, false);
        m.set_word("t2", Some("idle".into()), None, false);
        m.set_word("t3", Some("working".into()), None, false);
        m.set_word("t4", Some("idle".into()), None, false);
        m.set_word("t5", Some("error".into()), None, false);
        let statuses: Vec<_> = ["t1", "t2", "t3", "t4", "t5"].iter().filter_map(|a| m.status(a)).collect();
        assert_eq!(statuses, ["blocked", "done", "working", "idle", "blocked"]);
        assert_eq!(count(statuses), 3, "blocked, finished unseen and error wait; working and seen idle don't");
        m.mark_seen(&["t2".to_owned()]);
        assert_eq!(count(["t1", "t2", "t3", "t4", "t5"].iter().filter_map(|a| m.status(a))), 2, "looking at a finished agent stops it waiting");
    }

    #[test]
    fn the_settings_default_to_a_badge_and_no_bounce() {
        use crate::settings::{Person, Settings};
        let unset: Person = serde_json::from_str("{}").unwrap();
        assert!(unset.dock_badge && !unset.dock_bounce_on_wait);
        let json = serde_json::to_value(&unset).unwrap();
        assert!(json.get("dock_badge").is_none() && json.get("dock_bounce_on_wait").is_none(), "defaults stay out of settings.json");
        let set: Person = serde_json::from_value(serde_json::json!({ "dock_badge": false, "dock_bounce_on_wait": true })).unwrap();
        let json = serde_json::to_value(&set).unwrap();
        assert_eq!((json["dock_badge"].clone(), json["dock_bounce_on_wait"].clone()), (serde_json::json!(false), serde_json::json!(true)));
        let s = Settings::compose(&set, &crate::layout::Look::default(), true);
        assert!(!s.dock_badge && s.dock_bounce_on_wait);
        assert!(Settings::default().dock_badge && !Settings::default().dock_bounce_on_wait);
        assert!(serde_json::from_value::<Person>(serde_json::json!({ "dock_badge": "yes" })).is_err(), "not a boolean");
    }

    #[test]
    fn counts_rows_as_the_agents_list_shows_them() {
        let row = |address, status, pinned| Listed { address, status, pinned };
        let live = |address, parent, word| Live { address, parent, word };
        // t1 waits on its own; t2 works but its pane-less sub-agent t3 is blocked;
        // t4's sub-agent t5 has a pane (so it's listed) and finished: nested, no news.
        let listed = [row("t1", "blocked", false), row("t2", "working", false), row("t4", "idle", false), row("t5", "done", false)];
        let all = [
            live("t1", None, Some("blocked")),
            live("t2", None, Some("working")),
            live("t3", Some("t2"), Some("blocked")),
            live("t4", None, Some("idle")),
            live("t5", Some("t4"), Some("idle")),
        ];
        assert_eq!(count_rows(&listed, &all), 2, "t1, and t2 through t3");
        // Two levels down, an error counts as blocked.
        let deep = [live("t2", None, Some("working")), live("t6", Some("t2"), Some("working")), live("t7", Some("t6"), Some("error"))];
        assert_eq!(count_rows(&[row("t2", "working", false)], &deep), 1);
        // A pinned sub-agent has its own row.
        let pinned = [row("t4", "idle", false), row("t5", "done", true)];
        assert_eq!(count_rows(&pinned, &all), 1);
        // Waiting parent and blocked sub-agent: one row, one count.
        assert_eq!(count_rows(&[row("t2", "blocked", false)], &all), 1);
        // A sub-agent whose parent isn't listed is a row of its own.
        assert_eq!(count_rows(&[row("t5", "done", false)], &all), 1);
        // A parent loop doesn't hang.
        let looped = [live("a", Some("b"), Some("blocked")), live("b", Some("a"), Some("idle"))];
        assert_eq!(count_rows(&[row("c", "idle", false)], &looped), 0);
        assert_eq!(count_rows(&[], &all), 0);
    }

    #[test]
    fn labels_the_count_and_clears_for_none_or_off() {
        assert_eq!(label(3, true), "3");
        assert_eq!(label(0, true), "");
        assert_eq!(label(3, false), "");
    }

    #[test]
    fn tells_the_dock_only_when_the_label_changes() {
        let mut b = Badge::default();
        assert_eq!(b.update(0, true), Change::default(), "nothing waits at start: the Dock already shows no badge");
        assert_eq!(b.update(2, true), Change { label: Some("2".into()), rose: true });
        assert_eq!(b.update(2, true), Change::default());
        assert_eq!(b.update(1, true), Change { label: Some("1".into()), rose: false });
        assert_eq!(b.update(1, false), Change { label: Some(String::new()), rose: false }, "turning it off clears the badge");
        assert_eq!(b.update(4, false), Change::default(), "off: counts change, the Dock doesn't");
        assert_eq!(b.waiting(), 4);
        assert_eq!(b.update(4, true), Change { label: Some("4".into()), rose: false }, "turning it on shows the count");
        assert_eq!(b.update(0, true), Change { label: Some(String::new()), rose: false });
        assert_eq!(b.update(1, false), Change { label: None, rose: true }, "rising from none is news even with the badge off");
    }
}
