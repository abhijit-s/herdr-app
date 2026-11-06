//! Names for panes nobody named (`pane_auto_names`): two calm words such as
//! "hazy-inlet", picked from the pane's id so a pane keeps its name across
//! redraws and restarts. Display only: never written into a layout or settings.
//! The words are data, `defaults/pane-names.json`.

use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

const SHIPPED: &str = include_str!("../defaults/pane-names.json");

#[derive(Debug, Deserialize)]
struct Words {
    adjectives: Vec<String>,
    nouns: Vec<String>,
}

impl Words {
    fn parse(text: &str) -> Result<Words, String> {
        let words: Words = serde_json::from_str(text).map_err(|e| e.to_string())?;
        for (list, name) in [(&words.adjectives, "adjectives"), (&words.nouns, "nouns")] {
            if list.is_empty() {
                return Err(format!("{name} is empty"));
            }
            let mut seen = HashSet::new();
            for w in list {
                if w.is_empty() || !w.bytes().all(|b| b.is_ascii_lowercase()) {
                    return Err(format!("{name}: {w:?} isn't a lower-case word"));
                }
                if !seen.insert(w) {
                    return Err(format!("{name}: {w:?} is there twice"));
                }
            }
        }
        Ok(words)
    }

    fn count(&self) -> usize {
        self.adjectives.len() * self.nouns.len()
    }

    fn name(&self, index: usize) -> String {
        let n = self.nouns.len();
        format!("{}-{}", self.adjectives[index / n], self.nouns[index % n])
    }

    /// Each pane's name, in pane-id order so it doesn't depend on where panes
    /// sit: a name another pane already has goes to the next one in the list.
    fn for_tab<'a>(&self, ids: impl IntoIterator<Item = &'a str>) -> HashMap<String, String> {
        let mut ids: Vec<&str> = ids.into_iter().collect();
        ids.sort_unstable();
        ids.dedup();
        let count = self.count();
        let mut taken = HashSet::new();
        let mut names = HashMap::new();
        for id in ids {
            let mut i = (fnv1a(id.as_bytes()) % count as u64) as usize;
            // More panes than names: they share (never loops forever).
            for _ in 0..count {
                if taken.insert(i) {
                    break;
                }
                i = (i + 1) % count;
            }
            names.insert(id.to_owned(), self.name(i));
        }
        names
    }
}

/// The shipped words; None (no automatic names) if the file doesn't hold up,
/// which `shipped_words_hold_up` catches before a release.
fn words() -> Option<&'static Words> {
    static WORDS: OnceLock<Option<Words>> = OnceLock::new();
    WORDS
        .get_or_init(|| Words::parse(SHIPPED).map_err(|e| tracing::warn!("defaults/pane-names.json: {e}")).ok())
        .as_ref()
}

/// 64-bit FNV-1a: the same on every run, unlike std's per-process SipHash keys.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
}

/// Names for one tab's panes (every pane in it, so a pane's name doesn't move
/// when another starts or stops showing its own), by pane id.
pub fn for_tab<'a>(ids: impl IntoIterator<Item = &'a str>) -> HashMap<String, String> {
    words().map(|w| w.for_tab(ids)).unwrap_or_default()
}

/// Whether a pane shows its automatic name: nobody named it, nothing but its
/// shell runs (`programs::of` never marks an agent or a command as a shell),
/// and its title says nothing more than a shell or a folder would.
pub fn shown(named: bool, program: &Value, title: &str, address: &str, cwd: Option<&str>, home: &str) -> bool {
    !named && program.get("shell").and_then(Value::as_bool).unwrap_or(false) && generic_title(title, address, cwd, home)
}

/// Empty, the terminal's address (its title before the shell sets one), a
/// shell's name (`zsh`, `-zsh`), or only a folder: `~/src`, `/tmp`, `C:\src`,
/// the cwd's last part, or any of those after `user@host:`.
fn generic_title(title: &str, address: &str, cwd: Option<&str>, home: &str) -> bool {
    let t = title.trim();
    if t.is_empty() || t == address || crate::programs::is_shell(t) {
        return true;
    }
    let path = match t.split_once(':') {
        Some((who, rest)) if who.contains('@') && !who.contains(char::is_whitespace) => rest.trim(),
        _ => t,
    };
    let b = path.as_bytes();
    let drive = b.len() > 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/');
    path.is_empty() || path.starts_with(['/', '~', '\u{2026}']) || drive || cwd.is_some_and(|c| path == c || path == crate::model::folder_name(c, home))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn shell() -> Value {
        json!({ "key": "shell", "name": "zsh", "shell": true, "agent": false })
    }

    #[test]
    fn shipped_words_hold_up() {
        let w = Words::parse(SHIPPED).expect("defaults/pane-names.json");
        for word in w.adjectives.iter().chain(&w.nouns) {
            assert!((3..=7).contains(&word.len()), "{word:?}: three to seven letters");
        }
        assert!(w.adjectives.len() >= 40 && w.nouns.len() >= 40, "enough names that a tab rarely shares one");
        assert!(words().is_some());
    }

    #[test]
    fn word_lists_are_checked_when_read() {
        assert!(Words::parse(r#"{"adjectives":["hazy"],"nouns":["inlet"]}"#).is_ok());
        for bad in [
            r#"{"adjectives":[],"nouns":["inlet"]}"#,
            r#"{"adjectives":["hazy"],"nouns":[]}"#,
            r#"{"adjectives":["Hazy"],"nouns":["inlet"]}"#,
            r#"{"adjectives":["hazy"],"nouns":["sea inlet"]}"#,
            r#"{"adjectives":["hazy"],"nouns":[""]}"#,
            r#"{"adjectives":["hazy","hazy"],"nouns":["inlet"]}"#,
            r#"{"adjectives":["hazy"],"nouns":["inlet","inlet"]}"#,
            r#"{"adjectives":["hazy"]}"#,
        ] {
            assert!(Words::parse(bad).is_err(), "{bad}");
        }
    }

    /// A fixed answer for a fixed id: a change to the hash or the words shows here.
    #[test]
    fn a_pane_keeps_its_name() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
        let id = "p0123456789abcdef0123456789abcdef";
        let first = for_tab([id]);
        assert_eq!(first[id], "velvet-ford");
        for _ in 0..3 {
            assert_eq!(for_tab([id]), first);
        }
        let other = "p00000000000000000000000000000001";
        assert_ne!(for_tab([other])[other], first[id], "another id, another name");
        let n = for_tab([id])[id].clone();
        let (adjective, noun) = n.split_once('-').unwrap();
        let w = words().unwrap();
        assert!(w.adjectives.iter().any(|a| a == adjective) && w.nouns.iter().any(|x| x == noun));
    }

    #[test]
    fn panes_sharing_a_name_in_a_tab_take_the_next_one_by_id() {
        let w = Words::parse(r#"{"adjectives":["hazy","pale"],"nouns":["inlet","tarn"]}"#).unwrap();
        // Ids whose names collide in a list of four.
        let ids: Vec<String> = (0..64).map(|i| format!("p{i}")).collect();
        let index = |id: &str| (fnv1a(id.as_bytes()) % 4) as usize;
        let a = &ids[0];
        let b = ids.iter().skip(1).find(|b| index(b) == index(a)).unwrap();
        let (low, high) = if a < b { (a, b) } else { (b, a) };
        let names = w.for_tab([high.as_str(), low.as_str()]);
        assert_eq!(names[low.as_str()], w.name(index(low)));
        assert_eq!(names[high.as_str()], w.name((index(low) + 1) % 4), "the later id moves on");
        assert_eq!(w.for_tab([low.as_str(), high.as_str()]), names, "whatever order the tab lists them in");
        // Alone, each has its own again.
        assert_eq!(w.for_tab([high.as_str()])[high.as_str()], w.name(index(high)));
        // More panes than names: every pane still gets one.
        assert_eq!(w.for_tab(ids.iter().map(String::as_str)).len(), 64);
    }

    #[test]
    fn only_a_bare_shell_shows_the_automatic_name() {
        let home = "/home/me";
        let cwd = Some("/home/me/src/herdr");
        for title in ["", "  ", "t1", "zsh", "-zsh", "bash", "pwsh.exe", "~/src/herdr", "/home/me/src/herdr", "herdr", "me@box: ~/src/herdr", "me@box:~", "C:\\src\\herdr", "\u{2026}/src/herdr"] {
            assert!(shown(false, &shell(), title, "t1", cwd, home), "{title:?} says nothing");
        }
        for title in ["deploy", "vim notes.md", "me@box: deploy", "herdr-app", "ping me@box: ~/src"] {
            assert!(!shown(false, &shell(), title, "t1", cwd, home), "{title:?} says something");
        }
        // The person's name, an agent, or a running command win.
        assert!(!shown(true, &shell(), "zsh", "t1", cwd, home));
        let agent = json!({ "key": "claude", "name": "Claude Code", "shell": false, "agent": true });
        assert!(!shown(false, &agent, "", "t1", cwd, home));
        let command = json!({ "key": "program", "name": "cargo", "shell": false, "agent": false });
        assert!(!shown(false, &command, "~/src/herdr", "t1", cwd, home));
        assert!(!shown(false, &Value::Null, "", "t1", cwd, home));
    }
}
