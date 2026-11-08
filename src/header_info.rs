//! `pane_header_info`: a template of extra words an agent pane's header shows
//! between its name and its status. Resolved here, in `publish`, when what it
//! is made of changes (status, the agent's request, the folder's cached git
//! status, settings) and on the minute for `{elapsed}`; header mods only draw
//! it and pick what fits their width (`fit`).
//!
//! Grammar, all of it:
//! - `{ask}`, `{state}`, `{elapsed}`, `{branch}`, `{ahead}`, `{behind}`,
//!   `{dirty}`, `{subagents}` are keys; any other text, an unknown `{word}`
//!   too, is literal.
//! - `·` and `|` split the template into segments. The separator is drawn as
//!   written, with the spaces around it, between two segments that show.
//! - A segment with keys shows only when one of them is not empty; one with no
//!   keys always shows. Spaces left over by empty keys collapse into one.

use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;

#[cfg(test)]
thread_local! {
    /// How many panes' info this thread resolved: they follow changes, not frames.
    pub static RESOLVED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

const SEPARATORS: [char; 2] = ['·', '|'];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Key {
    Ask,
    State,
    Elapsed,
    Branch,
    Ahead,
    Behind,
    Dirty,
    Subagents,
}

impl Key {
    fn named(name: &str) -> Option<Key> {
        Some(match name {
            "ask" => Key::Ask,
            "state" => Key::State,
            "elapsed" => Key::Elapsed,
            "branch" => Key::Branch,
            "ahead" => Key::Ahead,
            "behind" => Key::Behind,
            "dirty" => Key::Dirty,
            "subagents" => Key::Subagents,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Piece {
    Text(String),
    Key(Key),
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Segment {
    /// The separator before it as written, spaces included ("" for the first).
    sep: String,
    pieces: Vec<Piece>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Template(Vec<Segment>);

impl Template {
    pub fn parse(template: &str) -> Template {
        let mut segments = vec![Segment::default()];
        let mut text = String::new();
        let mut rest = template;
        while let Some(c) = rest.chars().next() {
            if SEPARATORS.contains(&c) {
                let trailing = text.len() - text.trim_end().len();
                let mut sep = text.split_off(text.len() - trailing);
                flush(segments.last_mut().unwrap(), &mut text);
                let after = &rest[c.len_utf8()..];
                let leading = after.len() - after.trim_start().len();
                sep.push(c);
                sep.push_str(&after[..leading]);
                segments.push(Segment { sep, pieces: vec![] });
                rest = &after[leading..];
                continue;
            }
            if c == '{'
                && let Some(end) = rest.find('}')
                && let Some(key) = Key::named(&rest[1..end])
            {
                flush(segments.last_mut().unwrap(), &mut text);
                segments.last_mut().unwrap().pieces.push(Piece::Key(key));
                rest = &rest[end + 1..];
                continue;
            }
            text.push(c);
            rest = &rest[c.len_utf8()..];
        }
        flush(segments.last_mut().unwrap(), &mut text);
        segments.retain(|s| !s.pieces.is_empty());
        if let Some(first) = segments.first_mut() {
            first.sep.clear();
        }
        Template(segments)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether it shows ahead, behind or dirty (folder info then counts them).
    pub fn uses_git_counts(&self) -> bool {
        self.0.iter().flat_map(|s| &s.pieces).any(|p| matches!(p, Piece::Key(Key::Ahead | Key::Behind | Key::Dirty)))
    }

    pub fn uses_elapsed(&self) -> bool {
        self.uses(Key::Elapsed)
    }

    /// Whether it shows `{subagents}` (publish then counts them).
    pub fn uses_subagents(&self) -> bool {
        self.uses(Key::Subagents)
    }

    fn uses(&self, key: Key) -> bool {
        self.0.iter().flat_map(|s| &s.pieces).any(|p| *p == Piece::Key(key))
    }

    /// The segments that show for these values (None: nothing does).
    pub fn resolve(&self, v: &Values) -> Option<Info> {
        #[cfg(test)]
        RESOLVED.with(|n| n.set(n.get() + 1));
        let mut segments: Vec<Shown> = Vec::new();
        for s in &self.0 {
            let mut text = String::new();
            let (mut keys, mut filled, mut ask, mut minor) = (0, 0, false, false);
            for p in &s.pieces {
                match p {
                    Piece::Text(t) => text.push_str(t),
                    Piece::Key(k) => {
                        keys += 1;
                        let value = v.get(*k);
                        if !value.is_empty() {
                            filled += 1;
                            ask |= (*k == Key::Ask && v.blocked) || (*k == Key::Subagents && v.subagents.blocked > 0);
                            minor |= *k == Key::Subagents;
                        }
                        text.push_str(&value);
                    }
                }
            }
            if keys > 0 && filled == 0 {
                continue;
            }
            let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            if text.is_empty() {
                continue;
            }
            let sep = if segments.is_empty() { String::new() } else { s.sep.clone() };
            segments.push(Shown { sep, text, ask, minor: minor && !ask });
        }
        if segments.is_empty() {
            return None;
        }
        let text = segments.iter().map(|s| format!("{}{}", s.sep, s.text)).collect();
        Some(Info { text, segments })
    }
}

fn flush(segment: &mut Segment, text: &mut String) {
    if !text.is_empty() {
        segment.pieces.push(Piece::Text(std::mem::take(text)));
    }
}

/// What the keys stand for, for one pane.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Values {
    /// The agent's request while it needs the person, else empty.
    pub ask: String,
    pub blocked: bool,
    /// working, blocked, done or idle.
    pub state: String,
    /// Time in that state, `elapsed()`; empty when unknown.
    pub elapsed: String,
    pub branch: String,
    pub ahead: u64,
    pub behind: u64,
    pub dirty: bool,
    pub subagents: Subagents,
}

/// An agent's live sub-agents: child terminals below it, and the OSC 7501
/// child records (`id=`) it and they report, such as Claude Code's own
/// sub-agents (`count_subagents`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Subagents {
    pub live: usize,
    /// Blocked or in error.
    pub blocked: usize,
}

impl Subagents {
    fn add(&mut self, word: Option<&str>) {
        self.live += 1;
        if matches!(word, Some("blocked" | "error")) {
            self.blocked += 1;
        }
    }

    /// `⧉ 3`, `⧉ 3 (1 blocked)`, or `⧉ 2 blocked` when all are; empty for none.
    fn text(&self) -> String {
        match (self.live, self.blocked) {
            (0, _) => String::new(),
            (n, 0) => format!("⧉ {n}"),
            (n, b) if b == n => format!("⧉ {n} blocked"),
            (n, b) => format!("⧉ {n} ({b} blocked)"),
        }
    }
}

/// The live sub-agents below `addr` in core's terminal records (`live`, keyed
/// by address, `status` a word): every live terminal whose parent chain reaches
/// it, in the state `status_of` gives (the app's, for a pane), else its own
/// word, and every child record of `addr` and of those terminals.
pub fn count_subagents<'a>(live: &'a HashMap<String, Value>, addr: &str, status_of: impl Fn(&str) -> Option<&'a str>) -> Subagents {
    let parent = |a: &str| live.get(a).and_then(|r| r["parent"].as_str()).filter(|p| !p.is_empty());
    let below = |kid: &str| {
        let mut cur = kid;
        for _ in 0..32 {
            match parent(cur) {
                Some(p) if p == addr => return true,
                Some(p) => cur = p,
                None => return false,
            }
        }
        false
    };
    let mut n = Subagents::default();
    let records = |r: &Value, n: &mut Subagents| r["records"].as_object().into_iter().flatten().for_each(|(_, rec)| n.add(rec["state"].as_str()));
    if let Some(r) = live.get(addr) {
        records(r, &mut n);
    }
    for (kid, r) in live {
        if r["state"] == "live" && below(kid) {
            records(r, &mut n);
            n.add(status_of(kid).or(r["status"].as_str()));
        }
    }
    n
}

impl Values {
    fn get(&self, k: Key) -> String {
        match k {
            Key::Ask => self.ask.clone(),
            Key::State => self.state.clone(),
            Key::Elapsed => self.elapsed.clone(),
            Key::Branch => self.branch.clone(),
            Key::Ahead => if self.ahead > 0 { format!("↑{}", self.ahead) } else { String::new() },
            Key::Behind => if self.behind > 0 { format!("↓{}", self.behind) } else { String::new() },
            Key::Dirty => if self.dirty { "●".into() } else { String::new() },
            Key::Subagents => self.subagents.text(),
        }
    }

    /// What the Settings window previews a template with.
    pub fn sample() -> Values {
        Values {
            ask: "approve Bash: make".into(),
            blocked: true,
            state: "blocked".into(),
            elapsed: "3m".into(),
            branch: "main".into(),
            ahead: 2,
            behind: 0,
            dirty: true,
            subagents: Subagents { live: 2, blocked: 0 },
        }
    }
}

/// Whether `pane_header_info` shows ahead, behind or dirty: folder info
/// counts them only then.
pub fn counts_git(template: Option<&str>) -> bool {
    template.is_some_and(|t| Template::parse(t).uses_git_counts())
}

/// `{ask}`: the agent's own summary from its 7501 report, else what kind of
/// request it is ("permission", "question"); only while it needs the person.
pub fn ask(blocked: bool, said: &crate::alert::Said) -> String {
    if !blocked {
        return String::new();
    }
    let msg = said.message.trim();
    if msg.is_empty() { said.kind.trim().to_owned() } else { msg.to_owned() }
}

const MINUTE: u64 = 60_000;

/// Time in a state, compact and changing only on the minute: `<1m`, then
/// `3m`, `1h`, `1h2m`.
pub fn elapsed(ms: u64) -> String {
    let m = ms / MINUTE;
    match m {
        0 => "<1m".into(),
        1..60 => format!("{m}m"),
        _ if m % 60 == 0 => format!("{}h", m / 60),
        _ => format!("{}h{}m", m / 60, m % 60),
    }
}

/// When `elapsed` next reads differently for a state entered at `since`:
/// the one-minute mark, then each minute after.
pub fn next_change(since: u64, now: u64) -> u64 {
    since + ((now.saturating_sub(since) / MINUTE) + 1) * MINUTE
}

/// A pane's resolved info, as `PaneInfo.header_info`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Info {
    /// Every segment with its separators: the tooltip.
    pub text: String,
    pub segments: Vec<Shown>,
}

#[derive(Clone, Debug, PartialEq, Serialize, serde::Deserialize)]
pub struct Shown {
    pub sep: String,
    pub text: String,
    /// It holds `{ask}` while the agent needs the person, or `{subagents}`
    /// while one is blocked: drawn in the blocked colour, and the last to go
    /// when space runs short.
    pub ask: bool,
    /// It holds `{subagents}` (none blocked): the first to go. Left out when
    /// false, so the segments mods already read are unchanged.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub minor: bool,
}

/// Below this many characters a lone segment is hidden, not cut.
const MIN_CHARS: f32 = 4.0;
/// A character's width as a share of the font size: a little over a UI font's
/// average, so what is kept fits; `truncate()` takes care of the rest.
const CHAR_EM: f32 = 0.6;

/// The segments that fit in `width` px of `font_px` text: whole segments go
/// from the right, `{subagents}` first and a blocked `{ask}` last. A lone segment that still doesn't
/// fit stays (its element truncates it with an ellipsis) unless fewer than
/// about four characters would show.
pub fn fit(segments: &[Shown], width: f32, font_px: f32) -> Vec<Shown> {
    let char_px = font_px * CHAR_EM;
    let measure = |s: &str| self::width(s, font_px);
    let mut keep: Vec<Shown> = segments.to_vec();
    let total = |keep: &[Shown]| keep.iter().enumerate().map(|(i, s)| measure(&s.text) + if i == 0 { 0.0 } else { measure(&s.sep) }).sum::<f32>();
    while keep.len() > 1 && total(&keep) > width {
        let i = keep.iter().rposition(|s| s.minor).or_else(|| keep.iter().rposition(|s| !s.ask)).unwrap_or(keep.len() - 1);
        keep.remove(i);
    }
    if let Some(first) = keep.first_mut() {
        first.sep.clear();
    }
    if keep.len() == 1 && total(&keep) > width && width < (MIN_CHARS + 1.0) * char_px {
        keep.clear();
    }
    keep
}

/// `fitHeaderInfo(segments, width, fontPx)` for mods.
pub fn fit_for_mods(segments: serde_json::Value, width: f32, font_px: f32) -> Result<serde_json::Value, String> {
    let segments: Vec<Shown> = serde_json::from_value(segments).map_err(|e| format!("fitHeaderInfo segments: {e}"))?;
    Ok(serde_json::json!(fit(&segments, width, font_px)))
}

/// `headerInfoPreview(template)` for mods: the template with `Values::sample()`.
pub fn preview_for_mods(template: &str) -> serde_json::Value {
    serde_json::json!(Template::parse(template).resolve(&Values::sample()))
}

/// About how wide `text` is in `font_px` text (`headerTextWidth` for mods, so a
/// header sizes what's beside the info as `fit` sizes the info).
pub fn width(text: &str, font_px: f32) -> f32 {
    text.chars().map(|c| if wide(c) { 2.0 } else { 1.0 }).sum::<f32>() * font_px * CHAR_EM
}

/// Two columns wide: East Asian wide and emoji, roughly.
fn wide(c: char) -> bool {
    matches!(c as u32, 0x1100..=0x115F | 0x2E80..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6 | 0x1F300..=0x1FAFF | 0x20000..=0x3FFFD)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(t: &str, v: &Values) -> Vec<(String, String, bool)> {
        Template::parse(t).resolve(v).map(|i| i.segments.into_iter().map(|s| (s.sep, s.text, s.ask)).collect()).unwrap_or_default()
    }

    fn seg(sep: &str, text: &str, ask: bool) -> Shown {
        Shown { sep: sep.into(), text: text.into(), ask, minor: false }
    }

    const SUGGESTED: &str = "{ask} · {branch}{ahead}{behind}{dirty} · {state} {elapsed} · {subagents}";

    #[test]
    fn templates_split_on_separators_and_drop_empty_segments() {
        let v = Values { state: "working".into(), elapsed: "3m".into(), branch: "main".into(), ahead: 2, dirty: true, ..Values::default() };
        assert_eq!(shown(SUGGESTED, &v), vec![("".into(), "main↑2●".into(), false), (" · ".into(), "working 3m".into(), false)], "no ask or sub-agents: no dangling separator");
        assert_eq!(shown("{state}|{branch}", &v), vec![("".into(), "working".into(), false), ("|".into(), "main".into(), false)], "separators as written");
        assert_eq!(shown("{state} {elapsed}", &Values { state: "idle".into(), ..Values::default() }), vec![("".into(), "idle".into(), false)], "leftover spaces collapse");
        assert_eq!(shown("on {branch} · hi", &Values::default()), vec![("".into(), "hi".into(), false)], "a segment of empty keys goes; plain text stays");
        assert_eq!(shown("{nope} · {state}", &v), vec![("".into(), "{nope}".into(), false), (" · ".into(), "working".into(), false)], "an unknown key is text");
        assert!(Template::parse("").is_empty() && Template::parse("  ·  | ").is_empty(), "nothing to show");
        assert!(Template::parse(SUGGESTED).uses_elapsed() && !Template::parse("{branch}").uses_elapsed());
        assert!(Template::parse(SUGGESTED).uses_git_counts() && !Template::parse("{branch} · {state}").uses_git_counts());
        assert!(counts_git(Some(SUGGESTED)) && !counts_git(Some("{branch}")) && !counts_git(None));
        let info = Template::parse(SUGGESTED).resolve(&Values::sample()).unwrap();
        assert_eq!(info.text, "approve Bash: make · main↑2● · blocked 3m · ⧉ 2", "the whole line, for the tooltip");
    }

    #[test]
    fn git_and_ask_keys_render_empty_when_unknown_or_zero() {
        assert_eq!(shown("{branch}{ahead}{behind}{dirty}", &Values::default()), vec![]);
        let v = Values { branch: "dev".into(), ahead: 0, behind: 3, ..Values::default() };
        assert_eq!(shown("{branch}{ahead}{behind}{dirty}", &v), vec![("".into(), "dev↓3".into(), false)]);
        let said = |kind: &str, msg: &str| crate::alert::Said { kind: kind.into(), title: String::new(), message: msg.into() };
        assert_eq!(ask(true, &said("permission", "approve Bash: make")), "approve Bash: make", "its own words first");
        assert_eq!(ask(true, &said("question", " ")), "question", "else the kind");
        assert_eq!(ask(true, &said("", "")), "");
        assert_eq!(ask(false, &said("permission", "approve Bash: make")), "", "only while it needs you");
        let blocked = Values { ask: "approve".into(), blocked: true, ..Values::default() };
        assert_eq!(shown("{ask} · x", &blocked), vec![("".into(), "approve".into(), true), (" · ".into(), "x".into(), false)]);
        assert_eq!(shown("{ask} · x", &Values { ask: "q".into(), ..Values::default() })[0].2, false, "the blocked colour only while blocked");
    }

    #[test]
    fn elapsed_is_under_a_minute_then_minutes_then_hours() {
        assert_eq!(elapsed(0), "<1m");
        assert_eq!(elapsed(45_400), "<1m", "no seconds: it changes only on the minute");
        assert_eq!(elapsed(59_999), "<1m");
        assert_eq!(elapsed(60_000), "1m");
        assert_eq!(elapsed(3 * 60_000 + 59_000), "3m");
        assert_eq!(elapsed(60 * 60_000), "1h");
        assert_eq!(elapsed(62 * 60_000), "1h2m");
        assert_eq!(elapsed(25 * 60 * 60_000 + 60_000), "25h1m");
        assert_eq!(next_change(1_000, 1_000), 61_000, "the one-minute mark first");
        assert_eq!(next_change(1_000, 30_000), 61_000);
        assert_eq!(next_change(1_000, 61_000), 121_000, "then each minute");
        assert_eq!(next_change(1_000, 200_000), 241_000);
    }

    // At 12 px a character is estimated at 7.2 px.
    #[test]
    fn segments_drop_from_the_right_to_fit() {
        let all = vec![seg("", "aaaa", false), seg(" · ", "bbbb", false), seg(" · ", "cccc", false)];
        assert_eq!(fit(&all, 1000.0, 12.0), all, "room for all");
        let w = |chars: f32| chars * 7.2 + 0.1;
        assert_eq!(fit(&all, w(11.0), 12.0), all[..2].to_vec(), "the last goes first");
        assert_eq!(fit(&all, w(10.9), 12.0), all[..1].to_vec());
    }

    #[test]
    fn a_blocked_ask_is_the_last_to_go() {
        let all = vec![seg("", "main", false), seg(" · ", "approve", true), seg(" · ", "working", false)];
        let w = |chars: f32| chars * 7.2 + 0.1;
        assert_eq!(fit(&all, w(14.0), 12.0), all[..2].to_vec());
        assert_eq!(fit(&all, w(10.0), 12.0), vec![seg("", "approve", true)], "the ask outlasts what came before it");
    }

    #[test]
    fn a_lone_segment_is_cut_not_hidden_until_too_narrow() {
        let ask = vec![seg("", "approve Bash: npm run build && npm test", true), seg(" · ", "main", false)];
        let w = |chars: f32| chars * 7.2 + 0.1;
        assert_eq!(fit(&ask, w(12.0), 12.0), vec![ask[0].clone()], "kept: its element ends it with an ellipsis");
        assert_eq!(fit(&ask, w(5.0), 12.0), vec![ask[0].clone()], "four characters and the ellipsis");
        assert_eq!(fit(&ask, w(4.5), 12.0), vec![], "fewer: hidden");
        assert_eq!(fit(&[seg("", "ok", false)], w(2.0), 12.0), vec![seg("", "ok", false)], "short and fits");
        let close = |a: f32, b: f32| (a - b).abs() < 0.01;
        assert!(close(width("ab", 12.0), 14.4) && close(width("日本", 12.0), 28.8) && close(width("🦊", 12.0), 14.4), "an emoji's two columns, as its two-character width");
        let wide = vec![seg("", "ab", false), seg("|", "日本", false)];
        assert_eq!(fit(&wide, w(6.5), 12.0), wide[..1].to_vec(), "wide characters count twice");
    }

    fn live(records: &[(&str, serde_json::Value)]) -> HashMap<String, Value> {
        records.iter().map(|(a, r)| ((*a).to_owned(), r.clone())).collect()
    }

    #[test]
    fn sub_agents_are_child_terminals_below_and_child_records() {
        use serde_json::json;
        let rec = |state: &str| json!({ "state": state });
        let live = live(&[
            ("t1", json!({ "state": "live", "status": "working", "records": { "agent/a": rec("working"), "agent/b": rec("blocked") } })),
            ("t2", json!({ "state": "live", "status": "idle", "parent": "t1" })),
            ("t3", json!({ "state": "live", "status": "error", "parent": "t2", "records": { "job": rec("idle") } })),
            ("t4", json!({ "state": "finished", "status": "working", "parent": "t1" })),
            ("t5", json!({ "state": "live", "status": "working", "parent": "" })),
        ]);
        let none = |_: &str| None;
        assert_eq!(count_subagents(&live, "t1", none), Subagents { live: 5, blocked: 2 }, "two records, t2, t3 below it and t3's record; not t4 (finished)");
        assert_eq!(count_subagents(&live, "t2", none), Subagents { live: 2, blocked: 1 }, "error counts as blocked");
        assert_eq!(count_subagents(&live, "t5", none), Subagents::default());
        assert_eq!(count_subagents(&live, "t1", |a| (a == "t2").then_some("blocked")).blocked, 3, "the app's state for a pane wins");
    }

    #[test]
    fn sub_agents_render_their_count_and_how_many_are_blocked() {
        let v = |live, blocked| Values { subagents: Subagents { live, blocked }, ..Values::default() };
        assert_eq!(shown("{subagents}", &v(0, 0)), vec![], "none: the segment drops");
        assert_eq!(shown("x · {subagents}", &v(3, 0)), vec![("".into(), "x".into(), false), (" · ".into(), "⧉ 3".into(), false)]);
        assert_eq!(shown("{subagents}", &v(3, 1)), vec![("".into(), "⧉ 3 (1 blocked)".into(), true)], "blocked: drawn as the ask is");
        assert_eq!(shown("{subagents}", &v(2, 2)), vec![("".into(), "⧉ 2 blocked".into(), true)]);
        let minor = |t: &str, v: &Values| Template::parse(t).resolve(v).unwrap().segments.iter().map(|s| s.minor).collect::<Vec<_>>();
        assert_eq!(minor("{state} · {subagents}", &Values { state: "idle".into(), ..v(3, 0) }), vec![false, true]);
        assert_eq!(minor("{state} · {subagents}", &Values { state: "idle".into(), ..v(3, 1) }), vec![false, false], "blocked is not minor");
        assert!(Template::parse(SUGGESTED).uses_subagents() && !Template::parse("{state}").uses_subagents());
    }

    #[test]
    fn sub_agents_go_first_unless_blocked() {
        let sub = Shown { sep: " · ".into(), text: "⧉ 3".into(), ask: false, minor: true };
        let all = vec![seg("", "main", false), sub.clone(), seg(" · ", "idle 3m", false)];
        let w = |chars: f32| chars * 7.2 + 0.1;
        assert_eq!(fit(&all, w(14.0), 12.0), vec![all[0].clone(), all[2].clone()], "the count goes before the segment right of it");
        assert_eq!(fit(&all, w(4.0), 12.0), vec![all[0].clone()]);
        let blocked = vec![seg("", "main", false), seg(" · ", "⧉ 3 (1 blocked)", true), seg(" · ", "idle", false)];
        assert_eq!(fit(&blocked, w(15.0), 12.0), vec![seg("", "⧉ 3 (1 blocked)", true)], "blocked outlasts the rest");
        let from_js: Vec<Shown> = serde_json::from_value(serde_json::json!([{ "sep": "", "text": "a", "ask": false }])).unwrap();
        assert!(!from_js[0].minor, "segments from an older mod read as not minor");
    }
}
