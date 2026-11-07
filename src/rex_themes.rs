//! `herdr-app rex-themes`: Herdr theme files (`themes/rex-<id>.json`) made
//! from the themes of the installed Rex app. Rex's themes aren't Herdr's to
//! ship, so they are read from the app each run and written only into the
//! person's themes folder.

use crate::vt::Rgb;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const REX_THEMES: &str = "/Applications/Rex Beta.app/Contents/Resources/SuperUI_SuperDesignTokens.bundle/Contents/Resources/themes.json";

/// Herdr's named colors from Rex's design tokens, where the meanings match:
/// (Herdr color, Rex token, its CSS variable). Rex keeps each token in
/// display-p3 (`semantic`, more saturated than its sRGB design) and in sRGB
/// (`cssVariables`); Herdr draws sRGB, so the CSS value is read.
const COLORS: &[(&str, &str, &str)] = &[
    ("canvas", "canvas", "--super-bg"),
    ("chrome", "surfaceSubtle", "--super-sidebar"),
    ("surface", "surface", "--super-pane"),
    ("popover", "surfaceRaised", "--super-raised"),
    ("raised", "controlOnOverlay", "--super-control-on-overlay"),
    ("border", "strokeSubtle", "--super-border"),
    ("input", "stroke", "--super-border-strong"),
    ("hover", "controlHover", "--super-hover"),
    ("selected", "controlPressed", "--super-selected"),
    ("foreground", "content", "--super-text"),
    ("muted", "contentSecondary", "--super-text-secondary"),
    ("dim", "contentTertiary", "--super-text-tertiary"),
    ("accent", "accent", "--super-accent"),
    ("accent_foreground", "accentOnSolid", "--super-accent-on-solid"),
    ("focus", "accentBorder", "--super-accent-border"),
    ("ring", "accentBorder", "--super-accent-border"),
    ("done", "success", "--super-success"),
    ("blocked", "danger", "--super-danger"),
];

const USAGE: &str = "\
herdr-app rex-themes: Herdr themes from the installed Rex app.

Usage:
  herdr-app rex-themes [--dry-run] [--from <themes.json>]

Writes themes/rex-<name>.json in Herdr's home (~/.config/herdr-app, or
HERDR_APP_HOME) for each of Rex's themes, replacing only rex-* files.
--dry-run lists what it would write. --from reads another themes.json.
";

#[derive(Deserialize)]
struct RexTheme {
    id: String,
    name: String,
    appearance: String,
    source: Source,
    #[serde(rename = "cssVariables")]
    css: BTreeMap<String, String>,
}

/// Rex's terminal palette, in sRGB hex.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Source {
    background: String,
    foreground: String,
    cursor: String,
    selection_background: String,
    ansi: Vec<String>,
}

/// A Herdr theme file (`settings::ThemeFile`), in its field order.
#[derive(Serialize)]
struct ThemeFile {
    background: String,
    foreground: String,
    selection: String,
    cursor: String,
    ansi: Vec<String>,
    colors: BTreeMap<String, String>,
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    Added,
    Updated,
    Unchanged,
    /// Not written, and why.
    Skipped(String),
}

/// One Rex theme's result: its file name (`rex-<id>`), its Rex name, whether it is light.
#[derive(Debug)]
pub struct Imported {
    pub file: String,
    pub name: String,
    pub light: bool,
    pub outcome: Outcome,
}

pub fn main(args: &[String]) -> i32 {
    let mut dry_run = false;
    let mut from = None;
    let mut args = args.iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--dry-run" => dry_run = true,
            "--from" => match args.next() {
                Some(path) => from = Some(PathBuf::from(path)),
                None => return usage("--from needs a file"),
            },
            "-h" | "--help" => {
                print!("{USAGE}");
                return 0;
            }
            other => return usage(&format!("unknown argument {other:?}")),
        }
    }
    let to = crate::mods::app_dir().join("themes");
    let imported = match import(from.as_deref().unwrap_or(Path::new(REX_THEMES)), &to, dry_run) {
        Ok(imported) => imported,
        Err(e) => {
            eprintln!("herdr-app rex-themes: {e}");
            return 1;
        }
    };
    let count = |o: &Outcome| imported.iter().filter(|i| i.outcome == *o).count();
    let skipped = imported.iter().filter(|i| matches!(i.outcome, Outcome::Skipped(_))).count();
    for i in &imported {
        let kind = if i.light { "light" } else { "dark" };
        let did = match (&i.outcome, dry_run) {
            (Outcome::Added, false) => "added",
            (Outcome::Added, true) => "would add",
            (Outcome::Updated, false) => "updated",
            (Outcome::Updated, true) => "would update",
            (Outcome::Unchanged, _) => "unchanged",
            (Outcome::Skipped(why), _) => {
                eprintln!("skipped    {}: {why}", i.file);
                continue;
            }
        };
        println!("{did:<13}{:<30}{} ({kind})", i.file, i.name);
    }
    println!(
        "{} themes in {}: {} {}, {} {}, {} unchanged{}.",
        imported.len(),
        to.display(),
        count(&Outcome::Added),
        if dry_run { "to add" } else { "added" },
        count(&Outcome::Updated),
        if dry_run { "to update" } else { "updated" },
        count(&Outcome::Unchanged),
        if skipped > 0 { format!(", {skipped} skipped") } else { String::new() },
    );
    if !dry_run && let Some(first) = imported.iter().find(|i| !matches!(i.outcome, Outcome::Skipped(_))) {
        println!("Pick one in Settings > Look (Dark theme or Light theme), or: herdr-app do app.theme --name {}", first.file);
    }
    i32::from(skipped > 0)
}

fn usage(problem: &str) -> i32 {
    eprint!("herdr-app rex-themes: {problem}\n\n{USAGE}");
    2
}

/// Reads Rex's `themes.json` at `from` and writes each theme to
/// `to/rex-<id>.json` (unless `dry_run`), leaving every other file alone. A
/// theme with a color Herdr can't read is skipped, the rest still written.
pub fn import(from: &Path, to: &Path, dry_run: bool) -> Result<Vec<Imported>, String> {
    let text = std::fs::read_to_string(from).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => format!("no Rex themes at {} (is Rex installed?)", from.display()),
        _ => format!("cannot read {}: {e}", from.display()),
    })?;
    let themes: Vec<RexTheme> = serde_json::from_str(&text).map_err(|e| format!("{} isn't a list of Rex themes: {e}", from.display()))?;
    if !dry_run {
        std::fs::create_dir_all(to).map_err(|e| format!("cannot make {}: {e}", to.display()))?;
    }
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for theme in &themes {
        let slug = slug(&theme.id);
        let file = format!("rex-{slug}");
        let light = theme.appearance == "light";
        let outcome = match convert(theme) {
            _ if slug.is_empty() => Outcome::Skipped(format!("id {:?} has no letters or digits", theme.id)),
            _ if !seen.insert(slug.clone()) => Outcome::Skipped(format!("id {:?} names the same file as an earlier theme", theme.id)),
            Err(why) => Outcome::Skipped(why),
            Ok(text) => {
                let path = to.join(format!("{file}.json"));
                let outcome = match std::fs::read_to_string(&path) {
                    Ok(now) if now == text => Outcome::Unchanged,
                    Ok(_) => Outcome::Updated,
                    Err(_) => Outcome::Added,
                };
                if !dry_run && outcome != Outcome::Unchanged {
                    crate::persist::write_atomic(&path, &text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
                }
                outcome
            }
        };
        out.push(Imported { file, name: theme.name.clone(), light, outcome });
    }
    Ok(out)
}

/// A Rex id as a file name: lowercase letters, digits and dashes.
fn slug(id: &str) -> String {
    let s: String = id.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-")
}

/// One Rex theme as a Herdr theme file's text, or why it can't be one.
fn convert(t: &RexTheme) -> Result<String, String> {
    let s = &t.source;
    let ansi: Vec<String> = s.ansi.iter().enumerate().map(|(i, c)| hex(&format!("ansi[{i}]"), c)).collect::<Result<_, _>>()?;
    if ansi.len() != 16 {
        return Err(format!("{} ANSI colors, not 16", ansi.len()));
    }
    // Rex draws terminals on the pane color, a step off the window (`--super-bg`, Herdr's `canvas`),
    // so panes read as cards; the palette's background is the window's.
    let background = match t.css.get("--super-term-bg").and_then(|v| css(v)).filter(|c| c.len() == 7) {
        Some(term) => term,
        None => hex("background", &s.background)?,
    };
    let bg = rgb(&background);
    let light = match t.appearance.as_str() {
        "light" => true,
        "dark" => false,
        other => return Err(format!("appearance {other:?} is not light or dark")),
    };
    // Herdr tells light from dark by the background, as it does for any theme file.
    if crate::settings::is_light(bg) != light {
        return Err(format!("Rex calls it {} but its background {background} reads {}", t.appearance, if light { "dark" } else { "light" }));
    }
    let mut colors = BTreeMap::new();
    for (herdr, token, var) in COLORS {
        let v = t.css.get(*var).ok_or(format!("no {token} ({var})"))?;
        let c = css(v).ok_or(format!("{token} ({var}) {v:?} is not an rgb color"))?;
        // Herdr guarantees focus and ring at 3:1; one that is translucent or under that is left to its readable default.
        if matches!(*herdr, "focus" | "ring") && (c.len() > 7 || crate::vt::contrast(to_rgb(rgb(&c)), to_rgb(bg)) < 3.0) {
            continue;
        }
        colors.insert(herdr.to_string(), c);
    }
    let file = ThemeFile {
        background,
        foreground: hex("foreground", &s.foreground)?,
        selection: hex("selectionBackground", &s.selection_background)?,
        cursor: hex("cursor", &s.cursor)?,
        ansi,
        colors,
    };
    Ok(serde_json::to_string_pretty(&file).expect("a theme file is plain strings") + "\n")
}

/// `#rrggbb` (any case) as lowercase, or why not.
fn hex(key: &str, v: &str) -> Result<String, String> {
    let h = v.strip_prefix('#').filter(|h| h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit()));
    h.map(|h| format!("#{}", h.to_lowercase())).ok_or(format!("{key} {v:?} is not #rrggbb"))
}

/// A CSS color as Rex writes it, `r g b`, `rgb(r g b)` or `rgb(r g b / a)`,
/// as `#rrggbb` or `#rrggbbaa`.
fn css(v: &str) -> Option<String> {
    let inner = v.trim().strip_prefix("rgb(").and_then(|r| r.strip_suffix(')')).unwrap_or(v.trim());
    let (channels, alpha) = match inner.split_once('/') {
        Some((c, a)) => (c, Some(a.trim().parse::<f32>().ok().filter(|a| (0.0..=1.0).contains(a))?)),
        None => (inner, None),
    };
    let rgb: Vec<u8> = channels.split_whitespace().map(|c| c.parse().ok()).collect::<Option<_>>()?;
    let [r, g, b] = rgb[..] else { return None };
    Some(match alpha {
        Some(a) if a < 1.0 => format!("#{r:02x}{g:02x}{b:02x}{:02x}", (a * 255.0).round() as u8),
        _ => format!("#{r:02x}{g:02x}{b:02x}"),
    })
}

fn rgb(hex: &str) -> u32 {
    u32::from_str_radix(&hex[1..7], 16).unwrap_or(0)
}

fn to_rgb(c: u32) -> Rgb {
    Rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A theme in Rex's schema with invented colors: `bg` and `fg` for the
    /// terminal, the panels a step from `bg`, the accent `accent`.
    fn rex(id: &str, appearance: &str, bg: &str, fg: &str, accent: &str) -> serde_json::Value {
        let ansi: Vec<String> = (0..16).map(|i| format!("#{:02x}{:02x}{:02x}", 16 * i, 255 - 16 * i, 128)).collect();
        let channels = |h: &str| { let c = rgb(h); format!("{} {} {}", c >> 16, (c >> 8) & 255, c & 255) };
        // Rex's terminals sit on the pane color, a step from the window toward mid grey.
        let pane = |h: &str| { let c = rgb(h); let step = |v: u32| if v < 128 { v + 2 } else { v - 2 }; format!("{} {} {}", step(c >> 16), step((c >> 8) & 255), step(c & 255)) };
        json!({
            "id": id, "name": format!("Theme {id}"), "appearance": appearance,
            "source": { "background": bg, "foreground": fg, "cursor": "#FF00AA", "cursorText": bg, "selectionBackground": "#334455", "selectionForeground": fg, "ansi": ansi, "displayP3": {} },
            "palette": [], "semantic": {},
            "cssVariables": {
                "--super-bg": channels(bg), "--super-term-bg": pane(bg), "--super-sidebar": "11 12 13", "--super-pane": "12 13 14", "--super-raised": "20 21 22",
                "--super-control-on-overlay": "25 26 27", "--super-border": "30 31 32", "--super-border-strong": "40 41 42",
                "--super-hover": format!("rgb({} / 0.05)", channels(fg)), "--super-selected": format!("rgb({} / 0.1)", channels(fg)),
                "--super-text": channels(fg), "--super-text-secondary": "150 150 150", "--super-text-tertiary": "110 110 110",
                "--super-accent": format!("rgb({})", channels(accent)), "--super-accent-on-solid": "0 0 0", "--super-accent-border": channels(accent),
                "--super-success": "0 200 100", "--super-danger": "230 60 60", "--super-warning": "200 180 0"
            }
        })
    }

    fn dark() -> serde_json::Value {
        rex("night-sea", "dark", "#0A0B0C", "#d0d0d0", "#40e0c0")
    }

    fn light() -> serde_json::Value {
        rex("Paper Day", "light", "#fafafa", "#303030", "#2060c0")
    }

    struct Dirs(PathBuf);

    impl Dirs {
        fn new(name: &str) -> Dirs {
            let dir = std::env::temp_dir().join(format!("herdr-rex-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Dirs(dir)
        }

        fn rex(&self, themes: &[serde_json::Value]) -> PathBuf {
            let path = self.0.join("themes.json");
            std::fs::write(&path, serde_json::to_string(themes).unwrap()).unwrap();
            path
        }

        fn themes(&self) -> PathBuf {
            self.0.join("themes")
        }
    }

    impl Drop for Dirs {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn outcomes(imported: &[Imported]) -> Vec<(&str, &Outcome)> {
        imported.iter().map(|i| (i.file.as_str(), &i.outcome)).collect()
    }

    #[test]
    fn maps_the_palette_and_the_matching_tokens() {
        let dirs = Dirs::new("map");
        import(&dirs.rex(&[dark()]), &dirs.themes(), false).unwrap();
        let text = std::fs::read_to_string(dirs.themes().join("rex-night-sea.json")).unwrap();
        let file: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!((&file["background"], &file["foreground"], &file["selection"], &file["cursor"]), (&json!("#0c0d0e"), &json!("#d0d0d0"), &json!("#334455"), &json!("#ff00aa")));
        assert_eq!(file["ansi"], dark()["source"]["ansi"]);
        assert_eq!(file["colors"], json!({
            "canvas": "#0a0b0c", "chrome": "#0b0c0d", "surface": "#0c0d0e", "popover": "#141516", "border": "#1e1f20", "input": "#28292a",
            "hover": "#d0d0d00d", "selected": "#d0d0d01a", "foreground": "#d0d0d0", "muted": "#969696", "dim": "#6e6e6e",
            "accent": "#40e0c0", "accent_foreground": "#000000", "focus": "#40e0c0", "ring": "#40e0c0", "raised": "#191a1b", "done": "#00c864", "blocked": "#e63c3c"
        }));
        assert!(crate::settings::theme_file_problems("rex-night-sea", &text).is_empty(), "{:?}", crate::settings::theme_file_problems("rex-night-sea", &text));
    }

    #[test]
    fn terminals_sit_on_the_pane_shade_and_fall_back_to_the_palette_background() {
        let dirs = Dirs::new("term-bg");
        let mut bare = dark();
        bare["id"] = json!("bare");
        bare["cssVariables"].as_object_mut().unwrap().remove("--super-term-bg");
        let mut translucent = dark();
        translucent["id"] = json!("glass");
        translucent["cssVariables"]["--super-term-bg"] = json!("rgb(12 13 14 / 0.5)");
        import(&dirs.rex(&[dark(), bare, translucent]), &dirs.themes(), false).unwrap();
        let file = |name: &str| serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(dirs.themes().join(format!("rex-{name}.json"))).unwrap()).unwrap();
        let sea = file("night-sea");
        assert_eq!((&sea["background"], &sea["colors"]["canvas"]), (&json!("#0c0d0e"), &json!("#0a0b0c")), "the terminal a step off the window");
        assert_eq!(file("bare")["background"], json!("#0a0b0c"), "no terminal color: the palette's background");
        assert_eq!(file("glass")["background"], json!("#0a0b0c"), "a translucent terminal color: the palette's background");
    }

    #[test]
    fn light_themes_load_as_light_and_a_mismatch_is_skipped() {
        let dirs = Dirs::new("light");
        let mut wrong = dark();
        wrong["id"] = json!("dusk");
        wrong["appearance"] = json!("light");
        let imported = import(&dirs.rex(&[light(), dark(), wrong]), &dirs.themes(), false).unwrap();
        assert_eq!(imported.iter().map(|i| (i.file.as_str(), i.light)).collect::<Vec<_>>(), [("rex-paper-day", true), ("rex-night-sea", false), ("rex-dusk", true)]);
        assert!(matches!(&imported[2].outcome, Outcome::Skipped(why) if why.contains("background #0c0d0e reads dark")), "{:?}", imported[2].outcome);
        assert!(!dirs.themes().join("rex-dusk.json").exists());
        let text = std::fs::read_to_string(dirs.themes().join("rex-paper-day.json")).unwrap();
        assert!(crate::settings::is_light(rgb(&serde_json::from_str::<serde_json::Value>(&text).unwrap()["background"].as_str().unwrap().to_owned())));
        assert!(crate::settings::theme_file_problems("rex-paper-day", &text).is_empty());
    }

    #[test]
    fn a_second_run_changes_only_what_changed_and_only_rex_files() {
        let dirs = Dirs::new("again");
        let from = dirs.rex(&[dark(), light()]);
        std::fs::create_dir_all(dirs.themes()).unwrap();
        std::fs::write(dirs.themes().join("mine.json"), "mine").unwrap();
        assert_eq!(outcomes(&import(&from, &dirs.themes(), true).unwrap()), [("rex-night-sea", &Outcome::Added), ("rex-paper-day", &Outcome::Added)]);
        assert_eq!(std::fs::read_dir(dirs.themes()).unwrap().count(), 1, "a dry run writes nothing");
        assert_eq!(outcomes(&import(&from, &dirs.themes(), false).unwrap()), [("rex-night-sea", &Outcome::Added), ("rex-paper-day", &Outcome::Added)]);
        assert_eq!(outcomes(&import(&from, &dirs.themes(), false).unwrap()), [("rex-night-sea", &Outcome::Unchanged), ("rex-paper-day", &Outcome::Unchanged)]);
        let mut changed = dark();
        changed["source"]["foreground"] = json!("#e0e0e0");
        let from = dirs.rex(&[changed, light()]);
        assert_eq!(outcomes(&import(&from, &dirs.themes(), true).unwrap()), [("rex-night-sea", &Outcome::Updated), ("rex-paper-day", &Outcome::Unchanged)]);
        assert_eq!(outcomes(&import(&from, &dirs.themes(), false).unwrap()), [("rex-night-sea", &Outcome::Updated), ("rex-paper-day", &Outcome::Unchanged)]);
        assert!(std::fs::read_to_string(dirs.themes().join("rex-night-sea.json")).unwrap().contains("#e0e0e0"));
        assert_eq!(std::fs::read_to_string(dirs.themes().join("mine.json")).unwrap(), "mine");
    }

    #[test]
    fn a_missing_or_unreadable_rex_file_is_an_error() {
        let dirs = Dirs::new("missing");
        let missing = dirs.0.join("nowhere/themes.json");
        let e = import(&missing, &dirs.themes(), false).unwrap_err();
        assert!(e.contains("no Rex themes at") && e.contains("is Rex installed"), "{e}");
        std::fs::write(dirs.0.join("bad.json"), "{\"not\": \"a list\"}").unwrap();
        let e = import(&dirs.0.join("bad.json"), &dirs.themes(), false).unwrap_err();
        assert!(e.contains("isn't a list of Rex themes"), "{e}");
        assert!(!dirs.themes().exists(), "nothing written");
    }

    #[test]
    fn a_bad_color_skips_that_theme_only() {
        let dirs = Dirs::new("bad");
        let mut short = dark();
        short["id"] = json!("short");
        short["source"]["background"] = json!("#0a0b0");
        short["cssVariables"].as_object_mut().unwrap().remove("--super-term-bg");
        let mut over = dark();
        over["id"] = json!("over");
        over["cssVariables"]["--super-border"] = json!("300 0 0");
        let mut alpha = dark();
        alpha["id"] = json!("alpha");
        alpha["cssVariables"]["--super-hover"] = json!("rgb(1 2 3 / 2)");
        let mut few = dark();
        few["id"] = json!("few");
        few["source"]["ansi"] = json!(["#000000"]);
        let imported = import(&dirs.rex(&[short, over, alpha, few, dark()]), &dirs.themes(), false).unwrap();
        let why = |i: usize| match &imported[i].outcome { Outcome::Skipped(why) => why.clone(), o => panic!("{o:?}") };
        assert!(why(0).contains("background \"#0a0b0\" is not #rrggbb"), "{}", why(0));
        assert!(why(1).contains("strokeSubtle (--super-border) \"300 0 0\""), "{}", why(1));
        assert!(why(2).contains("controlHover"), "{}", why(2));
        assert!(why(3).contains("1 ANSI colors"), "{}", why(3));
        assert_eq!(imported[4].outcome, Outcome::Added);
        assert_eq!(std::fs::read_dir(dirs.themes()).unwrap().count(), 1, "only the good one written");
    }

    #[test]
    fn a_focus_and_ring_under_3_to_1_or_translucent_are_left_to_herdr() {
        for border in ["20 21 22", "rgb(64 224 192 / 0.5)"] {
            let dirs = Dirs::new("faint");
            let mut faint = dark();
            faint["cssVariables"]["--super-accent-border"] = json!(border);
            import(&dirs.rex(&[faint]), &dirs.themes(), false).unwrap();
            let text = std::fs::read_to_string(dirs.themes().join("rex-night-sea.json")).unwrap();
            let file: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!((file["colors"].get("focus"), file["colors"].get("ring")), (None, None), "{border}");
            assert!(crate::settings::theme_file_problems("rex-night-sea", &text).is_empty());
        }
    }

    #[test]
    fn ids_that_make_no_file_or_the_same_file_are_skipped() {
        let dirs = Dirs::new("ids");
        let mut blank = dark();
        blank["id"] = json!("--");
        let mut again = light();
        again["id"] = json!("Night_Sea");
        let imported = import(&dirs.rex(&[dark(), blank, again]), &dirs.themes(), false).unwrap();
        assert_eq!(imported[0].outcome, Outcome::Added);
        assert!(matches!(&imported[1].outcome, Outcome::Skipped(why) if why.contains("no letters")), "{:?}", imported[1].outcome);
        assert!(matches!(&imported[2].outcome, Outcome::Skipped(why) if why.contains("same file")), "{:?}", imported[2].outcome);
        assert!(std::fs::read_to_string(dirs.themes().join("rex-night-sea.json")).unwrap().contains("#0a0b0c"), "the first keeps its file");
    }

    /// The installed Rex's own themes, when this machine has Rex: every one
    /// is written and loads in Herdr with nothing to report. Nothing is kept.
    #[test]
    fn the_installed_rex_themes_all_load() {
        let from = Path::new(REX_THEMES);
        if !from.exists() {
            eprintln!("no Rex here: {REX_THEMES}");
            return;
        }
        let dirs = Dirs::new("installed");
        let imported = import(from, &dirs.themes(), false).unwrap();
        assert!(!imported.is_empty());
        for i in &imported {
            assert_eq!(i.outcome, Outcome::Added, "{}", i.file);
            let text = std::fs::read_to_string(dirs.themes().join(format!("{}.json", i.file))).unwrap();
            assert_eq!(crate::settings::theme_file_problems(&i.file, &text), Vec::<String>::new());
        }
    }
}
