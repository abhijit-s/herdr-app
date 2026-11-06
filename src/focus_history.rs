//! Focus history: the places the person settled on, newest first, kept in the
//! core document `herdr.focus` so every client (other app windows, the TUI, a
//! CLI) reads and adds to the same "back to" and "recent" list.
//!
//! The document is `{"v":1,"places":[Place...]}`, newest first, at most `MAX`
//! places, one per pane. Writers merge rather than replace: the union by pane,
//! the later `at` winning, sorted by `at` (then pane) and cut to `MAX`. A place
//! counts once the person stays `SETTLE` on it, so quick hops through panes
//! leave nothing behind. Places on another machine stay in this machine's
//! document with their `machine`; nothing is written to remote cores.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::time::{Duration, Instant};

pub const NAME: &str = "herdr.focus";
pub const VERSION: u64 = 1;
pub const MAX: usize = 20;
pub const SETTLE: Duration = Duration::from_millis(300);
/// Writes that lose to another writer read, merge and try again this often.
const ATTEMPTS: usize = 4;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Place {
    pub pane: String,
    #[serde(default)]
    pub terminal: Option<String>,
    #[serde(default)]
    pub tab: String,
    #[serde(default)]
    pub space: String,
    #[serde(default)]
    pub machine: Option<String>,
    /// When the person arrived, in ms since the Unix epoch.
    #[serde(default)]
    pub at: u64,
    /// Fields another client added, kept when this one rewrites the list.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Place {
    fn same_spot(&self, other: &Place) -> bool {
        (&self.pane, &self.terminal, &self.tab, &self.space) == (&other.pane, &other.terminal, &other.tab, &other.space)
    }
}

/// Both lists as one: a place per pane (the later visit), newest first, at most `MAX`.
pub fn merge(a: &[Place], b: &[Place]) -> Vec<Place> {
    let mut all: Vec<Place> = Vec::new();
    for p in a.iter().chain(b).filter(|p| !p.pane.is_empty()) {
        match all.iter_mut().find(|q| q.pane == p.pane) {
            Some(q) if p.at > q.at => *q = p.clone(),
            Some(_) => {}
            None => all.push(p.clone()),
        }
    }
    all.sort_by(|x, y| y.at.cmp(&x.at).then_with(|| x.pane.cmp(&y.pane)));
    all.truncate(MAX);
    all
}

/// Where the person is, and whether they stayed long enough for it to count.
#[derive(Default)]
pub struct Settle {
    here: Option<(Place, Instant)>,
    noted: Option<Place>,
}

impl Settle {
    /// The person is at `place` now. Returns the place they left when they
    /// stayed there long enough and it wasn't noted yet.
    pub fn observe(&mut self, place: Option<Place>, now: Instant) -> Option<Place> {
        let same = match (&self.here, &place) {
            (Some((here, _)), Some(place)) => here.same_spot(place),
            (None, None) => true,
            _ => false,
        };
        if same {
            return None;
        }
        let left = self.settled(now);
        self.here = place.map(|p| (p, now));
        left
    }

    /// The current place, once, when the person has stayed `SETTLE` on it.
    /// Coming back to the place noted last after a quick hop notes nothing.
    pub fn settled(&mut self, now: Instant) -> Option<Place> {
        let (place, since) = self.here.as_ref()?;
        if now.saturating_duration_since(*since) < SETTLE || self.noted.as_ref().is_some_and(|n| n.same_spot(place)) {
            return None;
        }
        self.noted = Some(place.clone());
        Some(place.clone())
    }
}

/// The document as stored: its version and places, and whether a newer app
/// wrote it in a shape this one must not overwrite.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stored {
    pub version: u64,
    pub places: Vec<Place>,
    pub newer: bool,
}

/// The core's documents, as `sync` needs them (a fake one in tests).
pub trait Store {
    /// `doc.get`: `{value, version}`.
    fn get(&self) -> anyhow::Result<Value>;
    /// `doc.set`: the new version, or an error of kind `conflict`.
    fn set(&self, value: Value, version: u64) -> anyhow::Result<u64>;
}

/// This machine's core, each call bounded by `timeout`.
pub struct OnCore<'a>(pub &'a crate::hc::Core, pub Duration);

impl Store for OnCore<'_> {
    fn get(&self) -> anyhow::Result<Value> {
        self.0.call_timeout("doc.get", json!({ "name": NAME }), self.1)
    }
    fn set(&self, value: Value, version: u64) -> anyhow::Result<u64> {
        let reply = self.0.call_timeout("doc.set", json!({ "name": NAME, "value": value, "version": version }), self.1)?;
        reply["version"].as_u64().ok_or_else(|| anyhow::anyhow!("{NAME} write returned no version"))
    }
}

fn read(store: &impl Store) -> Result<Stored, String> {
    let reply = store.get().map_err(|e| format!("{e:#}"))?;
    let version = reply["version"].as_u64().ok_or_else(|| format!("{NAME} has no version"))?;
    let value = &reply["value"];
    if value.is_null() {
        return Ok(Stored { version, ..Default::default() });
    }
    let newer = value["v"].as_u64().is_none_or(|v| v > VERSION);
    let places = value["places"].as_array().into_iter().flatten().filter_map(|p| serde_json::from_value(p.clone()).ok()).collect();
    Ok(Stored { version, places, newer })
}

/// Adds `unsent` to the document: from `known` when given (a stale version
/// only costs a conflict), else read first. A write that loses to another
/// writer reads again and merges; nothing is written when nothing changes.
pub fn sync(store: &impl Store, known: Option<Stored>, unsent: &[Place]) -> Result<Stored, String> {
    let mut stored = match known {
        Some(known) => known,
        None => read(store)?,
    };
    for _ in 0..ATTEMPTS {
        let places = merge(&stored.places, unsent);
        if stored.newer || places == stored.places {
            return Ok(stored);
        }
        match store.set(json!({ "v": VERSION, "places": places }), stored.version) {
            Ok(version) => return Ok(Stored { version, places, newer: false }),
            Err(e) if crate::hc::error_kind(&e) == Some("conflict") => stored = read(store)?,
            Err(e) => return Err(format!("{e:#}")),
        }
    }
    Err(format!("{NAME} kept changing under this write"))
}

/// One window's view of the history: the document merged with what it noted
/// and hasn't written yet.
#[derive(Default)]
pub struct History {
    pub settle: Settle,
    stored: Option<Stored>,
    unsent: Vec<Place>,
    /// Read again before the next write (start, reconnect, a `doc` event).
    pub stale: bool,
    pub pending: bool,
    pub error: Option<String>,
}

impl History {
    /// Reads the document before its first write.
    pub fn new() -> Self {
        History { stale: true, ..Default::default() }
    }

    /// What mods see: newest first.
    pub fn places(&self) -> Vec<Place> {
        merge(self.stored.as_ref().map_or(&[][..], |s| &s.places), &self.unsent)
    }

    pub fn note(&mut self, place: Place) {
        self.unsent = merge(&self.unsent, &[place]);
    }

    /// The next round to run, if one is due: what's known and what to add.
    pub fn due(&mut self) -> Option<(Option<Stored>, Vec<Place>)> {
        if self.pending {
            return None;
        }
        let known = self.stored.clone().filter(|_| !self.stale);
        let unchanged = known.as_ref().is_some_and(|k| k.newer || merge(&k.places, &self.unsent) == k.places);
        if unchanged {
            self.unsent.clear();
            return None;
        }
        self.pending = true;
        Some((known, self.unsent.clone()))
    }

    /// A round's result. `sent` is what that round added.
    pub fn finished(&mut self, sent: &[Place], result: Result<Stored, String>) {
        self.pending = false;
        match result {
            Ok(stored) => {
                if !stored.newer {
                    self.unsent.retain(|p| !sent.contains(p));
                }
                self.stored = Some(stored);
                self.stale = false;
                self.error = None;
            }
            Err(e) => {
                if self.error.as_ref() != Some(&e) {
                    tracing::warn!("{NAME}: {e}");
                }
                self.error = Some(e);
                self.stale = true;
            }
        }
    }

    pub fn reconnected(&mut self) {
        self.pending = false;
        self.stale = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn at(pane: &str, at: u64) -> Place {
        Place { pane: pane.into(), terminal: Some(format!("t-{pane}")), tab: "tab".into(), space: "s".into(), at, ..Default::default() }
    }

    fn panes(list: &[Place]) -> Vec<&str> {
        list.iter().map(|p| p.pane.as_str()).collect()
    }

    #[test]
    fn merging_keeps_one_place_per_pane_newest_first_and_at_most_max() {
        let a = [at("p1", 10), at("p2", 30), at("p1", 5)];
        let b = [at("p1", 40), at("p3", 20)];
        let merged = merge(&a, &b);
        assert_eq!(panes(&merged), ["p1", "p2", "p3"]);
        assert_eq!(merged[0].at, 40, "the later visit wins");
        assert_eq!(merge(&b, &a), merged, "either order gives the same list");
        let many: Vec<Place> = (0..30).map(|i| at(&format!("p{i}"), i)).collect();
        let merged = merge(&many, &[]);
        assert_eq!(merged.len(), MAX);
        assert_eq!(merged[0].pane, "p29");
        assert_eq!(panes(&merge(&[at("b", 1), at("a", 1)], &[])), ["a", "b"], "ties order by pane");
    }

    #[test]
    fn a_place_counts_once_the_person_stays_on_it() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut settle = Settle::default();
        assert_eq!(settle.observe(Some(at("a", 0)), ms(0)), None);
        assert_eq!(settle.settled(ms(100)), None, "not yet");
        assert_eq!(settle.settled(ms(300)).map(|p| p.pane), Some("a".into()));
        assert_eq!(settle.settled(ms(900)), None, "once");
        // A quick hop through b and back to a leaves nothing.
        assert_eq!(settle.observe(Some(at("b", 1)), ms(1000)), None);
        assert_eq!(settle.observe(Some(at("a", 2)), ms(1100)), None);
        assert_eq!(settle.settled(ms(2000)), None, "a is noted already");
        // Moving on from a place before the tick noticed it still notes it.
        assert_eq!(settle.observe(Some(at("c", 3)), ms(2100)), None);
        assert_eq!(settle.observe(Some(at("d", 4)), ms(2500)).map(|p| p.pane), Some("c".into()));
        // The same pane in another tab is another place; nothing focused is none.
        let moved = Place { tab: "other".into(), ..at("d", 5) };
        assert_eq!(settle.observe(Some(moved), ms(2600)), None);
        assert_eq!(settle.observe(None, ms(3000)).map(|p| p.tab), Some("other".into()));
        assert_eq!(settle.settled(ms(9000)), None);
    }

    #[test]
    fn places_from_other_clients_keep_fields_this_app_does_not_know() {
        let p: Place = serde_json::from_value(json!({ "pane": "p", "at": 1, "client": "tui" })).unwrap();
        assert_eq!(p.terminal, None);
        assert_eq!(serde_json::to_value(&p).unwrap()["client"], "tui");
    }

    /// A core's documents: one value and version, and writes that lose
    /// `conflicts` times to another writer adding `theirs` first.
    struct Fake {
        doc: Mutex<(Value, u64)>,
        conflicts: Mutex<usize>,
        theirs: Place,
        sets: Mutex<usize>,
    }

    impl Fake {
        fn new(value: Value, version: u64, conflicts: usize) -> Self {
            Fake { doc: Mutex::new((value, version)), conflicts: Mutex::new(conflicts), theirs: at("theirs", 50), sets: Mutex::new(0) }
        }
        fn places(&self) -> Vec<Place> {
            serde_json::from_value(self.doc.lock().unwrap().0["places"].clone()).unwrap()
        }
    }

    impl Store for Fake {
        fn get(&self) -> anyhow::Result<Value> {
            let (value, version) = self.doc.lock().unwrap().clone();
            Ok(json!({ "value": value, "version": version }))
        }
        fn set(&self, value: Value, version: u64) -> anyhow::Result<u64> {
            *self.sets.lock().unwrap() += 1;
            let mut conflicts = self.conflicts.lock().unwrap();
            let mut doc = self.doc.lock().unwrap();
            if *conflicts > 0 {
                *conflicts -= 1;
                let mut places: Vec<Place> = serde_json::from_value(doc.0["places"].clone()).unwrap_or_default();
                places = merge(&places, &[Place { at: self.theirs.at + *conflicts as u64, ..self.theirs.clone() }]);
                *doc = (json!({ "v": 1, "places": places }), doc.1 + 1);
            }
            if version != doc.1 {
                return Err(anyhow::Error::new(crate::hc::RpcError(json!({ "message": "stale", "data": { "kind": "conflict" } }))));
            }
            *doc = (value, version + 1);
            Ok(version + 1)
        }
    }

    #[test]
    fn a_write_that_loses_reads_merges_and_tries_again() {
        let core = Fake::new(Value::Null, 0, 2);
        let stored = sync(&core, None, &[at("mine", 10)]).unwrap();
        assert_eq!(panes(&stored.places), ["theirs", "mine"]);
        assert_eq!(core.places(), stored.places);
        assert_eq!(stored.version, core.doc.lock().unwrap().1);
        assert_eq!(*core.sets.lock().unwrap(), 3);
        // Nothing new: no write.
        assert_eq!(sync(&core, Some(stored.clone()), &[at("mine", 10)]).unwrap(), stored);
        assert_eq!(*core.sets.lock().unwrap(), 3);
        // A stale version costs a conflict, not a lost place.
        let stale = Stored { version: 1, ..stored };
        let again = sync(&core, Some(stale), &[at("later", 99)]).unwrap();
        assert_eq!(panes(&again.places), ["later", "theirs", "mine"]);
    }

    #[test]
    fn a_writer_that_keeps_losing_gives_up_and_a_newer_shape_is_only_read() {
        let core = Fake::new(Value::Null, 0, 10);
        assert!(sync(&core, None, &[at("mine", 10)]).unwrap_err().contains("kept changing"));
        let core = Fake::new(json!({ "v": 2, "places": [at("p", 1)], "more": true }), 4, 0);
        let stored = sync(&core, None, &[at("mine", 10)]).unwrap();
        assert!(stored.newer);
        assert_eq!(panes(&stored.places), ["p"]);
        assert_eq!(*core.sets.lock().unwrap(), 0, "never overwrites a newer app's document");
    }

    #[test]
    fn a_window_writes_only_what_changed_and_forgets_what_landed() {
        let core = Fake::new(json!({ "v": 1, "places": [at("old", 1)] }), 3, 0);
        let mut history = History::new();
        let (known, unsent) = history.due().expect("reads first");
        assert!(known.is_none() && unsent.is_empty());
        assert!(history.due().is_none(), "one round at a time");
        history.finished(&unsent, sync(&core, known, &unsent));
        assert_eq!(panes(&history.places()), ["old"]);
        assert!(history.due().is_none(), "nothing to write");

        history.note(at("a", 10));
        let (known, unsent) = history.due().unwrap();
        // Noted while that round runs: kept for the next one.
        history.note(at("b", 20));
        assert_eq!(panes(&history.places()), ["b", "a", "old"]);
        history.finished(&unsent, sync(&core, known, &unsent));
        let (known, unsent) = history.due().unwrap();
        assert_eq!(panes(&unsent), ["b"]);
        history.finished(&unsent, sync(&core, known, &unsent));
        assert!(history.due().is_none());
        assert_eq!(panes(&core.places()), ["b", "a", "old"]);

        // Another client clears it: this window doesn't bring the places back.
        *core.doc.lock().unwrap() = (Value::Null, 9);
        history.stale = true;
        let (known, unsent) = history.due().unwrap();
        history.finished(&unsent, sync(&core, known, &unsent));
        assert!(history.places().is_empty());
        assert_eq!(*core.sets.lock().unwrap(), 2);

        // A failure reads again next time and keeps what wasn't written.
        history.note(at("c", 30));
        let (_, unsent) = history.due().unwrap();
        history.finished(&unsent, Err("offline".into()));
        assert!(history.stale);
        assert_eq!(panes(&history.places()), ["c"]);
    }
}
