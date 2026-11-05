//! Agents that don't report their status (OSC 7501), recognised by their
//! foreground process and read off their screen and window title.
//!
//! The rules are data: each `defaults/harnesses/*.toml` is a tuios harness
//! manifest, and `harnesses/*.toml` in the home folder adds or replaces one by
//! `id`. Only `[detect]`, `[screen]` and `[title]` are read; the other sections
//! a tuios manifest carries are ignored, so one is ported by copying it. The
//! matcher follows tuios's (`internal/harness`): the same regions, gates,
//! priorities, case folding and loader refusals, so a rule means here what it
//! means there.
//!
//! A real 7501 report always wins: `Watcher::update` looks only at terminals
//! without one.

use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

include!(concat!(env!("OUT_DIR"), "/harnesses.rs"));

/// The lines from the bottom a manifest's rules see when it doesn't say.
const DEFAULT_LINES: usize = 6;
const MAX_REGION_LINES: usize = 200;
/// Limits on what a manifest may ask of a scan that runs on the UI thread.
const MAX_PATTERN_BYTES: usize = 512;
const MAX_GATE_DEPTH: usize = 8;
/// A bare name shorter than this ("pi", "amp") needs `[detect.require]`: a
/// false positive labels an innocent pane as an agent.
const GENERIC_NAME_LIMIT: usize = 5;

/// An idle reading on a working pane is published once it held for
/// `IDLE_CONFIRM_COUNT` more looks at least `IDLE_CONFIRM_INTERVAL` apart, or
/// for `IDLE_CONFIRM_CAP`: the prompt box stays on screen during a turn, and a
/// redraw can drop the spinner for a frame. These are tuios's (and herdr's)
/// numbers.
const IDLE_CONFIRM_INTERVAL: Duration = Duration::from_millis(100);
const IDLE_CONFIRM_COUNT: u32 = 3;
const IDLE_CONFIRM_CAP: Duration = Duration::from_millis(700);
/// A working pane whose screen hasn't changed for this long, with no rule
/// saying anything, has stopped: tuios's stall heuristic and its default. It
/// is what ends a turn for a harness with no idle rule.
const STALL: Duration = Duration::from_secs(30);

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
    screen: Rules,
    #[serde(default)]
    title: Rules,
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
/// stands in for `exe_base` and a path-like name or argv[0] for `exe_glob`.
#[derive(Deserialize, Default)]
struct Require {
    #[serde(default)]
    exe_base: Vec<String>,
    #[serde(default)]
    exe_glob: Vec<String>,
}

/// A `[screen]` or `[title]` block.
#[derive(Deserialize, Default)]
struct Rules {
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
    /// A title rule reading the window title.
    Title,
    /// A title rule reading OSC 9;4 progress, which the core doesn't pass on:
    /// it never matches.
    Progress,
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

    fn parse_title(name: &str) -> Result<Region, String> {
        match name.trim().to_lowercase().as_str() {
            "" | "osc_title" => Ok(Region::Title),
            "osc_progress" => Ok(Region::Progress),
            other => Err(format!("region {other:?}: a title rule reads osc_title or osc_progress")),
        }
    }

    /// The span of `tail` the region covers; None when it isn't on the screen.
    /// `boxed` is `prompt_box(tail)`, found once per scan.
    fn bounds(self, tail: &[String], boxed: Option<(usize, usize)>) -> Option<(usize, usize)> {
        match self {
            Region::Tail => Some((0, tail.len())),
            Region::Bottom(n) => Some((tail.len().saturating_sub(n), tail.len())),
            Region::PromptBox => boxed.map(|(top, bottom)| (top + 1, bottom)),
            Region::AbovePromptBox => boxed.map(|(top, _)| (0, top)),
            Region::LastAbovePromptBox => {
                let (top, _) = boxed?;
                (0..top).rev().find(|&i| !tail[i].trim().is_empty()).map(|i| (i, i + 1))
            }
            // With no rule on screen nothing marks older output as settled.
            Region::AfterLastRule => Some((tail.iter().rposition(|l| is_box_border(l)).map_or(0, |i| i + 1), tail.len())),
            Region::Title | Region::Progress => None,
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

/// A title substring must stand as its own token (tuios `containsToken`):
/// "codex" in "~/src/codex-playground" is not the program saying anything.
fn contains_token(hay: &str, needle: &str) -> bool {
    let token = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.');
    let (Some(&head), Some(&tail)) = (needle.as_bytes().first(), needle.as_bytes().last()) else { return false };
    let bytes = hay.as_bytes();
    hay.match_indices(needle).any(|(i, _)| {
        let end = i + needle.len();
        (!token(head) || i == 0 || !token(bytes[i - 1])) && (!token(tail) || end == bytes.len() || !token(bytes[end]))
    })
}

fn contains(hay: &str, needle: &str) -> bool {
    hay.contains(needle)
}

/// Rewrites a Go RE2 pattern for Rust's regex: `\s \d \w \b` and their
/// negations are ASCII in RE2 and Unicode in Rust, so a rule written against
/// tuios would decide differently on a screen with non-ASCII digits or spaces.
fn ascii_classes(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len() + 16);
    let mut chars = pattern.chars().peekable();
    let mut in_class = false;
    let mut class_start = false;
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                let Some(e) = chars.next() else {
                    out.push('\\');
                    break;
                };
                let swap = match (in_class, e) {
                    (false, 's') => "[\\t\\n\\f\\r ]",
                    (false, 'S') => "[^\\t\\n\\f\\r ]",
                    (false, 'd') => "[0-9]",
                    (false, 'D') => "[^0-9]",
                    (false, 'w') => "[0-9A-Za-z_]",
                    (false, 'W') => "[^0-9A-Za-z_]",
                    (false, 'b') => "(?-u:\\b)",
                    (false, 'B') => "(?-u:\\B)",
                    (true, 's') => "\\t\\n\\f\\r ",
                    (true, 'd') => "0-9",
                    (true, 'w') => "0-9A-Za-z_",
                    _ => "",
                };
                if swap.is_empty() {
                    out.push('\\');
                    out.push(e);
                } else {
                    out.push_str(swap);
                }
                class_start = false;
            }
            '[' if !in_class => {
                in_class = true;
                class_start = true;
                out.push('[');
                if chars.peek() == Some(&'^') {
                    out.push(chars.next().unwrap());
                }
            }
            '[' if chars.peek() == Some(&':') => {
                // A POSIX class inside a bracket class, `[[:alpha:]]`.
                out.push('[');
                for p in chars.by_ref() {
                    out.push(p);
                    if p == ']' {
                        break;
                    }
                }
                class_start = false;
            }
            '[' => {
                // RE2 has no nested classes: a `[` inside one is literal.
                out.push_str("\\[");
                class_start = false;
            }
            ']' if in_class && !class_start => {
                in_class = false;
                out.push(']');
            }
            _ => {
                out.push(c);
                class_start = false;
            }
        }
    }
    out
}

impl Gate {
    fn positive(&self) -> bool {
        !(self.all.is_empty() && self.any.is_empty() && self.regex.is_empty() && self.all_of.is_empty() && self.any_of.is_empty())
    }

    fn empty(&self) -> bool {
        !self.positive() && self.not.is_empty() && self.not_regex.is_empty() && self.none_of.is_empty()
    }

    fn compile(&mut self, fold_case: bool, depth: usize) -> Result<(), String> {
        if depth > MAX_GATE_DEPTH {
            return Err(format!("gates nest deeper than {MAX_GATE_DEPTH}"));
        }
        if fold_case {
            for s in self.all.iter_mut().chain(&mut self.any).chain(&mut self.not) {
                *s = s.to_lowercase();
            }
        }
        // Multi-line, as tuios compiles them: `^` and `$` are each line's ends.
        let compile = |list: &[String]| {
            list.iter()
                .map(|p| {
                    if p.len() > MAX_PATTERN_BYTES {
                        return Err(format!("pattern {p:?} is {} bytes, limit {MAX_PATTERN_BYTES}", p.len()));
                    }
                    Regex::new(&format!("(?m){}", ascii_classes(p))).map_err(|e| format!("regex {p:?}: {e}"))
                })
                .collect::<Result<Vec<_>, _>>()
        };
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

    fn matches(&self, hay: &str, folded: &str, has: fn(&str, &str) -> bool) -> bool {
        self.all.iter().all(|s| has(folded, s))
            && (self.any.is_empty() || self.any.iter().any(|s| has(folded, s)))
            && !self.not.iter().any(|s| has(folded, s))
            && self.patterns.iter().all(|r| r.is_match(hay))
            && !self.not_patterns.iter().any(|r| r.is_match(hay))
            && self.all_of.iter().all(|g| g.matches(hay, folded, has))
            && (self.any_of.is_empty() || self.any_of.iter().any(|g| g.matches(hay, folded, has)))
            && !self.none_of.iter().any(|g| g.matches(hay, folded, has))
    }

    /// Whether every path to a match carries a pattern: what an idle rule
    /// reading more than the input box needs, since a loose substring could be
    /// any word in any output.
    fn proves_shape(&self) -> bool {
        !self.regex.is_empty() || self.all_of.iter().any(Gate::proves_shape) || (!self.any_of.is_empty() && self.any_of.iter().all(Gate::proves_shape))
    }
}

/// The Agent word for a rule's state.
fn word(state: &str) -> &'static str {
    match state {
        "needs_input" => "blocked",
        "working" => "working",
        _ => "idle",
    }
}

/// A foreground process as detection reads it, worked out once for every
/// manifest it's tried against.
struct Process<'a> {
    comm: String,
    argv0: String,
    run: Option<(String, Vec<String>)>,
    paths: Vec<&'a str>,
}

impl<'a> Process<'a> {
    fn new(name: &'a str, argv: &'a [String]) -> Process<'a> {
        let comm = base_name(name);
        let first = argv.first().map(|a| a.as_str()).unwrap_or_default();
        let run = run_token(&comm, argv).map(|r| (base_name(r), segments(r)));
        let paths = [name, first].into_iter().filter(|p| p.contains('/') || p.contains('\\')).collect();
        Process { comm, argv0: base_name(first), run, paths }
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
        if d.comm.is_empty() && d.argv0.is_empty() && d.argv_path.is_empty() && d.exe_glob.is_empty() {
            return Err(format!("{name}: manifest {:?} matches nothing", m.id));
        }
        if d.require.exe_base.is_empty() && d.require.exe_glob.is_empty()
            && let Some(n) = d.comm.iter().chain(&d.argv0).find(|n| n.len() < GENERIC_NAME_LIMIT)
        {
            return Err(format!("{name}: manifest {:?} matches on the generic name {n:?} alone; add a [detect.require] block", m.id));
        }
        if m.screen.lines == 0 {
            m.screen.lines = DEFAULT_LINES;
        }
        if m.screen.lines > MAX_REGION_LINES {
            return Err(format!("{name}: screen lines {}, limit {MAX_REGION_LINES}", m.screen.lines));
        }
        let lines = m.screen.lines;
        for (block, rules, title) in [("screen", &mut m.screen, false), ("title", &mut m.title, true)] {
            let fold = rules.fold_case;
            for (i, r) in rules.rule.iter_mut().enumerate() {
                let at = |e: String| format!("{name}: {block} rule {i}: {e}");
                if !matches!(r.state.as_str(), "working" | "needs_input" | "idle") {
                    return Err(at(format!("state {:?} is not working, needs_input or idle", r.state)));
                }
                r.area = if title { Region::parse_title(&r.region) } else { Region::parse(&r.region) }.map_err(at)?;
                if let Region::Bottom(n) = r.area
                    && n > lines
                {
                    return Err(at(format!("region reads past the {lines} lines the manifest reads; raise [screen] lines")));
                }
                if !title && r.state == "idle" && r.area != Region::PromptBox && !r.gate.proves_shape() {
                    return Err(at("an idle rule must read region = \"prompt_box\" or carry a regex that pins the input box".into()));
                }
                r.gate.compile(fold, 0).map_err(at)?;
            }
            // Highest priority first; ties go to the rule declared first.
            rules.rule.sort_by_key(|r| std::cmp::Reverse(r.priority));
        }
        Ok(m)
    }

    /// How many non-empty lines from the bottom its screen rules read (0: none).
    pub fn lines(&self) -> usize {
        if self.screen.enabled { self.screen.lines } else { 0 }
    }

    /// The Agent word the best matching screen rule names, or None: a rule
    /// that stops matching after an update must degrade to no opinion, never
    /// to a guess.
    pub fn classify(&self, tail: &[String]) -> Option<&'static str> {
        self.screen_rule(tail).map(|r| word(&r.state))
    }

    fn screen_rule(&self, tail: &[String]) -> Option<&Rule> {
        if self.lines() == 0 || tail.is_empty() {
            return None;
        }
        let boxed = prompt_box(tail);
        // Each region's text, joined and folded once per scan.
        let mut texts: Vec<(Region, String, Option<String>)> = Vec::new();
        self.screen.rule.iter().find(|r| {
            // tuios's dialog rules need its box finder, which isn't ported.
            if r.show == "dialog" || !r.gate.positive() {
                return false;
            }
            let at = match texts.iter().position(|(region, ..)| *region == r.area) {
                Some(at) => at,
                None => {
                    let hay = r.area.bounds(tail, boxed).map(|(lo, hi)| tail[lo..hi].join("\n")).unwrap_or_default();
                    let folded = self.screen.fold_case.then(|| hay.to_lowercase());
                    texts.push((r.area, hay, folded));
                    texts.len() - 1
                }
            };
            let (_, hay, folded) = &texts[at];
            !hay.is_empty() && r.gate.matches(hay, folded.as_deref().unwrap_or(hay), contains)
        })
    }

    /// The Agent word the best matching title rule names, or None.
    pub fn classify_title(&self, title: &str) -> Option<&'static str> {
        if !self.title.enabled || title.is_empty() {
            return None;
        }
        let folded = if self.title.fold_case { Cow::Owned(title.to_lowercase()) } else { Cow::Borrowed(title) };
        self.title.rule.iter().find(|r| r.area == Region::Title && r.gate.positive() && r.gate.matches(title, &folded, contains_token)).map(|r| word(&r.state))
    }

    fn identifies(&self, p: &Process) -> bool {
        let d = &self.detect;
        let has = |list: &[String], v: &str| !v.is_empty() && list.iter().any(|s| s == v);
        let required = (d.require.exe_base.is_empty() && d.require.exe_glob.is_empty())
            || has(&d.require.exe_base, &p.comm)
            || d.require.exe_glob.iter().any(|g| p.paths.iter().any(|path| glob_path(g, path)));
        if required && (has(&d.comm, &p.comm) || has(&d.argv0, &p.argv0) || p.run.as_ref().is_some_and(|(base, _)| has(&d.argv0, base))) {
            return true;
        }
        if d.exe_glob.iter().any(|g| p.paths.iter().any(|path| glob_path(g, path))) {
            return true;
        }
        p.run.as_ref().is_some_and(|(_, segs)| d.argv_path.iter().any(|want| package_named(segs, &segments(want))))
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
    // `**` in front is what "any suffix" means.
    let any_suffix = !pattern.starts_with('/') && pat.first().is_none_or(|p| p != "**");
    let pat: Vec<&str> = any_suffix.then_some("**").into_iter().chain(pat.iter().map(String::as_str)).collect();
    // matched[j]: the pattern so far matches the first j components. Linear
    // in pattern times path, however many `**` a person writes.
    let mut matched = vec![false; segs.len() + 1];
    matched[0] = true;
    for p in pat {
        if p == "**" {
            for j in 1..=segs.len() {
                matched[j] |= matched[j - 1];
            }
        } else {
            for j in (1..=segs.len()).rev() {
                matched[j] = matched[j - 1] && glob_one(p.as_bytes(), segs[j - 1].as_bytes());
            }
            matched[0] = false;
        }
    }
    matched[segs.len()]
}

/// One component against `*` and `?`, backtracking only to the last `*`, so
/// at worst pattern times name.
fn glob_one(p: &[u8], s: &[u8]) -> bool {
    let (mut pi, mut si) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while si < s.len() {
        match p.get(pi) {
            Some(b'*') => {
                star = Some((pi, si));
                pi += 1;
            }
            Some(&c) if c == b'?' || c == s[si] => {
                pi += 1;
                si += 1;
            }
            _ => match star {
                Some((sp, ss)) => {
                    pi = sp + 1;
                    si = ss + 1;
                    star = Some((sp, ss + 1));
                }
                None => return false,
            },
        }
    }
    p[pi..].iter().all(|&c| c == b'*')
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

    /// The index of the manifest naming a foreground process (core's `name`
    /// and `argv`).
    fn identify_index(&self, name: &str, argv: &[String]) -> Option<usize> {
        let process = Process::new(name, argv);
        self.manifests.iter().position(|m| m.identifies(&process))
    }

    #[cfg(test)]
    pub fn identify(&self, name: &str, argv: &[String]) -> Option<&Manifest> {
        self.identify_index(name, argv).map(|i| &self.manifests[i])
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

/// What the app has of a terminal's screen: how many times it was written
/// (output or resize; the screen can only have changed when this moves) and
/// its window title as the program set it.
pub struct Screen {
    pub writes: u64,
    pub title: String,
}

/// One unreported agent: its published word, its idle reading in waiting,
/// when its screen last changed, and the turns it finished.
struct Watch {
    harness: usize,
    word: &'static str,
    idle_since: Option<(Instant, Instant, u32)>,
    changed: Instant,
    /// The screen's write count and the verdict read from it.
    looked: Option<(u64, Option<&'static str>)>,
    /// Turn numbers carry on from core's and the shared seen turn, so a
    /// finished turn is newer than anything already seen.
    base_turn: u64,
    turns: u64,
}

impl Watch {
    /// Until a rule says otherwise the agent is taken to be at rest.
    fn new(harness: usize, base_turn: u64, now: Instant) -> Watch {
        Watch { harness, word: "idle", idle_since: None, changed: now, looked: None, base_turn, turns: 0 }
    }

    fn turn(&self) -> u64 {
        self.base_turn + self.turns
    }

    /// Takes one look's verdict and returns the word to publish. No opinion
    /// keeps the last word, except that work with no sign of life for
    /// `STALL` has stopped; idle on a working pane waits to be confirmed.
    /// Leaving `working` finishes a turn.
    fn observe(&mut self, verdict: Option<&'static str>, now: Instant) -> &'static str {
        let was = self.word;
        match verdict {
            None => {
                self.idle_since = None;
                if self.word == "working" && now.duration_since(self.changed) >= STALL {
                    self.word = "idle";
                }
            }
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
        if self.word != was {
            self.changed = now;
            if was == "working" {
                self.turns += 1;
            }
        }
        self.word
    }
}

/// One unreported agent's state as the app shows it.
#[derive(Debug, PartialEq)]
pub struct Unreported {
    pub word: &'static str,
    pub harness: String,
    pub turn: u64,
}

/// The agents without a 7501 report whose foreground process a manifest names.
#[derive(Default)]
pub struct Watcher {
    /// Each terminal's foreground record and the manifest it matched, so a
    /// process is identified once, not every tick.
    detected: HashMap<String, (Value, Option<usize>)>,
    watching: HashMap<String, Watch>,
}

impl Watcher {
    /// Looks at core's live terminal records once: for each unreported agent,
    /// its state. `screen(address)` says what the app has of a terminal's
    /// screen; `tail(address, lines)` reads its last non-empty lines, only
    /// when the screen changed. A terminal that reports, ends, or stops
    /// running the agent drops out.
    pub fn update(&mut self, registry: &Registry, live: &HashMap<String, Value>, now: Instant, mut screen: impl FnMut(&str) -> Option<Screen>, mut tail: impl FnMut(&str, usize) -> Vec<String>) -> HashMap<String, Unreported> {
        let mut out = HashMap::new();
        self.detected.retain(|a, _| live.contains_key(a));
        for (addr, rec) in live {
            if rec.get("status").is_some_and(|s| !s.is_null()) || rec["state"] != "live" {
                continue;
            }
            let Some(fg) = rec.get("foreground") else { continue };
            let index = match self.detected.get(addr) {
                Some((seen, index)) if seen == fg => *index,
                _ => {
                    let argv: Vec<String> = fg["argv"].as_array().into_iter().flatten().filter_map(|a| a.as_str().map(str::to_owned)).collect();
                    let index = registry.identify_index(fg["name"].as_str().unwrap_or_default(), &argv);
                    self.detected.insert(addr.clone(), (fg.clone(), index));
                    index
                }
            };
            let Some(index) = index else { continue };
            let m = &registry.manifests[index];
            let watch = self.watching.entry(addr.clone()).or_insert_with(|| Watch::new(index, base_turn(rec), now));
            if watch.harness != index {
                *watch = Watch::new(index, base_turn(rec), now);
            }
            let verdict = match screen(addr) {
                Some(s) if watch.looked.is_some_and(|(writes, _)| writes == s.writes) => watch.looked.and_then(|(_, v)| v),
                Some(s) => {
                    watch.changed = now;
                    let lines = m.lines();
                    let verdict = (if lines == 0 { None } else { m.classify(&tail(addr, lines)) }).or_else(|| m.classify_title(&s.title));
                    watch.looked = Some((s.writes, verdict));
                    verdict
                }
                None => None,
            };
            let word = watch.observe(verdict, now);
            out.insert(addr.clone(), Unreported { word, harness: m.id.clone(), turn: watch.turn() });
        }
        self.watching.retain(|a, _| out.contains_key(a));
        out
    }
}

/// Where an unreported agent's turns start: past core's turn and the turn the
/// person last saw, so a turn it finishes now reads as unseen.
fn base_turn(rec: &Value) -> u64 {
    let turn = rec.get("turn").and_then(Value::as_u64).unwrap_or(0);
    let seen = rec.pointer("/labels/herdr.seen").and_then(Value::as_str).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
    turn.max(seen)
}

#[cfg(test)]
#[path = "harness_tests.rs"]
mod tests;
