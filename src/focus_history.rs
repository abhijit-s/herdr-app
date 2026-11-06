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
//!
//! A place needs a non-empty string `pane` and an integer `at`; entries without
//! them are kept as they are (the first `MAX`), after the others, and so are top-level fields
//! besides `v` and `places`. A `v` above `VERSION` is read but never written; a
//! value that isn't an object with an integer `v` is unreadable and replaced by
//! the next write.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::time::{Duration, Instant};

pub const NAME: &str = "herdr.focus";
pub const VERSION: u64 = 1;
pub const MAX: usize = 20;
pub const SETTLE: Duration = Duration::from_millis(300);
/// Writes that lose to another writer read, merge and try again this often.
const ATTEMPTS: usize = 4;
/// After a failed round the next waits this long, doubling per failure in a row up to `BACKOFF_MAX`.
const BACKOFF: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Place {
    pub pane: String,
    #[serde(default)]
    pub terminal: Option<String>,
    /// Always set by this app; another client may leave them null or out.
    #[serde(default)]
    pub tab: Option<String>,
    #[serde(default)]
    pub space: Option<String>,
    #[serde(default)]
    pub machine: Option<String>,
    /// When the person arrived, in ms since the Unix epoch.
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

/// The document as stored: its version, places, and what this app keeps
/// without understanding it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stored {
    pub version: u64,
    pub places: Vec<Place>,
    /// Top-level fields besides `v` and `places`, written back as they were.
    pub rest: Map<String, Value>,
    /// Entries of `places` that aren't a place this app reads, written back
    /// after the others: the first `MAX`, so no client grows the document unbounded.
    pub foreign: Vec<Value>,
    /// Written by a newer app (`v` above `VERSION`): read, never overwritten.
    pub newer: bool,
    /// Why the value couldn't be read; the next write replaces it.
    pub unreadable: Option<String>,
}

impl Stored {
    /// Whether merging `unsent` in keeps a visit the stored list lacks (a new
    /// pane, or a later visit to one). Order, duplicates or length alone aren't
    /// a change: another client's list is rewritten only for news.
    fn gains(&self, unsent: &[Place]) -> bool {
        let merged = merge(&self.places, unsent);
        unsent.iter().any(|p| merged.contains(p) && !self.places.contains(p))
    }

    fn value(&self, places: &[Place]) -> Value {
        let mut value = self.rest.clone();
        let mut list: Vec<Value> = places.iter().filter_map(|p| serde_json::to_value(p).ok()).collect();
        list.extend(self.foreign.iter().cloned());
        value.insert("v".into(), json!(VERSION));
        value.insert("places".into(), Value::Array(list));
        Value::Object(value)
    }
}

/// The core's documents, as `sync` needs them (a fake one in tests).
pub trait Store {
    /// `doc.get`: `{value, version}`.
    fn get(&self) -> anyhow::Result<Value>;
    /// `doc.set`: the new version, or an error of kind `conflict`.
    fn set(&self, value: Value, version: u64) -> anyhow::Result<u64>;
}

/// This machine's core, every call of a round ending by `deadline`.
pub struct OnCore<'a>(pub &'a crate::hc::Core, pub Instant);

impl OnCore<'_> {
    fn left(&self) -> Duration {
        self.1.saturating_duration_since(Instant::now())
    }
}

impl Store for OnCore<'_> {
    fn get(&self) -> anyhow::Result<Value> {
        self.0.call_timeout("doc.get", json!({ "name": NAME }), self.left())
    }
    fn set(&self, value: Value, version: u64) -> anyhow::Result<u64> {
        let reply = self.0.call_timeout("doc.set", json!({ "name": NAME, "value": value, "version": version }), self.left())?;
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
    let v = value.as_object().and_then(|o| o.get("v")).and_then(Value::as_u64).filter(|v| *v >= 1);
    let listed = value.get("places").is_none_or(Value::is_array);
    let (Some(v), true) = (v, listed || v.is_some_and(|v| v > VERSION)) else {
        let unreadable = Some(format!("{NAME} isn't an object with an integer v and a places list; the next write replaces it"));
        return Ok(Stored { version, unreadable, ..Default::default() });
    };
    let mut rest = value.as_object().cloned().unwrap_or_default();
    rest.remove("v");
    let entries = match rest.remove("places") {
        Some(Value::Array(entries)) => entries,
        _ => vec![],
    };
    let (mut places, mut foreign) = (Vec::new(), Vec::new());
    for entry in entries {
        match serde_json::from_value::<Place>(entry.clone()) {
            Ok(place) if !place.pane.is_empty() => places.push(place),
            _ if foreign.len() < MAX => foreign.push(entry),
            _ => {}
        }
    }
    Ok(Stored { version, places, rest, foreign, newer: v > VERSION, unreadable: None })
}

/// Adds `unsent` to the document: from `known` when given (a stale version
/// only costs a conflict), else read first. A write that loses to another
/// writer reads again and merges; nothing is written unless it gains a visit.
pub fn sync(store: &impl Store, known: Option<Stored>, unsent: &[Place]) -> Result<Stored, String> {
    let mut stored = match known {
        Some(known) => known,
        None => read(store)?,
    };
    for _ in 0..ATTEMPTS {
        if stored.newer || !stored.gains(unsent) {
            return Ok(stored);
        }
        let places = merge(&stored.places, unsent);
        match store.set(stored.value(&places), stored.version) {
            Ok(version) => return Ok(Stored { version, places, unreadable: None, ..stored }),
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
    /// Read again before the next write (start, reconnect, a `doc` event, a failure).
    pub stale: bool,
    pub pending: bool,
    pub error: Option<String>,
    /// Failed rounds in a row, and when the next may start.
    failures: u32,
    retry_at: Option<Instant>,
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

    /// A new place: also worth trying again now after failures.
    pub fn note(&mut self, place: Place) {
        self.unsent = merge(&self.unsent, &[place]);
        self.retry_now();
    }

    pub fn has_unsent(&self) -> bool {
        !self.unsent.is_empty()
    }

    /// The document changed to `version` (None: unknown, as after `missed`).
    /// The echo of this window's own write needs no read.
    pub fn changed(&mut self, version: Option<u64>) {
        if version.is_none() || self.stored.as_ref().map(|s| s.version) != version {
            self.stale = true;
            self.retry_now();
        }
    }

    pub fn retry_now(&mut self) {
        self.failures = 0;
        self.retry_at = None;
    }

    /// The next round to run, if one is due: what's known and what to add. A
    /// change seen while it runs makes the next one read again. After failures,
    /// none until the backoff ends.
    pub fn due(&mut self, now: Instant) -> Option<(Option<Stored>, Vec<Place>)> {
        if self.pending || (!self.stale && self.unsent.is_empty()) || self.retry_at.is_some_and(|at| now < at) {
            return None;
        }
        let known = self.stored.clone().filter(|_| !self.stale);
        if known.as_ref().is_some_and(|k| k.newer || !k.gains(&self.unsent)) {
            self.unsent.clear();
            return None;
        }
        self.pending = true;
        self.stale = false;
        Some((known, self.unsent.clone()))
    }

    /// A round's result. `sent` is what that round added.
    pub fn finished(&mut self, sent: &[Place], result: Result<Stored, String>, now: Instant) {
        self.pending = false;
        let problem = match result {
            Ok(stored) => {
                self.retry_now();
                // A newer app's document is never written: what this window
                // noted would only show in recentPlaces() without being shared.
                if stored.newer {
                    self.unsent.clear();
                } else {
                    self.unsent.retain(|p| !sent.contains(p));
                }
                let problem = stored.unreadable.clone();
                self.stored = Some(stored);
                problem
            }
            Err(e) => {
                self.stale = true;
                self.failures += 1;
                let wait = BACKOFF.saturating_mul(1 << (self.failures - 1).min(5)).min(BACKOFF_MAX);
                self.retry_at = Some(now + wait);
                Some(e)
            }
        };
        if let Some(e) = &problem
            && self.error.as_ref() != Some(e)
        {
            tracing::warn!("{NAME}: {e}");
        }
        self.error = problem;
    }

    pub fn reconnected(&mut self) {
        self.pending = false;
        self.stale = true;
        self.retry_now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn at(pane: &str, at: u64) -> Place {
        Place { pane: pane.into(), terminal: Some(format!("t-{pane}")), tab: Some("tab".into()), space: Some("s".into()), at, ..Default::default() }
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
        let moved = Place { tab: Some("other".into()), ..at("d", 5) };
        assert_eq!(settle.observe(Some(moved), ms(2600)), None);
        assert_eq!(settle.observe(None, ms(3000)).and_then(|p| p.tab), Some("other".into()));
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
        gets: Mutex<usize>,
        /// Writes fail outright (a full disk, the size limit).
        refuse: Mutex<bool>,
    }

    impl Fake {
        fn new(value: Value, version: u64, conflicts: usize) -> Self {
            Fake { doc: Mutex::new((value, version)), conflicts: Mutex::new(conflicts), theirs: at("theirs", 50), sets: Mutex::new(0), gets: Mutex::new(0), refuse: Mutex::new(false) }
        }
        fn places(&self) -> Vec<Place> {
            serde_json::from_value(self.doc.lock().unwrap().0["places"].clone()).unwrap()
        }
    }

    impl Store for Fake {
        fn get(&self) -> anyhow::Result<Value> {
            *self.gets.lock().unwrap() += 1;
            let (value, version) = self.doc.lock().unwrap().clone();
            Ok(json!({ "value": value, "version": version }))
        }
        fn set(&self, value: Value, version: u64) -> anyhow::Result<u64> {
            *self.sets.lock().unwrap() += 1;
            if *self.refuse.lock().unwrap() {
                return Err(anyhow::anyhow!("no space left on device"));
            }
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
        let (known, unsent) = history.due(Instant::now()).expect("reads first");
        assert!(known.is_none() && unsent.is_empty());
        assert!(history.due(Instant::now()).is_none(), "one round at a time");
        history.finished(&unsent, sync(&core, known, &unsent), Instant::now());
        assert_eq!(panes(&history.places()), ["old"]);
        assert!(history.due(Instant::now()).is_none(), "nothing to write");

        history.note(at("a", 10));
        let (known, unsent) = history.due(Instant::now()).unwrap();
        // Noted while that round runs: kept for the next one.
        history.note(at("b", 20));
        assert_eq!(panes(&history.places()), ["b", "a", "old"]);
        history.finished(&unsent, sync(&core, known, &unsent), Instant::now());
        let (known, unsent) = history.due(Instant::now()).unwrap();
        assert_eq!(panes(&unsent), ["b"]);
        history.finished(&unsent, sync(&core, known, &unsent), Instant::now());
        assert!(history.due(Instant::now()).is_none());
        assert_eq!(panes(&core.places()), ["b", "a", "old"]);

        // Another client clears it: this window doesn't bring the places back.
        *core.doc.lock().unwrap() = (Value::Null, 9);
        history.stale = true;
        let (known, unsent) = history.due(Instant::now()).unwrap();
        history.finished(&unsent, sync(&core, known, &unsent), Instant::now());
        assert!(history.places().is_empty());
        assert_eq!(*core.sets.lock().unwrap(), 2);

        // A failure reads again next time and keeps what wasn't written.
        history.note(at("c", 30));
        let (_, unsent) = history.due(Instant::now()).unwrap();
        history.finished(&unsent, Err("offline".into()), Instant::now());
        assert!(history.stale);
        assert_eq!(panes(&history.places()), ["c"]);
    }

    #[test]
    fn a_write_keeps_what_this_app_does_not_read() {
        let doc = json!({ "v": 1, "owner": "tui", "places": [at("old", 1), { "terminal": "t9", "at": 5 }, { "pane": "p", "at": "soon" }, { "pane": "", "at": 2 }] });
        let core = Fake::new(doc, 1, 0);
        let stored = sync(&core, None, &[at("mine", 10)]).unwrap();
        assert_eq!(panes(&stored.places), ["mine", "old"]);
        let written = core.doc.lock().unwrap().0.clone();
        assert_eq!(written["owner"], "tui", "top-level fields stay");
        assert_eq!(written["v"], 1);
        let entries = written["places"].as_array().unwrap();
        assert_eq!(entries.len(), 5);
        assert_eq!(entries[2..], [json!({ "terminal": "t9", "at": 5 }), json!({ "pane": "p", "at": "soon" }), json!({ "pane": "", "at": 2 })], "unreadable places kept as they were, after the others");
    }

    #[test]
    fn another_clients_untidy_list_is_rewritten_only_for_news() {
        // 25 places, oldest first, p3 twice.
        let mut untidy: Vec<Place> = (0..25).map(|i| at(&format!("p{i}"), i + 100)).collect();
        untidy.push(at("p3", 1));
        let core = Fake::new(json!({ "v": 1, "places": untidy }), 7, 0);
        // What this window noted is already there, or older than what is.
        let stored = sync(&core, None, &[at("p24", 124), at("p5", 50)]).unwrap();
        assert_eq!((stored.version, *core.sets.lock().unwrap()), (7, 0));
        let mut history = History::new();
        let (known, unsent) = history.due(Instant::now()).unwrap();
        history.finished(&unsent, sync(&core, known, &unsent), Instant::now());
        history.note(at("p24", 124));
        assert!(history.due(Instant::now()).is_none(), "no focus change, no write");
        // A visit it lacks: written, tidied.
        let stored = sync(&core, None, &[at("new", 500)]).unwrap();
        assert_eq!(*core.sets.lock().unwrap(), 1);
        assert_eq!(stored.places.len(), MAX);
        assert_eq!(panes(&stored.places[..3]), ["new", "p24", "p23"]);
    }

    #[test]
    fn an_unreadable_value_is_said_once_and_replaced_by_the_next_write() {
        for broken in [json!("garbage"), json!({ "places": [] }), json!({ "v": "1", "places": [] }), json!({ "v": 1, "places": {} })] {
            let core = Fake::new(broken.clone(), 2, 0);
            let mut history = History::new();
            let (known, unsent) = history.due(Instant::now()).unwrap();
            history.finished(&unsent, sync(&core, known, &unsent), Instant::now());
            assert!(history.error.as_deref().is_some_and(|e| e.contains("next write replaces it")), "{broken}");
            assert_eq!(*core.sets.lock().unwrap(), 0, "nothing to add: left alone");
            history.note(at("mine", 10));
            let (known, unsent) = history.due(Instant::now()).unwrap();
            history.finished(&unsent, sync(&core, known, &unsent), Instant::now());
            assert_eq!(core.doc.lock().unwrap().0, json!({ "v": 1, "places": [at("mine", 10)] }), "{broken}");
            assert!(history.error.is_none());
        }
    }

    #[test]
    fn a_change_seen_during_a_round_is_read_after_it_and_an_echo_is_not() {
        let core = Fake::new(Value::Null, 0, 0);
        let mut history = History::new();
        history.note(at("a", 10));
        let (known, unsent) = history.due(Instant::now()).unwrap();
        assert!(!history.stale, "the round reads");
        history.changed(Some(4));
        history.finished(&unsent, sync(&core, known, &unsent), Instant::now());
        assert!(history.stale, "the change during the round is still to read");
        let (known, _) = history.due(Instant::now()).unwrap();
        assert!(known.is_none());
        history.finished(&[], sync(&core, known, &[]), Instant::now());
        let own = core.doc.lock().unwrap().1;
        history.changed(Some(own));
        assert!(!history.stale, "this window's own write");
        history.changed(None);
        assert!(history.stale, "missed: unknown");
    }

    #[test]
    fn a_failing_core_is_tried_less_often_until_something_changes() {
        let core = Fake::new(Value::Null, 0, 0);
        *core.refuse.lock().unwrap() = true;
        let t0 = Instant::now();
        let secs = |n: u64| t0 + Duration::from_secs(n);
        let mut history = History::new();
        history.note(at("a", 10));
        let round = |history: &mut History, now: Instant| match history.due(now) {
            Some((known, unsent)) => {
                history.finished(&unsent, sync(&core, known, &unsent), now);
                true
            }
            None => false,
        };
        // Fails at 0; waits 1 s, then 2, 4, 8, 16, then 30 at most.
        let mut tried = vec![];
        for tick in 0..120 {
            if round(&mut history, secs(tick)) { tried.push(tick); }
        }
        assert_eq!(tried, [0, 1, 3, 7, 15, 31, 61, 91]);
        assert_eq!(*core.sets.lock().unwrap(), 8, "one write a round, not one a tick");
        assert!(history.error.as_deref().is_some_and(|e| e.contains("no space")));
        // A new place, a doc event or a reconnect tries again at once, however long the wait.
        assert!(!round(&mut history, secs(100)), "waiting until 121");
        history.note(at("b", 20));
        assert!(round(&mut history, secs(100)));
        let mut tick = 100;
        for reset in ["doc event", "reconnect"] {
            for _ in 0..20 {
                tick += 1;
                round(&mut history, secs(tick));
            }
            assert!(!round(&mut history, secs(tick)), "{reset}: still backing off");
            if reset == "doc event" { history.changed(None) } else { history.reconnected() }
            assert!(round(&mut history, secs(tick)), "{reset} tries again at once");
            assert!(!round(&mut history, secs(tick)), "{reset}: and backs off again");
        }
        // The core writes again: success ends the backoff.
        *core.refuse.lock().unwrap() = false;
        assert!(round(&mut history, secs(200)));
        assert!(history.error.is_none() && !history.has_unsent());
        assert_eq!(panes(&core.places()), ["b", "a"]);
        history.note(at("c", 30));
        assert!(round(&mut history, secs(200)));
    }

    #[test]
    fn idle_ticks_ask_the_core_nothing() {
        let core = Fake::new(json!({ "v": 1, "places": [at("a", 1)] }), 1, 0);
        let mut history = History::new();
        let (known, unsent) = history.due(Instant::now()).unwrap();
        history.finished(&unsent, sync(&core, known, &unsent), Instant::now());
        for _ in 0..10 {
            assert!(history.due(Instant::now()).is_none());
        }
        assert_eq!((*core.gets.lock().unwrap(), *core.sets.lock().unwrap()), (1, 0));
    }

    #[test]
    fn a_place_needs_only_pane_and_at_and_unreadable_entries_are_bounded() {
        let mut places = vec![json!({ "pane": "p", "tab": null, "space": null, "terminal": null, "machine": null, "at": 7 }), json!({ "pane": "q", "at": 3 })];
        places.extend((0..30).map(|i| json!({ "pane": format!("bad{i}") })));
        let core = Fake::new(json!({ "v": 1, "places": places }), 1, 0);
        let stored = sync(&core, None, &[]).unwrap();
        assert_eq!(panes(&stored.places), ["p", "q"], "null or missing tab and space: a normal place");
        assert_eq!(stored.places[0].tab, None);
        assert_eq!(stored.foreign.len(), MAX, "the first unreadable entries, no more");
        assert_eq!(stored.foreign[0]["pane"], "bad0");
        let stored = sync(&core, Some(stored), &[at("p", 9)]).unwrap();
        assert_eq!(panes(&stored.places), ["p", "q"], "deduplicated with it");
        let written = core.doc.lock().unwrap().0["places"].as_array().unwrap().len();
        assert_eq!(written, 2 + MAX);
    }

    #[test]
    fn places_noted_against_a_newer_apps_document_are_not_shown() {
        let core = Fake::new(json!({ "v": 2, "places": [at("theirs", 5)] }), 3, 0);
        let mut history = History::new();
        history.note(at("mine", 10));
        assert_eq!(panes(&history.places()), ["mine"]);
        let (known, unsent) = history.due(Instant::now()).unwrap();
        history.finished(&unsent, sync(&core, known, &unsent), Instant::now());
        assert_eq!(panes(&history.places()), ["theirs"], "only what is shared");
        assert!(!history.has_unsent());
        assert_eq!(*core.sets.lock().unwrap(), 0);
    }
}
