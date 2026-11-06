//! Live settings and the resolved layout look, including persisted app zoom.
//!
//! A theme is a palette plus optional color overrides: a built-in preset
//! (graphite, ink, moss, paper) or `themes/<name>.json`. Chrome colors are
//! derived from the palette (dark or light by its background), so every
//! theme is consistent and overrides only need what they change.

use crate::vt::Rgb;
use gpui::{Hsla, Pixels, Rgba, px};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Mutex;

/// A terminal palette, as herdr-app shipped them.
#[derive(Clone)]
struct Palette {
    background: u32,
    foreground: u32,
    selection: u32,
    cursor: Option<u32>,
    ansi: [u32; 16],
    /// working, blocked, done: the presets pick their own; theme files take
    /// them from ansi (yellow, red, blue). Idle is grey: it needs nobody.
    status: Option<[u32; 3]>,
}

const DARK_STATUS: Option<[u32; 3]> = Some([0xe5c07b, 0xf0705f, 0x76a8ff]);

const GRAPHITE: Palette = Palette {
    background: 0x17191d,
    foreground: 0xd2d4d9,
    selection: 0x394454,
    cursor: None,
    status: DARK_STATUS,
    ansi: [
        0x45484f, 0xdc8c95, 0xa6bd91, 0xd8be87, 0x96b2d7, 0xb5a2ce, 0x8cbdc3, 0xd2d4d9, 0x818793, 0xf0a5ad, 0xbad5a2, 0xead2a1, 0xb0cbef,
        0xcbb7e6, 0xa4d5db, 0xf0f0f2,
    ],
};
const INK: Palette = Palette {
    background: 0x111720,
    foreground: 0xcdd6e4,
    selection: 0x344861,
    cursor: None,
    status: DARK_STATUS,
    ansi: [
        0x414b5a, 0xe58c9a, 0x89c5aa, 0xd9be82, 0x91b6ee, 0xb9a0e2, 0x85c4d0, 0xcdd6e4, 0x7f91a7, 0xf5a5b2, 0xa5dec3, 0xedd39c, 0xaccbfa,
        0xd0b7f4, 0xa0dce5, 0xf0f5fc,
    ],
};
const MOSS: Palette = Palette {
    background: 0x151c19,
    foreground: 0xcdd7ce,
    selection: 0x354a40,
    cursor: None,
    status: DARK_STATUS,
    ansi: [
        0x435248, 0xdf978d, 0xa0c4a2, 0xd4bc8b, 0x99b9cd, 0xbba4c8, 0x8bbdb5, 0xcdd7ce, 0x82968a, 0xf1afa3, 0xb8dab9, 0xe9d2a5, 0xb1d0e2,
        0xd1bddc, 0xa4d6ca, 0xeff5ee,
    ],
};

/// Light: white paper, dark ink. Every ANSI color stays readable on white
/// (bright white is ink, not white: programs write it as "strong" text).
const PAPER: Palette = Palette {
    background: 0xffffff,
    foreground: 0x25272c,
    selection: 0xcfdcf5,
    cursor: Some(0x25272c),
    status: Some([0x9a6700, 0xc93c2c, 0x2f6fe4]),
    ansi: [
        0x25272c, 0xc62a2a, 0x1a7f37, 0x8f5d00, 0x1f62d0, 0x7a45c8, 0x0f6f74, 0x5f6068, 0x7b7e87, 0xd1242f, 0x1f883d, 0x9a6700, 0x0969da,
        0x8250df, 0x1b7c83, 0x111214,
    ],
};

pub const PRESETS: &[&str] = &["graphite", "ink", "moss", "paper"];

/// The theme a layout without one shows, by appearance.
pub const DEFAULT_DARK: &str = "graphite";
pub const DEFAULT_LIGHT: &str = "paper";

fn preset(name: &str) -> Option<&'static Palette> {
    match name {
        "graphite" => Some(&GRAPHITE),
        "ink" => Some(&INK),
        "moss" => Some(&MOSS),
        "paper" => Some(&PAPER),
        _ => None,
    }
}

/// The shipped theme files (defaults/themes), written to `themes/` when missing.
const THEMES: &[(&str, &str)] = &[
    ("atom-one-dark", include_str!("../defaults/themes/atom-one-dark.json")),
    ("win98", include_str!("../defaults/themes/win98.json")),
];
pub const SHIPPED_THEMES: &[&str] = &["atom-one-dark", "win98"];

/// `themes/<name>.json`: a palette, plus `colors` overrides like settings.json's.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeFile {
    background: String,
    foreground: String,
    selection: String,
    cursor: Option<String>,
    ansi: [String; 16],
    #[serde(default)]
    colors: BTreeMap<String, String>,
}

fn theme_path(name: &str) -> std::path::PathBuf {
    crate::mods::app_dir().join("themes").join(format!("{name}.json"))
}

// Only these exact shipped versions may be replaced; personal themes stay intact.
const PREVIOUS_THEMES: &[(&str, &str)] = &[
    ("atom-one-dark", r##"{
  "background": "#21252b",
  "foreground": "#abb2bf",
  "selection": "#323844",
  "cursor": "#abb2bf",
  "ansi": [
    "#21252b", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#abb2bf",
    "#767676", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#abb2bf"
  ]
}
"##),
    ("win98", r##"{
  "background": "#ffffff",
  "foreground": "#c0c0c0",
  "selection": "#000080",
  "cursor": "#c0c0c0",
  "ansi": [
    "#000000", "#aa0000", "#00aa00", "#aa5500", "#0000aa", "#aa00aa", "#00aaaa", "#aaaaaa",
    "#555555", "#ff5555", "#55ff55", "#ffff55", "#5555ff", "#ff55ff", "#55ffff", "#ffffff"
  ],
  "colors": {
    "foreground": "#000000",
    "surface": "#c0c0c0",
    "popover": "#c0c0c0",
    "raised": "#dfdfdf",
    "hover": "#dfdfdf",
    "border": "#808080",
    "input": "#ffffff",
    "muted": "#404040",
    "dim": "#808080",
    "accent": "#000080",
    "accent_foreground": "#ffffff",
    "ring": "#000000",
    "working": "#808000",
    "blocked": "#ff0000",
    "done": "#0000ff",
    "idle": "#808080",
    "unknown": "#808080"
  }
}
"##),
];

pub fn install_themes() {
    let _ = std::fs::create_dir_all(crate::mods::app_dir().join("themes"));
    for (name, text) in THEMES {
        install_theme(&theme_path(name), name, text);
    }
}

fn install_theme(path: &std::path::Path, name: &str, text: &str) {
    let previous = PREVIOUS_THEMES.iter().find(|(n, _)| *n == name).map(|(_, s)| *s);
    if !path.exists() || std::fs::read_to_string(path).ok().as_deref().is_some_and(|s| Some(s) == previous) {
        let _ = crate::persist::write_atomic(path, text.as_bytes());
    }
}

/// A theme by name: a preset, or `themes/<name>.json`.
fn palette(name: &str) -> Result<(Palette, BTreeMap<String, String>), String> {
    if let Some(p) = preset(name) {
        return Ok((p.clone(), BTreeMap::new()));
    }
    let path = theme_path(name);
    let text = std::fs::read_to_string(&path)
        .map_err(|_| format!("theme {name:?} is not one of {} and there is no themes/{name}.json", PRESETS.join(", ")))?;
    parse_theme_file(name, &text)
}

fn parse_theme_file(name: &str, text: &str) -> Result<(Palette, BTreeMap<String, String>), String> {
    let f: ThemeFile = serde_json::from_str(text).map_err(|e| format!("themes/{name}.json: {e}"))?;
    let c = |k: &str, v: &str| {
        let h = v.trim().trim_start_matches('#');
        (h.len() == 6).then(|| u32::from_str_radix(h, 16).ok()).flatten().ok_or(format!("themes/{name}.json: {k} {v:?} is not #rrggbb"))
    };
    let mut ansi = [0; 16];
    for (i, v) in f.ansi.iter().enumerate() {
        ansi[i] = c(&format!("ansi[{i}]"), v)?;
    }
    for (k, v) in &f.colors {
        if parse_hex(v).is_none() {
            return Err(format!("themes/{name}.json: colors.{k}: {v:?} is not #rrggbb or #rrggbbaa"));
        }
    }
    let cursor = f.cursor.as_deref().map(|v| c("cursor", v)).transpose()?;
    let p = Palette {
        background: c("background", &f.background)?,
        foreground: c("foreground", &f.foreground)?,
        selection: c("selection", &f.selection)?,
        cursor,
        ansi,
        status: None,
    };
    Ok((p, f.colors))
}

#[test]
fn shipped_theme_upgrades_preserve_personal_edits() {
    let root = std::env::temp_dir().join(format!("herdr-theme-upgrades-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    for (name, previous) in PREVIOUS_THEMES {
        let path = root.join(format!("{name}.json"));
        let current = THEMES.iter().find(|(n, _)| n == name).unwrap().1;
        install_theme(&path, name, current);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), current);
        std::fs::write(&path, previous).unwrap();
        install_theme(&path, name, current);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), current);
        let edited = previous.replace("#abb2bf", "#abcdef").replace("#c0c0c0", "#abcdef");
        std::fs::write(&path, &edited).unwrap();
        install_theme(&path, name, current);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), edited);
        std::fs::remove_file(&path).unwrap();
    }
    std::fs::remove_dir(root).unwrap();
}

#[test]
fn shipped_theme_files() {
    let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("defaults/themes");
    let mut found = std::collections::BTreeSet::new();
    for entry in std::fs::read_dir(folder).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") { continue; }
        let name = path.file_stem().unwrap().to_str().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        parse_theme_file(name, &text).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(THEMES.iter().find(|(n, _)| *n == name).unwrap().1, text);
        found.insert(name.to_owned());
    }
    assert_eq!(found, THEMES.iter().map(|(n, _)| n.to_string()).collect());
    assert_eq!(found, SHIPPED_THEMES.iter().map(|n| n.to_string()).collect());
}

/// Whether `name` is a theme that can be shown.
pub fn theme_exists(name: &str) -> bool {
    palette(name).is_ok()
}

/// Whether a theme is light (by its background), for picking its half of a pair.
pub fn theme_is_light(name: &str) -> bool {
    palette(name).map(|(p, _)| is_light(p.background)).unwrap_or(false)
}

fn is_light(c: u32) -> bool {
    let ch = |s: u32| ((c >> s) & 255) as f32 / 255.0;
    0.2126 * ch(16) + 0.7152 * ch(8) + 0.0722 * ch(0) > 0.5
}

/// Every theme that can be shown, presets first, each with its colors
/// (the settings screen draws them as swatches).
pub fn themes() -> Vec<(String, Theme)> {
    let mut files: Vec<String> = std::fs::read_dir(crate::mods::app_dir().join("themes"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.strip_suffix(".json").map(str::to_owned))
        .filter(|n| !PRESETS.contains(&n.as_str()))
        .collect();
    files.sort();
    PRESETS
        .iter()
        .map(|n| n.to_string())
        .chain(files)
        .filter_map(|n| palette(&n).ok().map(|(p, o)| (n, resolve(&p, &o, get().panel_colors, false))))
        .collect()
}

fn hex(c: u32) -> String {
    format!("#{c:06x}")
}

/// `a` moved toward `b` by `t`; a negative `t` moves away, clamped per channel.
fn mix(a: u32, b: u32, t: f32) -> u32 {
    [16, 8, 0].iter().fold(0, |acc, s| {
        let (x, y) = (((a >> s) & 255) as f32, ((b >> s) & 255) as f32);
        acc | ((x * (1.0 - t) + y * t).round().clamp(0.0, 255.0) as u32) << s
    })
}

fn with_alpha(c: u32, a: f32) -> String {
    format!("#{c:06x}{:02x}", (a * 255.0).round() as u32)
}

/// Every color the app and mods use, resolved. Values are `#rrggbb` or `#rrggbbaa`.
pub type Theme = BTreeMap<String, String>;

/// How far each panel color sits from the terminal's background, as a step
/// toward the theme's own foreground (negative steps away), so panels take
/// the theme's tint rather than plain grey. The steps are fitted to a
/// family of hand-tuned editor themes; light themes need smaller steps
/// toward their dark ink, except the sidebar, which they shade more.
struct Steps {
    canvas: f32,
    chrome: f32,
    /// `chrome` with `sidebar_shade` on: where a dark theme's window
    /// backdrop sits while active, level with `canvas`. Light themes keep
    /// their sidebar as is.
    chrome_active: f32,
    popover: f32,
    raised: f32,
    border: f32,
    input: f32,
    hover: f32,
    selected: f32,
    muted: f32,
    dim: f32,
}

const DARK_STEPS: Steps =
    Steps { canvas: -0.022, chrome: 0.016, chrome_active: -0.022, popover: 0.066, raised: 0.086, border: 0.153, input: 0.26, hover: 0.042, selected: 0.075, muted: 0.614, dim: 0.459 };
const LIGHT_STEPS: Steps =
    Steps { canvas: 0.015, chrome: 0.068, chrome_active: 0.068, popover: 0.0, raised: 0.035, border: 0.096, input: 0.214, hover: 0.048, selected: 0.09, muted: 0.752, dim: 0.598 };

/// WCAG contrast ratio of two colors.
fn contrast(a: u32, b: u32) -> f32 {
    let lum = |c: u32| {
        let ch = |s: u32| {
            let v = ((c >> s) & 255) as f32 / 255.0;
            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * ch(16) + 0.7152 * ch(8) + 0.0722 * ch(0)
    };
    let (x, y) = (lum(a), lum(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// Panel colors, each a step from the terminal's background toward its
/// foreground (`Steps`). Panel text uses a neutral ink (`ink`) for
/// contrast, its quieter tiers stepped toward it the same way. `shaded`
/// takes `chrome` a shade darker (`sidebar_shade`).
fn tinted_panels(p: &Palette, ink: u32, shaded: bool) -> Vec<(&'static str, String)> {
    let bg = p.background;
    let s = if is_light(bg) { &LIGHT_STEPS } else { &DARK_STEPS };
    let tint = |t: f32| hex(mix(bg, p.foreground, t));
    let ring = if contrast(p.ansi[4], bg) >= 3.0 { p.ansi[4] } else { mix(bg, ink, 0.7) };
    let dim = hex(mix(bg, ink, s.dim));
    vec![
        ("canvas", tint(s.canvas)),
        ("chrome", tint(if shaded { s.chrome_active } else { s.chrome })),
        ("popover", tint(s.popover)),
        ("raised", tint(s.raised)),
        ("hover", with_alpha(p.foreground, s.hover)),
        ("selected", with_alpha(p.foreground, s.selected)),
        ("border", tint(s.border)),
        ("input", tint(s.input)),
        ("muted", hex(mix(bg, ink, s.muted))),
        ("dim", dim.clone()),
        ("ring", hex(ring)),
        ("idle", dim.clone()),
        ("unknown", dim),
    ]
}

/// Panel colors as Herdr first drew them: plain white, black and warm grey
/// around the background, translucent borders. `chrome` (sidebars, bars)
/// and `canvas` (behind cards) step away from the terminal's background:
/// darker on a dark theme, toward warm grey on a light one.
fn classic_panels(bg: u32, ink: u32) -> Vec<(&'static str, String)> {
    let light = is_light(bg);
    let (popover, raised, hover, selected, border, input, canvas, chrome): (String, String, &str, &str, &str, &str, u32, u32) = if light {
        let warm = 0xe8e8e2;
        (hex(bg), hex(mix(bg, 0, 0.035)), "#0000000c", "#00000012", "#00000017", "#00000026", mix(bg, warm, 0.2), mix(bg, warm, 0.6))
    } else {
        (hex(mix(bg, 0xffffff, 0.06)), hex(mix(bg, 0xffffff, 0.09)), "#ffffff0e", "#ffffff17", "#ffffff18", "#ffffff24", mix(bg, 0, 0.16), mix(bg, 0, 0.32))
    };
    vec![
        ("canvas", hex(canvas)),
        ("chrome", hex(chrome)),
        ("popover", popover),
        ("raised", raised),
        ("hover", hover.into()),
        ("selected", selected.into()),
        ("border", border.into()),
        ("input", input.into()),
        ("muted", hex(mix(bg, ink, if light { 0.66 } else { 0.6 }))),
        ("dim", hex(mix(bg, ink, if light { 0.46 } else { 0.38 }))),
        ("ring", hex(mix(bg, ink, 0.7))),
        ("idle", hex(mix(bg, ink, 0.38))),
        ("unknown", hex(mix(bg, ink, 0.38))),
    ]
}

/// Whether a theme keeps the classic panels for whatever it leaves unset
/// even while tinted: one that paints its own panels (a deliberate look,
/// such as Win98's), or whose foreground can't carry a tint (barely apart
/// from the background, or on the wrong side of it).
fn keeps_classic_panels(p: &Palette, overrides: &BTreeMap<String, String>) -> bool {
    let (bg, fg) = (p.background, p.foreground);
    let own = ["surface", "popover", "raised"].iter().any(|k| overrides.contains_key(*k));
    let backwards = (contrast(fg, 0) > contrast(bg, 0)) == is_light(bg);
    own || contrast(fg, bg) < 3.0 || backwards
}

/// Every color around a palette: the terminal's own, then the panels
/// (`panel_colors`, `sidebar_shade`), then `overrides` over all of them.
fn resolve(p: &Palette, overrides: &BTreeMap<String, String>, panels: PanelColors, shaded: bool) -> Theme {
    let bg = p.background;
    let light = is_light(bg);
    let ink = if light { 0x1b1b1e } else { 0xeeeeef };
    let [working, blocked, done] = p.status.unwrap_or([p.ansi[3], p.ansi[1], p.ansi[4]]);
    let panels = match panels {
        PanelColors::Tinted if !keeps_classic_panels(p, overrides) => tinted_panels(p, ink, shaded),
        _ => classic_panels(bg, ink),
    };
    let mut t: Theme = [
        ("background", hex(bg)),
        ("terminal_foreground", hex(p.foreground)),
        ("foreground", hex(ink)),
        ("surface", hex(bg)),
        ("accent", hex(p.ansi[4])),
        ("accent_foreground", hex(bg)),
        ("solid", hex(ink)),
        ("solid_foreground", hex(if light { 0xffffff } else { mix(bg, 0, 0.28) })),
        ("selection", hex(p.selection)),
        ("cursor", hex(p.cursor.unwrap_or(p.foreground))),
        ("working", hex(working)),
        ("blocked", hex(blocked)),
        ("done", hex(done)),
    ]
    .into_iter()
    .chain(panels)
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    for (i, c) in p.ansi.iter().enumerate() {
        t.insert(format!("ansi{i}"), hex(*c));
    }
    for (k, v) in overrides {
        t.insert(k.clone(), v.clone());
    }
    t
}

#[cfg(test)]
mod derivation_tests {
    use super::*;

    /// Shapes the steps have to hold for: presets, shipped files (with their
    /// own `colors`), and public themes at the extremes (pure black, near
    /// black, pastel and plain light).
    fn themes() -> Vec<(String, Palette, BTreeMap<String, String>)> {
        let file = |bg: &str, fg: &str, accent: &str| {
            let mut ansi = vec!["\"#808080\"".to_owned(); 16];
            ansi[4] = format!("\"{accent}\"");
            let text = format!(r#"{{"background":"{bg}","foreground":"{fg}","selection":"{fg}","ansi":[{}]}}"#, ansi.join(","));
            parse_theme_file("fixture", &text).unwrap()
        };
        let mut out: Vec<_> = PRESETS.iter().map(|n| (n.to_string(), preset(n).unwrap().clone(), BTreeMap::new())).collect();
        for (name, text) in THEMES {
            let (p, own) = parse_theme_file(name, text).unwrap();
            out.push((name.to_string(), p, own));
        }
        for (name, bg, fg, accent) in [
            ("ayu-mirage", "#1f2430", "#cccac2", "#6dcbfa"),
            ("ayu-light", "#f8f9fa", "#5c6166", "#3199e1"),
            ("github-dark-default", "#0d1117", "#e6edf3", "#58a6ff"),
            ("modus-vivendi", "#000000", "#ffffff", "#2fafff"),
            ("moonfly", "#080808", "#bdbdbd", "#80a0ff"),
            ("rose-pine-dawn", "#faf4ed", "#575279", "#56949f"),
            ("zenburn", "#3f3f3f", "#dcdccc", "#8cd0d3"),
        ] {
            let (p, own) = file(bg, fg, accent);
            out.push((name.into(), p, own));
        }
        out
    }

    /// The palettes alone, for rules about what a theme leaves unset.
    fn palettes() -> Vec<(String, Palette)> {
        themes().into_iter().map(|(n, p, _)| (n, p)).collect()
    }

    fn rgb(t: &Theme, k: &str) -> u32 {
        u32::from_str_radix(&t[k][1..7], 16).unwrap()
    }

    fn lightness(c: u32) -> f32 {
        contrast(c, 0)
    }

    #[test]
    fn panels_layer_away_from_the_terminal() {
        for ((name, p), shaded) in palettes().into_iter().flat_map(|p| [(p.clone(), false), (p, true)]) {
            let t = resolve(&p, &BTreeMap::new(), PanelColors::Tinted, shaded);
            let (chrome, canvas) = (lightness(rgb(&t, "chrome")), lightness(rgb(&t, "canvas")));
            assert!(if is_light(p.background) { chrome < canvas } else { canvas <= chrome }, "{name}: chrome {chrome} against canvas {canvas}, shaded {shaded}");
            let [canvas, surface, popover, raised] = ["canvas", "surface", "popover", "raised"].map(|k| lightness(rgb(&t, k)));
            if is_light(p.background) {
                assert!(canvas < surface && raised < popover && popover <= surface, "{name}: light layers {canvas} {surface} {popover} {raised}");
            } else {
                assert!(canvas <= surface && surface < popover && popover < raised, "{name}: dark layers {canvas} {surface} {popover} {raised}");
            }
        }
    }

    /// Floors are the lowest each text tier reached on any panel across 90
    /// public themes before the steps were tuned, so no theme reads worse.
    #[test]
    fn panel_text_keeps_its_contrast() {
        for ((name, p), shaded) in palettes().into_iter().flat_map(|p| [(p.clone(), false), (p, true)]) {
            let t = resolve(&p, &BTreeMap::new(), PanelColors::Tinted, shaded);
            for panel in ["surface", "canvas", "chrome", "popover", "raised"] {
                for (text, floor) in [("foreground", 6.9), ("muted", 3.35), ("dim", 2.1)] {
                    let c = contrast(rgb(&t, text), rgb(&t, panel));
                    assert!(c >= floor, "{name}: {text} on {panel} is {c:.2}, under {floor}");
                }
            }
            assert!(contrast(rgb(&t, "ring"), p.background) >= 3.0 || t["ring"] == hex(mix(p.background, rgb(&t, "foreground"), 0.7)), "{name}: ring");
        }
    }

    #[test]
    fn explicit_colors_win() {
        let colors: BTreeMap<String, String> =
            ["canvas", "chrome", "popover", "raised", "border", "hover", "dim", "ring"].iter().map(|k| (k.to_string(), "#123456".into())).collect();
        for ((name, p), shaded) in palettes().into_iter().flat_map(|p| [(p.clone(), false), (p, true)]) {
            let t = resolve(&p, &colors, PanelColors::Tinted, shaded);
            for k in colors.keys() {
                assert_eq!(t[k], "#123456", "{name}: {k}, shaded {shaded}");
            }
        }
    }

    #[test]
    fn panels_take_the_theme_tint() {
        let p = parse_theme_file("t", r##"{"background":"#101820","foreground":"#80c0ff","selection":"#203040","ansi":["#000000","#000000","#000000","#000000","#4080ff","#000000","#000000","#000000","#000000","#000000","#000000","#000000","#000000","#000000","#000000","#000000"]}"##).unwrap().0;
        let t = resolve(&p, &BTreeMap::new(), PanelColors::Tinted, false);
        assert_eq!(t["border"], hex(mix(0x101820, 0x80c0ff, DARK_STEPS.border)));
        assert_eq!(t["hover"], with_alpha(0x80c0ff, DARK_STEPS.hover));
        let border = rgb(&t, "border");
        assert!(border & 255 > (border >> 16) & 255, "a blue foreground tints the border blue: {border:06x}");
    }

    #[test]
    fn sidebar_shade_darkens_chrome_only_when_it_shades() {
        let shades = |mode: SidebarShade| [true, false].map(|active| mode.shades(active));
        assert_eq!(shades(SidebarShade::Flat), [false, false]);
        assert_eq!(shades(SidebarShade::Active), [true, false]);
        assert_eq!(shades(SidebarShade::Always), [true, true]);
        for (name, p) in palettes() {
            let [flat, shaded] = [false, true].map(|s| resolve(&p, &BTreeMap::new(), PanelColors::Tinted, s));
            assert_eq!(flat.iter().filter(|(k, v)| shaded[*k] != **v).map(|(k, _)| k.as_str()).collect::<Vec<_>>(), if is_light(p.background) || flat["chrome"] == shaded["chrome"] { vec![] } else { vec!["chrome"] }, "{name}");
            if !is_light(p.background) && p.background != 0 {
                assert!(lightness(rgb(&shaded, "chrome")) < lightness(rgb(&flat, "chrome")), "{name}: the shade is darker");
                assert_eq!(shaded["chrome"], shaded["canvas"], "{name}: the shade sits level with canvas");
            }
        }
        let p = preset("graphite").unwrap();
        let own: BTreeMap<String, String> = [("chrome".to_owned(), "#202020".to_owned())].into();
        assert_eq!(resolve(p, &own, PanelColors::Tinted, true)["chrome"], "#202020", "a theme's own chrome is never shaded");
    }

    #[test]
    fn sidebar_shade_defaults_to_flat_and_refuses_others() {
        let default: Person = serde_json::from_str("{}").unwrap();
        assert_eq!(default.sidebar_shade, SidebarShade::Flat);
        assert_eq!(serde_json::to_value(&default).unwrap().get("sidebar_shade"), None);
        for (value, mode) in [("flat", SidebarShade::Flat), ("active", SidebarShade::Active), ("always", SidebarShade::Always)] {
            let person: Person = serde_json::from_value(serde_json::json!({"sidebar_shade": value})).unwrap();
            assert_eq!(Settings::compose(&person, &crate::layout::Look::default(), true).sidebar_shade, mode);
        }
        for (mode, value) in [(SidebarShade::Active, "active"), (SidebarShade::Always, "always")] {
            assert_eq!(serde_json::to_value(Person { sidebar_shade: mode, ..Person::default() }).unwrap()["sidebar_shade"], value);
        }
        for bad in [serde_json::json!({"sidebar_shade": "dark"}), serde_json::json!({"sidebar_shade": true})] {
            assert!(serde_json::from_value::<Person>(bad.clone()).is_err(), "{bad} is refused");
        }
    }

    /// Classic is the derivation Herdr shipped before tinted panels, value for
    /// value: the fixture is that build's `resolve` output for these themes.
    #[test]
    fn classic_panels_are_the_first_derivation_exactly() {
        let before: BTreeMap<String, Theme> = serde_json::from_str(include_str!("../tests/fixtures/master-panels.json")).unwrap();
        for (name, p, own) in themes() {
            for shaded in [false, true] {
                assert_eq!(resolve(&p, &own, PanelColors::Classic, shaded), before[&name], "{name}, shaded {shaded}");
            }
        }
        assert_eq!(before.len(), themes().len());
    }

    /// Over its own colors, a theme's hover and selected fills stay visible
    /// on its popovers and surfaces, and its text reads no worse than the
    /// classic panels drew it.
    #[test]
    fn own_colors_keep_fills_visible_and_text_readable() {
        let over = |top: &str, under: &str| {
            let (c, a) = (u32::from_str_radix(&top[1..7], 16).unwrap(), if top.len() == 9 { u32::from_str_radix(&top[7..9], 16).unwrap() as f32 / 255.0 } else { 1.0 });
            let under = u32::from_str_radix(&under[1..7], 16).unwrap();
            mix(under, c, a)
        };
        let apart = |a: u32, b: u32| [16, 8, 0].iter().map(|s| (((a >> s) & 255) as i32 - ((b >> s) & 255) as i32).abs()).max().unwrap();
        for (name, p, own) in themes() {
            for shaded in [false, true] {
                let t = resolve(&p, &own, PanelColors::Tinted, shaded);
                let classic = resolve(&p, &own, PanelColors::Classic, shaded);
                for fill in ["hover", "selected"] {
                    for panel in ["popover", "surface"] {
                        let under = rgb(&t, panel);
                        assert!(apart(over(&t[fill], &t[panel]), under) >= 4, "{name}: {fill} {} on {panel} {} is barely visible", t[fill], t[panel]);
                    }
                }
                for panel in ["surface", "canvas", "chrome", "popover", "raised"] {
                    for (text, floor) in [("foreground", 6.9f32), ("muted", 3.35), ("dim", 2.1)] {
                        let c = contrast(rgb(&t, text), rgb(&t, panel));
                        let before = contrast(rgb(&classic, text), rgb(&classic, panel));
                        assert!(c >= floor.min(before) - 0.005, "{name}: {text} on {panel} is {c:.2}, under {floor} and classic's {before:.2}");
                    }
                }
            }
        }
    }

    /// A theme that paints its own panels, or whose foreground can't carry a
    /// tint, keeps the classic derivation for everything it leaves unset.
    #[test]
    fn deliberate_looks_and_weak_foregrounds_keep_classic_panels() {
        let win98 = THEMES.iter().find(|(n, _)| *n == "win98").unwrap().1;
        let (p, own) = parse_theme_file("win98", win98).unwrap();
        assert_eq!(resolve(&p, &own, PanelColors::Tinted, true), resolve(&p, &own, PanelColors::Classic, true), "win98 draws as before");
        let terminals: serde_json::Value = serde_json::from_str(include_str!("../defaults/layouts/terminals.json")).unwrap();
        let look: BTreeMap<String, String> = serde_json::from_value(terminals.pointer("/colors").or_else(|| terminals.pointer("/look/colors")).cloned().unwrap_or_default()).unwrap();
        assert!(look.contains_key("surface"), "the Terminals look paints its own panels");
        for name in PRESETS {
            let p = preset(name).unwrap();
            assert_eq!(resolve(p, &look, PanelColors::Tinted, true), resolve(p, &look, PanelColors::Classic, true), "Terminals over {name}");
        }
        let weak = |bg: &str, fg: &str| parse_theme_file("weak", &format!(r##"{{"background":"{bg}","foreground":"{fg}","selection":"{fg}","ansi":["#000000","#000000","#000000","#000000","#4080ff","#000000","#000000","#000000","#000000","#000000","#000000","#000000","#000000","#000000","#000000","#000000"]}}"##)).unwrap().0;
        for (bg, fg) in [("#202020", "#2a2a2a"), ("#303845", "#101010"), ("#f4f4f4", "#ffffff"), ("#e0e0e0", "#c8c8c8"), ("#707070", "#000000")] {
            let p = weak(bg, fg);
            assert_eq!(resolve(&p, &BTreeMap::new(), PanelColors::Tinted, true), resolve(&p, &BTreeMap::new(), PanelColors::Classic, true), "{fg} on {bg}");
        }
    }

    #[test]
    fn panel_colors_default_to_tinted_and_refuse_others() {
        let default: Person = serde_json::from_str("{}").unwrap();
        assert_eq!(default.panel_colors, PanelColors::Tinted);
        assert_eq!(serde_json::to_value(&default).unwrap().get("panel_colors"), None);
        for (value, mode) in [("tinted", PanelColors::Tinted), ("classic", PanelColors::Classic)] {
            let person: Person = serde_json::from_value(serde_json::json!({"panel_colors": value})).unwrap();
            assert_eq!(Settings::compose(&person, &crate::layout::Look::default(), true).panel_colors, mode);
        }
        assert_eq!(serde_json::to_value(Person { panel_colors: PanelColors::Classic, ..Person::default() }).unwrap()["panel_colors"], "classic");
        for bad in [serde_json::json!({"panel_colors": "rex"}), serde_json::json!({"panel_colors": 1})] {
            assert!(serde_json::from_value::<Person>(bad.clone()).is_err(), "{bad} is refused");
        }
    }

    #[test]
    fn mix_clamps_past_the_ends() {
        assert_eq!(mix(0x000000, 0xffffff, -0.5), 0x000000);
        assert_eq!(mix(0xffffff, 0x000000, -0.5), 0xffffff);
    }
}

/// settings.json: preferences used where a layout leaves its look unset.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Person {
    /// How big the whole app is drawn (1 = as designed). The zoom keys change it.
    pub zoom: f32,
    /// How big the panels are drawn on top of `zoom` (chrome, mods, overlays;
    /// not the terminals' text or cells). 0.75..1.5.
    #[serde(skip_serializing_if = "is_one")]
    pub ui_zoom: f32,
    /// How big the panels' text is drawn on top of their zoom; spacing and
    /// icons keep their size. 0.8..1.3.
    #[serde(skip_serializing_if = "is_one")]
    pub ui_text_scale: f32,
    pub sounds: Sounds,
    /// When an alert also goes to the system's notification center (alert.rs).
    #[serde(skip_serializing_if = "SystemNotifications::is_background")]
    pub system_notifications: SystemNotifications,
    /// Bounce the Dock icon when an agent needs you while Herdr is in the background.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub dock_bounce: bool,
    /// What happens to recorded agents whose terminals are back to a shell
    /// after Herdr starts or the core comes back (resume.rs).
    #[serde(skip_serializing_if = "ResumeAgents::is_ask")]
    pub resume_agents: ResumeAgents,
    /// A program run for each alert instead of the system notification (alert.rs).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alert_command: Option<AlertCommand>,
    /// The pointer resting on a terminal pane focuses it (window/follow_mouse.rs).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub focus_follows_mouse: bool,
    /// How long it rests there first, in ms (0 to 2000; read through `focus_follows_mouse_delay`).
    #[serde(skip_serializing_if = "is_follow_delay_default")]
    pub focus_follows_mouse_delay_ms: u32,
    /// The pane.arrange mode new tabs start in (read through `default_arrange`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_arrange: Option<String>,
    /// The herdr-core to use, when it isn't where Herdr looks (core_bin.rs).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub core: Option<String>,
    /// Fonts for layouts that don't set their own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f32>,
    /// The terminals' row height, as a multiple of the font's height.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_height: Option<f32>,
    /// The terminals' text weight, 100 to 900; bold text is at least 700.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_weight: Option<u16>,
    /// The weight bold terminal text draws at, 100 to 900; unset, 700 or
    /// `font_weight` when that is heavier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_weight_bold: Option<u16>,
    /// Families terminal text takes characters from, in order, when its own
    /// font lacks them, before the system's. Nerd Font icons (private use)
    /// the font lacks come from the bundled symbols font regardless.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub font_fallbacks: Vec<String>,
    /// Enable programming ligatures in terminal fonts.
    pub font_ligatures: bool,
    /// Thicken terminal text as Ghostty's `font-thicken` does: the same for
    /// every text color (macOS), or turn font smoothing off.
    #[serde(skip_serializing_if = "Thicken::is_auto")]
    pub font_thicken: Thicken,
    /// How much `font_thicken` thickens, 0 to 255.
    #[serde(skip_serializing_if = "is_full_strength")]
    pub font_thicken_strength: u16,
    /// Thicken the panels' text (every mod, tab, popup and toast) the same
    /// way, or turn its font smoothing off, apart from the terminals'
    /// `font_thicken` (macOS).
    #[serde(skip_serializing_if = "Thicken::is_auto")]
    pub ui_font_thicken: Thicken,
    /// How much `ui_font_thicken` thickens, 0 to 255.
    #[serde(skip_serializing_if = "is_full_strength")]
    pub ui_font_thicken_strength: u16,
    /// Sharpen terminal text's edges, 0 (off) to 1: coverage is pulled toward
    /// an S-curve when blending, so strokes look crisper without widening.
    #[serde(skip_serializing_if = "is_zero")]
    pub text_contrast: f32,
    /// The same for the panels' text, apart from the terminals'.
    #[serde(skip_serializing_if = "is_zero")]
    pub ui_text_contrast: f32,
    /// Faint (SGR 2) terminal text's opacity, 0 to 1, as Ghostty's
    /// `faint-opacity`.
    #[serde(skip_serializing_if = "is_faint_default")]
    pub faint_opacity: f32,
    /// While the window isn't active, it fades this fraction toward the
    /// background, 0 (off) to 1.
    #[serde(skip_serializing_if = "is_zero")]
    pub unfocused_dim: f32,
    /// What `unfocused_dim` fades: the whole window, or terminal text only.
    #[serde(skip_serializing_if = "DimScope::is_window")]
    pub unfocused_dim_scope: DimScope,
    /// The color the window fades toward (`"#rrggbb"`), else the theme's
    /// background.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unfocused_dim_color: Option<String>,
    /// When the sidebars and bars take a shade darker: `flat`, `active` or `always`.
    #[serde(skip_serializing_if = "SidebarShade::is_flat")]
    pub sidebar_shade: SidebarShade,
    /// How panels derive unset colors: `tinted` or `classic`.
    #[serde(skip_serializing_if = "PanelColors::is_tinted")]
    pub panel_colors: PanelColors,
    /// Trial: smooth terminal text with a coverage per color channel
    /// (subpixel, on 1x displays) rather than one (grayscale) (macOS).
    #[serde(skip_serializing_if = "TextRendering::is_grayscale")]
    pub text_rendering: TextRendering,
    /// The same for the panels' text, apart from the terminals'.
    #[serde(skip_serializing_if = "TextRendering::is_grayscale")]
    pub ui_text_rendering: TextRendering,
    /// The display's color subpixels left to right, for subpixel smoothing.
    #[serde(skip_serializing_if = "SubpixelOrder::is_rgb")]
    pub subpixel_order: SubpixelOrder,
    /// The panels' font (their sizes follow zoom).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui_font: Option<String>,
    /// The panels' regular text weight, 100 to 900; text set heavier or
    /// lighter keeps its distance from it.
    #[serde(skip_serializing_if = "is_ui_weight_default")]
    pub ui_font_weight: u16,
    /// Anonymous usage data (telemetry.rs). On unless set to false.
    #[serde(skip_serializing_if = "is_true")]
    pub telemetry: bool,
    /// Allow terminal programs (including remote ones) to copy text locally.
    #[serde(skip_serializing_if = "is_true")]
    pub clipboard_write: bool,
    /// A draggable scroll position thumb at a terminal pane's right edge.
    #[serde(skip_serializing_if = "is_true")]
    pub pane_scrollbars: bool,
    /// Minimum terminal text contrast ratio; 1 leaves program colors unchanged.
    pub minimum_contrast: f32,
    /// Px (at zoom 1) added to (or, negative, taken from) the terminal cell width.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub letter_spacing: Option<f32>,
    /// Terminal cell width: the font's own advance, or rounded down to whole device pixels.
    #[serde(skip_serializing_if = "CellWidth::is_font")]
    pub cell_width: CellWidth,
    /// macOS Option sends Alt shortcuts, or follows the keyboard layout.
    #[serde(skip_serializing_if = "OptionAsAlt::is_auto")]
    pub option_as_alt: OptionAsAlt,
    /// Light or dark: the system's, or one always.
    #[serde(skip_serializing_if = "Appearance::is_system")]
    pub appearance: Appearance,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<crate::layout::ThemeChoice>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub colors: BTreeMap<String, String>,
}

/// `background`: only while Herdr isn't the app in front. `always`: also while
/// it is, unless the person is looking at that agent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SystemNotifications {
    #[default]
    Background,
    Always,
    Off,
}

impl SystemNotifications {
    fn is_background(&self) -> bool {
        *self == SystemNotifications::Background
    }
}

/// After a restart: offer to resume agents (a toast), resume them, or neither.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResumeAgents {
    #[default]
    Ask,
    Auto,
    Off,
}

impl ResumeAgents {
    fn is_ask(&self) -> bool {
        *self == ResumeAgents::Ask
    }
}

/// One string, run by `/bin/sh -c`, or a program and its arguments.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AlertCommand {
    Shell(String),
    Argv(Vec<String>),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    fn is_system(&self) -> bool {
        *self == Appearance::System
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OptionAsAlt {
    #[default]
    Auto,
    Off,
    On,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CellWidth {
    #[default]
    Font,
    Pixel,
}

impl CellWidth {
    fn is_font(&self) -> bool {
        *self == CellWidth::Font
    }
}

impl OptionAsAlt {
    fn is_auto(&self) -> bool {
        *self == Self::Auto
    }
}

/// How glyph edges are smoothed: one coverage per pixel, or one per color
/// channel, which only shows right on 1x displays (grayscale elsewhere).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TextRendering {
    #[default]
    Grayscale,
    Subpixel,
}

impl TextRendering {
    fn is_grayscale(&self) -> bool {
        *self == TextRendering::Grayscale
    }

    /// Whether glyphs ask for subpixel coverage (`Window::with_glyph_subpixel`).
    pub fn subpixel(self) -> bool {
        self == TextRendering::Subpixel
    }
}

/// What `unfocused_dim` fades while the window isn't active: everything in
/// it, or only the terminals' text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DimScope {
    #[default]
    Window,
    Text,
}

impl DimScope {
    fn is_window(&self) -> bool {
        *self == DimScope::Window
    }
}

/// When the sidebars and bars (`chrome`) take a shade darker than their
/// flat color: never, while the window is active, or always.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SidebarShade {
    #[default]
    Flat,
    Active,
    Always,
}

impl SidebarShade {
    fn is_flat(&self) -> bool {
        *self == SidebarShade::Flat
    }

    fn shades(self, window_active: bool) -> bool {
        self == SidebarShade::Always || (self == SidebarShade::Active && window_active)
    }
}

/// How panels derive the colors a theme leaves unset: tinted toward the
/// theme's own foreground, or classic white, black and warm grey.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PanelColors {
    #[default]
    Tinted,
    Classic,
}

impl PanelColors {
    fn is_tinted(&self) -> bool {
        *self == PanelColors::Tinted
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubpixelOrder {
    #[default]
    Rgb,
    Bgr,
}

impl SubpixelOrder {
    fn is_rgb(&self) -> bool {
        *self == SubpixelOrder::Rgb
    }
}

/// Ghostty's `font-thicken-strength` default.
pub const THICKEN_STRENGTH_DEFAULT: u16 = 255;

fn is_full_strength(v: &u16) -> bool {
    *v == THICKEN_STRENGTH_DEFAULT
}

/// `font_thicken` and `ui_font_thicken`: absent or `"auto"` leaves macOS
/// font smoothing to pick by text color, `true` thickens at a fixed strength
/// and `false` turns smoothing off, both the same for every text color. Any
/// other string is kept, so `problems()` can name it, and draws as `"auto"`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(from = "ThickenValue", into = "ThickenValue")]
pub enum Thicken {
    #[default]
    Auto,
    On,
    Off,
    Unknown(String),
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum ThickenValue {
    On(bool),
    Named(String),
}

impl From<ThickenValue> for Thicken {
    fn from(v: ThickenValue) -> Self {
        match v {
            ThickenValue::On(true) => Thicken::On,
            ThickenValue::On(false) => Thicken::Off,
            ThickenValue::Named(s) if s == "auto" => Thicken::Auto,
            ThickenValue::Named(s) => Thicken::Unknown(s),
        }
    }
}

impl From<Thicken> for ThickenValue {
    fn from(t: Thicken) -> Self {
        match t {
            Thicken::Auto => ThickenValue::Named("auto".into()),
            Thicken::On => ThickenValue::On(true),
            Thicken::Off => ThickenValue::On(false),
            Thicken::Unknown(s) => ThickenValue::Named(s),
        }
    }
}

impl Thicken {
    fn is_auto(&self) -> bool {
        *self == Thicken::Auto
    }

    /// How the glyphs rasterise: thickened at `strength`, unsmoothed, or
    /// (auto or unknown) by text color.
    pub fn glyph(&self, strength: u16) -> gpui::GlyphThicken {
        match self {
            Thicken::On => gpui::GlyphThicken::Strength(strength.min(255) as u8),
            Thicken::Off => gpui::GlyphThicken::Off,
            Thicken::Auto | Thicken::Unknown(_) => gpui::GlyphThicken::Auto,
        }
    }

    fn problem(&self, key: &str) -> Option<String> {
        match self {
            Thicken::Unknown(s) => Some(format!("{key} {s:?} is not \"auto\", true or false")),
            _ => None,
        }
    }
}

/// The weight the panels' text has unless something sets one: GPUI's normal.
pub const UI_FONT_WEIGHT_DEFAULT: u16 = 400;

fn is_ui_weight_default(v: &u16) -> bool {
    *v == UI_FONT_WEIGHT_DEFAULT
}

fn is_true(b: &bool) -> bool {
    *b
}

fn is_one(v: &f32) -> bool {
    *v == 1.0
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

/// Ghostty's `faint-opacity` default.
pub const FAINT_OPACITY_DEFAULT: f32 = 0.5;

fn is_faint_default(v: &f32) -> bool {
    *v == FAINT_OPACITY_DEFAULT
}

pub const FOLLOW_DELAY_DEFAULT_MS: u32 = 150;
pub const FOLLOW_DELAY_MAX_MS: u32 = 2000;

fn is_follow_delay_default(ms: &u32) -> bool {
    *ms == FOLLOW_DELAY_DEFAULT_MS
}

impl Person {
    /// `focus_follows_mouse_delay_ms`, at most `FOLLOW_DELAY_MAX_MS`.
    pub fn focus_follows_mouse_delay(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.focus_follows_mouse_delay_ms.min(FOLLOW_DELAY_MAX_MS).into())
    }

    /// `default_arrange` as a mode; a name that isn't one is none.
    pub fn default_arrange(&self) -> Option<crate::arrange::Mode> {
        self.default_arrange.as_deref().and_then(crate::arrange::Mode::parse)
    }
}

pub const UI_ZOOM_RANGE: (f32, f32) = (0.75, 1.5);
pub const UI_TEXT_SCALE_RANGE: (f32, f32) = (0.8, 1.3);

/// A person's scale, within `(lo, hi)`; anything unreadable is 1.
fn clamp_scale(v: f32, (lo, hi): (f32, f32)) -> f32 {
    if v.is_finite() { v.clamp(lo, hi) } else { 1.0 }
}

impl Default for Person {
    fn default() -> Self {
        Person {
            zoom: 1.0,
            ui_zoom: 1.0,
            ui_text_scale: 1.0,
            sounds: Sounds::default(),
            system_notifications: SystemNotifications::Background,
            dock_bounce: false,
            resume_agents: ResumeAgents::Ask,
            alert_command: None,
            focus_follows_mouse: false,
            focus_follows_mouse_delay_ms: FOLLOW_DELAY_DEFAULT_MS,
            default_arrange: None,
            core: None,
            font: None,
            font_size: None,
            line_height: None,
            font_weight: None,
            font_weight_bold: None,
            font_fallbacks: Vec::new(),
            font_ligatures: false,
            font_thicken: Thicken::Auto,
            font_thicken_strength: THICKEN_STRENGTH_DEFAULT,
            ui_font_thicken: Thicken::Auto,
            ui_font_thicken_strength: THICKEN_STRENGTH_DEFAULT,
            text_contrast: 0.0,
            ui_text_contrast: 0.0,
            faint_opacity: FAINT_OPACITY_DEFAULT,
            unfocused_dim: 0.0,
            unfocused_dim_scope: DimScope::Window,
            unfocused_dim_color: None,
            sidebar_shade: SidebarShade::Flat,
            panel_colors: PanelColors::Tinted,
            text_rendering: TextRendering::Grayscale,
            ui_text_rendering: TextRendering::Grayscale,
            subpixel_order: SubpixelOrder::Rgb,
            ui_font: None,
            ui_font_weight: UI_FONT_WEIGHT_DEFAULT,
            telemetry: true,
            clipboard_write: true,
            pane_scrollbars: true,
            minimum_contrast: 1.0,
            letter_spacing: None,
            cell_width: CellWidth::Font,
            option_as_alt: OptionAsAlt::Auto,
            appearance: Appearance::System,
            theme: None,
            colors: BTreeMap::new(),
        }
    }
}

/// What the app runs with: the shown layout's look over the defaults, at
/// the person's zoom. Everything reads this one value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub font_family: String,
    pub font_size: f32,
    /// The terminals' row height, as a multiple of the font's height.
    pub line_height: f32,
    /// The terminals' text weight (bold text is at least 700).
    pub font_weight: u16,
    /// The weight bold terminal text draws at; unset, see `bold_weight`.
    pub font_weight_bold: Option<u16>,
    /// Families terminal text falls back to, before the system's; built once
    /// here so every cell font shares it. None when there are none.
    pub font_fallbacks: Option<gpui::FontFallbacks>,
    pub font_ligatures: bool,
    pub font_thicken: Thicken,
    pub font_thicken_strength: u16,
    pub ui_font_thicken: Thicken,
    pub ui_font_thicken_strength: u16,
    /// Terminal text's edge contrast, 0 (off) to 1.
    pub text_contrast: f32,
    /// The panels' text's edge contrast, 0 (off) to 1.
    pub ui_text_contrast: f32,
    /// Faint terminal text's opacity, as written (`faint_alpha` reads it).
    pub faint_opacity: f32,
    /// How far the window fades toward the background while it isn't
    /// active, 0 (off) to 1.
    pub unfocused_dim: f32,
    /// What that fades: the whole window, or terminal text only.
    pub unfocused_dim_scope: DimScope,
    /// The color the window fades toward, as written (`dim_color` reads it).
    pub unfocused_dim_color: Option<String>,
    /// When the sidebars and bars take a shade darker.
    pub sidebar_shade: SidebarShade,
    /// How panels derive the colors a theme leaves unset.
    pub panel_colors: PanelColors,
    /// Terminal text's smoothing (subpixel only where the display allows).
    pub text_rendering: TextRendering,
    /// The panels' text's smoothing.
    pub ui_text_rendering: TextRendering,
    pub subpixel_order: SubpixelOrder,
    pub ui_font_family: String,
    pub ui_font_size: f32,
    /// The panels' regular text weight (see `ui_weight_shift`).
    pub ui_font_weight: u16,
    /// How big the whole app is drawn (1 = as designed). The zoom keys change it.
    pub zoom: f32,
    /// The panels' size on top of `zoom` (terminals' text and cells excluded).
    pub ui_zoom: f32,
    /// The panels' text size on top of their zoom.
    pub ui_text_scale: f32,
    /// `ui_zoom` and `ui_text_scale` as settings.json has them, before
    /// clamping: what `problems` reports.
    #[serde(skip)]
    pub asked_ui_scales: (f32, f32),
    /// The theme showing: a preset or a file in themes/.
    pub theme: String,
    /// The layout's theme for each appearance (the same one twice when it names one).
    pub theme_light: String,
    pub theme_dark: String,
    /// Whether the window is dark now (the person's `appearance`, else the system's).
    pub dark: bool,
    /// Any theme color by name (see `theme()` in herdr.d.ts), `#rrggbb` or `#rrggbbaa`.
    pub colors: BTreeMap<String, String>,
    /// A draggable scroll position thumb at a terminal pane's right edge.
    pub pane_scrollbars: bool,
    /// Space between a terminal's edge and its text, in px: [x, y].
    pub terminal_padding: [f32; 2],
    /// 0..1: below 1, what's behind a terminal shows through.
    pub terminal_opacity: f32,
    pub clipboard_write: bool,
    pub minimum_contrast: f32,
    pub letter_spacing: f32,
    pub cell_width: CellWidth,
    pub option_as_alt: OptionAsAlt,
    pub sounds: Sounds,
    pub system_notifications: SystemNotifications,
    pub dock_bounce: bool,
    pub alert_command: Option<AlertCommand>,
}

/// Attention sounds: when an agent needs you, and when one finished while you
/// weren't looking. `done` / `blocked`: an MP3 file instead of the bundled one
/// (absolute, or relative to ~/.config/herdr-app).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sounds {
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub done: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked: Option<String>,
}

impl Default for Sounds {
    fn default() -> Self {
        Sounds { enabled: true, done: None, blocked: None }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            font_family: DEFAULT_FONT.into(),
            font_size: 14.0,
            line_height: 1.15,
            font_weight: 500,
            font_weight_bold: None,
            font_fallbacks: None,
            font_ligatures: false,
            font_thicken: Thicken::Auto,
            font_thicken_strength: THICKEN_STRENGTH_DEFAULT,
            ui_font_thicken: Thicken::Auto,
            ui_font_thicken_strength: THICKEN_STRENGTH_DEFAULT,
            text_contrast: 0.0,
            ui_text_contrast: 0.0,
            faint_opacity: FAINT_OPACITY_DEFAULT,
            unfocused_dim: 0.0,
            unfocused_dim_scope: DimScope::Window,
            unfocused_dim_color: None,
            sidebar_shade: SidebarShade::Flat,
            panel_colors: PanelColors::Tinted,
            text_rendering: TextRendering::Grayscale,
            ui_text_rendering: TextRendering::Grayscale,
            subpixel_order: SubpixelOrder::Rgb,
            ui_font_family: "Geist".into(),
            ui_font_size: 13.0,
            ui_font_weight: UI_FONT_WEIGHT_DEFAULT,
            zoom: 1.0,
            ui_zoom: 1.0,
            ui_text_scale: 1.0,
            asked_ui_scales: (1.0, 1.0),
            theme: DEFAULT_DARK.into(),
            theme_light: DEFAULT_LIGHT.into(),
            theme_dark: DEFAULT_DARK.into(),
            dark: true,
            colors: BTreeMap::new(),
            pane_scrollbars: true,
            terminal_padding: [10.0, 6.0],
            terminal_opacity: 1.0,
            clipboard_write: true,
            minimum_contrast: 1.0,
            letter_spacing: 0.0,
            cell_width: CellWidth::Font,
            option_as_alt: OptionAsAlt::Auto,
            sounds: Sounds::default(),
            system_notifications: SystemNotifications::Background,
            dock_bounce: false,
            alert_command: None,
        }
    }
}

impl Settings {
    pub fn compose(person: &Person, look: &crate::layout::Look, system_dark: bool) -> Settings {
        let d = Settings::default();
        let mut colors = person.colors.clone();
        colors.extend(look.colors.clone().unwrap_or_default());
        let dark = match person.appearance {
            Appearance::System => system_dark,
            Appearance::Light => false,
            Appearance::Dark => true,
        };
        let (theme_light, theme_dark) = match look.theme.as_ref().or(person.theme.as_ref()) {
            Some(c) => (c.pick(false).to_owned(), c.pick(true).to_owned()),
            None => (DEFAULT_LIGHT.to_owned(), DEFAULT_DARK.to_owned()),
        };
        Settings {
            // The layout's own, else the person's, else Herdr's.
            font_family: look.font.clone().or(person.font.clone()).unwrap_or(d.font_family),
            font_size: look.font_size.or(person.font_size).unwrap_or(d.font_size),
            line_height: look.line_height.or(person.line_height).unwrap_or(d.line_height),
            font_weight: look.font_weight.or(person.font_weight).unwrap_or(d.font_weight),
            font_weight_bold: look.font_weight_bold.or(person.font_weight_bold),
            font_fallbacks: (!person.font_fallbacks.is_empty()).then(|| gpui::FontFallbacks::from_fonts(person.font_fallbacks.clone())),
            font_ligatures: person.font_ligatures,
            font_thicken: person.font_thicken.clone(),
            font_thicken_strength: person.font_thicken_strength,
            ui_font_thicken: person.ui_font_thicken.clone(),
            ui_font_thicken_strength: person.ui_font_thicken_strength,
            text_contrast: person.text_contrast,
            ui_text_contrast: person.ui_text_contrast,
            faint_opacity: person.faint_opacity,
            unfocused_dim: person.unfocused_dim,
            unfocused_dim_scope: person.unfocused_dim_scope,
            unfocused_dim_color: person.unfocused_dim_color.clone(),
            sidebar_shade: person.sidebar_shade,
            panel_colors: person.panel_colors,
            text_rendering: person.text_rendering,
            ui_text_rendering: person.ui_text_rendering,
            subpixel_order: person.subpixel_order,
            ui_font_family: look.ui_font.clone().or(person.ui_font.clone()).unwrap_or(d.ui_font_family),
            ui_font_size: d.ui_font_size,
            ui_font_weight: person.ui_font_weight,
            zoom: person.zoom,
            ui_zoom: clamp_scale(person.ui_zoom, UI_ZOOM_RANGE),
            ui_text_scale: clamp_scale(person.ui_text_scale, UI_TEXT_SCALE_RANGE),
            asked_ui_scales: (person.ui_zoom, person.ui_text_scale),
            theme: if dark { theme_dark.clone() } else { theme_light.clone() },
            theme_light,
            theme_dark,
            dark,
            colors,
            pane_scrollbars: person.pane_scrollbars,
            terminal_padding: look.terminal_padding.unwrap_or(d.terminal_padding),
            terminal_opacity: look.terminal_opacity.unwrap_or(1.0).clamp(0.0, 1.0),
            sounds: person.sounds.clone(),
            system_notifications: person.system_notifications,
            dock_bounce: person.dock_bounce,
            alert_command: person.alert_command.clone(),
            clipboard_write: person.clipboard_write,
            minimum_contrast: person.minimum_contrast,
            letter_spacing: look.letter_spacing.or(person.letter_spacing).unwrap_or(d.letter_spacing),
            cell_width: person.cell_width,
            option_as_alt: person.option_as_alt,
        }
    }
}

pub fn parse_hex(s: &str) -> Option<Hsla> {
    let h = s.trim().trim_start_matches('#');
    let v = u32::from_str_radix(h, 16).ok()?;
    let (rgb, a) = match h.len() {
        6 => (v, 255),
        8 => (v >> 8, v & 255),
        _ => return None,
    };
    Some(
        Rgba {
            r: ((rgb >> 16) & 255) as f32 / 255.0,
            g: ((rgb >> 8) & 255) as f32 / 255.0,
            b: (rgb & 255) as f32 / 255.0,
            a: a as f32 / 255.0,
        }
        .into(),
    )
}

pub fn rgb(s: &str) -> Rgb {
    let h = s.trim().trim_start_matches('#');
    let v = u32::from_str_radix(&h[..h.len().min(6)], 16).unwrap_or(0xff00ff);
    Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

impl Settings {
    /// `unfocused_dim_color` when it's a `"#rrggbb"` color, else None (the
    /// window fades toward the theme's background).
    pub fn dim_color(&self) -> Option<Hsla> {
        let color = self.unfocused_dim_color.as_deref()?;
        (color.len() == 7 && color.starts_with('#')).then(|| parse_hex(color)).flatten()
    }

    /// Problems an agent should hear about before the file is applied.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        for name in [&self.theme_light, &self.theme_dark] {
            if let Err(e) = palette(name) {
                out.push(e);
            }
        }
        out.dedup();
        for (k, v) in &self.colors {
            if parse_hex(v).is_none() {
                out.push(format!("colors.{k}: {v:?} is not #rrggbb or #rrggbbaa"));
            }
        }
        if !(1.0..=21.0).contains(&self.minimum_contrast) {
            out.push(format!("minimum_contrast {} is outside 1..21", self.minimum_contrast));
        }
        out.extend(self.font_thicken.problem("font_thicken"));
        out.extend(self.ui_font_thicken.problem("ui_font_thicken"));
        if self.font_thicken_strength > 255 {
            out.push(format!("font_thicken_strength {} is outside 0..255", self.font_thicken_strength));
        }
        if self.ui_font_thicken_strength > 255 {
            out.push(format!("ui_font_thicken_strength {} is outside 0..255", self.ui_font_thicken_strength));
        }
        if !(0.0..=1.0).contains(&self.text_contrast) {
            out.push(format!("text_contrast {} is outside 0..1", self.text_contrast));
        }
        if !(0.0..=1.0).contains(&self.ui_text_contrast) {
            out.push(format!("ui_text_contrast {} is outside 0..1", self.ui_text_contrast));
        }
        if !(0.0..=1.0).contains(&self.faint_opacity) {
            out.push(format!("faint_opacity {} is outside 0..1", self.faint_opacity));
        }
        if !(0.0..=1.0).contains(&self.unfocused_dim) {
            out.push(format!("unfocused_dim {} is outside 0..1", self.unfocused_dim));
        }
        if let Some(color) = &self.unfocused_dim_color
            && self.dim_color().is_none()
        {
            out.push(format!("unfocused_dim_color {color:?} isn't a \"#rrggbb\" color; the window fades toward the background"));
        }
        if !(100..=900).contains(&self.ui_font_weight) {
            out.push(format!("ui_font_weight {} is outside 100..900", self.ui_font_weight));
        }
        if !(100..=900).contains(&self.font_weight) {
            out.push(format!("font_weight {} is outside 100..900", self.font_weight));
        }
        if let Some(bold) = self.font_weight_bold.filter(|w| !(100..=900).contains(w)) {
            out.push(format!("font_weight_bold {bold} is outside 100..900"));
        }
        if !(-2.0..=10.0).contains(&self.letter_spacing) {
            out.push(format!("letter_spacing {} is outside -2..10", self.letter_spacing));
        }
        if !(6.0..=72.0).contains(&self.font_size) {
            out.push(format!("font_size {} is outside 6..72", self.font_size));
        }
        if !(0.8..=2.0).contains(&self.line_height) {
            out.push(format!("line_height {} is outside 0.8..2", self.line_height));
        }
        let (ui_zoom, ui_text_scale) = self.asked_ui_scales;
        for (key, asked, (lo, hi), used) in [("ui_zoom", ui_zoom, UI_ZOOM_RANGE, self.ui_zoom), ("ui_text_scale", ui_text_scale, UI_TEXT_SCALE_RANGE, self.ui_text_scale)] {
            if !(lo..=hi).contains(&asked) {
                out.push(format!("{key} {asked} is outside {lo}..{hi}: using {used}"));
            }
        }
        out
    }
}

pub fn path() -> std::path::PathBuf {
    crate::mods::app_dir().join("settings.json")
}

thread_local! {
    static CURRENT: RefCell<(Rc<Settings>, Rc<Theme>)> =
        RefCell::new((Rc::new(Settings::default()), Rc::new(resolve(&GRAPHITE, &BTreeMap::new(), PanelColors::Tinted, false))));
    static WINDOW_ACTIVE: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

const ZOOM_MIN: f32 = 0.5;
const ZOOM_MAX: f32 = 3.0;

pub fn get() -> Rc<Settings> {
    CURRENT.with(|c| c.borrow().0.clone())
}

/// The resolved theme: every color by name.
pub fn theme() -> Rc<Theme> {
    CURRENT.with(|c| c.borrow().1.clone())
}

/// A theme color as GPUI paint (magenta if missing, so mistakes show).
pub fn color(name: &str) -> Hsla {
    theme().get(name).and_then(|v| parse_hex(v)).unwrap_or(gpui::hsla(0.83, 1.0, 0.5, 1.0))
}

/// A theme color name, or a literal `#rrggbb[aa]`.
pub fn color_or_literal(v: &str) -> Hsla {
    parse_hex(v).unwrap_or_else(|| color(v))
}

/// Reads settings.json (writing the defaults if it's missing), or says why it can't be used.
pub fn load_person() -> Result<Person, String> {
    let p = path();
    if !p.exists() {
        let _ = crate::persist::write_atomic(&p, serde_json::to_string_pretty(&Person::default()).unwrap() + "\n");
    }
    let text = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
    let person: Person = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if !(ZOOM_MIN..=ZOOM_MAX).contains(&person.zoom) {
        return Err(format!("zoom {} is outside {ZOOM_MIN}..{ZOOM_MAX}", person.zoom));
    }
    Ok(person)
}

/// The value of settings.json's own `telemetry`, read even when the file has
/// another problem: an explicit `false` always counts.
pub fn telemetry_setting(person: &Result<Person, String>) -> bool {
    match person {
        Ok(p) => p.telemetry,
        Err(_) => {
            let raw = std::fs::read_to_string(path()).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok());
            raw.and_then(|v| v.get("telemetry").and_then(serde_json::Value::as_bool)).unwrap_or(true)
        }
    }
}

pub fn write_person(person: &Person) -> std::io::Result<()> {
    crate::persist::write_atomic(&path(), serde_json::to_string_pretty(person).unwrap_or_default() + "\n")
}

/// Notes whether the window is active, which `sidebar_shade` follows;
/// `set` applies it.
pub fn set_window_active(active: bool) {
    WINDOW_ACTIVE.with(|w| w.set(active));
}

/// Installs new settings; returns whether anything changed.
pub fn set(s: Settings) -> bool {
    #[cfg(target_os = "macos")]
    gpui_macos::set_option_as_alt(match s.option_as_alt {
        OptionAsAlt::Auto => None,
        OptionAsAlt::Off => Some(gpui_macos::OptionAsAlt::None),
        OptionAsAlt::On => Some(gpui_macos::OptionAsAlt::Both),
        OptionAsAlt::Left => Some(gpui_macos::OptionAsAlt::Left),
        OptionAsAlt::Right => Some(gpui_macos::OptionAsAlt::Right),
    });
    CURRENT.with(|c| {
        // Read the theme even when the settings are the same: its file may
        // have new colors under the same name.
        let (p, mut colors) = palette(&s.theme).unwrap_or_else(|_| (GRAPHITE.clone(), BTreeMap::new()));
        colors.extend(s.colors.clone());
        let theme = resolve(&p, &colors, s.panel_colors, s.sidebar_shade.shades(WINDOW_ACTIVE.with(|w| w.get())));
        if *c.borrow().0 == s && *c.borrow().1 == theme {
            return false;
        }
        let (pixels, text) = s.mod_scales();
        gpui_shell::set_pixel_scale(pixels);
        gpui_shell::set_text_scale(text);
        *c.borrow_mut() = (Rc::new(s), Rc::new(theme));
        true
    })
}

/// The zoom a first start picks: the system's text size (GNOME's large text,
/// Windows' "make text bigger"), a little more on a big screen at 100%
/// (logical px: a scaled screen already draws bigger), in tenths.
pub fn first_zoom(display_width: Option<f32>) -> f32 {
    let text = system_text_scale().clamp(1.0, 2.0);
    let screen = if display_width.is_some_and(|w| w >= 2400.0) { 1.15 } else { 1.0 };
    ((text * screen * 10.0).round() / 10.0).clamp(ZOOM_MIN, ZOOM_MAX)
}

fn system_text_scale() -> f32 {
    let run = |cmd: &str, args: &[&str]| -> Option<String> {
        let out = crate::spawn::command(cmd).args(args).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    if cfg!(windows) {
        // Percent, 100..225, as REG_DWORD hex.
        return run("reg", &["query", r"HKCU\Software\Microsoft\Accessibility", "/v", "TextScaleFactor"])
            .and_then(|o| o.split_whitespace().last().and_then(|h| u32::from_str_radix(h.trim_start_matches("0x"), 16).ok()))
            .map_or(1.0, |p| p as f32 / 100.0);
    }
    if cfg!(target_os = "linux") {
        return run("gsettings", &["get", "org.gnome.desktop.interface", "text-scaling-factor"]).and_then(|o| o.trim().parse().ok()).unwrap_or(1.0);
    }
    1.0
}

/// The next zoom level in `steps` tenths (0 resets).
pub fn next_zoom(steps: i32) -> f32 {
    if steps == 0 {
        return 1.0;
    }
    ((zoom() * 10.0).round() + steps as f32).clamp(ZOOM_MIN * 10.0, ZOOM_MAX * 10.0) / 10.0
}

/// The terminals' zoom: their text, cells and padding.
pub fn zoom() -> f32 {
    get().zoom
}

/// The panels' zoom: `zoom` times `ui_zoom`. Every designed UI px uses it.
pub fn ui_scale() -> f32 {
    get().ui_scale()
}

/// A designed size in px, at the panels' zoom.
pub fn z(v: f32) -> Pixels {
    px(get().ui_px(v))
}

/// A designed text size in px, at the panels' zoom and text scale.
pub fn zt(v: f32) -> Pixels {
    px(get().ui_text_px(v))
}

pub fn font_size() -> f32 {
    get().terminal_font_size()
}

/// The panels' base text size, with the text scale.
pub fn ui_font_size() -> f32 {
    get().ui_font_size_px()
}

/// The window's rem: the panels' base text size at their zoom but without
/// the text scale, so rem-based spacing, radii and panels keep their size.
pub fn ui_rem() -> Pixels {
    px(get().ui_rem_px())
}

/// A text size in rems (`text_xs` is 0.75) that also takes the text scale,
/// which the rem leaves out.
pub fn text_rems(rems: f32) -> Pixels {
    px(get().text_rems_px(rems))
}

/// Sizes at these settings. Terminals (text, cells, padding) follow `zoom`
/// only; everything else follows `ui_scale`, and text also `ui_text_scale`.
impl Settings {
    /// How `ui_font_thicken` rasterises every glyph outside the terminals'
    /// grids.
    pub fn ui_glyph_thicken(&self) -> gpui::GlyphThicken {
        self.ui_font_thicken.glyph(self.ui_font_thicken_strength)
    }

    /// The opacity faint terminal text draws at: `faint_opacity` within 0..1.
    pub fn faint_alpha(&self) -> f32 {
        self.faint_opacity.clamp(0.0, 1.0)
    }

    /// The weight bold terminal text draws at: `font_weight_bold`, else 700
    /// or `font_weight` when that is heavier.
    pub fn bold_weight(&self) -> u16 {
        let weight = self.font_weight.clamp(100, 900);
        self.font_weight_bold.map_or(weight.max(700), |bold| bold.clamp(100, 900))
    }

    /// How much heavier every run of the panels' text is shaped than it asks
    /// for (`Window::set_font_weight_shift`): regular text lands on
    /// `ui_font_weight` and bold stays as much bolder. Terminals shape at 0.
    pub fn ui_weight_shift(&self) -> f32 {
        f32::from(self.ui_font_weight.clamp(100, 900)) - f32::from(UI_FONT_WEIGHT_DEFAULT)
    }

    /// At most the largest `zoom`: a big zoom and a big `ui_zoom` don't compound past it.
    pub fn ui_scale(&self) -> f32 {
        (self.zoom * self.ui_zoom).min(ZOOM_MAX)
    }

    pub fn ui_px(&self, v: f32) -> f32 {
        v * self.ui_scale()
    }

    pub fn ui_text_px(&self, v: f32) -> f32 {
        self.ui_px(v) * self.ui_text_scale
    }

    pub fn ui_font_size_px(&self) -> f32 {
        self.ui_text_px(self.ui_font_size)
    }

    pub fn ui_rem_px(&self) -> f32 {
        self.ui_px(self.ui_font_size)
    }

    /// In the order GPUI turns rems into px (rems × rem), so at text scale 1
    /// it is `text_xs()` and friends bit for bit.
    pub fn text_rems_px(&self, rems: f32) -> f32 {
        rems * self.ui_rem_px() * self.ui_text_scale
    }

    /// What mods get as `settings()`: these settings, and `ui_scale` (what
    /// their px are drawn at, to divide window px by) and the ranges of
    /// `ui_zoom` and `ui_text_scale`.
    pub fn for_mods(&self) -> serde_json::Value {
        let mut v = serde_json::to_value(self).unwrap_or(serde_json::Value::Null);
        if let Some(o) = v.as_object_mut() {
            o.insert("ui_scale".into(), serde_json::json!(self.ui_scale()));
            o.insert("ui_scale_ranges".into(), serde_json::json!({ "ui_zoom": UI_ZOOM_RANGE, "ui_text_scale": UI_TEXT_SCALE_RANGE }));
        }
        v
    }

    pub fn terminal_font_size(&self) -> f32 {
        self.font_size * self.zoom
    }

    /// The terminals' padding in px: [x, y].
    pub fn terminal_padding_px(&self) -> (f32, f32) {
        (self.terminal_padding[0] * self.zoom, self.terminal_padding[1] * self.zoom)
    }

    /// What mods' px are multiplied by: every px, and text sizes on top.
    pub fn mod_scales(&self) -> (f32, f32) {
        (self.ui_scale(), self.ui_text_scale)
    }
}

/// Font families on this machine (the bundled ones and the home's included).
static FONTS: Mutex<Option<std::collections::HashSet<String>>> = Mutex::new(None);

pub fn note_fonts(names: Vec<String>) {
    *FONTS.lock().unwrap() = Some(names.into_iter().map(|n| n.to_lowercase()).collect());
}

/// Font files the home brings: `fonts/` and each mod's `fonts/` (a layout's
/// look travels with its mod, as Herdr 98's does). Loads the ones not loaded
/// yet; true when any was.
pub fn load_home_fonts(cx: &mut gpui::App) -> bool {
    static LOADED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
    let home = crate::mods::app_dir();
    let mut dirs = vec![home.join("fonts")];
    dirs.extend(std::fs::read_dir(home.join("mods")).into_iter().flatten().flatten().map(|e| e.path().join("fonts")));
    let mut new = Vec::new();
    let mut loaded = LOADED.lock().unwrap();
    for dir in dirs {
        for path in std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()) {
            let font = path.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_lowercase().as_str(), "ttf" | "otf" | "ttc"));
            if font && !loaded.contains(&path) && let Some(bytes) = map_font(&path) {
                new.push(std::borrow::Cow::Borrowed(bytes));
                loaded.push(path);
            }
        }
    }
    if new.is_empty() {
        return false;
    }
    if let Err(e) = cx.text_system().add_fonts(new) {
        tracing::warn!("fonts in {} not loaded: {e:#}", home.display());
    }
    FONTS_ADDED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    note_fonts(cx.text_system().all_font_names());
    true
}

/// Mapped, not read: the text system keeps borrowed bytes without copying
/// them, so the font's pages stay file-backed and clean (megabytes per family
/// that never count against the app). Leaked because fonts are never removed.
/// A font file truncated in place while mapped would crash the app on its next
/// read; replacing the file (a new inode, as copies and installers do) is safe.
fn map_font(path: &std::path::Path) -> Option<&'static [u8]> {
    let file = std::fs::File::open(path).ok()?;
    let map = unsafe { memmap2::Mmap::map(&file) }.ok()?;
    Some(&**Box::leak(Box::new(map)))
}

static FONTS_ADDED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How many times fonts were added while running: text shaped before may now
/// fall back to other glyphs.
pub fn fonts_added() -> u64 {
    FONTS_ADDED.load(std::sync::atomic::Ordering::Relaxed)
}

/// The terminal font, or the bundled default when this machine lacks it:
/// the platform's own fallback is proportional and breaks the grid.
pub fn terminal_font() -> String {
    let family = get().font_family.clone();
    match FONTS.lock().unwrap().as_ref() {
        Some(have) if !have.contains(&family.to_lowercase()) => DEFAULT_FONT.into(),
        _ => family,
    }
}

/// Fonts Herdr ships, so the defaults look the same everywhere.
pub const BUNDLED_FONTS: &[&[u8]] = &[
    include_bytes!("../assets/fonts/GeistMono-Variable.ttf"),
    include_bytes!("../assets/fonts/GeistMono-Italic-Variable.ttf"),
    include_bytes!("../assets/fonts/Geist.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Variable.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Italic-Variable.ttf"),
    SYMBOLS_FONT_FILE,
];

/// The symbols font itself: terminals read icons' glyphs from it directly.
pub const SYMBOLS_FONT_FILE: &[u8] = include_bytes!("../assets/fonts/SymbolsNerdFont-Regular.ttf");

/// Nerd Font icons for terminals, under any font (MIT, nerd-fonts).
pub const SYMBOLS_FONT: &str = "Symbols Nerd Font";

/// Terminals' font unless a layout or settings.json picks another.
pub const DEFAULT_FONT: &str = "JetBrains Mono";

#[cfg(test)]
mod tests {
    #[test]
    fn terminal_ligatures_default_off_and_can_be_enabled() {
        use super::*;
        let person: Person = serde_json::from_str("{}").unwrap();
        assert!(!Settings::compose(&person, &crate::layout::Look::default(), true).font_ligatures);
        assert!(!Settings::default().font_ligatures);
        let person: Person = serde_json::from_str(r#"{"font_ligatures":true}"#).unwrap();
        let saved = serde_json::to_string(&person).unwrap();
        let restored: Person = serde_json::from_str(&saved).unwrap();
        assert!(Settings::compose(&restored, &crate::layout::Look::default(), true).font_ligatures);
        assert!(serde_json::from_str::<Person>(r#"{"font_ligatures":"yes"}"#).is_err());
    }

    use super::*;

    #[test]
    fn font_thicken_defaults_auto_at_ghosttys_strength_and_validates_it() {
        let default: Person = serde_json::from_str("{}").unwrap();
        assert_eq!(default.font_thicken, Thicken::Auto);
        assert_eq!(default.font_thicken_strength, 255);
        assert_eq!(serde_json::to_value(&default).unwrap().get("font_thicken"), None);
        assert_eq!(serde_json::to_value(&default).unwrap().get("font_thicken_strength"), None);
        for strength in [0, 30, 255, 256] {
            let person: Person = serde_json::from_value(serde_json::json!({"font_thicken": true, "font_thicken_strength": strength})).unwrap();
            let settings = Settings::compose(&person, &crate::layout::Look::default(), true);
            assert_eq!(settings.font_thicken, Thicken::On);
            assert_eq!(settings.font_thicken_strength, strength);
            assert_eq!(settings.problems().is_empty(), strength <= 255, "strength {strength}");
        }
        assert_eq!(Thicken::On.glyph(30), gpui::GlyphThicken::Strength(30));
        assert_eq!(Thicken::On.glyph(256), gpui::GlyphThicken::Strength(255));
    }

    #[test]
    fn font_thicken_is_auto_true_or_false_and_names_anything_else() {
        let cases = [
            (serde_json::json!("auto"), Thicken::Auto, gpui::GlyphThicken::Auto, None),
            (serde_json::json!(true), Thicken::On, gpui::GlyphThicken::Strength(30), Some(serde_json::json!(true))),
            (serde_json::json!(false), Thicken::Off, gpui::GlyphThicken::Off, Some(serde_json::json!(false))),
            (serde_json::json!("on"), Thicken::Unknown("on".into()), gpui::GlyphThicken::Auto, Some(serde_json::json!("on"))),
        ];
        for (value, thicken, glyph, saved) in cases {
            let person: Person = serde_json::from_value(serde_json::json!({"font_thicken": value, "font_thicken_strength": 30})).unwrap();
            assert_eq!(person.font_thicken, thicken, "{value}");
            assert_eq!(serde_json::to_value(&person).unwrap().get("font_thicken"), saved.as_ref(), "{value}");
            let settings = Settings::compose(&person, &crate::layout::Look::default(), true);
            assert_eq!(settings.font_thicken.glyph(settings.font_thicken_strength), glyph, "{value}");
            assert_eq!(settings.problems(), match thicken {
                Thicken::Unknown(_) => vec![r#"font_thicken "on" is not "auto", true or false"#.to_string()],
                _ => Vec::new(),
            });
        }
        assert!(serde_json::from_str::<Person>(r#"{"font_thicken":1}"#).is_err());
    }

    #[test]
    fn layout_look_wins_and_settings_fill_what_it_leaves_unset() {
        let person = Person {
            font: Some("person".into()), font_size: Some(19.0), line_height: Some(1.0), ui_font: Some("person-ui".into()),
            theme: Some(crate::layout::ThemeChoice::One("paper".into())),
            colors: [("accent".into(), "#123456".into())].into(),
            ..Person::default()
        };
        let look = crate::layout::Look {
            font: Some("layout".into()), font_size: Some(14.0), line_height: Some(1.3), ui_font: Some("layout-ui".into()),
            theme: Some(crate::layout::ThemeChoice::One("ink".into())),
            colors: Some([("accent".into(), "#abcdef".into()), ("text".into(), "#ffffff".into())].into()),
            ..Default::default()
        };
        let inherited = Settings::compose(&person, &crate::layout::Look::default(), true);
        assert_eq!((inherited.font_family.as_str(), inherited.font_size, inherited.ui_font_family.as_str(), inherited.theme.as_str()), ("person", 19.0, "person-ui", "paper"));
        assert_eq!(inherited.colors["accent"], "#123456");
        assert_eq!(inherited.line_height, 1.0);
        let own = Settings::compose(&person, &look, true);
        assert_eq!((own.font_family.as_str(), own.font_size, own.ui_font_family.as_str(), own.theme.as_str()), ("layout", 14.0, "layout-ui", "ink"));
        assert_eq!(own.line_height, 1.3);
        assert_eq!(own.colors["accent"], "#abcdef");
        assert_eq!(own.colors["text"], "#ffffff");
    }

    #[test]
    fn option_as_alt_defaults_to_auto_and_round_trips() {
        let default: Person = serde_json::from_str("{}").unwrap();
        assert_eq!(default.option_as_alt, OptionAsAlt::Auto);
        for (value, expected) in [("auto", OptionAsAlt::Auto), ("off", OptionAsAlt::Off), ("on", OptionAsAlt::On), ("left", OptionAsAlt::Left), ("right", OptionAsAlt::Right)] {
            let person: Person = serde_json::from_value(serde_json::json!({"option_as_alt": value})).unwrap();
            let settings = Settings::compose(&person, &crate::layout::Look::default(), true);
            assert_eq!(settings.option_as_alt, expected);
            assert_eq!(serde_json::to_value(settings).unwrap()["option_as_alt"], value);
        }
        assert!(serde_json::from_value::<Person>(serde_json::json!({"option_as_alt": "invalid"})).is_err());
    }

    #[test]
    fn font_weight_defaults_to_medium_and_validates_the_range() {
        let default: Person = serde_json::from_str("{}").unwrap();
        assert_eq!(Settings::compose(&default, &crate::layout::Look::default(), true).font_weight, 500);
        assert!(serde_json::to_value(&default).unwrap().get("font_weight").is_none());
        for weight in [100, 300, 400, 900, 50, 950] {
            let person: Person = serde_json::from_value(serde_json::json!({"font_weight": weight})).unwrap();
            let settings = Settings::compose(&person, &crate::layout::Look::default(), true);
            assert_eq!(settings.font_weight, weight);
            assert_eq!(settings.problems().is_empty(), (100..=900).contains(&weight));
        }
        let look = crate::layout::Look { font_weight: Some(300), ..Default::default() };
        let person = Person { font_weight: Some(400), ..Person::default() };
        assert_eq!(Settings::compose(&person, &look, true).font_weight, 300);
    }

    #[test]
    fn font_weight_bold_defaults_to_700_or_the_weight_and_validates_the_range() {
        let compose = |person: serde_json::Value, look: crate::layout::Look| Settings::compose(&serde_json::from_value(person).unwrap(), &look, true);
        let default = compose(serde_json::json!({}), Default::default());
        assert_eq!((default.font_weight_bold, default.bold_weight()), (None, 700));
        assert!(serde_json::to_value(Person::default()).unwrap().get("font_weight_bold").is_none());
        for (weight, bold) in [(250, 700), (700, 700), (800, 800)] {
            assert_eq!(compose(serde_json::json!({"font_weight": weight}), Default::default()).bold_weight(), bold);
        }
        for bold in [100, 550, 900, 50, 950] {
            let settings = compose(serde_json::json!({"font_weight": 250, "font_weight_bold": bold}), Default::default());
            assert_eq!(settings.font_weight_bold, Some(bold));
            assert_eq!(settings.problems().is_empty(), (100..=900).contains(&bold));
        }
        assert_eq!(compose(serde_json::json!({"font_weight_bold": 550}), Default::default()).bold_weight(), 550);
        let look = crate::layout::Look { font_weight_bold: Some(600), ..Default::default() };
        assert_eq!(compose(serde_json::json!({"font_weight_bold": 550}), look).bold_weight(), 600);
    }

    #[test]
    fn letter_spacing_defaults_to_zero_and_validates_the_range() {
        let default: Person = serde_json::from_str("{}").unwrap();
        assert_eq!(default.letter_spacing, None);
        assert_eq!(Settings::default().letter_spacing, 0.0);
        assert!(serde_json::to_value(&default).unwrap().get("letter_spacing").is_none());
        for spacing in [-2.0, -0.2, 0.0, 1.5, 10.0, -2.5, 10.5] {
            let person: Person = serde_json::from_value(serde_json::json!({"letter_spacing": spacing})).unwrap();
            let settings = Settings::compose(&person, &crate::layout::Look::default(), true);
            assert_eq!(settings.letter_spacing, spacing);
            assert_eq!(settings.problems().is_empty(), (-2.0..=10.0).contains(&spacing));
        }
        let person = Person { letter_spacing: Some(1.0), ..Person::default() };
        let look = crate::layout::Look { letter_spacing: Some(-0.5), ..Default::default() };
        assert_eq!(Settings::compose(&person, &crate::layout::Look::default(), true).letter_spacing, 1.0);
        assert_eq!(Settings::compose(&person, &look, true).letter_spacing, -0.5);
    }

    #[test]
    fn cell_width_defaults_to_the_font_advance() {
        let default: Person = serde_json::from_str("{}").unwrap();
        assert_eq!(default.cell_width, CellWidth::Font);
        assert!(serde_json::to_value(&default).unwrap().get("cell_width").is_none());
        let person: Person = serde_json::from_value(serde_json::json!({"cell_width": "pixel"})).unwrap();
        assert_eq!(Settings::compose(&person, &crate::layout::Look::default(), true).cell_width, CellWidth::Pixel);
        assert!(serde_json::from_value::<Person>(serde_json::json!({"cell_width": "round"})).is_err());
    }

    #[test]
    fn text_rendering_defaults_to_grayscale_rgb_and_takes_each_value() {
        let default: Person = serde_json::from_str("{}").unwrap();
        let saved = serde_json::to_value(&default).unwrap();
        for key in ["text_rendering", "ui_text_rendering", "subpixel_order"] {
            assert_eq!(saved.get(key), None, "{key} is left out while default");
        }
        let settings = Settings::compose(&default, &crate::layout::Look::default(), true);
        assert_eq!((settings.text_rendering, settings.ui_text_rendering, settings.subpixel_order), (TextRendering::Grayscale, TextRendering::Grayscale, SubpixelOrder::Rgb));
        let person: Person = serde_json::from_value(serde_json::json!({"text_rendering": "subpixel", "subpixel_order": "bgr"})).unwrap();
        let settings = Settings::compose(&person, &crate::layout::Look::default(), true);
        assert_eq!((settings.text_rendering, settings.ui_text_rendering, settings.subpixel_order), (TextRendering::Subpixel, TextRendering::Grayscale, SubpixelOrder::Bgr));
        let person: Person = serde_json::from_value(serde_json::json!({"ui_text_rendering": "subpixel"})).unwrap();
        assert_eq!(Settings::compose(&person, &crate::layout::Look::default(), true).ui_text_rendering, TextRendering::Subpixel);
        for bad in [serde_json::json!({"text_rendering": "lcd"}), serde_json::json!({"ui_text_rendering": true}), serde_json::json!({"subpixel_order": "vrgb"})] {
            assert!(serde_json::from_value::<Person>(bad.clone()).is_err(), "{bad} is refused");
        }
    }

    #[test]
    fn text_contrast_defaults_off_and_validates_both() {
        let default: Person = serde_json::from_str("{}").unwrap();
        assert_eq!((default.text_contrast, default.ui_text_contrast), (0.0, 0.0));
        let saved = serde_json::to_value(&default).unwrap();
        assert_eq!((saved.get("text_contrast"), saved.get("ui_text_contrast")), (None, None));
        for (value, ok) in [(0.0, true), (0.5, true), (1.0, true), (-0.1, false), (1.1, false)] {
            let person: Person = serde_json::from_value(serde_json::json!({"text_contrast": value})).unwrap();
            let settings = Settings::compose(&person, &crate::layout::Look::default(), true);
            assert_eq!((settings.text_contrast, settings.ui_text_contrast), (value, 0.0));
            assert_eq!(settings.problems().is_empty(), ok, "text_contrast {value}");
            let person: Person = serde_json::from_value(serde_json::json!({"ui_text_contrast": value})).unwrap();
            let settings = Settings::compose(&person, &crate::layout::Look::default(), true);
            assert_eq!((settings.text_contrast, settings.ui_text_contrast), (0.0, value));
            assert_eq!(settings.problems().is_empty(), ok, "ui_text_contrast {value}");
        }
    }

    #[test]
    fn faint_opacity_defaults_to_ghosttys_half_and_validates() {
        let default: Person = serde_json::from_str("{}").unwrap();
        assert_eq!(default.faint_opacity, 0.5);
        assert_eq!(serde_json::to_value(&default).unwrap().get("faint_opacity"), None);
        assert_eq!(Settings::default().faint_alpha(), 0.5);
        for (value, alpha, ok) in [(0.0, 0.0, true), (0.6, 0.6, true), (1.0, 1.0, true), (-0.1, 0.0, false), (1.5, 1.0, false)] {
            let person: Person = serde_json::from_value(serde_json::json!({"faint_opacity": value})).unwrap();
            let settings = Settings::compose(&person, &crate::layout::Look::default(), true);
            assert_eq!((settings.faint_opacity, settings.faint_alpha()), (value, alpha));
            assert_eq!(settings.problems().is_empty(), ok, "faint_opacity {value}");
        }
    }

    #[test]
    fn font_fallbacks_default_empty_and_keep_their_order() {
        let default: Person = serde_json::from_str("{}").unwrap();
        assert!(default.font_fallbacks.is_empty());
        assert_eq!(serde_json::to_value(&default).unwrap().get("font_fallbacks"), None);
        let person: Person = serde_json::from_value(serde_json::json!({"font_fallbacks": ["ZedMono Nerd Font Mono", "Menlo"]})).unwrap();
        let settings = Settings::compose(&person, &crate::layout::Look::default(), true);
        assert_eq!(settings.font_fallbacks.unwrap().fallback_list(), ["ZedMono Nerd Font Mono", "Menlo"]);
        assert_eq!(Settings::compose(&default, &crate::layout::Look::default(), true).font_fallbacks, None);
    }

    #[test]
    fn unfocused_dim_defaults_off_and_validates() {
        let default: Person = serde_json::from_str("{}").unwrap();
        assert_eq!(default.unfocused_dim, 0.0);
        assert_eq!(serde_json::to_value(&default).unwrap().get("unfocused_dim"), None);
        for (value, ok) in [(0.0, true), (0.4, true), (1.0, true), (-0.1, false), (1.1, false)] {
            let person: Person = serde_json::from_value(serde_json::json!({"unfocused_dim": value})).unwrap();
            let settings = Settings::compose(&person, &crate::layout::Look::default(), true);
            assert_eq!(settings.unfocused_dim, value);
            assert_eq!(settings.problems().is_empty(), ok, "unfocused_dim {value}");
        }
    }

    #[test]
    fn unfocused_dim_scope_defaults_to_the_window_and_refuses_others() {
        let default: Person = serde_json::from_str("{}").unwrap();
        assert_eq!(default.unfocused_dim_scope, DimScope::Window);
        assert_eq!(serde_json::to_value(&default).unwrap().get("unfocused_dim_scope"), None);
        for (value, scope) in [("window", DimScope::Window), ("text", DimScope::Text)] {
            let person: Person = serde_json::from_value(serde_json::json!({"unfocused_dim_scope": value})).unwrap();
            assert_eq!(Settings::compose(&person, &crate::layout::Look::default(), true).unfocused_dim_scope, scope);
        }
        assert_eq!(serde_json::to_value(Person { unfocused_dim_scope: DimScope::Text, ..Person::default() }).unwrap()["unfocused_dim_scope"], "text");
        for bad in [serde_json::json!({"unfocused_dim_scope": "panels"}), serde_json::json!({"unfocused_dim_scope": true})] {
            assert!(serde_json::from_value::<Person>(bad.clone()).is_err(), "{bad} is refused");
        }
    }

    #[test]
    fn unfocused_dim_color_is_a_hex_color_or_the_background() {
        let default: Person = serde_json::from_str("{}").unwrap();
        assert_eq!(serde_json::to_value(&default).unwrap().get("unfocused_dim_color"), None);
        let settings = Settings::compose(&default, &crate::layout::Look::default(), true);
        assert_eq!((settings.dim_color(), settings.problems().is_empty()), (None, true));
        for (value, ok) in [("#4d4d4d", true), ("#FFFFFF", true), ("4d4d4d", false), ("#4d4d4d80", false), ("#4d4", false), ("grey", false), ("#zzzzzz", false), ("", false)] {
            let person: Person = serde_json::from_value(serde_json::json!({"unfocused_dim_color": value})).unwrap();
            let settings = Settings::compose(&person, &crate::layout::Look::default(), true);
            assert_eq!(settings.dim_color(), ok.then(|| parse_hex(value).unwrap()), "{value}");
            assert_eq!(settings.problems().iter().any(|p| p.contains("unfocused_dim_color")), !ok, "{value}");
        }
    }

    #[test]
    fn ui_font_weight_defaults_to_normal_and_shifts_only_the_panels() {
        let default: Person = serde_json::from_str("{}").unwrap();
        assert_eq!(default.ui_font_weight, 400);
        assert_eq!(serde_json::to_value(&default).unwrap().get("ui_font_weight"), None);
        let d = Settings::compose(&default, &crate::layout::Look::default(), true);
        assert_eq!((d.ui_font_weight, d.ui_weight_shift()), (400, 0.0));
        for (weight, shift, ok) in [(99, -300.0, false), (100, -300.0, true), (550, 150.0, true), (900, 500.0, true), (901, 500.0, false)] {
            let person: Person = serde_json::from_value(serde_json::json!({"ui_font_weight": weight})).unwrap();
            let settings = Settings::compose(&person, &crate::layout::Look::default(), true);
            assert_eq!((settings.ui_font_weight, settings.ui_weight_shift()), (weight, shift));
            assert_eq!(settings.font_weight, 500, "the terminals keep theirs");
            assert_eq!(settings.problems().is_empty(), ok, "weight {weight}");
        }
    }

    #[test]
    fn ui_font_thicken_defaults_auto_at_full_strength_apart_from_the_terminals() {
        let default: Person = serde_json::from_str("{}").unwrap();
        assert_eq!(default.ui_font_thicken, Thicken::Auto);
        assert_eq!(default.ui_font_thicken_strength, 255);
        assert_eq!(serde_json::to_value(&default).unwrap().get("ui_font_thicken"), None);
        assert_eq!(serde_json::to_value(&default).unwrap().get("ui_font_thicken_strength"), None);
        let d = Settings::compose(&default, &crate::layout::Look::default(), true);
        assert_eq!((d.ui_font_thicken.clone(), d.ui_glyph_thicken()), (Thicken::Auto, gpui::GlyphThicken::Auto));
        for strength in [0, 30, 255, 256] {
            let person: Person = serde_json::from_value(serde_json::json!({"ui_font_thicken": true, "ui_font_thicken_strength": strength})).unwrap();
            let settings = Settings::compose(&person, &crate::layout::Look::default(), true);
            assert_eq!((settings.ui_font_thicken.clone(), settings.font_thicken.clone()), (Thicken::On, Thicken::Auto));
            assert_eq!(settings.ui_font_thicken_strength, strength);
            assert_eq!(settings.ui_glyph_thicken(), gpui::GlyphThicken::Strength(strength.min(255) as u8));
            assert_eq!(settings.problems().is_empty(), strength <= 255, "strength {strength}");
        }
        let terminal: Person = serde_json::from_value(serde_json::json!({"font_thicken": true})).unwrap();
        assert_eq!(Settings::compose(&terminal, &crate::layout::Look::default(), true).ui_glyph_thicken(), gpui::GlyphThicken::Auto);
    }

    #[test]
    fn ui_font_thicken_takes_auto_true_or_false_as_font_thicken_does() {
        for (value, glyph) in [(serde_json::json!("auto"), gpui::GlyphThicken::Auto), (serde_json::json!(true), gpui::GlyphThicken::Strength(192)), (serde_json::json!(false), gpui::GlyphThicken::Off)] {
            let person: Person = serde_json::from_value(serde_json::json!({"ui_font_thicken": value, "ui_font_thicken_strength": 192})).unwrap();
            let saved = serde_json::to_value(&person).unwrap();
            assert_eq!(saved.get("ui_font_thicken"), (value != "auto").then_some(&value), "{value}");
            let settings = Settings::compose(&person, &crate::layout::Look::default(), true);
            assert_eq!(settings.ui_glyph_thicken(), glyph, "{value}");
            assert_eq!(settings.font_thicken.glyph(settings.font_thicken_strength), gpui::GlyphThicken::Auto, "the terminals keep theirs");
            assert!(settings.problems().is_empty(), "{value}");
        }
        let person: Person = serde_json::from_value(serde_json::json!({"ui_font_thicken": "thin"})).unwrap();
        let settings = Settings::compose(&person, &crate::layout::Look::default(), true);
        assert_eq!(settings.ui_glyph_thicken(), gpui::GlyphThicken::Auto);
        assert_eq!(settings.problems(), vec![r#"ui_font_thicken "thin" is not "auto", true or false"#.to_string()]);
    }

    #[test]
    fn minimum_contrast_defaults_off_and_validates_the_ratio() {
        let default: Person = serde_json::from_str("{}").unwrap();
        assert_eq!(default.minimum_contrast, 1.0);
        assert_eq!(Settings::default().minimum_contrast, 1.0);
        for ratio in [1.0, 1.6, 21.0, 0.0, 21.1] {
            let person: Person = serde_json::from_value(serde_json::json!({"minimum_contrast": ratio})).unwrap();
            let settings = Settings::compose(&person, &crate::layout::Look::default(), true);
            assert_eq!(settings.minimum_contrast, ratio);
            assert_eq!(serde_json::to_value(person).unwrap()["minimum_contrast"], serde_json::json!(ratio));
            assert_eq!(settings.problems().is_empty(), (1.0..=21.0).contains(&ratio));
        }
    }

    #[test]
    fn terminal_line_height_keeps_its_default_and_rejects_extremes() {
        let default = Settings::compose(&Person::default(), &crate::layout::Look::default(), true);
        assert_eq!(default.line_height, 1.15);
        assert!(default.problems().is_empty());
        let tight = Person { line_height: Some(0.5), ..Person::default() };
        let problems = Settings::compose(&tight, &crate::layout::Look::default(), true).problems();
        assert!(problems.iter().any(|p| p.contains("line_height")), "{problems:?}");
    }

    #[test]
    fn program_clipboard_writes_default_on_and_can_be_disabled() {
        let default: Person = serde_json::from_str("{}").unwrap();
        assert!(default.clipboard_write);
        let off: Person = serde_json::from_str(r#"{"clipboard_write":false}"#).unwrap();
        assert!(!Settings::compose(&off, &crate::layout::Look::default(), true).clipboard_write);
        assert_eq!(serde_json::to_value(off).unwrap()["clipboard_write"], false);
    }

    #[test]
    fn shipped_looks_and_theme_clicks_have_the_same_precedence() {
        use crate::layout::{Node, ThemeChoice};
        let person = Person { font_size: Some(19.0), theme: Some(ThemeChoice::One("moss".into())), ..Person::default() };
        let main: Node = serde_json::from_str(include_str!("../defaults/layouts/main.json")).unwrap();
        assert!(main.look.theme.is_none() && main.look.colors.is_none() && main.look.font.is_none() && main.look.font_size.is_none() && main.look.ui_font.is_none());
        assert_eq!(Settings::compose(&Person::default(), &main.look, true).font_size, 14.0);
        assert_eq!(main.look.terminal_padding, Some([14.0, 12.0]));
        let terminals: Node = serde_json::from_str(include_str!("../defaults/layouts/terminals.json")).unwrap();
        assert_eq!(Settings::compose(&person, &terminals.look, true).font_size, 15.0);
        let main_settings = Settings::compose(&person, &main.look, true);
        assert_eq!((main_settings.font_size, main_settings.theme.as_str()), (19.0, "moss"));
        let mut win98: Node = serde_json::from_str(include_str!("../defaults/layouts/win98.json")).unwrap();
        let win_settings = Settings::compose(&person, &win98.look, true);
        assert_eq!(win_settings.theme, "win98");
        assert_eq!(win_settings.font_family, "PxPlus IBM VGA 8x16");
        let clicks = crate::layout_sync::State { theme: Some(ThemeChoice::One("ink".into())), ..Default::default() };
        clicks.apply(&mut win98);
        assert_eq!(Settings::compose(&person, &win98.look, true).theme, "ink");
    }

    fn scaled(zoom: f32, ui_zoom: f32, ui_text_scale: f32) -> Settings {
        let person = Person { zoom, ui_zoom, ui_text_scale, ..Person::default() };
        Settings::compose(&person, &crate::layout::Look::default(), true)
    }

    #[test]
    fn ui_scales_default_to_one_and_stay_out_of_settings_json() {
        let person = Person::default();
        assert_eq!((person.ui_zoom, person.ui_text_scale), (1.0, 1.0));
        let json = serde_json::to_value(&person).unwrap();
        assert!(json.get("ui_zoom").is_none() && json.get("ui_text_scale").is_none(), "{json}");
        let read: Person = serde_json::from_str(r#"{"zoom": 1.3}"#).unwrap();
        assert_eq!((read.ui_zoom, read.ui_text_scale), (1.0, 1.0));
    }

    /// At 1 and 1, every size is the zoom-only one: `v * zoom`, bit for bit.
    #[test]
    fn ui_scales_at_one_draw_every_size_as_zoom_alone() {
        for zoom in [0.5, 0.9, 1.0, 1.1, 1.3, 1.7, 2.0, 3.0] {
            let s = scaled(zoom, 1.0, 1.0);
            assert_eq!(s.ui_scale().to_bits(), zoom.to_bits());
            for v in [0.5, 1.0, 6.5, 9.75, 12.5, 13.0, 16.0, 240.0] {
                assert_eq!(s.ui_px(v).to_bits(), (v * zoom).to_bits(), "z({v}) at zoom {zoom}");
                assert_eq!(s.ui_text_px(v).to_bits(), (v * zoom).to_bits(), "zt({v}) at zoom {zoom}");
            }
            assert_eq!(s.ui_font_size_px().to_bits(), (13.0 * zoom).to_bits());
            assert_eq!(s.terminal_font_size().to_bits(), (14.0 * zoom).to_bits());
            assert_eq!(s.terminal_padding_px(), (10.0 * zoom, 6.0 * zoom));
            assert_eq!(s.mod_scales(), (zoom, 1.0));
        }
    }

    #[test]
    fn ui_zoom_scales_the_panels_but_not_the_terminals() {
        let (plain, bigger) = (scaled(1.1, 1.0, 1.0), scaled(1.1, 1.2, 1.0));
        assert_eq!(bigger.terminal_font_size(), plain.terminal_font_size());
        assert_eq!(bigger.terminal_padding_px(), plain.terminal_padding_px());
        assert!((bigger.ui_px(10.0) - 10.0 * 1.1 * 1.2).abs() < 1e-4);
        assert!((bigger.ui_font_size_px() - 13.0 * 1.1 * 1.2).abs() < 1e-4);
        assert!((bigger.mod_scales().0 - 1.1 * 1.2).abs() < 1e-6);
        assert_eq!(bigger.mod_scales().1, 1.0);
    }

    #[test]
    fn ui_text_scale_scales_text_only() {
        let (plain, bigger) = (scaled(1.0, 1.2, 1.0), scaled(1.0, 1.2, 1.1));
        assert_eq!(bigger.ui_px(10.0), plain.ui_px(10.0), "spacing keeps its size");
        assert_eq!(bigger.terminal_font_size(), plain.terminal_font_size());
        assert!((bigger.ui_text_px(12.0) - 12.0 * 1.2 * 1.1).abs() < 1e-4);
        assert!((bigger.ui_font_size_px() - 13.0 * 1.2 * 1.1).abs() < 1e-4);
        assert_eq!(bigger.mod_scales(), (plain.mod_scales().0, 1.1));
    }

    /// The window's rem keeps rem spacing at the panels' zoom; only text takes the text scale.
    #[test]
    fn the_rem_leaves_the_text_scale_to_the_text() {
        for zoom in [1.0, 1.1, 1.3, 2.0] {
            let s = scaled(zoom, 1.0, 1.0);
            assert_eq!(s.ui_rem_px().to_bits(), (13.0 * zoom).to_bits());
            assert_eq!(s.text_rems_px(0.75).to_bits(), (0.75 * s.ui_rem_px()).to_bits(), "text_xs at zoom {zoom}");
            assert_eq!(s.text_rems_px(1.0).to_bits(), s.ui_font_size_px().to_bits());
        }
        let (plain, bigger) = (scaled(1.0, 1.2, 1.0), scaled(1.0, 1.2, 1.25));
        assert_eq!(bigger.ui_rem_px(), plain.ui_rem_px(), "rem spacing keeps its size");
        assert!((bigger.text_rems_px(0.75) - 0.75 * 13.0 * 1.2 * 1.25).abs() < 1e-4);
        assert!((bigger.ui_font_size_px() - 13.0 * 1.2 * 1.25).abs() < 1e-4);
    }

    #[test]
    fn the_panels_zoom_stops_at_the_largest_zoom() {
        assert_eq!(scaled(3.0, 1.5, 1.0).ui_scale(), ZOOM_MAX);
        assert_eq!(scaled(2.5, 1.5, 1.0).ui_scale(), ZOOM_MAX);
        assert!((scaled(1.5, 1.5, 1.0).ui_scale() - 2.25).abs() < 1e-6);
        assert_eq!(scaled(3.0, 1.5, 1.0).mod_scales().0, ZOOM_MAX);
    }

    #[test]
    fn out_of_range_ui_scales_are_problems() {
        assert!(scaled(1.0, 1.5, 0.8).problems().is_empty(), "the ends are in range");
        let problems = scaled(1.0, 2.0, 0.5).problems();
        assert!(problems.iter().any(|p| p.starts_with("ui_zoom 2 is outside 0.75..1.5: using 1.5")), "{problems:?}");
        assert!(problems.iter().any(|p| p.starts_with("ui_text_scale 0.5 is outside 0.8..1.3: using 0.8")), "{problems:?}");
        assert!(scaled(1.0, f32::NAN, 1.0).problems().iter().any(|p| p.contains("ui_zoom NaN")));
    }

    /// Mods divide window px by `ui_scale`, and the Settings window steps within the ranges.
    #[test]
    fn mods_get_the_panels_zoom_and_the_scale_ranges() {
        let v = scaled(1.5, 1.2, 1.1).for_mods();
        assert!((v["ui_scale"].as_f64().unwrap() - 1.8).abs() < 1e-6, "{v}");
        let range = |key: &str| serde_json::from_value::<(f32, f32)>(v["ui_scale_ranges"][key].clone()).unwrap();
        assert_eq!((range("ui_zoom"), range("ui_text_scale")), (UI_ZOOM_RANGE, UI_TEXT_SCALE_RANGE));
        assert!(v.get("asked_ui_scales").is_none());
        assert_eq!(v["zoom"], serde_json::json!(1.5));
    }

    #[test]
    fn ui_scales_are_clamped() {
        for (asked, ui_zoom, text) in [(0.1, 0.75, 0.8), (9.0, 1.5, 1.3), (f32::NAN, 1.0, 1.0), (f32::INFINITY, 1.0, 1.0), (1.25, 1.25, 1.25)] {
            let s = scaled(1.0, asked, asked);
            assert_eq!((s.ui_zoom, s.ui_text_scale), (ui_zoom, text), "asked {asked}");
        }
    }

    #[test]
    fn focus_follows_mouse_is_off_by_default_and_stays_out_of_settings_json() {
        let person = Person::default();
        assert!(!person.focus_follows_mouse);
        assert_eq!(person.focus_follows_mouse_delay(), std::time::Duration::from_millis(150));
        let json = serde_json::to_value(&person).unwrap();
        assert!(json.get("focus_follows_mouse").is_none() && json.get("focus_follows_mouse_delay_ms").is_none(), "{json}");
        let on: Person = serde_json::from_str(r#"{"focus_follows_mouse": true, "focus_follows_mouse_delay_ms": 400}"#).unwrap();
        assert!(on.focus_follows_mouse);
        assert_eq!(on.focus_follows_mouse_delay(), std::time::Duration::from_millis(400));
        let json = serde_json::to_value(&on).unwrap();
        assert_eq!((json["focus_follows_mouse"].as_bool(), json["focus_follows_mouse_delay_ms"].as_u64()), (Some(true), Some(400)));
        assert!(serde_json::from_str::<Person>(r#"{"focus_follows_mouse": "yes"}"#).is_err());
    }

    #[test]
    fn focus_follows_mouse_delay_is_clamped() {
        for (asked, ms) in [(0, 0), (2000, 2000), (2001, 2000), (u32::MAX, 2000)] {
            let person = Person { focus_follows_mouse_delay_ms: asked, ..Person::default() };
            assert_eq!(person.focus_follows_mouse_delay(), std::time::Duration::from_millis(ms), "asked {asked}");
        }
    }

    #[test]
    fn default_arrange_is_off_by_default_and_unknown_modes_are_none() {
        use crate::arrange::Mode;
        let person: Person = serde_json::from_str("{}").unwrap();
        assert_eq!(person.default_arrange(), None);
        assert!(serde_json::to_value(&person).unwrap().get("default_arrange").is_none());
        let bsp: Person = serde_json::from_str(r#"{"default_arrange": "bsp"}"#).unwrap();
        assert_eq!(bsp.default_arrange(), Some(Mode::Bsp));
        for odd in [r#"{"default_arrange": "spiral"}"#, r#"{"default_arrange": "next"}"#, r#"{"default_arrange": "off"}"#] {
            assert_eq!(serde_json::from_str::<Person>(odd).unwrap().default_arrange(), None, "{odd}");
        }
    }

    /// The guide agents read (written to the home folder) explains every
    /// settings.json key.
    #[test]
    fn the_guide_explains_every_setting() {
        let all = Person { core: Some(String::new()), font: Some(String::new()), font_size: Some(1.0), line_height: Some(1.0), font_weight: Some(400), font_weight_bold: Some(550), font_thicken: Thicken::Off, font_thicken_strength: 30, ui_font_thicken: Thicken::On, ui_font_thicken_strength: 30, text_rendering: TextRendering::Subpixel, ui_text_rendering: TextRendering::Subpixel, subpixel_order: SubpixelOrder::Bgr, letter_spacing: Some(1.0), cell_width: CellWidth::Pixel, ui_font: Some(String::new()), ui_font_weight: 600, ui_zoom: 1.2, ui_text_scale: 1.1, telemetry: false, clipboard_write: false, appearance: Appearance::Light, theme: Some(crate::layout::ThemeChoice::One(String::new())), colors: [("text".into(), String::new())].into(), system_notifications: SystemNotifications::Off, dock_bounce: true, resume_agents: ResumeAgents::Off, alert_command: Some(AlertCommand::Shell(String::new())), focus_follows_mouse: true, focus_follows_mouse_delay_ms: 300, default_arrange: Some("bsp".into()), unfocused_dim: 0.3, unfocused_dim_scope: DimScope::Text, unfocused_dim_color: Some("#4d4d4d".into()), sidebar_shade: SidebarShade::Active, panel_colors: PanelColors::Classic, ..Person::default() };
        let keys: Vec<String> = serde_json::to_value(&all).unwrap().as_object().unwrap().keys().cloned().collect();
        let guide = include_str!("../defaults/AGENTS.md");
        let section = guide.split("## The person").nth(1).and_then(|s| s.split("\n## ").next()).unwrap_or_default();
        let missing: Vec<&String> = keys.iter().filter(|k| !section.contains(&format!("`{k}`"))).collect();
        assert!(missing.is_empty(), "defaults/AGENTS.md (## The person) doesn't explain: {missing:?}");
    }
}
