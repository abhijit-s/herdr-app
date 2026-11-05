//! Agents that don't report their status (OSC 7501), recognised by their
//! foreground process and read off their screen.
//!
//! The rules are data: each `defaults/harnesses/*.toml` is a tuios harness
//! manifest, and `harnesses/*.toml` in the home folder adds or replaces one by
//! `id`. Only `[detect]` and `[screen]` are read; the other sections a tuios
//! manifest carries are ignored, so one is ported by copying it. The matcher
//! follows tuios's (`internal/harness`): the same regions, gates, priorities and
//! case folding, so a rule means here what it means there.
//!
//! A real 7501 report always wins: `Watcher::update` looks only at terminals
//! without one.

use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

include!(concat!(env!("OUT_DIR"), "/harnesses.rs"));

/// The lines from the bottom a manifest's rules see when it doesn't say.
const DEFAULT_LINES: usize = 6;
const MAX_REGION_LINES: usize = 200;

/// An idle reading on a working pane is published once it held for
/// `IDLE_CONFIRM_COUNT` more looks at least `IDLE_CONFIRM_INTERVAL` apart, or
/// for `IDLE_CONFIRM_CAP`: the prompt box stays on screen during a turn, and a
/// redraw can drop the spinner for a frame. These are tuios's (and herdr's)
/// numbers.
const IDLE_CONFIRM_INTERVAL: Duration = Duration::from_millis(100);
const IDLE_CONFIRM_COUNT: u32 = 3;
const IDLE_CONFIRM_CAP: Duration = Duration::from_millis(700);

/// Programs that run an agent as a script rather than being one: only their
/// run token is read as identity (tuios `ProcInfo.RunToken`).
const INTERPRETERS: &[&str] = &["node", "nodejs", "deno", "bun", "python", "python2", "python3", "uv", "uvx", "npx", "pnpm", "yarn", "bunx", "sh", "bash", "zsh", "fish", "env"];
/// The words a launcher puts before what it runs (`bun run x`, `uv run x`).
const RUNNER_SUBCOMMANDS: &[&str] = &["run", "exec", "dlx", "x", "tool", "--"];
const SCRIPT_EXTENSIONS: &[&str] = &[".js", ".mjs", ".cjs", ".ts", ".py"];
const PACKAGE_ROOTS: &[&str] = &["node_modules", "site-packages", "dist-packages"];

#[derive(Deserialize)]
pub struct Manifest {
    pub id: String,
    #[serde(default)]
    priority: i64,
    #[serde(default)]
    detect: Detect,
    #[serde(default)]
    screen: Screen,
}

/// Any one predicate matching is enough (tuios `Detect`).
#[derive(Deserialize, Default)]
struct Detect {
    #[serde(default)]
    comm: Vec<String>,
    #[serde(default)]
    argv0: Vec<String>,
    #[serde(default)]
    argv_path: Vec<String>,
    #[serde(default)]
    exe_glob: Vec<String>,
    #[serde(default)]
    require: Require,
}

/// Corroboration a bare-name match needs. tuios checks the resolved
/// executable; the core reports only the process name and argv, so the name
/// stands in for `exe_base` and a path-like argv[0] for `exe_glob`.
#[derive(Deserialize, Default)]
struct Require {
    #[serde(default)]
    exe_base: Vec<String>,
    #[serde(default)]
    exe_glob: Vec<String>,
}

#[derive(Deserialize, Default)]
struct Screen {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    fold_case: bool,
    #[serde(default)]
    lines: usize,
    #[serde(default)]
    rule: Vec<Rule>,
}

#[derive(Deserialize)]
struct Rule {
    state: String,
    #[serde(default)]
    priority: i64,
    #[serde(default)]
    region: String,
    #[serde(default)]
    show: String,
    #[serde(flatten)]
    gate: Gate,
    #[serde(skip)]
    area: Region,
}

/// What a rule tests its region against (tuios `Gate`): every `all` present,
/// one `any` present, no `not`, every `regex` matching, no `not_regex`, and the
/// nested groups likewise. Substrings read the folded text, patterns the plain.
#[derive(Deserialize, Default)]
struct Gate {
    #[serde(default)]
    all: Vec<String>,
    #[serde(default)]
    any: Vec<String>,
    #[serde(default)]
    not: Vec<String>,
    #[serde(default)]
    regex: Vec<String>,
    #[serde(default)]
    not_regex: Vec<String>,
    #[serde(default)]
    all_of: Vec<Gate>,
    #[serde(default)]
    any_of: Vec<Gate>,
    #[serde(default)]
    none_of: Vec<Gate>,
    #[serde(skip)]
    patterns: Vec<Regex>,
    #[serde(skip)]
    not_patterns: Vec<Regex>,
}

#[derive(Clone, Copy, Default, PartialEq, Debug)]
enum Region {
    #[default]
    Tail,
    Bottom(usize),
    PromptBox,
    AbovePromptBox,
    LastAbovePromptBox,
    AfterLastRule,
}

impl Region {
    fn parse(name: &str) -> Result<Region, String> {
        let name = name.trim().to_lowercase();
        Ok(match name.as_str() {
            "" | "tail" | "whole_recent" => Region::Tail,
            "prompt_box" | "prompt_box_body" => Region::PromptBox,
            "above_prompt_box" => Region::AbovePromptBox,
            "last_non_empty_above_prompt_box" => Region::LastAbovePromptBox,
            "after_last_horizontal_rule" => Region::AfterLastRule,
            _ => {
                let n = name.strip_prefix("bottom_non_empty_lines(").and_then(|r| r.strip_suffix(')'));
                match n.filter(|n| !n.starts_with(['0', '+'])).and_then(|n| n.parse::<usize>().ok()) {
                    Some(n) if (1..=MAX_REGION_LINES).contains(&n) => Region::Bottom(n),
                    _ => return Err(format!("unknown region {name:?}")),
                }
            }
        })
    }

    /// The span of `tail` the region covers; None when it isn't on the screen.
    fn bounds(self, tail: &[String]) -> Option<(usize, usize)> {
        match self {
            Region::Tail => Some((0, tail.len())),
            Region::Bottom(n) => Some((tail.len().saturating_sub(n), tail.len())),
            Region::PromptBox => prompt_box(tail).map(|(top, bottom)| (top + 1, bottom)),
            Region::AbovePromptBox => prompt_box(tail).map(|(top, _)| (0, top)),
            Region::LastAbovePromptBox => {
                let (top, _) = prompt_box(tail)?;
                (0..top).rev().find(|&i| !tail[i].trim().is_empty()).map(|i| (i, i + 1))
            }
            // With no rule on screen nothing marks older output as settled.
            Region::AfterLastRule => Some((tail.iter().rposition(|l| is_box_border(l)).map_or(0, |i| i + 1), tail.len())),
        }
    }
}

/// A run of at least three box-drawing dashes, after an optional corner: the
/// rules a TUI frames its input box with.
fn is_box_border(line: &str) -> bool {
    let t = line.trim().trim_start_matches(|c| "╭╰┌└├┏┗┣".contains(c));
    t.chars().take_while(|&c| c == '─' || c == '━').count() >= 3
}

/// The input box at the bottom: the last two border lines.
fn prompt_box(tail: &[String]) -> Option<(usize, usize)> {
    let mut borders = (0..tail.len()).rev().filter(|&i| is_box_border(&tail[i]));
    let bottom = borders.next()?;
    Some((borders.next()?, bottom))
}

impl Gate {
    fn positive(&self) -> bool {
        !(self.all.is_empty() && self.any.is_empty() && self.regex.is_empty() && self.all_of.is_empty() && self.any_of.is_empty())
    }

    fn empty(&self) -> bool {
        !self.positive() && self.not.is_empty() && self.not_regex.is_empty() && self.none_of.is_empty()
    }

    fn compile(&mut self, fold_case: bool, depth: usize) -> Result<(), String> {
        if depth > 8 {
            return Err("gates nest deeper than 8".into());
        }
        if fold_case {
            for s in self.all.iter_mut().chain(&mut self.any).chain(&mut self.not) {
                *s = s.to_lowercase();
            }
        }
        // Multi-line, as tuios compiles them: `^` and `$` are each line's ends.
        let compile = |list: &[String]| list.iter().map(|p| Regex::new(&format!("(?m){p}")).map_err(|e| format!("regex {p:?}: {e}"))).collect::<Result<Vec<_>, _>>();
        self.patterns = compile(&self.regex)?;
        self.not_patterns = compile(&self.not_regex)?;
        for (name, veto, groups) in [("all_of", false, &mut self.all_of), ("any_of", false, &mut self.any_of), ("none_of", true, &mut self.none_of)] {
            for (i, g) in groups.iter_mut().enumerate() {
                // A claim must name something present; a veto may be vetoes all the way down.
                if if veto { g.empty() } else { !g.positive() } {
                    return Err(format!("{name} group {i} names nothing that must be present"));
                }
                g.compile(fold_case, depth + 1).map_err(|e| format!("{name} group {i}: {e}"))?;
            }
        }
        Ok(())
    }

    fn matches(&self, hay: &str, folded: &str) -> bool {
        self.all.iter().all(|s| folded.contains(s.as_str()))
            && (self.any.is_empty() || self.any.iter().any(|s| folded.contains(s.as_str())))
            && !self.not.iter().any(|s| folded.contains(s.as_str()))
            && self.patterns.iter().all(|r| r.is_match(hay))
            && !self.not_patterns.iter().any(|r| r.is_match(hay))
            && self.all_of.iter().all(|g| g.matches(hay, folded))
            && (self.any_of.is_empty() || self.any_of.iter().any(|g| g.matches(hay, folded)))
            && !self.none_of.iter().any(|g| g.matches(hay, folded))
    }

    /// Whether every path to a match carries a pattern: what an idle rule
    /// reading more than the input box needs, since a loose substring could be
    /// any word in any output.
    fn proves_shape(&self) -> bool {
        !self.regex.is_empty() || self.all_of.iter().any(Gate::proves_shape) || (!self.any_of.is_empty() && self.any_of.iter().all(Gate::proves_shape))
    }
}

impl Manifest {
    fn parse(name: &str, text: &str) -> Result<Manifest, String> {
        let mut m: Manifest = toml::from_str(text).map_err(|e| format!("{name}: {e}"))?;
        if m.id.trim().is_empty() {
            return Err(format!("{name}: no id"));
        }
        let d = &mut m.detect;
        for list in [&mut d.comm, &mut d.argv0, &mut d.argv_path, &mut d.exe_glob, &mut d.require.exe_base, &mut d.require.exe_glob] {
            *list = list.iter().map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect();
        }
        let fold = m.screen.fold_case;
        for (i, r) in m.screen.rule.iter_mut().enumerate() {
            let at = |e: String| format!("{name}: screen rule {i}: {e}");
            if !matches!(r.state.as_str(), "working" | "needs_input" | "idle") {
                return Err(at(format!("state {:?} is not working, needs_input or idle", r.state)));
            }
            r.area = Region::parse(&r.region).map_err(at)?;
            if r.state == "idle" && r.area != Region::PromptBox && !r.gate.proves_shape() {
                return Err(at("an idle rule must read region = \"prompt_box\" or carry a regex that pins the input box".into()));
            }
            r.gate.compile(fold, 0).map_err(at)?;
        }
        // Highest priority first; ties go to the rule declared first.
        m.screen.rule.sort_by_key(|r| std::cmp::Reverse(r.priority));
        Ok(m)
    }

    /// How many non-empty lines from the bottom its rules read (0: none).
    pub fn lines(&self) -> usize {
        match (self.screen.enabled, self.screen.lines) {
            (false, _) => 0,
            (true, 0) => DEFAULT_LINES,
            (true, n) => n,
        }
    }

    /// The Agent word the best matching rule names, or None: a rule that
    /// stops matching after an update must degrade to no opinion, never to a
    /// guess.
    pub fn classify(&self, tail: &[String]) -> Option<&'static str> {
        if self.lines() == 0 || tail.is_empty() {
            return None;
        }
        let rule = self.screen.rule.iter().find(|r| {
            // tuios's dialog rules need its box finder, which isn't ported.
            if r.show == "dialog" || !r.gate.positive() {
                return false;
            }
            let Some((lo, hi)) = r.area.bounds(tail) else { return false };
            let hay = tail[lo..hi].join("\n");
            if hay.is_empty() {
                return false;
            }
            let folded = if self.screen.fold_case { hay.to_lowercase() } else { hay.clone() };
            r.gate.matches(&hay, &folded)
        })?;
        Some(match rule.state.as_str() {
            "needs_input" => "blocked",
            "working" => "working",
            _ => "idle",
        })
    }

    fn identifies(&self, name: &str, argv: &[String]) -> bool {
        let d = &self.detect;
        let comm = base_name(name);
        let argv0 = argv.first().map(|a| a.as_str()).unwrap_or_default();
        let run = run_token(&comm, argv);
        let has = |list: &[String], v: &str| !v.is_empty() && list.iter().any(|s| s == v);
        let paths: Vec<&str> = [name, argv0].into_iter().filter(|p| p.contains('/') || p.contains('\\')).collect();
        let required = (d.require.exe_base.is_empty() && d.require.exe_glob.is_empty())
            || has(&d.require.exe_base, &comm)
            || d.require.exe_glob.iter().any(|g| paths.iter().any(|p| glob_path(g, p)));
        if required && (has(&d.comm, &comm) || has(&d.argv0, &base_name(argv0)) || run.is_some_and(|r| has(&d.argv0, &base_name(r)))) {
            return true;
        }
        if d.exe_glob.iter().any(|g| paths.iter().any(|p| glob_path(g, p))) {
            return true;
        }
        run.is_some_and(|r| d.argv_path.iter().any(|want| package_named(&segments(r), &segments(want))))
    }
}

/// A process name or argv token as matched: base name, no login-shell `-`,
/// no script extension, lowercased.
fn base_name(s: &str) -> String {
    let s = s.trim_end_matches('\0').trim().replace('\\', "/");
    let base = s.rsplit('/').next().unwrap_or_default().trim_start_matches('-').to_lowercase();
    SCRIPT_EXTENSIONS.iter().find_map(|e| base.strip_suffix(e)).map(str::to_owned).unwrap_or(base)
}

/// The one argv token naming what an interpreter runs (`node /x/cli.js`),
/// None when the process isn't an interpreter. One token, not a scan: an
/// agent's name anywhere in an argument (`tail -f ~/dev/opencode/main.go`)
/// is not evidence.
fn run_token<'a>(comm: &str, argv: &'a [String]) -> Option<&'a str> {
    if !(comm.is_empty() || INTERPRETERS.contains(&comm)) {
        return None;
    }
    let skipped = |t: &str| t.is_empty() || (t.starts_with('-') && t != "--") || RUNNER_SUBCOMMANDS.contains(&t.to_lowercase().as_str());
    argv.iter().skip(1).map(|t| t.trim_end_matches('\0')).find(|t| !skipped(t))
}

fn segments(s: &str) -> Vec<String> {
    s.trim_end_matches('\0').to_lowercase().replace('\\', "/").split('/').filter(|p| !p.is_empty()).map(str::to_owned).collect()
}

/// Whether `want` names a package in `have`: right after a package manager's
/// folder, anywhere for a scoped name (`@anthropic-ai/claude-code`), or as a
/// bare token (`npx opencode@latest`). A directory named after an agent is a
/// checkout, not the agent.
fn package_named(have: &[String], want: &[String]) -> bool {
    if want.is_empty() || have.len() < want.len() {
        return false;
    }
    let scoped = want[0].starts_with('@');
    let root = |i: usize| PACKAGE_ROOTS.contains(&have[i].as_str()) || (i >= 2 && have[i - 2] == ".bun" && have[i - 1] == "install" && have[i] == "cache");
    (0..=have.len() - want.len()).any(|i| {
        let equal = want.iter().enumerate().all(|(j, w)| {
            let h = &have[i + j];
            h == w || (j == want.len() - 1 && h.strip_prefix(w.as_str()).is_some_and(|r| r.starts_with('@') || r.starts_with("==")))
        });
        equal && (have.len() == 1 || scoped || (i >= 1 && root(i - 1)) || (i >= 2 && have[i - 1].starts_with('@') && root(i - 2)))
    })
}

/// A component-wise glob: `*` and `?` stay inside a component, `**` spans
/// any number, and a pattern not starting with `/` matches any suffix.
fn glob_path(pattern: &str, path: &str) -> bool {
    let pat = segments(pattern);
    let segs = segments(path);
    if pattern.starts_with('/') {
        return glob_segments(&pat, &segs);
    }
    (0..=segs.len()).any(|i| glob_segments(&pat, &segs[i..]))
}

fn glob_segments(pat: &[String], segs: &[String]) -> bool {
    match pat.split_first() {
        None => segs.is_empty(),
        Some((p, rest)) if p == "**" => (0..=segs.len()).any(|i| glob_segments(rest, &segs[i..])),
        Some((p, rest)) => segs.split_first().is_some_and(|(s, more)| glob_one(p.as_bytes(), s.as_bytes()) && glob_segments(rest, more)),
    }
}

fn glob_one(p: &[u8], s: &[u8]) -> bool {
    match p.split_first() {
        None => s.is_empty(),
        Some((b'*', rest)) => (0..=s.len()).any(|i| glob_one(rest, &s[i..])),
        Some((b'?', rest)) => !s.is_empty() && glob_one(rest, &s[1..]),
        Some((c, rest)) => s.first() == Some(c) && glob_one(rest, &s[1..]),
    }
}

/// Every manifest, highest priority first, then by id, so two that match one
/// process always resolve to the same one.
pub struct Registry {
    manifests: Vec<Manifest>,
}

impl Registry {
    /// Manifests from `(file name, text)` pairs, later ones replacing earlier
    /// ones with the same id, and what couldn't be read.
    pub fn parse<'a>(sources: impl IntoIterator<Item = (&'a str, &'a str)>) -> (Registry, Vec<String>) {
        let mut by_id: Vec<Manifest> = Vec::new();
        let mut errors = Vec::new();
        for (name, text) in sources {
            match Manifest::parse(name, text) {
                Ok(m) => {
                    by_id.retain(|o| o.id != m.id);
                    by_id.push(m);
                }
                Err(e) => errors.push(e),
            }
        }
        by_id.sort_by(|a, b| b.priority.cmp(&a.priority).then_with(|| a.id.cmp(&b.id)));
        (Registry { manifests: by_id }, errors)
    }

    /// The manifest naming a foreground process (core's `name` and `argv`).
    pub fn identify(&self, name: &str, argv: &[String]) -> Option<&Manifest> {
        self.manifests.iter().find(|m| m.identifies(name, argv))
    }

    #[cfg(test)]
    pub fn get(&self, id: &str) -> Option<&Manifest> {
        self.manifests.iter().find(|m| m.id == id)
    }
}

/// The shipped manifests with the person's `harnesses/*.toml` on top, read once.
pub fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut sources: Vec<(String, String)> = HARNESSES
            .iter()
            .filter(|(name, _)| name.ends_with(".toml"))
            .map(|(name, bytes)| (name.to_string(), String::from_utf8_lossy(bytes).into_owned()))
            .collect();
        let dir = crate::mods::app_dir().join("harnesses");
        let mut mine: Vec<_> = std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "toml")).collect();
        mine.sort();
        for path in mine {
            match std::fs::read_to_string(&path) {
                Ok(text) => sources.push((path.display().to_string(), text)),
                Err(e) => tracing::warn!("{}: {e}", path.display()),
            }
        }
        let (registry, errors) = Registry::parse(sources.iter().map(|(n, t)| (n.as_str(), t.as_str())));
        for e in errors {
            tracing::warn!("harness manifest skipped: {e}");
        }
        registry
    })
}

/// One unreported agent's published word and its idle reading in waiting.
struct Watch {
    harness: String,
    word: &'static str,
    idle_since: Option<(Instant, Instant, u32)>,
}

impl Watch {
    /// Until a rule says otherwise the agent is taken to be at rest.
    fn new(harness: &str) -> Watch {
        Watch { harness: harness.to_owned(), word: "idle", idle_since: None }
    }

    /// Takes one look's verdict and returns the word to publish. No opinion
    /// keeps the last word; idle on a working pane waits to be confirmed.
    fn observe(&mut self, verdict: Option<&'static str>, now: Instant) -> &'static str {
        match verdict {
            None => self.idle_since = None,
            Some("idle") if self.word == "working" => match &mut self.idle_since {
                None => self.idle_since = Some((now, now, 0)),
                Some((started, last, confirmations)) => {
                    if now.duration_since(*last) >= IDLE_CONFIRM_INTERVAL {
                        *confirmations += 1;
                        *last = now;
                    }
                    if *confirmations >= IDLE_CONFIRM_COUNT || now.duration_since(*started) >= IDLE_CONFIRM_CAP {
                        self.idle_since = None;
                        self.word = "idle";
                    }
                }
            },
            Some(word) => {
                self.idle_since = None;
                self.word = word;
            }
        }
        self.word
    }
}

/// The agents without a 7501 report whose foreground process a manifest names.
#[derive(Default)]
pub struct Watcher {
    watching: HashMap<String, Watch>,
}

impl Watcher {
    /// Looks at core's live terminal records once: for each unreported agent,
    /// its word and harness id. `tail(address, lines)` reads a terminal's last
    /// non-empty screen lines. A terminal that reports, ends, or stops running
    /// the agent drops out.
    pub fn update(&mut self, registry: &Registry, live: &HashMap<String, Value>, now: Instant, mut tail: impl FnMut(&str, usize) -> Vec<String>) -> HashMap<String, (&'static str, String)> {
        let mut words = HashMap::new();
        for (addr, rec) in live {
            if rec.get("status").is_some_and(|s| !s.is_null()) || rec["state"] != "live" {
                continue;
            }
            let Some(fg) = rec.get("foreground") else { continue };
            let argv: Vec<String> = fg["argv"].as_array().into_iter().flatten().filter_map(|a| a.as_str().map(str::to_owned)).collect();
            let Some(m) = registry.identify(fg["name"].as_str().unwrap_or_default(), &argv) else { continue };
            let watch = self.watching.entry(addr.clone()).or_insert_with(|| Watch::new(&m.id));
            if watch.harness != m.id {
                *watch = Watch::new(&m.id);
            }
            let lines = m.lines();
            let verdict = if lines == 0 { None } else { m.classify(&tail(addr, lines)) };
            words.insert(addr.clone(), (watch.observe(verdict, now), m.id.clone()));
        }
        self.watching.retain(|a, _| words.contains_key(a));
        words
    }
}

#[cfg(test)]
#[path = "harness_tests.rs"]
mod tests;
