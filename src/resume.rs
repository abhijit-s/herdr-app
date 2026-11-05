//! Programs come back after a reboot. herdr-core brings kept terminals back at
//! their addresses as fresh shells in their last folder; what ran in them is
//! gone. Programs with a `resume` rule in the programs table (programs.rs) are
//! recorded while they run, in `data/resume.json` in the home folder, as the
//! command line that resumes them; `session.resume` types it again into
//! terminals that are back to a bare shell.
//!
//! A record lives as long as its program could still come back: it is
//! replaced when another resumable program (or another session) runs in that
//! terminal, and removed only when the app sees the program end in a terminal
//! that kept running (the person quit it) or sees the terminal itself go. A
//! reboot or a core restart kills programs out of sight, or with the terminal's
//! own process replaced, so their records stay.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

/// Where a rule's `append` takes the session id.
pub const SESSION_ID: &str = "{session_id}";
/// Where a session file's path takes the program's process id.
pub const PID: &str = "{pid}";
/// A label an agent (its hook) sets on its terminal to report its session:
/// `herdr.session.<program key>=<id>`. It wins over the session file.
pub const REPORTED_LABEL: &str = "herdr.session.";
const LONGEST_ID: usize = 256;

/// How a program in the programs table resumes (`"resume"` on its entry).
#[derive(Deserialize, Serialize, Clone, Default, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    /// Where its session id is, when it doesn't report one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionFile>,
    /// Flags taken out of the command line it ran with, and how many values
    /// each takes at most (a value never starts with `-`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub drop: BTreeMap<String, u8>,
    /// Words added at the end; `{session_id}` is the session.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub append: Vec<String>,
    /// It resumes by running the same command line again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub rerun: bool,
}

/// A JSON file per process that names the session it runs.
#[derive(Deserialize, Serialize, Clone, Default, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SessionFile {
    /// Its path; `{pid}` is the program's process id, `~/` its machine's home.
    pub file: String,
    /// The field holding the session id.
    pub field: String,
    /// Fields that must hold exactly these values, or there's nothing to resume.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub when: BTreeMap<String, Value>,
}

impl Rule {
    /// `{}`: no rule (a person's `programs.json` turns a shipped one off this way).
    pub fn is_off(&self) -> bool {
        !self.rerun && self.session.is_none() && self.drop.is_empty() && self.append.is_empty()
    }

    /// What's wrong with it, checked when the table loads.
    pub fn check(&self) -> Result<(), String> {
        for word in &self.append {
            let literal = word.replace(SESSION_ID, "");
            if !literal.chars().all(id_char) {
                return Err(format!("append word {word:?} may only use letters, digits and _ . / : = + -"));
            }
        }
        if let Some(s) = &self.session {
            if s.field.is_empty() {
                return Err("session.field is empty".into());
            }
            if s.file.is_empty() {
                return Err("session.file is empty".into());
            }
        }
        Ok(())
    }

    fn needs_session(&self) -> bool {
        self.append.iter().any(|w| w.contains(SESSION_ID))
    }
}

fn id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "_./:=+-".contains(c)
}

/// A session id safe to put on a command line as it is: never a flag, never
/// anything a shell reads.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= LONGEST_ID && !id.starts_with('-') && id.chars().all(id_char)
}

/// The session a session file names, if its `when` holds.
pub fn session_in(source: &SessionFile, text: &str) -> Result<String, String> {
    let record: Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
    for (field, want) in &source.when {
        if record.get(field) != Some(want) {
            return Err(format!("{field} is {}, not {want}", record.get(field).unwrap_or(&Value::Null)));
        }
    }
    let id = record.get(&source.field).and_then(Value::as_str).ok_or_else(|| format!("no {} in it", source.field))?;
    if !valid_id(id) {
        return Err(format!("{} {id:?} is not a session id", source.field));
    }
    Ok(id.to_owned())
}

/// The session file's path for a process: on this machine, `~/` is `home`; on
/// another, the path is left relative, which that machine reads from its home.
pub fn session_path(file: &str, pid: u64, home: Option<&str>) -> String {
    let path = file.replace(PID, &pid.to_string());
    match (path.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => format!("{}/{rest}", home.trim_end_matches('/')),
        (Some(rest), None) => rest.to_owned(),
        (None, _) => path,
    }
}

/// The command line that resumes it: `argv` without the dropped flags (`--flag
/// value`, or `--flag=value` alone), then `append`.
pub fn rewrite(argv: &[String], rule: &Rule, session: Option<&str>) -> Result<Vec<String>, String> {
    if rule.needs_session() && session.is_none() {
        return Err("no session to resume".into());
    }
    let mut kept = Vec::new();
    let mut skip = 0;
    for (i, word) in argv.iter().enumerate() {
        // A flag's value is never itself a flag: `-r` alone opens a picker.
        if skip > 0 && !word.starts_with('-') {
            skip -= 1;
            continue;
        }
        skip = 0;
        let (flag, inline) = match word.split_once('=') {
            Some((flag, _)) => (flag, true),
            None => (word.as_str(), false),
        };
        // The program itself is never a flag to drop.
        if i > 0 && let Some(&arity) = rule.drop.get(flag) {
            skip = if inline { 0 } else { arity };
            continue;
        }
        kept.push(word.clone());
    }
    kept.extend(rule.append.iter().map(|w| w.replace(SESSION_ID, session.unwrap_or_default())));
    Ok(kept)
}

/// `cd <cwd> && <argv>` for the shell in the terminal, each word quoted.
pub fn command_line(cwd: &str, argv: &[String], powershell: bool) -> String {
    if powershell {
        let words: Vec<String> = argv.to_vec();
        return format!("Set-Location -LiteralPath {}; {}", crate::programs::powershell_line(&[cwd.to_owned()]).trim_start_matches("& "), crate::programs::powershell_line(&words));
    }
    let words: Vec<String> = argv.iter().map(|w| sh_quote(w)).collect();
    format!("cd {} && {}", sh_quote(cwd), words.join(" "))
}

fn sh_quote(word: &str) -> String {
    if !word.is_empty() && word.chars().all(|c| c.is_ascii_alphanumeric() || "_./:=+,@%-".contains(c)) {
        return word.to_owned();
    }
    format!("'{}'", word.replace('\'', "'\\''"))
}

/// One resumable program, as it last ran in a terminal.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Record {
    pub terminal: String,
    /// The terminal's `where` label ("space / tab"), when it had one.
    #[serde(default, rename = "where", skip_serializing_if = "Option::is_none")]
    pub place: Option<String>,
    /// Its key in the programs table, and the name it shows.
    pub program: String,
    pub name: String,
    pub cwd: String,
    /// The command line that resumes it.
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// When it was saved (ms since the epoch).
    pub saved_ms: u64,
}

#[derive(Serialize, Deserialize, Default)]
struct File {
    #[serde(default)]
    sessions: Vec<Record>,
}

/// A resumable program the app just saw start or move on (another turn, a
/// reported session): what to look up to record it.
#[derive(Clone, Debug)]
pub struct Lookup {
    pub terminal: String,
    pub key: String,
    pub name: String,
    pub rule: Rule,
    pub argv: Vec<String>,
    pub pid: u64,
    pub cwd: String,
    pub place: Option<String>,
    /// The session the agent reported (its `herdr.session.<key>` label).
    pub reported: Option<String>,
}

impl Lookup {
    pub fn machine(&self) -> Option<&str> {
        self.terminal.split_once('/').map(|(m, _)| m)
    }
}

/// What the app last saw in a terminal.
#[derive(Clone, PartialEq, Default)]
struct Seen {
    running: Option<Running>,
    terminal_pid: Option<u64>,
}

#[derive(Clone, PartialEq)]
struct Running {
    key: String,
    pid: u64,
    turn: u64,
    reported: Option<String>,
}

/// The records, and what this run of the app has seen of their terminals.
#[derive(Default)]
pub struct Tracker {
    records: BTreeMap<String, Record>,
    seen: HashMap<String, Seen>,
}

/// Why a record can't be resumed now, or `None`: it can.
pub type Readiness = Option<String>;

impl Tracker {
    pub fn load(path: &std::path::Path) -> Tracker {
        let file: File = std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        Tracker { records: file.sessions.into_iter().map(|r| (r.terminal.clone(), r)).collect(), seen: HashMap::new() }
    }

    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = File { sessions: self.records.values().cloned().collect() };
        crate::persist::write_atomic(path, serde_json::to_string_pretty(&file).unwrap_or_default() + "\n")
    }

    #[cfg(test)]
    pub fn records(&self) -> impl Iterator<Item = &Record> {
        self.records.values()
    }

    #[cfg(test)]
    pub fn insert(&mut self, record: Record) {
        self.records.insert(record.terminal.clone(), record);
    }

    /// The core is another one, or back: what ran before isn't known any more.
    pub fn forget_seen(&mut self) {
        self.seen.clear();
    }

    /// A listing of the live terminals (address to the core's record): the
    /// programs to look up, and whether records went.
    pub fn observe(&mut self, live: &HashMap<String, Value>, rule_of: impl Fn(&[String]) -> Option<(String, String, Rule)>) -> (Vec<Lookup>, bool) {
        let mut changed = false;
        let gone: Vec<String> = self.seen.keys().filter(|a| !live.contains_key(*a)).cloned().collect();
        for addr in gone {
            self.seen.remove(&addr);
            changed |= self.records.remove(&addr).is_some();
        }
        let mut lookups = Vec::new();
        for (addr, rec) in live {
            let terminal_pid = rec.get("pid").and_then(Value::as_u64);
            let fg = rec.get("foreground");
            let argv = foreground_argv(rec);
            let pid = fg.and_then(|f| f.get("pid")).and_then(Value::as_u64);
            let resumable = pid.and_then(|pid| rule_of(&argv).map(|r| (pid, r)));
            let before = self.seen.get(addr).cloned().unwrap_or_default();
            let running = match resumable {
                Some((pid, (key, name, rule))) => {
                    let label = rec.pointer("/labels").and_then(|l| l.get(format!("{REPORTED_LABEL}{key}"))).and_then(Value::as_str);
                    let reported = label.and_then(|l| reported_session(l, pid));
                    let turn = rec.get("turn").and_then(Value::as_u64).unwrap_or(0);
                    let running = Running { key: key.clone(), pid, turn, reported: reported.clone() };
                    if before.running.as_ref() != Some(&running) {
                        let cwd = fg.and_then(|f| f.get("cwd")).or_else(|| rec.get("cwd")).and_then(Value::as_str).unwrap_or_default().to_owned();
                        let place = rec.pointer("/labels/where").and_then(Value::as_str).filter(|w| !w.is_empty()).map(str::to_owned);
                        lookups.push(Lookup { terminal: addr.clone(), key, name, rule, argv, pid, cwd, place, reported });
                    }
                    Some(running)
                }
                None => {
                    // It ended in a terminal that went on running: the person quit it.
                    if before.running.is_some() && terminal_pid.is_some() && before.terminal_pid == terminal_pid {
                        changed |= self.records.remove(addr).is_some();
                    }
                    None
                }
            };
            self.seen.insert(addr.clone(), Seen { running, terminal_pid });
        }
        (lookups, changed)
    }

    /// A lookup's answer: the session it found (or why not). Whether a record changed.
    pub fn found(&mut self, lookup: &Lookup, session: Result<Option<String>, String>, now_ms: u64) -> bool {
        let current = self.seen.get(&lookup.terminal).and_then(|s| s.running.as_ref()).is_some_and(|r| r.pid == lookup.pid && r.key == lookup.key);
        if !current {
            return false;
        }
        let session = match session {
            Ok(s) => s,
            Err(why) => {
                tracing::debug!("{} in {}: not recorded: {why}", lookup.name, lookup.terminal);
                return false;
            }
        };
        let argv = match rewrite(&lookup.argv, &lookup.rule, session.as_deref()) {
            Ok(argv) => argv,
            Err(why) => {
                tracing::debug!("{} in {}: not recorded: {why}", lookup.name, lookup.terminal);
                return false;
            }
        };
        let record = Record {
            terminal: lookup.terminal.clone(),
            place: lookup.place.clone(),
            program: lookup.key.clone(),
            name: lookup.name.clone(),
            cwd: lookup.cwd.clone(),
            argv,
            session,
            saved_ms: now_ms,
        };
        let same = self.records.get(&lookup.terminal).is_some_and(|r| Record { saved_ms: r.saved_ms, ..record.clone() } == *r);
        if !same {
            self.records.insert(lookup.terminal.clone(), record);
        }
        !same
    }

    /// Each record, and why it can't be resumed now (`None`: it can): its
    /// terminal must be running a bare shell, and its session must be its own.
    pub fn readiness(&self, live: &HashMap<String, Value>, is_shell: impl Fn(&[String]) -> bool) -> Vec<(&Record, Readiness)> {
        let mut owners: HashMap<(&str, &str), usize> = HashMap::new();
        for r in self.records.values() {
            if let Some(s) = &r.session {
                *owners.entry((r.program.as_str(), s.as_str())).or_default() += 1;
            }
        }
        self.records
            .values()
            .map(|r| {
                let why = match live.get(&r.terminal) {
                    None => Some("its terminal isn't running".to_owned()),
                    Some(rec) => {
                        let argv = foreground_argv(rec);
                        if !argv.is_empty() && !is_shell(&argv) {
                            Some(format!("its terminal runs {}", crate::programs::of(None, &argv)["name"].as_str().unwrap_or("a program")))
                        } else if r.session.as_ref().is_some_and(|s| owners[&(r.program.as_str(), s.as_str())] > 1) {
                            Some("another terminal has the same session".to_owned())
                        } else if r.session.as_deref().is_some_and(|s| !valid_id(s)) {
                            Some("its session id is not valid".to_owned())
                        } else {
                            None
                        }
                    }
                };
                (r, why)
            })
            .collect()
    }
}

/// A `herdr.session.<key>` label's session for the program with process id
/// `pid`: `<id>`, or `<id>@<pid>` from a hook that names its program (a nested
/// `claude -p` reports the same terminal, with its own pid). Empty: cleared.
fn reported_session(label: &str, pid: u64) -> Option<String> {
    let id = match label.rsplit_once('@') {
        Some((id, of)) if of.parse::<u64>().is_ok() => (of.parse::<u64>().ok() == Some(pid)).then_some(id)?,
        _ => label,
    };
    (!id.is_empty()).then(|| id.to_owned())
}

/// The session a lookup's program runs: the one it reported, else the one
/// its session file names, read where the program runs (`read(path, machine)`;
/// on this machine `~/` is `home`). `None`: the rule needs none.
pub fn find_session(l: &Lookup, home: &str, read: impl FnOnce(&str, Option<&str>) -> Result<String, String>) -> Result<Option<String>, String> {
    if let Some(id) = &l.reported {
        return if valid_id(id) { Ok(Some(id.clone())) } else { Err(format!("reported session {id:?} is not a session id")) };
    }
    let Some(source) = &l.rule.session else { return Ok(None) };
    let machine = l.machine();
    let path = session_path(&source.file, l.pid, machine.is_none().then_some(home));
    session_in(source, &read(&path, machine)?).map(Some)
}

/// What runs in the terminal now, by the core's `foreground`.
pub fn foreground_argv(rec: &Value) -> Vec<String> {
    rec.pointer("/foreground/argv").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default()
}

/// A record as `session.saved` shows it.
pub fn shown(r: &Record, why: &Readiness) -> Value {
    json!({
        "terminal": r.terminal, "where": r.place, "program": r.program, "name": r.name,
        "cwd": r.cwd, "argv": r.argv, "session": r.session, "saved_ms": r.saved_ms,
        "ready": why.is_none(), "why": why,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &[&str]) -> Vec<String> {
        s.iter().map(|w| w.to_string()).collect()
    }

    fn claude() -> Rule {
        serde_json::from_value(json!({
            "session": { "file": "~/.claude/sessions/{pid}.json", "field": "sessionId", "when": { "kind": "interactive" } },
            "drop": { "--resume": 1, "-r": 1, "--session-id": 1, "--continue": 0, "-c": 0 },
            "append": ["--resume", "{session_id}"]
        }))
        .unwrap()
    }

    #[test]
    fn session_picking_flags_go_in_every_form_and_the_rest_stays() {
        let rule = claude();
        let cases: &[(&[&str], &[&str])] = &[
            (&["claude"], &["claude"]),
            (&["claude", "--model", "opus", "--resume", "old"], &["claude", "--model", "opus"]),
            (&["claude", "-r", "old", "--dangerously-skip-permissions"], &["claude", "--dangerously-skip-permissions"]),
            (&["claude", "--resume=old", "-c", "--session-id", "x", "--continue"], &["claude"]),
            (&["claude", "--session-id=x", "--add-dir", "/a b"], &["claude", "--add-dir", "/a b"]),
            (&["/opt/bin/claude", "-c", "--model=sonnet"], &["/opt/bin/claude", "--model=sonnet"]),
            // A value-taking flag at the end, or before another flag, takes no value.
            (&["claude", "--resume"], &["claude"]),
            (&["claude", "-r", "--model", "opus"], &["claude", "--model", "opus"]),
        ];
        for (argv, kept) in cases {
            let mut want = words(kept);
            want.extend(words(&["--resume", "abc-1"]));
            assert_eq!(rewrite(&words(argv), &rule, Some("abc-1")).unwrap(), want, "{argv:?}");
        }
        assert!(rewrite(&words(&["claude"]), &rule, None).is_err(), "it needs a session");
        let rerun = Rule { rerun: true, ..Rule::default() };
        assert_eq!(rewrite(&words(&["htop", "-d", "5"]), &rerun, None).unwrap(), words(&["htop", "-d", "5"]));
        assert!(Rule::default().is_off() && !rerun.is_off());
    }

    #[test]
    fn session_ids_are_never_flags_or_shell() {
        for ok in ["ad9fab11-7f52-4024-b4c0-a6e6ae39f851", "a_b.c/d:e=f+g", "x"] {
            assert!(valid_id(ok), "{ok}");
        }
        for bad in ["", "-rf", "a b", "a;b", "$(x)", "a'b", "\u{e9}", &"a".repeat(257)] {
            assert!(!valid_id(bad), "{bad}");
        }
        let bad_rule = Rule { append: words(&["--resume", "{session_id}", "$(boom)"]), ..Rule::default() };
        assert!(bad_rule.check().is_err());
        assert!(claude().check().is_ok());
    }

    #[test]
    fn a_session_file_counts_only_when_its_fields_match() {
        let source = claude().session.unwrap();
        assert_eq!(session_in(&source, r#"{"sessionId":"s1","kind":"interactive","pid":7}"#).unwrap(), "s1");
        assert!(session_in(&source, r#"{"sessionId":"s1","kind":"bg"}"#).unwrap_err().contains("kind"));
        assert!(session_in(&source, r#"{"sessionId":"s1"}"#).is_err(), "no kind: no match");
        assert!(session_in(&source, r#"{"kind":"interactive"}"#).is_err());
        assert!(session_in(&source, r#"{"sessionId":"--evil","kind":"interactive"}"#).is_err());
        assert!(session_in(&source, "not json").is_err());
        assert_eq!(session_path(&source.file, 42, Some("/home/me/")), "/home/me/.claude/sessions/42.json");
        assert_eq!(session_path(&source.file, 42, None), ".claude/sessions/42.json", "another machine reads it from its home");
        assert_eq!(session_path("/abs/{pid}", 1, Some("/h")), "/abs/1");
    }

    #[test]
    fn command_lines_quote_every_word_but_the_safe_ones() {
        let argv = words(&["claude", "--append-system-prompt", "it's $HOME", "--resume", "s1"]);
        assert_eq!(command_line("/a b/c", &argv, false), r#"cd '/a b/c' && claude --append-system-prompt 'it'\''s $HOME' --resume s1"#);
        assert_eq!(command_line("C:\\it's", &words(&["claude", "--resume", "s1"]), true), "Set-Location -LiteralPath 'C:\\it''s'; & 'claude' '--resume' 's1'");
    }

    fn term(pid: u64, fg: Option<(u64, &[&str])>, turn: u64) -> Value {
        let mut t = json!({ "pid": pid, "turn": turn, "cwd": "/w", "labels": { "where": "s / tab" } });
        if let Some((fpid, argv)) = fg {
            t["foreground"] = json!({ "pid": fpid, "argv": argv, "cwd": "/w/sub" });
        }
        t
    }

    fn rule_of(argv: &[String]) -> Option<(String, String, Rule)> {
        (argv.first().map(String::as_str) == Some("claude")).then(|| ("claude".to_owned(), "Claude".to_owned(), claude()))
    }

    fn listing(terms: &[(&str, Value)]) -> HashMap<String, Value> {
        terms.iter().map(|(a, t)| (a.to_string(), t.clone())).collect()
    }

    #[test]
    fn records_follow_what_the_app_sees_and_survive_what_it_doesnt() {
        let mut tr = Tracker::default();
        let running = listing(&[("t1", term(10, Some((11, &["claude", "-c"])), 0))]);
        let (lookups, _) = tr.observe(&running, rule_of);
        assert_eq!(lookups.len(), 1);
        assert_eq!((lookups[0].cwd.as_str(), lookups[0].place.as_deref()), ("/w/sub", Some("s / tab")));
        assert!(tr.found(&lookups[0], Ok(Some("s1".into())), 1));
        assert_eq!(tr.records().next().unwrap().argv, words(&["claude", "--resume", "s1"]));
        assert!(tr.observe(&running, rule_of).0.is_empty(), "nothing new: no lookup");
        let next_turn = listing(&[("t1", term(10, Some((11, &["claude", "-c"])), 1))]);
        let (lookups, _) = tr.observe(&next_turn, rule_of);
        assert_eq!(lookups.len(), 1, "another turn may be another session");
        assert!(!tr.found(&lookups[0], Ok(Some("s1".into())), 2), "same session: nothing to write");
        assert!(tr.found(&lookups[0], Ok(Some("s2".into())), 3));
        assert!(!tr.found(&lookups[0], Err("bg".into()), 4), "a failed lookup keeps the record");
        assert_eq!(tr.records().count(), 1);

        // The core went away and came back with a new shell (a reboot): kept.
        tr.forget_seen();
        let rebooted = listing(&[("t1", term(20, Some((20, &["-zsh"])), 0))]);
        assert!(!tr.observe(&rebooted, rule_of).1);
        // Lost then restarted under the app's eyes: the terminal's own process changed, kept.
        let mut tr2 = Tracker::default();
        tr2.observe(&running, rule_of);
        let l = tr2.observe(&next_turn, rule_of).0;
        tr2.found(&l[0], Ok(Some("s1".into())), 1);
        assert!(!tr2.observe(&listing(&[("t1", json!({ "pid": null }))]), rule_of).1);
        assert!(!tr2.observe(&rebooted, rule_of).1);
        assert_eq!(tr2.records().count(), 1);

        // Quit while the app watched: gone.
        let mut tr3 = Tracker::default();
        let l = tr3.observe(&running, rule_of).0;
        tr3.found(&l[0], Ok(Some("s1".into())), 1);
        assert!(tr3.observe(&listing(&[("t1", term(10, Some((10, &["zsh"])), 0))]), rule_of).1);
        assert_eq!(tr3.records().count(), 0);

        // The terminal went while the app watched: gone.
        let mut tr4 = Tracker::default();
        let l = tr4.observe(&running, rule_of).0;
        tr4.found(&l[0], Ok(Some("s1".into())), 1);
        assert!(tr4.observe(&HashMap::new(), rule_of).1);
        assert_eq!(tr4.records().count(), 0);

        // A stale answer (the program changed since) is ignored.
        let mut tr5 = Tracker::default();
        let old = tr5.observe(&running, rule_of).0;
        tr5.observe(&listing(&[("t1", term(10, Some((12, &["claude"])), 0))]), rule_of);
        assert!(!tr5.found(&old[0], Ok(Some("s1".into())), 1));
    }

    #[test]
    fn a_reported_session_wins_and_reports_are_looked_up_again() {
        let mut tr = Tracker::default();
        let mut t = term(10, Some((11, &["claude"])), 0);
        let (l, _) = tr.observe(&listing(&[("t1", t.clone())]), rule_of);
        assert_eq!(l[0].reported, None);
        t["labels"]["herdr.session.claude"] = json!("rep-1");
        let (l, _) = tr.observe(&listing(&[("t1", t.clone())]), rule_of);
        assert_eq!(l[0].reported.as_deref(), Some("rep-1"));
        assert_eq!(reported_session("rep-2@11", 11).as_deref(), Some("rep-2"), "its own pid");
        assert_eq!(reported_session("rep-2@99", 11), None, "a nested run's report is not this program's");
        assert_eq!(reported_session("", 11), None, "cleared");
        t["labels"]["herdr.session.claude"] = json!("");
        let (l, _) = tr.observe(&listing(&[("t1", t.clone())]), rule_of);
        assert_eq!(l[0].reported, None, "a cleared report is looked up from the file again");
        t["labels"]["herdr.session.codex"] = json!("other");
        assert!(tr.observe(&listing(&[("t1", t)]), rule_of).0.is_empty(), "another program's report is not this one's");
    }

    #[test]
    fn sessions_are_read_where_the_program_runs_and_reports_win() {
        let lookup = |terminal: &str, reported: Option<&str>| Lookup {
            terminal: terminal.into(), key: "claude".into(), name: "Claude".into(), rule: claude(), argv: words(&["claude"]),
            pid: 42, cwd: "/w".into(), place: None, reported: reported.map(str::to_owned),
        };
        let file = r#"{"sessionId":"s1","kind":"interactive"}"#;
        let read_at = |want_path: &'static str, want_machine: Option<&'static str>| move |path: &str, machine: Option<&str>| {
            assert_eq!((path, machine), (want_path, want_machine));
            Ok(file.to_owned())
        };
        assert_eq!(find_session(&lookup("t1", None), "/home/me", read_at("/home/me/.claude/sessions/42.json", None)).unwrap().as_deref(), Some("s1"));
        assert_eq!(find_session(&lookup("box/t1", None), "/home/me", read_at(".claude/sessions/42.json", Some("box"))).unwrap().as_deref(), Some("s1"));
        let never = |_: &str, _: Option<&str>| -> Result<String, String> { panic!("a report needs no file") };
        assert_eq!(find_session(&lookup("t1", Some("rep")), "/h", never).unwrap().as_deref(), Some("rep"));
        assert!(find_session(&lookup("t1", Some("-x; rm")), "/h", never).is_err());
        assert!(find_session(&lookup("t1", None), "/h", |_, _| Err("not_found".into())).is_err());
    }

    #[test]
    fn only_bare_shells_with_a_session_of_their_own_are_ready() {
        let mut tr = Tracker::default();
        let rec = |t: &str, s: &str| Record { terminal: t.into(), place: None, program: "claude".into(), name: "Claude".into(), cwd: "/w".into(), argv: words(&["claude", "--resume", s]), session: Some(s.into()), saved_ms: 0 };
        for (t, s) in [("t1", "a"), ("t2", "b"), ("t3", "c"), ("t4", "d"), ("t5", "d")] {
            tr.insert(rec(t, s));
        }
        let live = listing(&[
            ("t1", term(1, Some((1, &["-zsh"])), 0)),
            ("t2", term(2, None, 0)),
            ("t3", term(3, Some((4, &["vim", "x"])), 0)),
            ("t4", term(4, Some((4, &["zsh"])), 0)),
            ("t5", term(5, Some((5, &["zsh"])), 0)),
        ]);
        let is_shell = |argv: &[String]| crate::programs::of(None, argv)["shell"] == true;
        let ready: Vec<(String, Option<String>)> = tr.readiness(&live, is_shell).into_iter().map(|(r, w)| (r.terminal.clone(), w)).collect();
        assert_eq!(ready[0], ("t1".into(), None));
        assert_eq!(ready[1], ("t2".into(), None), "no foreground: a bare terminal");
        assert!(ready[2].1.as_deref().is_some_and(|w| w.contains("runs")), "{:?}", ready[2]);
        assert!(ready[3].1.as_deref().is_some_and(|w| w.contains("same session")));
        assert!(ready[4].1.is_some());
        tr.insert(rec("t9", "z"));
        assert_eq!(tr.readiness(&live, is_shell).last().unwrap().1.as_deref(), Some("its terminal isn't running"));
    }

    #[test]
    fn records_round_trip_through_the_file() {
        let dir = std::env::temp_dir().join(format!("herdr-resume-{}", std::process::id()));
        let path = dir.join("data/resume.json");
        let mut tr = Tracker::default();
        tr.insert(Record { terminal: "box/t3".into(), place: Some("s / t".into()), program: "claude".into(), name: "Claude".into(), cwd: "/w".into(), argv: words(&["claude"]), session: None, saved_ms: 5 });
        tr.save(&path).unwrap();
        let back = Tracker::load(&path);
        assert_eq!(back.records().cloned().collect::<Vec<_>>(), tr.records().cloned().collect::<Vec<_>>());
        assert!(std::fs::read_to_string(&path).unwrap().contains("\"where\""));
        let _ = std::fs::remove_dir_all(dir);
    }
}
