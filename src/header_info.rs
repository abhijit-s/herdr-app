//! `pane_header_info`: a template of extra words an agent pane's header shows
//! between its name and its status. Resolved here, in `publish`, when what it
//! is made of changes (status, the agent's request, the folder's cached git
//! status, settings) and on the minute for `{elapsed}`; header mods only draw
//! it and pick what fits their width (`fit`).
//!
//! Grammar, all of it:
//! - `{ask}`, `{state}`, `{elapsed}`, `{branch}`, `{ahead}`, `{behind}`,
//!   `{dirty}` are keys; any other text, an unknown `{word}` too, is literal.
//! - `·` and `|` split the template into segments. The separator is drawn as
//!   written, with the spaces around it, between two segments that show.
//! - A segment with keys shows only when one of them is not empty; one with no
//!   keys always shows. Spaces left over by empty keys collapse into one.
//! - Keys written side by side (`{branch}{dirty}`) are drawn a space apart
//!   when both show, so markers never glue to the word before them.
//! - Header mods draw a separator as its mark alone with even padding either
//!   side (`sep` keeps it as written, spaces included, for the tooltip).

use serde::Serialize;

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
        self.0.iter().flat_map(|s| &s.pieces).any(|p| *p == Piece::Key(Key::Elapsed))
    }

    /// The segments that show for these values (None: nothing does).
    pub fn resolve(&self, v: &Values) -> Option<Info> {
        #[cfg(test)]
        RESOLVED.with(|n| n.set(n.get() + 1));
        let mut segments: Vec<Shown> = Vec::new();
        for s in &self.0 {
            let mut text = String::new();
            let (mut keys, mut filled, mut ask) = (0, 0, false);
            // The last thing written was a key that showed: the next one that
            // shows goes a space after it.
            let mut after_key = false;
            for p in &s.pieces {
                match p {
                    Piece::Text(t) => {
                        text.push_str(t);
                        after_key = false;
                    }
                    Piece::Key(k) => {
                        keys += 1;
                        let value = v.get(*k);
                        if value.is_empty() {
                            continue;
                        }
                        filled += 1;
                        ask |= *k == Key::Ask && v.blocked;
                        if after_key {
                            text.push(' ');
                        }
                        text.push_str(&value);
                        after_key = true;
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
            segments.push(Shown { sep, text, ask });
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
            // A bullet sits on the text's own line in a UI font; a black circle
            // comes from a fallback font, larger and off the baseline.
            Key::Dirty => if self.dirty { "•".into() } else { String::new() },
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
    /// It holds `{ask}` while the agent needs the person: drawn in the blocked
    /// colour, and the last to go when space runs short.
    pub ask: bool,
}

/// Below this many characters a lone segment is hidden, not cut.
const MIN_CHARS: f32 = 4.0;
/// A character's width as a share of the font size: a little over a UI font's
/// average, so what is kept fits; `truncate()` takes care of the rest.
const CHAR_EM: f32 = 0.6;

/// The segments that fit in `width` px of `font_px` text: whole segments go
/// from the right, a blocked `{ask}` last. A lone segment that still doesn't
/// fit stays (its element truncates it with an ellipsis) unless fewer than
/// about four characters would show.
pub fn fit(segments: &[Shown], width: f32, font_px: f32) -> Vec<Shown> {
    let char_px = font_px * CHAR_EM;
    let measure = |s: &str| self::width(s, font_px);
    let mut keep: Vec<Shown> = segments.to_vec();
    let total = |keep: &[Shown]| keep.iter().enumerate().map(|(i, s)| measure(&s.text) + if i == 0 { 0.0 } else { measure(&s.sep) }).sum::<f32>();
    while keep.len() > 1 && total(&keep) > width {
        let i = keep.iter().rposition(|s| !s.ask).unwrap_or(keep.len() - 1);
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
        Shown { sep: sep.into(), text: text.into(), ask }
    }

    const SUGGESTED: &str = "{ask} · {branch}{ahead}{behind}{dirty} · {state} {elapsed}";

    #[test]
    fn templates_split_on_separators_and_drop_empty_segments() {
        let v = Values { state: "working".into(), elapsed: "3m".into(), branch: "main".into(), ahead: 2, dirty: true, ..Values::default() };
        assert_eq!(shown(SUGGESTED, &v), vec![("".into(), "main ↑2 •".into(), false), (" · ".into(), "working 3m".into(), false)], "no ask: no dangling separator");
        assert_eq!(shown("{state}|{branch}", &v), vec![("".into(), "working".into(), false), ("|".into(), "main".into(), false)], "separators as written");
        assert_eq!(shown("{state} {elapsed}", &Values { state: "idle".into(), ..Values::default() }), vec![("".into(), "idle".into(), false)], "leftover spaces collapse");
        assert_eq!(shown("on {branch} · hi", &Values::default()), vec![("".into(), "hi".into(), false)], "a segment of empty keys goes; plain text stays");
        assert_eq!(shown("{nope} · {state}", &v), vec![("".into(), "{nope}".into(), false), (" · ".into(), "working".into(), false)], "an unknown key is text");
        assert!(Template::parse("").is_empty() && Template::parse("  ·  | ").is_empty(), "nothing to show");
        assert!(Template::parse(SUGGESTED).uses_elapsed() && !Template::parse("{branch}").uses_elapsed());
        assert!(Template::parse(SUGGESTED).uses_git_counts() && !Template::parse("{branch} · {state}").uses_git_counts());
        assert!(counts_git(Some(SUGGESTED)) && !counts_git(Some("{branch}")) && !counts_git(None));
        let info = Template::parse(SUGGESTED).resolve(&Values::sample()).unwrap();
        assert_eq!(info.text, "approve Bash: make · main ↑2 • · blocked 3m", "the whole line, for the tooltip");
    }

    #[test]
    fn keys_side_by_side_are_drawn_a_space_apart() {
        let v = Values { branch: "main".into(), ahead: 2, behind: 1, dirty: true, state: "idle".into(), elapsed: "3m".into(), ..Values::default() };
        assert_eq!(shown("{branch}{ahead}{behind}{dirty}", &v)[0].1, "main ↑2 ↓1 •");
        assert_eq!(shown("{branch}{dirty}", &Values { dirty: false, ..v.clone() })[0].1, "main", "no trailing space for an empty key");
        assert_eq!(shown("{dirty}{branch}", &Values { branch: String::new(), ..v.clone() })[0].1, "•", "nor a leading one");
        assert_eq!(shown("{branch}/{state}", &v)[0].1, "main/idle", "text written between keys is the person's own");
        assert_eq!(shown("{state}{elapsed}", &v)[0].1, "idle 3m", "any keys, not only git's");
        assert_eq!(Values { dirty: true, ..Values::default() }.get(Key::Dirty), "•", "a bullet in the text's own font, not a fallback circle");
    }

    #[test]
    fn git_and_ask_keys_render_empty_when_unknown_or_zero() {
        assert_eq!(shown("{branch}{ahead}{behind}{dirty}", &Values::default()), vec![]);
        let v = Values { branch: "dev".into(), ahead: 0, behind: 3, ..Values::default() };
        assert_eq!(shown("{branch}{ahead}{behind}{dirty}", &v), vec![("".into(), "dev ↓3".into(), false)], "an empty key between two leaves one space");
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
}
