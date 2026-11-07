//! What a pane or a tab shows as its icon: `icon` on `panes()`, `agents()` and
//! each tab in `spaces()`, so every mod draws the same one. `null` means the
//! program's icon, drawn as it always was.
//!
//! A pane's, first that applies:
//! 1. the icon someone chose (`pane.set_icon`), kept by pane id;
//! 2. with `space_avatar` at glyph, emoji or repo (glyph), a bare shell
//!    showing its automatic name (`pane_auto_names`): the icon of one of the
//!    name's words (meadow: wheat), noun first; neither word listed: one picked
//!    from the pane id's hash;
//! 3. null: an agent or program shows its program's icon, a bare shell the terminal's.
//!
//! A tab's: the icon someone chose (`tab.set_icon`), else its focused pane's.
//! The words' icons are data, `defaults/pane-icons.json`.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::OnceLock;

const SHIPPED: &str = include_str!("../defaults/pane-icons.json");

/// What someone chose for a pane or a tab (`pane.set_icon`, `tab.set_icon`):
/// `{ kind, value? }`, the shape spaces' own icons have.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Glyph,
    Emoji,
    /// The first letter of what the pane or tab is called, worked out when shown.
    Letter,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Glyph => "glyph",
            Kind::Emoji => "emoji",
            Kind::Letter => "letter",
        }
    }
}

impl Choice {
    /// A command's `icon` argument: null (clear it) or `{ kind, value }`; a
    /// glyph or emoji needs a short value, a letter's value is dropped.
    pub fn parse(v: &Value) -> Result<Option<Choice>, String> {
        if v.is_null() {
            return Ok(None);
        }
        let mut c: Choice = serde_json::from_value(v.clone()).map_err(|_| "icon is null or { kind: \"glyph\" | \"emoji\" | \"letter\", value }".to_owned())?;
        if c.kind == Kind::Letter {
            c.value = None;
            return Ok(Some(c));
        }
        match c.value.as_deref() {
            Some(t) if !t.is_empty() && t.len() <= 32 && !t.chars().any(|ch| ch.is_control() || ch.is_whitespace()) => Ok(Some(c)),
            _ => Err(format!("a {} icon needs a value: one glyph or emoji", c.kind.name())),
        }
    }
}

#[derive(Debug, Deserialize)]
struct Word {
    emoji: String,
    /// The Nerd Font code point, hex.
    glyph: String,
    /// Its Nerd Font name, for people (and the test that checks the font has it).
    #[cfg_attr(not(test), allow(dead_code))]
    nf: String,
}

struct Words {
    by_word: BTreeMap<String, (String, String)>,
}

impl Words {
    fn parse(text: &str) -> Result<Words, String> {
        #[derive(Deserialize)]
        struct File {
            words: BTreeMap<String, Word>,
        }
        let file: File = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if file.words.is_empty() {
            return Err("words is empty".into());
        }
        let mut by_word = BTreeMap::new();
        for (w, word) in file.words {
            let glyph = u32::from_str_radix(&word.glyph, 16).ok().and_then(char::from_u32).ok_or_else(|| format!("{w}: {:?} isn't a code point", word.glyph))?;
            if word.emoji.chars().count() != 1 {
                return Err(format!("{w}: {:?} isn't one emoji", word.emoji));
            }
            by_word.insert(w, (glyph.to_string(), word.emoji));
        }
        Ok(Words { by_word })
    }

    /// The icon for an automatic name ("hazy-inlet"): its noun's, else its
    /// adjective's, else one from the id's hash among all the words' icons.
    fn pick(&self, name: &str, id: &str, kind: Kind) -> String {
        let of = |(glyph, emoji): &(String, String)| if kind == Kind::Emoji { emoji.clone() } else { glyph.clone() };
        if let Some(icon) = name.rsplit('-').find_map(|w| self.by_word.get(w)) {
            return of(icon);
        }
        let i = (crate::pane_names::fnv1a(id.as_bytes()) % self.by_word.len() as u64) as usize;
        of(self.by_word.values().nth(i).expect("i < len"))
    }
}

/// The shipped words' icons; None (no word icons) if the file doesn't hold
/// up, which `shipped_icons_hold_up` catches before a release.
fn words() -> Option<&'static Words> {
    static WORDS: OnceLock<Option<Words>> = OnceLock::new();
    WORDS
        .get_or_init(|| Words::parse(SHIPPED).map_err(|e| tracing::warn!("defaults/pane-icons.json: {e}")).ok())
        .as_ref()
}

/// The automatic icon's style for `space_avatar`: none for letter (or unset),
/// which keeps every icon as it was.
pub fn style(space_avatar: Option<&str>) -> Option<Kind> {
    match space_avatar {
        Some("glyph" | "repo") => Some(Kind::Glyph),
        Some("emoji") => Some(Kind::Emoji),
        _ => None,
    }
}

#[cfg(test)]
thread_local! {
    /// How many icons this thread resolved: they follow changes, not frames.
    pub static RESOLVED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// A pane's icon: what someone chose, else (in `style`) its automatic name's,
/// else null. `label` gives a letter its letter.
pub fn pane(chosen: Option<&Choice>, auto_name: Option<&str>, id: &str, label: &str, style: Option<Kind>) -> Value {
    #[cfg(test)]
    RESOLVED.with(|n| n.set(n.get() + 1));
    if let Some(c) = chosen {
        return shown(c, label);
    }
    match (style, auto_name, words()) {
        (Some(kind), Some(name), Some(w)) => json!({ "kind": kind.name(), "text": w.pick(name, id, kind) }),
        _ => Value::Null,
    }
}

/// A tab's icon: what someone chose (a letter: of `name`), else its focused pane's.
pub fn tab(chosen: Option<&Choice>, name: &str, focused: Option<&Value>) -> Value {
    match chosen {
        Some(c) => shown(c, name),
        None => focused.cloned().unwrap_or(Value::Null),
    }
}

fn shown(c: &Choice, name: &str) -> Value {
    let text = match c.kind {
        Kind::Letter => letter(name),
        _ => c.value.clone().unwrap_or_default(),
    };
    json!({ "kind": c.kind.name(), "text": text })
}

/// The first letter or digit, upper case ("?" without one): what a letter icon shows.
pub fn letter(name: &str) -> String {
    name.chars().find(|c| c.is_alphanumeric()).map_or_else(|| "?".to_owned(), |c| c.to_uppercase().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words_listed() -> Vec<String> {
        let names: Value = serde_json::from_str(include_str!("../defaults/pane-names.json")).unwrap();
        ["adjectives", "nouns"].iter().flat_map(|k| names[k].as_array().unwrap().iter().map(|w| w.as_str().unwrap().to_owned())).collect()
    }

    /// Every word the names use has an icon, and nothing else: each glyph is in
    /// the bundled Symbols Nerd Font under the name the file gives it.
    #[test]
    fn shipped_icons_hold_up() {
        #[derive(Deserialize)]
        struct File {
            words: BTreeMap<String, Word>,
        }
        let file: File = serde_json::from_str(SHIPPED).unwrap();
        let w = Words::parse(SHIPPED).expect("defaults/pane-icons.json");
        let mut listed = words_listed();
        listed.sort();
        assert_eq!(file.words.keys().cloned().collect::<Vec<_>>(), listed, "an icon for every word of pane-names.json, and only those");
        let face = ttf_parser::Face::parse(crate::settings::SYMBOLS_FONT_FILE, 0).unwrap();
        let names = face.tables().post.unwrap().names();
        let named: BTreeMap<u16, &str> = names.enumerate().map(|(i, n)| (i as u16, n)).collect();
        for (word, entry) in &file.words {
            let ch = w.by_word[word].0.chars().next().unwrap();
            let id = face.glyph_index(ch).unwrap_or_else(|| panic!("{word}: U+{:X} isn't in the Nerd Font", ch as u32));
            assert_eq!(face.glyph_name(id).or(named.get(&id.0).copied()), Some(entry.nf.as_str()), "{word}: the glyph is the one named");
        }
        assert!(words().is_some());
    }

    #[test]
    fn icon_files_are_checked_when_read() {
        assert!(Words::parse(r#"{"words":{"inlet":{"emoji":"🐬","glyph":"f0e68","nf":"x"}}}"#).is_ok());
        for bad in [
            r#"{"words":{}}"#,
            r#"{"words":{"inlet":{"emoji":"🐬","glyph":"zz","nf":"x"}}}"#,
            r#"{"words":{"inlet":{"emoji":"🐬","glyph":"d800","nf":"x"}}}"#,
            r#"{"words":{"inlet":{"emoji":"🐬🐬","glyph":"f0e68","nf":"x"}}}"#,
            r#"{"words":{"inlet":{"emoji":"🐬"}}}"#,
        ] {
            assert!(Words::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_name_shows_its_nouns_icon_else_its_adjectives() {
        let w = Words::parse(r#"{"words":{"meadow":{"emoji":"🌾","glyph":"f0d23","nf":""},"hazy":{"emoji":"🌇","glyph":"f0f30","nf":""},"river":{"emoji":"🌊","glyph":"f078d","nf":""}}}"#).unwrap();
        assert_eq!(w.pick("hazy-meadow", "p1", Kind::Emoji), "🌾", "the noun first");
        assert_eq!(w.pick("hazy-meadow", "p1", Kind::Glyph), "\u{f0d23}");
        assert_eq!(w.pick("hazy-ford", "p1", Kind::Emoji), "🌇", "an unlisted noun: the adjective's");
        // The shipped words: the brief's examples.
        let shipped = words().unwrap();
        assert_eq!(shipped.pick("sunny-meadow", "p", Kind::Emoji), "🌾");
        assert_eq!(shipped.pick("sunny-meadow", "p", Kind::Glyph), "\u{f0073}", "md-barley, wheat");
        assert_eq!(shipped.pick("hazy-glen", "p", Kind::Emoji), "🦊");
    }

    /// Neither word listed: the id's hash picks, the same every time, another
    /// id usually another, always one of the words' icons.
    #[test]
    fn a_name_without_listed_words_keeps_a_stable_icon_by_id() {
        let w = Words::parse(r#"{"words":{"a":{"emoji":"🌾","glyph":"f0d23","nf":""},"b":{"emoji":"🌇","glyph":"f0f30","nf":""},"c":{"emoji":"🌊","glyph":"f078d","nf":""}}}"#).unwrap();
        let id = "p0123456789abcdef0123456789abcdef";
        let first = w.pick("odd-word", id, Kind::Emoji);
        for _ in 0..3 {
            assert_eq!(w.pick("odd-word", id, Kind::Emoji), first);
        }
        assert_eq!(w.pick("other-thing", id, Kind::Emoji), first, "the id picks, not the words");
        // FNV-1a of the id, modulo three, in word order: pinned so a change to the hash shows.
        assert_eq!(first, ["🌾", "🌇", "🌊"][(crate::pane_names::fnv1a(id.as_bytes()) % 3) as usize]);
        let picks: std::collections::BTreeSet<String> = (0..32).map(|i| w.pick("x-y", &format!("p{i}"), Kind::Emoji)).collect();
        assert_eq!(picks.len(), 3, "ids spread over every icon");
        assert!(picks.iter().all(|p| ["🌾", "🌇", "🌊"].contains(&p.as_str())));
    }

    #[test]
    fn a_choice_wins_and_letter_mode_keeps_the_program_icon() {
        let glyph = Choice { kind: Kind::Glyph, value: Some("\u{f135}".into()) };
        let own_letter = Choice { kind: Kind::Letter, value: None };
        let expected = words().unwrap().pick("hazy-meadow", "p1", Kind::Emoji);
        assert_eq!(pane(None, Some("hazy-meadow"), "p1", "hazy-meadow", Some(Kind::Emoji)), json!({ "kind": "emoji", "text": expected }));
        assert_eq!(pane(None, Some("hazy-meadow"), "p1", "hazy-meadow", None), Value::Null, "letter mode: as before");
        assert_eq!(pane(None, None, "p1", "zsh", Some(Kind::Glyph)), Value::Null, "no automatic name: the program's icon");
        for style in [None, Some(Kind::Glyph), Some(Kind::Emoji)] {
            assert_eq!(pane(Some(&glyph), Some("hazy-meadow"), "p1", "x", style), json!({ "kind": "glyph", "text": "\u{f135}" }), "a choice wins in {style:?}");
            assert_eq!(pane(Some(&own_letter), None, "p1", "\u{2733} build", style), json!({ "kind": "letter", "text": "B" }));
        }
        let focused = json!({ "kind": "emoji", "text": "🌾" });
        assert_eq!(tab(None, "1", Some(&focused)), focused, "a tab shows its focused pane's");
        assert_eq!(tab(None, "1", None), Value::Null);
        assert_eq!(tab(Some(&own_letter), "deploy", Some(&focused)), json!({ "kind": "letter", "text": "D" }), "its own wins");
        assert_eq!(letter("..."), "?");
        for s in ["glyph", "repo"] {
            assert_eq!(style(Some(s)), Some(Kind::Glyph));
        }
        assert_eq!(style(Some("emoji")), Some(Kind::Emoji));
        assert_eq!((style(Some("letter")), style(None), style(Some("odd"))), (None, None, None));
    }

    #[test]
    fn choices_are_checked() {
        assert_eq!(Choice::parse(&Value::Null), Ok(None));
        assert_eq!(Choice::parse(&json!({ "kind": "emoji", "value": "🌾" })), Ok(Some(Choice { kind: Kind::Emoji, value: Some("🌾".into()) })));
        assert_eq!(Choice::parse(&json!({ "kind": "letter", "value": "anything" })), Ok(Some(Choice { kind: Kind::Letter, value: None })));
        for bad in [json!("🌾"), json!({ "kind": "repo" }), json!({ "kind": "glyph" }), json!({ "kind": "emoji", "value": "" }), json!({ "kind": "emoji", "value": "a b" }), json!({ "kind": "emoji", "value": "x".repeat(33) })] {
            assert!(Choice::parse(&bad).is_err(), "{bad}");
        }
        // As kept: `{ kind, value? }`.
        assert_eq!(serde_json::to_value(Choice { kind: Kind::Letter, value: None }).unwrap(), json!({ "kind": "letter" }));
    }
}
