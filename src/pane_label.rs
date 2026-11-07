//! What a pane or an agent is called: `label` and `session` on `panes()`,
//! `agents()` and `herdr-app ls`, so every mod names a terminal the same way.
//!
//! `label`, first that applies:
//! 1. the name someone gave the pane (`pane.rename`), when it has a letter or digit;
//! 2. its automatic name (`pane_auto_names`, shown only at a bare shell whose
//!    title says nothing more than a folder);
//! 3. its title, when it is telling (`telling`);
//! 4. the agent session's own name (`session`);
//! 5. the program's name;
//! 6. the terminal's address.
//!
//! `session` is read from the program's session file (`session_name` in the
//! programs table: Claude Code's `~/.claude/sessions/<pid>.json`), off the
//! frame path: once per process, again only when the file's mtime moves.

use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::{Duration, Instant, SystemTime};

/// Titles that name nothing although they read like words: anything holding a
/// UUID, temporary files (`claude-prompt-<uuid>.md`, `tmp.XXXX`) and an editor's
/// own title (` - Nvim`, ` - VIM`). Neovim as Claude Code's Ctrl+G prompt editor
/// leaves `claude-prompt-<uuid>.md + (/private/tmp/…) - Nvim` behind, and Claude
/// Code doesn't set its title again until its next turn.
const THROWAWAY: &str = r"(?i)[0-9a-f]{8}-[0-9a-f]{4}-|^(claude-prompt|tmp)[-.]| - n?vim$";

/// How often terminals running a program with a session file are looked at
/// again when nothing else changed (a `/rename` only touches the file).
pub const SESSION_RECHECK: Duration = Duration::from_secs(3);

/// What a pane's label is made from.
pub struct Parts<'a> {
    pub name: Option<&'a str>,
    pub auto_name: Option<&'a str>,
    pub title: &'a str,
    pub session: Option<&'a str>,
    /// `{ key, name, … }` from the programs table.
    pub program: &'a Value,
    pub address: &'a str,
}

#[cfg(test)]
thread_local! {
    /// How many labels this thread resolved: they follow changes, not frames.
    pub static RESOLVED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub fn label(p: &Parts) -> String {
    #[cfg(test)]
    RESOLVED.with(|n| n.set(n.get() + 1));
    given(p.name)
        .or(p.auto_name)
        .or_else(|| telling(p.title, p.program))
        .or(p.session)
        .or_else(|| p.program.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()))
        .unwrap_or(p.address)
        .to_owned()
}

/// A name someone gave: it counts once it has a letter or digit (a stray `.`
/// or a pasted spinner glyph doesn't).
pub fn given(name: Option<&str>) -> Option<&str> {
    name.filter(|n| n.chars().any(char::is_alphanumeric))
}

/// The title, trimmed, when it says something the program's name doesn't. It is
/// judged without what leads up to its first letter, digit or path (spinners
/// are already gone, `terminal::shown_title`; a status glyph such as `✓` or a
/// `[3/5]` stays in what is shown): not empty or only digits, not the
/// program's name, not its command line (a first word that is its key or
/// name: `claude attach 1f2e`, `CLAUDE`), not a path (`~`, `/`, `C:\`, `C:/`,
/// `\\server`), no `@` (`user@host`), and not `THROWAWAY`.
pub fn telling<'a>(title: &'a str, program: &Value) -> Option<&'a str> {
    static THROWAWAY_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(THROWAWAY).expect("THROWAWAY"));
    let t = title.trim_start_matches(|c: char| !c.is_alphanumeric() && !matches!(c, '~' | '/' | '\\')).trim_end();
    let name = program.get("name").and_then(Value::as_str).unwrap_or_default();
    let key = program.get("key").and_then(Value::as_str).unwrap_or_default();
    let first = t.split_whitespace().next().unwrap_or_default().rsplit(['/', '\\']).next().unwrap_or_default().to_lowercase();
    let invokes = !first.is_empty() && (first == key || first == name.to_lowercase());
    let b = t.as_bytes();
    let path = t.starts_with(['~', '/', '\\']) || (b.len() > 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/'));
    let empty_or_digits = t.bytes().all(|c| c.is_ascii_digit());
    let says_nothing = empty_or_digits || t.to_lowercase() == name.to_lowercase() || invokes || path || t.contains('@');
    (!says_nothing && !THROWAWAY_RE.is_match(t)).then(|| title.trim())
}

/// One terminal's session file, to look at off the main thread.
#[derive(Clone, Debug, PartialEq)]
pub struct Check {
    pub address: String,
    pub pid: u64,
    pub path: PathBuf,
    pub source: crate::resume::SessionFile,
    /// The file's mtime when last looked at for this process (None: never, or no file).
    pub read_at: Option<SystemTime>,
}

/// What a `Check` found: the file's mtime and, when it was read, the name.
#[derive(Debug)]
pub struct Found {
    pub check: Check,
    pub modified: Option<SystemTime>,
    /// None: unchanged since `read_at`, not read again.
    pub name: Option<Option<String>>,
}

/// Looks at a session file: reads it only when its mtime moved since the last
/// look. A missing or unreadable file names no session.
pub fn look(check: Check) -> Found {
    let modified = std::fs::metadata(&check.path).and_then(|m| m.modified()).ok();
    let name = (modified != check.read_at)
        .then(|| std::fs::read_to_string(&check.path).ok().and_then(|text| name_in(&check.source, &text)));
    Found { check, modified, name }
}

/// The session's name in a session file's text: its `field`, when `when` holds
/// and it has a letter or digit.
pub fn name_in(source: &crate::resume::SessionFile, text: &str) -> Option<String> {
    let record: Value = serde_json::from_str(text).ok()?;
    if source.when.iter().any(|(field, want)| record.get(field) != Some(want)) {
        return None;
    }
    let name = record.get(&source.field)?.as_str()?.trim();
    given(Some(name)).map(str::to_owned)
}

#[derive(Debug)]
struct Known {
    pid: u64,
    modified: Option<SystemTime>,
    name: Option<String>,
}

/// The session names the app knows, by terminal address.
#[derive(Default)]
pub struct Sessions {
    known: HashMap<String, Known>,
    checked: Option<Instant>,
    pending: bool,
}

impl Sessions {
    /// The session name of the process `pid` running in a terminal: none once
    /// another runs there, before the next look has even forgotten it.
    pub fn name(&self, address: &str, pid: Option<u64>) -> Option<&str> {
        self.known.get(address).filter(|k| Some(k.pid) == pid)?.name.as_deref()
    }

    /// The session files to look at now: right after a listing (`listed`),
    /// else every `SESSION_RECHECK`, never while a look is under way. Each
    /// terminal on this machine whose foreground program has a `session_name`
    /// source (`source_of` its command line); terminals that stopped running
    /// one are forgotten.
    pub fn checks(
        &mut self,
        live: &HashMap<String, Value>,
        listed: bool,
        now: Instant,
        home: &str,
        source_of: impl Fn(&[String]) -> Option<crate::resume::SessionFile>,
    ) -> Vec<Check> {
        if self.pending || !(listed || self.checked.is_none_or(|at| now.duration_since(at) >= SESSION_RECHECK)) {
            return Vec::new();
        }
        self.checked = Some(now);
        let mut checks = Vec::new();
        for (address, rec) in live {
            // Another machine's files are there, not here.
            if address.contains('/') {
                continue;
            }
            let pid = rec.pointer("/foreground/pid").and_then(Value::as_u64);
            let Some((pid, source)) = pid.zip(source_of(&crate::resume::foreground_argv(rec))) else { continue };
            let path = PathBuf::from(crate::resume::session_path(&source.file, pid, Some(home)));
            let read_at = self.known.get(address).filter(|k| k.pid == pid).and_then(|k| k.modified);
            checks.push(Check { address: address.clone(), pid, path, source, read_at });
        }
        let running: Vec<&String> = checks.iter().map(|c| &c.address).collect();
        self.known.retain(|a, _| running.contains(&a));
        self.pending = !checks.is_empty();
        checks
    }

    /// What the looks found; true when a name changed.
    pub fn found(&mut self, found: Vec<Found>) -> bool {
        self.pending = false;
        let mut changed = false;
        for f in found {
            let Check { address, pid, .. } = f.check;
            let known = self.known.entry(address).or_insert(Known { pid, modified: None, name: None });
            if known.pid != pid {
                *known = Known { pid, modified: None, name: None };
                changed = true;
            }
            known.modified = f.modified;
            if let Some(name) = f.name {
                changed |= known.name != name;
                known.name = name;
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn claude() -> Value {
        json!({ "key": "claude", "name": "Claude Code", "agent": true, "shell": false })
    }

    fn parts<'a>(program: &'a Value) -> Parts<'a> {
        Parts { name: None, auto_name: None, title: "", session: None, program, address: "t7" }
    }

    #[test]
    fn telling_titles_are_shown_as_the_program_set_them() {
        let c = claude();
        assert_eq!(telling("  Herdr session names  ", &c), Some("Herdr session names"));
        // Judged without what leads up to the first letter or digit, shown with it.
        for t in ["\u{2713} build passed", "[3/5] Building crates", "#42 Fix", "\"quoted\""] {
            assert_eq!(telling(t, &c), Some(t), "{t}");
        }
        // These stay telling: a word in them only looks like a reason to reject.
        for t in ["Fix nvim config", "tmpfs mount issue", "Fix bug in commit 1f2e3d4", "vim tips", "a - b", "Plan for 2027"] {
            assert_eq!(telling(t, &c), Some(t), "{t}");
        }
    }

    #[test]
    fn titles_that_say_nothing_are_not_telling() {
        let c = claude();
        let nvim = "claude-prompt-9acc2eb1-37f2-4d7e-9c1a-5b6f0e2d8a11.md + (/private/tmp/claude-501) - Nvim";
        for (t, why) in [
            ("", "empty"),
            ("\u{2733} ", "only a spinner"),
            ("Claude Code", "the program's name"),
            ("CLAUDE", "the program's key"),
            ("~/src/herdr", "a home path"),
            ("/tmp", "a path"),
            ("C:\\src", "a Windows path"),
            ("C:/src", "a Windows path with forward slashes"),
            ("\\\\server\\share", "a UNC path"),
            ("\u{2713} ~/src", "a path after a glyph"),
            ("\u{2713} 1234", "digits after a glyph"),
            ("me@host: ~/src", "user@host"),
            ("1234", "only digits"),
            ("claude attach 1f2e", "the program's command line"),
            ("node_modules/.bin/claude -c", "its command line by path"),
            ("claude code --help", "its name as a command line"),
            (nvim, "Neovim's title for Claude Code's prompt file"),
            ("notes.md - VIM", "Vim's title"),
            ("tmp.XyZ12", "a temporary file"),
            ("claude-prompt.md", "Claude Code's prompt file"),
            ("session 9acc2eb1-37f2-4d7e", "a UUID"),
        ] {
            assert_eq!(telling(t, &c), None, "{why}: {t:?}");
        }
        // A program whose name isn't its key.
        assert_eq!(telling("neovim notes.md", &json!({ "key": "nvim", "name": "Neovim" })), None);
        let lazygit = json!({ "key": "lazygit", "name": "Git Changes" });
        assert_eq!(telling("Git Changes", &lazygit), None);
        assert_eq!(telling("git changes", &lazygit), None, "its name in any case");
        assert_eq!(telling("lazygit", &lazygit), None);
        assert_eq!(telling("Git log", &lazygit), Some("Git log"));
    }

    #[test]
    fn a_label_takes_the_first_that_applies() {
        let c = claude();
        let p = || parts(&c);
        assert_eq!(label(&Parts { name: Some("review"), auto_name: Some("hazy-inlet"), title: "Fix it", session: Some("s"), ..p() }), "review");
        assert_eq!(label(&Parts { name: Some(" . "), auto_name: Some("hazy-inlet"), ..p() }), "hazy-inlet", "a name without a letter or digit is none");
        assert_eq!(label(&Parts { auto_name: Some("hazy-inlet"), title: "Fix it", session: Some("s"), ..p() }), "hazy-inlet");
        assert_eq!(label(&Parts { title: "\u{2713} Fix it", session: Some("vendor-insights"), ..p() }), "\u{2713} Fix it");
        assert_eq!(label(&Parts { title: "claude-prompt-9acc2eb1-37f2-4d7e-9c1a-5b6f0e2d8a11.md + (/private/tmp/x) - Nvim", session: Some("vendor-insights"), ..p() }), "vendor-insights");
        assert_eq!(label(&Parts { title: "~/src", ..p() }), "Claude Code");
        assert_eq!(label(&Parts { title: "~/src", program: &json!({ "key": "program", "name": "" }), ..p() }), "t7");
        assert_eq!(label(&Parts { program: &Value::Null, ..p() }), "t7");
    }

    fn source() -> crate::resume::SessionFile {
        serde_json::from_value(json!({ "file": "~/.claude/sessions/{pid}.json", "field": "name" })).unwrap()
    }

    #[test]
    fn a_session_files_name_counts_when_it_names_something() {
        let s = source();
        assert_eq!(name_in(&s, r#"{"pid":1,"name":" vendor-insights ","kind":"interactive"}"#).as_deref(), Some("vendor-insights"));
        assert_eq!(name_in(&s, r#"{"pid":1}"#), None);
        assert_eq!(name_in(&s, r#"{"name":"..."}"#), None);
        assert_eq!(name_in(&s, r#"{"name":7}"#), None);
        assert_eq!(name_in(&s, "{not json"), None);
        let only_interactive: crate::resume::SessionFile = serde_json::from_value(json!({ "file": "x", "field": "name", "when": { "kind": "interactive" } })).unwrap();
        assert_eq!(name_in(&only_interactive, r#"{"name":"a","kind":"interactive"}"#).as_deref(), Some("a"));
        assert_eq!(name_in(&only_interactive, r#"{"name":"a","kind":"print"}"#), None);
    }

    fn live(pid: u64, argv: &[&str]) -> Value {
        json!({ "pid": 1, "foreground": { "pid": pid, "argv": argv } })
    }

    #[test]
    fn session_names_are_read_once_per_process_and_again_when_the_file_changes() {
        let home = std::env::temp_dir().join(format!("herdr-session-names-{}", std::process::id()));
        let dir = home.join(".claude/sessions");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("4242.json");
        std::fs::write(&file, r#"{"name":"vendor-insights"}"#).unwrap();
        let home_s = home.to_str().unwrap();
        let source_of = |argv: &[String]| (argv.first().map(String::as_str) == Some("claude")).then(source);
        let mut sessions = Sessions::default();
        let t0 = Instant::now();
        let terminals = HashMap::from([
            ("t1".to_owned(), live(4242, &["claude"])),
            ("t2".to_owned(), live(99, &["zsh"])),
            ("mac/t3".to_owned(), live(4242, &["claude"])),
        ]);
        let run = |s: &mut Sessions, listed: bool, at: Instant| {
            let checks = s.checks(&terminals, listed, at, home_s, source_of);
            let n = checks.len();
            let found: Vec<Found> = checks.into_iter().map(look).collect();
            let reads = found.iter().filter(|f| f.name.is_some()).count();
            (s.found(found), n, reads)
        };

        let checks = sessions.checks(&terminals, true, t0, home_s, source_of);
        assert_eq!(checks.len(), 1, "only the local terminal running claude: {checks:?}");
        assert_eq!(checks[0].path, file);
        assert!(sessions.checks(&terminals, true, t0, home_s, source_of).is_empty(), "never while a look is under way");
        assert!(sessions.found(checks.into_iter().map(look).collect()));
        assert_eq!(sessions.name("t1", Some(4242)), Some("vendor-insights"));

        assert_eq!(run(&mut sessions, false, t0 + Duration::from_secs(1)), (false, 0, 0), "not before SESSION_RECHECK");
        assert_eq!(run(&mut sessions, true, t0 + Duration::from_secs(1)), (false, 1, 0), "a listing looks; an unchanged file isn't read");

        std::fs::write(&file, r#"{"name":"renamed"}"#).unwrap();
        let later = SystemTime::now() + Duration::from_secs(5);
        std::fs::File::options().write(true).open(&file).unwrap().set_modified(later).unwrap();
        assert_eq!(run(&mut sessions, false, t0 + Duration::from_secs(1) + SESSION_RECHECK), (true, 1, 1), "a moved mtime is read again");
        assert_eq!(sessions.name("t1", Some(4242)), Some("renamed"));

        // Another process in the same terminal: its own file, read afresh, and not
        // again while it doesn't change.
        let next_file = dir.join("5151.json");
        std::fs::write(&next_file, r#"{"name":"next"}"#).unwrap();
        let next = HashMap::from([("t1".to_owned(), live(5151, &["claude"]))]);
        let checks = sessions.checks(&next, true, t0, home_s, source_of);
        assert_eq!(checks[0].read_at, None, "the mtime read for 4242 is not 5151's");
        assert!(sessions.found(checks.into_iter().map(look).collect()));
        assert_eq!(sessions.name("t1", Some(5151)), Some("next"));
        let checks = sessions.checks(&next, true, t0, home_s, source_of);
        assert!(checks[0].read_at.is_some());
        assert!(!sessions.found(checks.into_iter().map(look).collect()), "same process, same file: unchanged");

        let mut moved = later;
        let mut rewrite = |text: &str| {
            std::fs::write(&next_file, text).unwrap();
            moved += Duration::from_secs(5);
            std::fs::File::options().write(true).open(&next_file).unwrap().set_modified(moved).unwrap();
        };
        let look_next = |s: &mut Sessions| {
            let checks = s.checks(&next, true, t0, home_s, source_of);
            let found: Vec<Found> = checks.into_iter().map(look).collect();
            let reads = found.iter().filter(|f| f.name.is_some()).count();
            (s.found(found), reads)
        };
        rewrite("{bad json");
        assert_eq!(look_next(&mut sessions), (true, 1));
        assert_eq!(sessions.name("t1", Some(5151)), None, "bad JSON names no session");
        rewrite(r#"{"name":"back"}"#);
        assert_eq!(look_next(&mut sessions), (true, 1));
        std::fs::remove_file(&next_file).unwrap();
        assert_eq!(look_next(&mut sessions), (true, 1), "a file that went names none");
        assert_eq!(sessions.name("t1", Some(5151)), None);
        assert_eq!(look_next(&mut sessions), (false, 0), "and isn't read while it stays gone");

        // A process without a file: the name it had goes with the process.
        assert_eq!(run(&mut sessions, true, t0), (true, 1, 1));
        assert_eq!(sessions.name("t1", Some(4242)), Some("renamed"));
        assert_eq!(sessions.name("t1", Some(31)), None, "another process before any look: not its name");
        assert_eq!(sessions.name("t1", None), None, "nothing in the foreground");
        let silent = HashMap::from([("t1".to_owned(), live(6161, &["claude"]))]);
        let checks = sessions.checks(&silent, true, t0, home_s, source_of);
        assert!(sessions.found(checks.into_iter().map(look).collect()));
        assert_eq!(sessions.name("t1", Some(6161)), None);

        // It stopped running claude: forgotten.
        let shell = HashMap::from([("t1".to_owned(), live(77, &["zsh"]))]);
        assert!(sessions.checks(&shell, true, t0, home_s, source_of).is_empty());
        assert_eq!(sessions.name("t1", Some(77)), None);
        std::fs::remove_dir_all(&home).unwrap();
    }
}
