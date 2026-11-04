//! Git status for mods (`gitStatus`, `nextGitStatus`, `refreshGitStatus`).
//!
//! One cached answer per folder, shared by every mod copy. A local repo is
//! looked at again only when something in it changes on disk (one OS watcher
//! for all; ignored folders, git's object store and lock files don't count),
//! after the burst settles and never more than about once a second. A folder
//! on another machine has no watcher: it is looked at again when a mod asks
//! (`refreshGitStatus`), or when read once its answer is old.
//!
//! Git runs off the window thread, one folder at a time per machine. Answers
//! come back as `Request::Git`; the window keeps them (thread-local, like the
//! snapshots) and redraws only the copies that read that folder.

use notify::Watcher as _;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// A folder a mod asked about, on this computer (`on: None`) or another.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct Place {
    pub on: Option<String>,
    pub folder: String,
}

impl Place {
    /// What a copy's read of it is recorded under (`mods::LiveReads`).
    pub fn read_key(&self) -> String {
        format!("git\u{0}{}\u{0}{}", self.on.as_deref().unwrap_or(""), self.folder)
    }
}

/// A finished look: `false` (not a repo, or git couldn't say) or the status.
#[derive(Debug)]
pub struct Update {
    pub place: Place,
    pub value: Value,
    /// It holds until told otherwise: a watched repo (changes on disk bring the
    /// next answer), or git's own "not a git repository". Others (another
    /// machine, git couldn't say) are asked again once old.
    pub lasting: bool,
}

/// A save or a checkout is a burst of events: look once it settles.
const QUIET: Duration = Duration::from_millis(300);
/// Never look at one folder more often than this (or 4x as long as git took).
const MIN_GAP: Duration = Duration::from_secs(1);
/// Answers that aren't lasting are looked at again when read after this.
const STALE: Duration = Duration::from_secs(60);
/// Lasting ones too, in case the OS dropped an event.
const LASTING_STALE: Duration = Duration::from_secs(600);
const MAX_WATCHED: usize = 64;
const MAX_FILES: usize = 5000;

const STATUS: &[&str] = &["--no-optional-locks", "-c", "core.fsmonitor=false", "status", "--porcelain=v2", "-z", "--branch", "--no-renames", "--untracked-files=normal"];
const RESOLVE: &[&str] = &["rev-parse", "--path-format=absolute", "--show-toplevel", "--git-dir", "--git-common-dir"];

// ---- the window thread -----------------------------------------------------

struct Entry {
    /// `null` until the first look, then `false` or the status.
    value: Value,
    revision: u64,
    checked: Instant,
    pending: bool,
    lasting: bool,
    waiters: Vec<Waiter>,
}

struct Waiter {
    after: Option<u64>,
    files: bool,
    tx: async_channel::Sender<Value>,
}

thread_local! {
    static ENTRIES: RefCell<HashMap<Place, Entry>> = RefCell::new(HashMap::new());
    static WORKERS: RefCell<HashMap<Option<String>, mpsc::Sender<Msg>>> = RefCell::new(HashMap::new());
}

fn ask(place: &Place) {
    WORKERS.with_borrow_mut(|workers| {
        let tx = workers.entry(place.on.clone()).or_insert_with(|| Worker::spawn(place.on.clone(), Box::new(|u| crate::mods::request(crate::mods::Request::Git(u)))));
        let _ = tx.send(Msg::Look(place.clone()));
    });
}

fn entry<R>(place: &Place, f: impl FnOnce(&mut Entry) -> R) -> R {
    ENTRIES.with_borrow_mut(|all| {
        let now = Instant::now();
        let e = all.entry(place.clone()).or_insert_with(|| {
            ask(place);
            Entry { value: Value::Null, revision: 0, checked: now, pending: true, lasting: false, waiters: Vec::new() }
        });
        let stale = if e.lasting { LASTING_STALE } else { STALE };
        if !e.pending && now.duration_since(e.checked) > stale {
            ask(place);
            e.pending = true;
        }
        f(e)
    })
}

/// The answer for `place` now (`null` while git is first asked), and its revision.
pub fn read<R>(place: &Place, f: impl FnOnce(&Value, u64) -> R) -> R {
    entry(place, |e| f(&e.value, e.revision))
}

/// Resolves with `{ status, revision }` once there is an answer newer than revision `after`
/// (with no `after`, as soon as there is one at all).
pub fn next(place: &Place, after: Option<u64>, files: bool) -> async_channel::Receiver<Value> {
    let (tx, rx) = async_channel::bounded(1);
    entry(place, |e| {
        if !e.value.is_null() && after.is_none_or(|a| e.revision > a) {
            let _ = tx.try_send(news(&e.value, e.revision, files));
        } else {
            e.waiters.push(Waiter { after, files, tx });
        }
    });
    rx
}

/// Looks again now (as soon as the folder's last look allows).
pub fn refresh(place: &Place) {
    entry(place, |e| {
        if !e.pending {
            ask(place);
            e.pending = true;
        }
    });
}

/// Keeps a finished look; returns whether the answer changed.
pub fn apply(u: Update) -> bool {
    ENTRIES.with_borrow_mut(|all| {
        let Some(e) = all.get_mut(&u.place) else { return false };
        e.pending = false;
        e.checked = Instant::now();
        e.lasting = u.lasting;
        let changed = e.value != u.value;
        if changed {
            e.value = u.value;
            e.revision += 1;
            crate::mods::bump_revision(&u.place.read_key());
        }
        let (value, revision) = (&e.value, e.revision);
        e.waiters.retain(|w| {
            if w.after.is_none_or(|a| revision > a) {
                let _ = w.tx.try_send(news(value, revision, w.files));
                false
            } else {
                !w.tx.is_closed()
            }
        });
        changed
    })
}

/// What `nextGitStatus` resolves with: the answer and its revision (`false` has none of its own).
fn news(value: &Value, revision: u64, files: bool) -> Value {
    json!({ "status": shown(value, revision, files), "revision": revision })
}

/// The answer as a mod gets it: with its revision, the file list only when asked for.
pub fn shown(value: &Value, revision: u64, files: bool) -> Value {
    match value {
        Value::Object(o) => {
            let mut out: serde_json::Map<String, Value> = o.iter().filter(|(k, _)| files || (k.as_str() != "files" && k.as_str() != "truncated")).map(|(k, v)| (k.clone(), v.clone())).collect();
            out.insert("revision".into(), json!(revision));
            Value::Object(out)
        }
        v => v.clone(),
    }
}

// ---- the workers -------------------------------------------------------------

enum Msg {
    Look(Place),
    Fs(notify::Result<notify::Event>),
}

struct Resolved {
    root: String,
    git_dir: String,
    common_dir: String,
}

/// A watched repo: the folders asked about inside it, what counts as a change.
struct Repo {
    places: HashSet<Place>,
    root: PathBuf,
    git_dirs: Vec<PathBuf>,
    ignored: HashSet<PathBuf>,
}

impl Repo {
    fn counts(&self, path: &Path) -> bool {
        if let Some(rel) = self.git_dirs.iter().find_map(|d| path.strip_prefix(d).ok()) {
            // Objects and logs follow ref changes; locks are git's own scratch.
            let first = rel.components().next().map(|c| c.as_os_str().to_owned());
            return !(first.as_deref() == Some("objects".as_ref()) || first.as_deref() == Some("logs".as_ref()) || is_lock(path));
        }
        let Ok(rel) = path.strip_prefix(&self.root) else { return false };
        if rel.components().next().is_some_and(|c| c.as_os_str() == ".git") {
            return false;
        }
        !path.ancestors().take_while(|a| *a != self.root).any(|a| self.ignored.contains(a))
    }
}

fn is_lock(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "lock")
}

struct Worker {
    on: Option<String>,
    sink: Box<dyn Fn(Update) + Send>,
    tx: mpsc::Sender<Msg>,
    due: HashMap<Place, Instant>,
    last: HashMap<Place, (Instant, Duration)>,
    resolved: HashMap<Place, Resolved>,
    watcher: Option<notify::RecommendedWatcher>,
    repos: HashMap<PathBuf, Repo>,
}

impl Worker {
    fn spawn(on: Option<String>, sink: Box<dyn Fn(Update) + Send>) -> mpsc::Sender<Msg> {
        let (tx, rx) = mpsc::channel();
        let worker = Worker { on, sink, tx: tx.clone(), due: HashMap::new(), last: HashMap::new(), resolved: HashMap::new(), watcher: None, repos: HashMap::new() };
        std::thread::Builder::new().name("git-status".into()).spawn(move || worker.run(rx)).expect("git status thread starts");
        tx
    }

    fn run(mut self, rx: mpsc::Receiver<Msg>) {
        loop {
            let msg = match self.due.values().min() {
                Some(at) => match rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
                    Ok(m) => Some(m),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                },
                None => match rx.recv() {
                    Ok(m) => Some(m),
                    Err(_) => return,
                },
            };
            match msg {
                Some(Msg::Look(place)) => self.schedule(place, Instant::now()),
                Some(Msg::Fs(event)) => self.changed(event),
                None => {}
            }
            let now = Instant::now();
            let ready: Vec<Place> = self.due.iter().filter(|(_, at)| **at <= now).map(|(p, _)| p.clone()).collect();
            for place in ready {
                self.due.remove(&place);
                self.look(&place);
            }
        }
    }

    /// Wants a look at `place` from `at` on, no sooner than its last look allows.
    /// One already due keeps its time: a stream of events can't put it off.
    fn schedule(&mut self, place: Place, at: Instant) {
        let earliest = self.last.get(&place).map(|(end, took)| *end + MIN_GAP.max(*took * 4));
        let at = earliest.map_or(at, |e| e.max(at));
        self.due.entry(place).and_modify(|d| *d = (*d).min(at)).or_insert(at);
    }

    fn changed(&mut self, event: notify::Result<notify::Event>) {
        let Ok(event) = event else {
            // Lost track: every watched folder may have changed.
            let all: Vec<Place> = self.repos.values().flat_map(|r| r.places.iter().cloned()).collect();
            let at = Instant::now() + QUIET;
            all.into_iter().for_each(|p| self.schedule(p, at));
            return;
        };
        if matches!(event.kind, notify::EventKind::Access(_)) {
            return;
        }
        let rescan = event.need_rescan();
        let mut hit = Vec::new();
        // A new folder may be one the rules ignore (a first build's `target/`).
        let new_folder = matches!(event.kind, notify::EventKind::Create(_));
        for repo in self.repos.values_mut() {
            if event.paths.iter().any(|p| p.starts_with(&repo.root) && (p.file_name().is_some_and(|n| n == ".gitignore" || n == "exclude") || (new_folder && p.is_dir() && repo.counts(p)))) {
                repo.ignored = ignored(&repo.root);
            }
            if rescan || event.paths.iter().any(|p| repo.counts(p)) {
                hit.extend(repo.places.iter().cloned());
            }
        }
        let at = Instant::now() + QUIET;
        for place in hit {
            self.schedule(place, at);
        }
    }

    fn look(&mut self, place: &Place) {
        let start = Instant::now();
        let (value, no_repo) = self.status(place);
        self.last.insert(place.clone(), (Instant::now(), start.elapsed()));
        let lasting = no_repo || self.repos.values().any(|r| r.places.contains(place));
        (self.sink)(Update { place: place.clone(), value, lasting });
    }

    /// The answer, and whether git said there is no repo.
    fn status(&mut self, place: &Place) -> (Value, bool) {
        if !self.resolved.contains_key(place) {
            // No repo isn't remembered here: the next look asks again (a `git init`, a machine back).
            let r = match self.git(&place.folder, RESOLVE) {
                Ok(out) => resolved(&out),
                Err(e) => return (Value::Bool(false), e.to_lowercase().contains("not a git repository")),
            };
            let Some(r) = r else { return (Value::Bool(false), false) };
            self.watch(place, &r);
            self.resolved.insert(place.clone(), r);
        }
        let r = &self.resolved[place];
        match self.git(&r.root, STATUS) {
            Ok(out) => (parse(&out, r), false),
            Err(_) => {
                // Gone (a removed worktree) or unreachable: find it again next time.
                self.resolved.remove(place);
                (Value::Bool(false), false)
            }
        }
    }

    fn git(&self, cwd: &str, args: &[&str]) -> Result<String, String> {
        let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        match &self.on {
            Some(machine) => match crate::mods::run_remote(machine, "git", &args, Some(cwd.to_owned()))? {
                (0, out, _) => Ok(String::from_utf8_lossy(&out).into_owned()),
                (_, _, err) => Err(String::from_utf8_lossy(&err).into_owned()),
            },
            None => {
                let out = crate::spawn::command("git").args(&args).current_dir(cwd).stdin(std::process::Stdio::null()).output().map_err(|e| e.to_string())?;
                if out.status.success() { Ok(String::from_utf8_lossy(&out.stdout).into_owned()) } else { Err(String::from_utf8_lossy(&out.stderr).into_owned()) }
            }
        }
    }

    fn watch(&mut self, place: &Place, r: &Resolved) {
        if self.on.is_some() {
            return;
        }
        let canon = |p: &str| Path::new(p).canonicalize().unwrap_or_else(|_| PathBuf::from(p));
        let root = canon(&r.root);
        if let Some(repo) = self.repos.get_mut(&root) {
            repo.places.insert(place.clone());
            return;
        }
        if self.repos.len() >= MAX_WATCHED {
            return;
        }
        if self.watcher.is_none() {
            let tx = self.tx.clone();
            match notify::recommended_watcher(move |e| {
                let _ = tx.send(Msg::Fs(e));
            }) {
                Ok(w) => self.watcher = Some(w),
                Err(e) => {
                    tracing::warn!("git status: file watching unavailable ({e}); answers refresh when old");
                    return;
                }
            }
        }
        let mut git_dirs = vec![canon(&r.git_dir)];
        if r.common_dir != r.git_dir {
            git_dirs.push(canon(&r.common_dir));
        }
        let watcher = self.watcher.as_mut().expect("made above");
        for dir in watch_list(&root, &git_dirs) {
            if let Err(e) = watcher.watch(&dir, watch_mode()) {
                tracing::warn!("git status: cannot watch {} ({e}); its answer refreshes when old", dir.display());
                return;
            }
        }
        let ignored = ignored(&root);
        self.repos.insert(root.clone(), Repo { places: HashSet::from([place.clone()]), root, git_dirs, ignored });
    }
}

/// Folders to register: the work tree and git's own folders where they live
/// elsewhere (a linked worktree's). Inotify needs one per directory, so on Linux
/// it is the work tree's non-ignored folders and git's, without objects.
fn watch_list(root: &Path, git_dirs: &[PathBuf]) -> Vec<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        let mut all: HashSet<PathBuf> = crate::files::watch_directories(root).unwrap_or_else(|_| HashSet::from([root.to_path_buf()]));
        all.extend(git_dirs.iter().cloned());
        all.extend(git_dirs.iter().map(|d| d.join("refs")).filter(|d| d.is_dir()));
        all.into_iter().collect()
    }
    #[cfg(not(target_os = "linux"))]
    {
        std::iter::once(root.to_path_buf()).chain(git_dirs.iter().filter(|d| !d.starts_with(root)).cloned()).collect()
    }
}

fn watch_mode() -> notify::RecursiveMode {
    if cfg!(target_os = "linux") { notify::RecursiveMode::NonRecursive } else { notify::RecursiveMode::Recursive }
}

/// Ignored files and folders in the work tree (a build folder's churn is no change).
fn ignored(root: &Path) -> HashSet<PathBuf> {
    let Ok(out) = crate::spawn::command("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "--others", "--ignored", "--exclude-standard", "--directory", "-z"])
        .stdin(std::process::Stdio::null())
        .output()
    else {
        return HashSet::new();
    };
    String::from_utf8_lossy(&out.stdout).split('\0').filter(|p| !p.is_empty()).map(|p| root.join(p.trim_end_matches('/'))).collect()
}

fn resolved(out: &str) -> Option<Resolved> {
    let mut lines = out.lines();
    let (root, git_dir, common_dir) = (lines.next()?, lines.next()?, lines.next()?);
    Some(Resolved { root: root.to_owned(), git_dir: git_dir.to_owned(), common_dir: common_dir.to_owned() })
}

/// `git status --porcelain=v2 -z --branch` as a mod reads it.
fn parse(out: &str, r: &Resolved) -> Value {
    let (mut branch, mut head, mut upstream) = (None, None, None);
    let (mut ahead, mut behind) = (0i64, 0i64);
    let (mut staged, mut unstaged, mut untracked, mut conflicted) = (0, 0, 0, 0);
    let mut files = Vec::new();
    let mut truncated = false;
    let mut fields = out.split('\0');
    let mut file = |path: &str, xy: &str, files: &mut Vec<Value>| {
        if files.len() < MAX_FILES {
            files.push(json!({ "path": path, "xy": xy.replace('.', " ") }));
        } else {
            truncated = true;
        }
    };
    while let Some(entry) = fields.next() {
        if let Some(header) = entry.strip_prefix("# ") {
            match header.split_once(' ') {
                Some(("branch.oid", v)) => head = (v != "(initial)").then(|| v.to_owned()),
                Some(("branch.head", v)) => branch = (v != "(detached)").then(|| v.to_owned()),
                Some(("branch.upstream", v)) => upstream = Some(v.to_owned()),
                Some(("branch.ab", v)) => {
                    let mut ab = v.split(' ').map(|n| n.trim_start_matches(['+', '-']).parse::<i64>().unwrap_or(0));
                    ahead = ab.next().unwrap_or(0);
                    behind = ab.next().unwrap_or(0);
                }
                _ => {}
            }
            continue;
        }
        let xy = entry.get(2..4).unwrap_or("");
        let changed = |xy: &str, staged: &mut i32, unstaged: &mut i32| {
            *staged += i32::from(!xy.starts_with('.'));
            *unstaged += i32::from(!xy.ends_with('.'));
        };
        match entry.as_bytes().first() {
            Some(b'1') => {
                changed(xy, &mut staged, &mut unstaged);
                file(entry.splitn(9, ' ').nth(8).unwrap_or(""), xy, &mut files);
            }
            Some(b'2') => {
                changed(xy, &mut staged, &mut unstaged);
                file(entry.splitn(10, ' ').nth(9).unwrap_or(""), xy, &mut files);
                fields.next(); // the path it came from
            }
            Some(b'u') => {
                conflicted += 1;
                file(entry.splitn(11, ' ').nth(10).unwrap_or(""), xy, &mut files);
            }
            Some(b'?') => {
                untracked += 1;
                file(&entry[2..], "??", &mut files);
            }
            _ => {}
        }
    }
    let common = r.common_dir.trim_end_matches('/');
    let name = match common.strip_suffix("/.git") {
        Some(main) => base(main),
        None => base(common).trim_end_matches(".git"),
    };
    json!({
        "root": r.root, "gitDir": r.git_dir, "commonDir": r.common_dir, "name": name,
        "linked": r.git_dir.trim_end_matches('/') != common,
        "branch": branch, "head": head, "upstream": upstream, "ahead": ahead, "behind": behind,
        "staged": staged, "unstaged": unstaged, "untracked": untracked, "conflicted": conflicted,
        "files": files, "truncated": truncated,
    })
}

fn base(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved_at(root: &str) -> Resolved {
        Resolved { root: root.into(), git_dir: format!("{root}/.git"), common_dir: format!("{root}/.git") }
    }

    #[test]
    fn porcelain_v2_counts_and_files() {
        let out = [
            "# branch.oid 1234abcd",
            "# branch.head feature",
            "# branch.upstream origin/feature",
            "# branch.ab +2 -1",
            "1 M. N... 100644 100644 100644 aaa bbb src/staged.rs",
            "1 .M N... 100644 100644 100644 aaa bbb with space.rs",
            "1 MM N... 100644 100644 100644 aaa bbb both.rs",
            "2 R. N... 100644 100644 100644 aaa bbb R100 new.rs",
            "old.rs",
            "u UU N... 100644 100644 100644 100644 aaa bbb ccc conflict.rs",
            "? fresh.txt",
            "",
        ]
        .join("\0");
        let v = parse(&out, &resolved_at("/src/herdr"));
        assert_eq!(v["branch"], "feature");
        assert_eq!(v["head"], "1234abcd");
        assert_eq!(v["upstream"], "origin/feature");
        assert_eq!((v["ahead"].as_i64(), v["behind"].as_i64()), (Some(2), Some(1)));
        assert_eq!((v["staged"].as_i64(), v["unstaged"].as_i64(), v["untracked"].as_i64(), v["conflicted"].as_i64()), (Some(3), Some(2), Some(1), Some(1)));
        assert_eq!(v["name"], "herdr");
        assert_eq!(v["linked"], false);
        let files: Vec<(&str, &str)> = v["files"].as_array().unwrap().iter().map(|f| (f["path"].as_str().unwrap(), f["xy"].as_str().unwrap())).collect();
        assert_eq!(files, [("src/staged.rs", "M "), ("with space.rs", " M"), ("both.rs", "MM"), ("new.rs", "R "), ("conflict.rs", "UU"), ("fresh.txt", "??")]);
    }

    #[test]
    fn detached_initial_and_linked_worktrees() {
        let out = "# branch.oid (initial)\0# branch.head (detached)\0";
        let r = Resolved { root: "/w/spike".into(), git_dir: "/src/herdr/.git/worktrees/spike".into(), common_dir: "/src/herdr/.git".into() };
        let v = parse(out, &r);
        assert_eq!((v["branch"].clone(), v["head"].clone(), v["upstream"].clone()), (Value::Null, Value::Null, Value::Null));
        assert_eq!(v["linked"], true);
        assert_eq!(v["name"], "herdr");
        let bare = Resolved { root: "/w/x".into(), git_dir: "/srv/repo.git".into(), common_dir: "/srv/repo.git".into() };
        assert_eq!(parse("", &bare)["name"], "repo");
    }

    #[test]
    fn mods_get_the_file_list_only_when_they_ask() {
        let v = json!({ "branch": "main", "files": [{ "path": "a", "xy": " M" }], "truncated": false });
        assert_eq!(shown(&v, 3, false), json!({ "branch": "main", "revision": 3 }));
        assert_eq!(shown(&v, 3, true)["files"][0]["path"], "a");
        assert_eq!(shown(&Value::Bool(false), 3, true), Value::Bool(false));
    }

    #[test]
    fn events_in_ignored_folders_objects_and_locks_do_not_count() {
        let repo = Repo {
            places: HashSet::new(),
            root: "/r".into(),
            git_dirs: vec!["/r/.git".into(), "/main/.git".into()],
            ignored: HashSet::from([PathBuf::from("/r/target"), PathBuf::from("/r/.DS_Store")]),
        };
        assert!(repo.counts(Path::new("/r/src/main.rs")));
        assert!(repo.counts(Path::new("/r")));
        assert!(repo.counts(Path::new("/r/.git/index")));
        assert!(repo.counts(Path::new("/main/.git/refs/heads/main")));
        assert!(!repo.counts(Path::new("/r/target/debug/build/x")));
        assert!(!repo.counts(Path::new("/r/.DS_Store")));
        assert!(!repo.counts(Path::new("/r/.git/objects/ab/cdef")));
        assert!(!repo.counts(Path::new("/r/.git/index.lock")));
        assert!(!repo.counts(Path::new("/main/.git/logs/HEAD")));
        assert!(!repo.counts(Path::new("/elsewhere/x")));
    }

    #[test]
    fn a_burst_of_events_is_one_look_after_it_settles_and_looks_are_spaced() {
        let (tx, _rx) = mpsc::channel();
        let mut w = Worker { on: None, sink: Box::new(|_| {}), tx, due: HashMap::new(), last: HashMap::new(), resolved: HashMap::new(), watcher: None, repos: HashMap::new() };
        let p = Place { on: None, folder: "/r".into() };
        let t0 = Instant::now();
        w.schedule(p.clone(), t0 + QUIET);
        w.schedule(p.clone(), t0 + QUIET * 3);
        assert_eq!(w.due[&p], t0 + QUIET, "a later event must not put the look off");
        w.due.clear();
        w.last.insert(p.clone(), (t0, Duration::from_millis(500)));
        w.schedule(p.clone(), t0);
        assert_eq!(w.due[&p], t0 + Duration::from_secs(2), "slow repos are looked at less often");
    }

    fn repo(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("herdr-git-status-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let git = |args: &[&str]| assert!(std::process::Command::new("git").arg("-C").arg(&root).args(args).status().unwrap().success());
        git(&["init", "-q", "-b", "main"]);
        std::fs::write(root.join(".gitignore"), "target/\nbuild/\n").unwrap();
        std::fs::write(root.join("a.txt"), "one").unwrap();
        git(&["add", "."]);
        git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "init"]);
        root.canonicalize().unwrap()
    }

    fn next_update(rx: &mpsc::Receiver<Update>, within: Duration) -> Option<Update> {
        rx.recv_timeout(within).ok()
    }

    #[test]
    fn a_watched_repo_is_looked_at_again_when_it_changes_and_not_for_ignored_churn() {
        let root = repo("watch");
        let (tx, rx) = mpsc::channel();
        let worker = Worker::spawn(None, Box::new(move |u| {
            let _ = tx.send(u);
        }));
        let place = Place { on: None, folder: root.join("src").to_string_lossy().into_owned() };
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        worker.send(Msg::Look(place.clone())).unwrap();
        let first = next_update(&rx, Duration::from_secs(10)).expect("first look");
        assert_eq!(first.value["branch"], "main");
        assert_eq!(first.value["unstaged"], 0);
        assert!(first.lasting, "watched");
        // Wait for the first look's spacing to pass: what follows is from the watcher alone.
        std::thread::sleep(MIN_GAP);
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::write(root.join("target/debug/out"), "build").unwrap();
        std::fs::create_dir_all(root.join("build")).unwrap();
        assert!(next_update(&rx, Duration::from_millis(1500)).is_none(), "ignored churn must not run git");
        std::fs::write(root.join("a.txt"), "two").unwrap();
        let after = next_update(&rx, Duration::from_secs(10)).expect("a change on disk is looked at");
        assert_eq!(after.value["unstaged"], 1);
        assert_eq!(after.value["files"][0]["path"], "a.txt");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_folder_outside_any_repo_is_false() {
        let dir = std::env::temp_dir().join(format!("herdr-git-status-none-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (tx, rx) = mpsc::channel();
        let worker = Worker::spawn(None, Box::new(move |u| {
            let _ = tx.send(u);
        }));
        worker.send(Msg::Look(Place { on: None, folder: dir.to_string_lossy().into_owned() })).unwrap();
        let u = next_update(&rx, Duration::from_secs(10)).unwrap();
        assert_eq!(u.value, Value::Bool(false));
        assert!(u.lasting, "git's own no holds");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reads_share_one_look_and_answers_redraw_only_on_change() {
        let (tx, rx) = mpsc::channel();
        WORKERS.with_borrow_mut(|w| w.insert(Some("test-machine".into()), tx));
        let place = Place { on: Some("test-machine".into()), folder: "/r".into() };
        assert_eq!(read(&place, |v, r| (v.clone(), r)), (Value::Null, 0));
        assert_eq!(read(&place, |v, r| (v.clone(), r)), (Value::Null, 0));
        let asked = || rx.try_iter().filter(|m| matches!(m, Msg::Look(_))).count();
        assert_eq!(asked(), 1, "a pending look is not asked for again");
        let waiting = next(&place, None, false);
        let status = json!({ "branch": "main", "files": [] });
        assert!(apply(Update { place: place.clone(), value: status.clone(), lasting: false }));
        assert_eq!(waiting.try_recv().unwrap(), json!({ "status": { "branch": "main", "revision": 1 }, "revision": 1 }));
        assert!(!apply(Update { place: place.clone(), value: status, lasting: false }), "the same answer is no change");
        let later = next(&place, Some(1), true);
        assert!(later.try_recv().is_err());
        refresh(&place);
        assert_eq!(asked(), 1);
        assert!(apply(Update { place: place.clone(), value: json!({ "branch": "other", "files": [] }), lasting: false }));
        assert_eq!(later.try_recv().unwrap()["status"]["files"], json!([]));
        assert_eq!(read(&place, |_, r| r), 2);
    }
}
