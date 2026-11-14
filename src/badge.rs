//! The badge on the app's icon (the Dock on macOS): only the app's own process
//! can draw it. Two things ask for it: mods with `setBadge`, and the window's
//! count of waiting agents (`dock_badge`). This is its one writer: a mod's
//! badge shows over the count, and clearing it brings the count back. Other
//! platforms keep no badge for now; the calls are accepted and do nothing there.

use std::cell::RefCell;

thread_local! {
    static BADGE: RefCell<Badge> = RefCell::new(Badge::default());
}

/// What each side asks for, and what the icon shows.
#[derive(Default)]
struct Badge {
    from_mod: Option<String>,
    count: Option<String>,
    shown: Option<String>,
}

impl Badge {
    /// The label to put on the icon when it changes; `Some(None)` clears it.
    fn settle(&mut self) -> Option<Option<String>> {
        let wanted = self.from_mod.clone().or_else(|| self.count.clone());
        if wanted == self.shown {
            return None;
        }
        self.shown.clone_from(&wanted);
        Some(wanted)
    }
}

/// Blank text is no badge.
fn text(label: Option<&str>) -> Option<String> {
    label.map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned)
}

fn update(f: impl FnOnce(&mut Badge)) {
    let change = BADGE.with_borrow_mut(|b| {
        f(b);
        b.settle()
    });
    if let Some(label) = change {
        platform::set(label.as_deref());
    }
}

/// A mod's badge (`setBadge`): shown over the waiting count; `None` or blank
/// clears it and shows the count again. Runs on the UI thread.
pub fn set(label: Option<&str>) {
    update(|b| b.from_mod = text(label));
}

/// The waiting count's label (`dock_badge`); empty when nothing waits or it's off.
pub fn set_count(label: &str) {
    update(|b| b.count = text(Some(label)));
}

/// Only on the main thread of a macOS build; anywhere else (tests) nothing.
#[cfg(all(target_os = "macos", not(test)))]
mod platform {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    use objc2_foundation::NSString;

    pub fn set(text: Option<&str>) {
        let Some(mtm) = MainThreadMarker::new() else {
            tracing::warn!("app badge set off the main thread");
            return;
        };
        let tile = NSApplication::sharedApplication(mtm).dockTile();
        tile.setBadgeLabel(text.map(NSString::from_str).as_deref());
        tracing::debug!(badge = %text.unwrap_or_default(), "dock badge");
    }
}

#[cfg(not(all(target_os = "macos", not(test))))]
mod platform {
    pub fn set(_text: Option<&str>) {}
}

#[cfg(test)]
mod tests {
    use super::{Badge, text};

    fn mod_badge(b: &mut Badge, label: Option<&str>) -> Option<Option<String>> {
        b.from_mod = text(label);
        b.settle()
    }

    fn count(b: &mut Badge, label: &str) -> Option<Option<String>> {
        b.count = text(Some(label));
        b.settle()
    }

    #[test]
    fn a_mods_badge_shows_over_the_waiting_count_and_clearing_it_brings_the_count_back() {
        let mut b = Badge::default();
        let shows = |s: &str| Some(Some(s.to_owned()));
        assert_eq!(count(&mut b, "2"), shows("2"));
        assert_eq!(mod_badge(&mut b, Some("x")), shows("x"), "the mod's badge covers the count");
        assert_eq!(count(&mut b, "3"), None, "the count moves under it without touching the icon");
        assert_eq!(mod_badge(&mut b, Some("x")), None, "the same badge again changes nothing");
        assert_eq!(mod_badge(&mut b, Some("y")), shows("y"));
        assert_eq!(mod_badge(&mut b, None), shows("3"), "clearing it shows the count as it is now");
        assert_eq!(mod_badge(&mut b, Some("x")), shows("x"), "a badge set again after a clear shows again");
        assert_eq!(mod_badge(&mut b, Some("  ")), shows("3"), "blank is a clear");
        assert_eq!(count(&mut b, ""), Some(None), "no count and no mod badge: no badge");
        assert_eq!(mod_badge(&mut b, Some("")), None);
        assert_eq!(mod_badge(&mut b, Some(" 5 ")), shows("5"));
        assert_eq!(count(&mut b, "1"), None);
        assert_eq!(mod_badge(&mut b, None), shows("1"));
    }
}
