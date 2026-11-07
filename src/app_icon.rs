//! The app icon (settings.json `app_icon`): a built-in variant of Herdr's
//! artwork (scripts/app-icons.py), the artwork in the theme's colours, or an
//! image file. macOS shows it in the Dock from start-up and whenever it
//! changes; installs from a checkout also write a built-in or a file into the
//! bundle or launcher (scripts/app-icon).

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum AppIcon {
    #[default]
    Classic,
    Midnight,
    Mirage,
    Glass,
    Phosphor,
    Graphite,
    /// Drawn at run time from the theme showing (`ThemeColors`), so it
    /// changes with the theme, light and dark included.
    Theme,
    /// An .icns, .png or .svg, by an absolute path or one from `~/` (the
    /// home folder): Herdr and the install scripts run from different folders,
    /// so a relative path would name a different file in each.
    File(String),
}

const BUILT_IN: [(&str, AppIcon); 6] = [
    ("classic", AppIcon::Classic),
    ("midnight", AppIcon::Midnight),
    ("mirage", AppIcon::Mirage),
    ("glass", AppIcon::Glass),
    ("phosphor", AppIcon::Phosphor),
    ("graphite", AppIcon::Graphite),
];
const THEME: &str = "theme";
const FILE_TYPES: [&str; 3] = ["icns", "png", "svg"];

impl AppIcon {
    pub fn is_classic(&self) -> bool {
        *self == AppIcon::Classic
    }

    /// The image: built-ins are in the binary, so any build shows them.
    #[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
    pub fn bytes(&self) -> std::io::Result<std::borrow::Cow<'static, [u8]>> {
        let built_in: &'static [u8] = match self {
            AppIcon::Classic => include_bytes!("../assets/icons/Herdr.icns"),
            AppIcon::Midnight => include_bytes!("../assets/icons/variants/midnight.icns"),
            AppIcon::Mirage => include_bytes!("../assets/icons/variants/mirage.icns"),
            AppIcon::Glass => include_bytes!("../assets/icons/variants/glass.icns"),
            AppIcon::Phosphor => include_bytes!("../assets/icons/variants/phosphor.icns"),
            AppIcon::Graphite => include_bytes!("../assets/icons/variants/graphite.icns"),
            AppIcon::File(path) => return std::fs::read(expand_home(path)).map(Into::into),
            AppIcon::Theme => return Err(std::io::Error::other("the theme icon is drawn from the theme (theme_icns)")),
        };
        Ok(built_in.into())
    }
}

#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn expand_home(path: &str) -> std::path::PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => std::env::home_dir().unwrap_or_default().join(rest),
        None => path.into(),
    }
}

impl TryFrom<String> for AppIcon {
    type Error = String;

    fn try_from(v: String) -> Result<Self, String> {
        if let Some((_, icon)) = BUILT_IN.iter().find(|(name, _)| *name == v) {
            return Ok(icon.clone());
        }
        if v == THEME {
            return Ok(AppIcon::Theme);
        }
        let ext = std::path::Path::new(&v).extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
        if (v.starts_with('/') || v.starts_with("~/")) && ext.is_some_and(|e| FILE_TYPES.contains(&e.as_str())) {
            return Ok(AppIcon::File(v));
        }
        let names: Vec<_> = BUILT_IN.iter().map(|(name, _)| *name).collect();
        Err(format!("app_icon {v:?} is not one of {}, {THEME} or an absolute or ~/ path to an .icns, .png or .svg file", names.join(", ")))
    }
}

impl From<AppIcon> for String {
    fn from(icon: AppIcon) -> String {
        match icon {
            AppIcon::File(path) => path,
            AppIcon::Theme => THEME.to_owned(),
            built_in => BUILT_IN.iter().find(|(_, i)| *i == built_in).map(|(name, _)| name.to_string()).unwrap_or_default(),
        }
    }
}

/// The "theme" icon's colours, each a role in its template
/// (assets/icons/variants/theme.svg, from scripts/app-icons.py): the tile, a
/// vertical gradient from `tile_top`; a line `edge` just inside it; and the
/// artwork, a diagonal gradient from `glyph_from`. Equal colours draw the same
/// icon, so they are what the Dock's icon is cached and compared by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ThemeColors {
    pub tile_top: u32,
    pub tile_bottom: u32,
    pub edge: u32,
    pub light: bool,
    pub glyph_from: u32,
    pub glyph_to: u32,
}

impl ThemeColors {
    /// From a theme's background, its terminal foreground and its accent.
    /// The tile is the background, lifted toward the foreground at the top of
    /// a dark theme and shaded toward it at the bottom of a light one, which
    /// keeps a light tile apart from a light Dock. The artwork is the accent,
    /// fading toward the foreground: the accent is what tells themes apart.
    /// Both ends are moved toward the foreground's side until they read on
    /// the whole tile (3:1, the foreground itself 4.5:1).
    pub fn new(background: u32, foreground: u32, accent: u32) -> ThemeColors {
        use crate::settings::{is_light, mix};
        let light = is_light(background);
        let (tile_top, tile_bottom) = if light { (mix(background, 0xffffff, 0.4), mix(background, foreground, 0.16)) } else { (mix(background, foreground, 0.12), mix(background, 0, 0.35)) };
        let tile = [tile_top, tile_bottom];
        let ink = readable(foreground, tile, if light { 0 } else { 0xffffff }, 4.5);
        let glyph_from = readable(accent, tile, ink, 3.0);
        ThemeColors { tile_top, tile_bottom, edge: foreground, light, glyph_from, glyph_to: readable(mix(glyph_from, ink, 0.45), tile, ink, 3.0) }
    }

    /// From a resolved theme (settings.rs): `background`, `terminal_foreground`
    /// and `accent`. Focus only changes `chrome`, so it never redraws the icon.
    pub fn of(theme: &crate::settings::Theme) -> ThemeColors {
        let get = |k: &str, or: u32| theme.get(k).and_then(|v| u32::from_str_radix(v.trim_start_matches('#').get(..6)?, 16).ok()).unwrap_or(or);
        ThemeColors::new(get("background", 0x17191d), get("terminal_foreground", 0xd2d4d9), get("accent", 0x96b2d7))
    }

    /// `template` with its `{field}`s filled.
    fn svg(&self, template: &str) -> String {
        let hex = |c: u32| format!("#{c:06x}");
        [
            ("{tile_top}", hex(self.tile_top)),
            ("{tile_bottom}", hex(self.tile_bottom)),
            ("{edge}", hex(self.edge)),
            ("{edge_opacity}", (if self.light { ".22" } else { ".14" }).to_owned()),
            ("{glyph_from}", hex(self.glyph_from)),
            ("{glyph_to}", hex(self.glyph_to)),
        ]
        .iter()
        .fold(template.to_owned(), |svg, (field, value)| svg.replace(field, value))
    }
}

/// `c` moved toward `toward` in twentieths until it reads at `minimum` on every one of `on`.
fn readable(c: u32, on: [u32; 2], toward: u32, minimum: f32) -> u32 {
    use crate::settings::{contrast, mix};
    (0..=20).map(|step| mix(c, toward, step as f32 / 20.0)).find(|m| on.iter().all(|bg| contrast(*m, *bg) >= minimum)).unwrap_or(toward)
}

const THEME_SVG: &str = include_str!("../assets/icons/variants/theme.svg");
const THEME_SMALL_SVG: &str = include_str!("../assets/icons/variants/theme-small.svg");
/// Up to this size the plain template draws it, as scripts/app-icons.py's SMALL_PX.
const SMALL_PX: u32 = 32;
/// Each size of an .icns: its type and pixels across, as iconutil writes
/// them (16 and 32 as PNG rather than its raw ARGB).
const ICNS_SIZES: [(&[u8; 4], u32); 10] =
    [(b"icp4", 16), (b"ic11", 32), (b"icp5", 32), (b"ic12", 64), (b"ic07", 128), (b"ic13", 256), (b"ic08", 256), (b"ic14", 512), (b"ic09", 512), (b"ic10", 1024)];

/// The theme icon at `px` across, as a PNG.
fn theme_png(colors: &ThemeColors, px: u32) -> Vec<u8> {
    use resvg::{tiny_skia, usvg};
    let svg = colors.svg(if px <= SMALL_PX { THEME_SMALL_SVG } else { THEME_SVG });
    let tree = usvg::Tree::from_str(&svg, &usvg::Options::default()).expect("the theme templates are valid SVG");
    let mut pixmap = tiny_skia::Pixmap::new(px, px).expect("a nonzero size");
    let scale = px as f32 / tree.size().width();
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    let rgba: Vec<u8> = pixmap.pixels().iter().flat_map(|p| {
        let c = p.demultiply();
        [c.red(), c.green(), c.blue(), c.alpha()]
    }).collect();
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, px, px);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    encoder.write_header().and_then(|mut w| w.write_image_data(&rgba)).expect("PNG into memory");
    out
}

/// The theme icon as an .icns with every size macOS draws an app icon at,
/// each drawn at its own size (scripts/app-icons.py's `icns`).
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub fn theme_icns(colors: &ThemeColors) -> Vec<u8> {
    let mut pngs: std::collections::BTreeMap<u32, Vec<u8>> = std::collections::BTreeMap::new();
    let mut body = Vec::new();
    for (kind, px) in ICNS_SIZES {
        let png = pngs.entry(px).or_insert_with(|| theme_png(colors, px));
        body.extend_from_slice(kind);
        body.extend_from_slice(&(png.len() as u32 + 8).to_be_bytes());
        body.extend_from_slice(png);
    }
    let mut icns = b"icns".to_vec();
    icns.extend_from_slice(&(body.len() as u32 + 8).to_be_bytes());
    icns.extend(body);
    icns
}

/// What the Dock shows, or was last asked to.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
enum Shown {
    Icon(AppIcon),
    Theme(ThemeColors),
}

impl Shown {
    #[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
    fn of(icon: &AppIcon, theme: &crate::settings::Theme) -> Shown {
        match icon {
            AppIcon::Theme => Shown::Theme(ThemeColors::of(theme)),
            other => Shown::Icon(other.clone()),
        }
    }
}

/// Theme icons drawn so far, newest first (switching between a light and a
/// dark theme shows one again without drawing it), and the one being drawn.
#[derive(Default)]
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
struct ThemeIcons {
    drawn: Vec<(ThemeColors, std::sync::Arc<Vec<u8>>)>,
    drawing: Option<ThemeColors>,
}

#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
const THEME_ICONS_KEPT: usize = 4;

#[derive(Debug, PartialEq)]
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
enum Next {
    Show(std::sync::Arc<Vec<u8>>),
    Draw,
    /// It is being drawn already.
    Wait,
}

#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
impl ThemeIcons {
    fn ask(&mut self, colors: ThemeColors) -> Next {
        if let Some(i) = self.drawn.iter().position(|(c, _)| *c == colors) {
            let hit = self.drawn.remove(i);
            let bytes = hit.1.clone();
            self.drawn.insert(0, hit);
            return Next::Show(bytes);
        }
        if self.drawing == Some(colors) {
            return Next::Wait;
        }
        self.drawing = Some(colors);
        Next::Draw
    }

    fn drawn(&mut self, colors: ThemeColors, icns: std::sync::Arc<Vec<u8>>) {
        if self.drawing == Some(colors) {
            self.drawing = None;
        }
        self.drawn.retain(|(c, _)| *c != colors);
        self.drawn.insert(0, (colors, icns));
        self.drawn.truncate(THEME_ICONS_KEPT);
    }
}

#[cfg(target_os = "macos")]
thread_local! {
    /// What the Dock shows, once set: None until an icon loads.
    static SHOWN: std::cell::RefCell<Option<Shown>> = const { std::cell::RefCell::new(None) };
    /// What the settings last asked for: a theme icon drawn after the theme
    /// moved on isn't shown.
    static WANTED: std::cell::RefCell<Option<Shown>> = const { std::cell::RefCell::new(None) };
    static THEME_ICONS: std::cell::RefCell<ThemeIcons> = std::cell::RefCell::new(ThemeIcons::default());
}

/// Shows `icon` in the Dock unless it already is; the theme icon in `theme`'s
/// colours. Settings arrive on the main thread; anywhere else (tests) this
/// does nothing. A file that can't be read or decoded is tried again with the
/// next settings, so one made afterwards still shows. A theme icon not drawn
/// yet is drawn on its own thread (tens of milliseconds) and shown from the
/// main thread when done; until then the Dock keeps what it shows.
#[cfg(target_os = "macos")]
pub fn follow(icon: &AppIcon, theme: &crate::settings::Theme) {
    let Some(mtm) = objc2::MainThreadMarker::new() else { return };
    let want = Shown::of(icon, theme);
    WANTED.set(Some(want.clone()));
    if SHOWN.with_borrow(|shown| shown.as_ref() == Some(&want)) {
        return;
    }
    match want {
        Shown::Icon(icon) => match icon.bytes() {
            Ok(bytes) => show(mtm, Shown::Icon(icon), &bytes),
            Err(e) => tracing::warn!("app_icon {}: {e}", String::from(icon)),
        },
        Shown::Theme(colors) => match THEME_ICONS.with_borrow_mut(|icons| icons.ask(colors)) {
            Next::Show(icns) => show(mtm, Shown::Theme(colors), &icns),
            Next::Wait => {}
            Next::Draw => {
                let drawing = std::thread::Builder::new().name("app-icon".into()).spawn(move || {
                    let start = std::time::Instant::now();
                    let icns = std::sync::Arc::new(theme_icns(&colors));
                    tracing::debug!("app_icon theme: drawn in {:?}", start.elapsed());
                    dispatch2::DispatchQueue::main().exec_async(move || theme_icon_drawn(colors, icns));
                });
                if let Err(e) = drawing {
                    THEME_ICONS.with_borrow_mut(|icons| icons.drawing = None);
                    tracing::warn!("app_icon theme: {e}");
                }
            }
        },
    }
}

/// Back on the main thread with a drawn theme icon: kept, and shown if the
/// settings still ask for it.
#[cfg(target_os = "macos")]
fn theme_icon_drawn(colors: ThemeColors, icns: std::sync::Arc<Vec<u8>>) {
    let Some(mtm) = objc2::MainThreadMarker::new() else { return };
    THEME_ICONS.with_borrow_mut(|icons| icons.drawn(colors, icns.clone()));
    let want = Shown::Theme(colors);
    if WANTED.with_borrow(|wanted| wanted.as_ref() == Some(&want)) && SHOWN.with_borrow(|shown| shown.as_ref() != Some(&want)) {
        show(mtm, want, &icns);
    }
}

#[cfg(target_os = "macos")]
fn show(mtm: objc2::MainThreadMarker, shown: Shown, bytes: &[u8]) {
    use objc2::AllocAnyThread as _;
    let data = objc2_foundation::NSData::with_bytes(bytes);
    let Some(image) = objc2_app_kit::NSImage::initWithData(objc2_app_kit::NSImage::alloc(), &data) else {
        let name = match shown {
            Shown::Icon(icon) => String::from(icon),
            Shown::Theme(_) => THEME.to_owned(),
        };
        return tracing::warn!("app_icon {name}: not an image macOS can read");
    };
    // SAFETY: on the main thread (`mtm`), with an image AppKit itself decoded.
    unsafe { objc2_app_kit::NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&image)) };
    SHOWN.set(Some(shown));
}

#[cfg(not(target_os = "macos"))]
pub fn follow(_icon: &AppIcon, _theme: &crate::settings::Theme) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_icon_reads_names_and_image_paths() {
        for (name, icon) in BUILT_IN {
            assert_eq!(AppIcon::try_from(name.to_string()), Ok(icon.clone()));
            assert_eq!(String::from(icon), name);
        }
        assert_eq!(AppIcon::try_from("theme".to_string()), Ok(AppIcon::Theme));
        assert_eq!(String::from(AppIcon::Theme), "theme");
        assert!(AppIcon::Theme.bytes().is_err(), "drawn, not read");
        for path in ["~/icons/mine.icns", "/tmp/a.PNG", "/x y/z.svg"] {
            assert_eq!(AppIcon::try_from(path.to_string()), Ok(AppIcon::File(path.into())));
        }
        for bad in ["blueprint", "Mirage", "Theme", "themes", "custom", "mine.icns", "./x.svg", "icons/x.png", "~user/x.png", "/tmp/a.jpg", ""] {
            assert!(AppIcon::try_from(bad.to_string()).is_err(), "{bad:?}");
        }
    }

    /// Each built-in is its own .icns, and classic is the bundle's icon.
    #[test]
    fn built_in_icons_are_distinct_icns_files() {
        let all: Vec<_> = BUILT_IN.iter().map(|(_, icon)| icon.bytes().unwrap().into_owned()).collect();
        for (i, bytes) in all.iter().enumerate() {
            assert_eq!(&bytes[..4], b"icns", "{}", BUILT_IN[i].0);
            assert!(all[..i].iter().all(|other| other != bytes), "{} repeats another", BUILT_IN[i].0);
        }
        assert_eq!(all[0], std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/Herdr.icns")).unwrap());
    }

    #[test]
    fn a_file_icon_reads_the_file_with_home_expanded() {
        let dir = std::env::temp_dir().join(format!("herdr-app-icon-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mine.png");
        std::fs::write(&path, b"png bytes").unwrap();
        assert_eq!(&*AppIcon::File(path.to_string_lossy().into()).bytes().unwrap(), b"png bytes");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(AppIcon::File("/no/such/icon.png".into()).bytes().is_err());
        assert_eq!(expand_home("~/x.png"), std::env::home_dir().unwrap().join("x.png"));
    }

    /// AppKit decodes each built-in into an image with its largest size, 512 pt
    /// (1024 px), as `follow` hands it to the Dock.
    #[cfg(target_os = "macos")]
    #[test]
    fn appkit_reads_every_built_in() {
        use objc2::AllocAnyThread as _;
        for (name, icon) in BUILT_IN {
            let data = objc2_foundation::NSData::with_bytes(&icon.bytes().unwrap());
            let image = objc2_app_kit::NSImage::initWithData(objc2_app_kit::NSImage::alloc(), &data).unwrap_or_else(|| panic!("{name}"));
            let size = image.size();
            assert_eq!((size.width, size.height), (512.0, 512.0), "{name}");
        }
    }

    /// The sizes in a built-in's .icns as AppKit decodes them: pixels across ->
    /// the alpha of every pixel, row by row.
    #[cfg(target_os = "macos")]
    fn icns_alpha(icns: &[u8]) -> std::collections::BTreeMap<isize, Vec<u8>> {
        use objc2_app_kit::{NSBitmapFormat, NSBitmapImageRep};
        let data = objc2_foundation::NSData::with_bytes(icns);
        let mut sizes = std::collections::BTreeMap::new();
        for rep in NSBitmapImageRep::imageRepsWithData(&data) {
            let Ok(rep) = rep.downcast::<NSBitmapImageRep>() else { continue };
            let (px, row, step) = (rep.pixelsWide(), rep.bytesPerRow() as usize, rep.bitsPerPixel() as usize / 8);
            let format = rep.bitmapFormat();
            assert!(rep.pixelsHigh() == px && !rep.isPlanar() && rep.bitsPerSample() == 8 && rep.hasAlpha(), "{px} px: a square 8-bit bitmap with alpha");
            assert!((format & !(NSBitmapFormat::AlphaFirst | NSBitmapFormat::AlphaNonpremultiplied)).is_empty(), "{px} px: bytes in RGBA or ARGB order");
            let alpha_at = if format.contains(NSBitmapFormat::AlphaFirst) { 0 } else { step - 1 };
            // SAFETY: the rep owns `row` bytes for each of its `px` rows while it lives.
            let bytes = unsafe { std::slice::from_raw_parts(rep.bitmapData(), row * px as usize) };
            let alpha = (0..px as usize).flat_map(|y| (0..px as usize).map(move |x| bytes[y * row + x * step + alpha_at])).collect();
            sizes.insert(px, alpha);
        }
        sizes
    }

    /// macOS draws an app icon at 16 to 1024 px; each built-in's .icns holds
    /// every one of those sizes, so none is scaled from another.
    #[cfg(target_os = "macos")]
    #[test]
    fn every_built_in_has_every_icon_size() {
        for (name, icon) in BUILT_IN {
            let sizes: Vec<_> = icns_alpha(&icon.bytes().unwrap()).into_keys().collect();
            assert_eq!(sizes, [16, 32, 64, 128, 256, 512, 1024], "{name}");
        }
        for (name, colors) in sample_themes() {
            let sizes: Vec<_> = icns_alpha(&theme_icns(&colors)).into_keys().collect();
            assert_eq!(sizes, [16, 32, 64, 128, 256, 512, 1024], "theme {name}");
        }
    }

    /// Each drawn variant (scripts/app-icons.py) sits on macOS's icon grid, as
    /// the system draws other apps' icons beside it in the Dock and the app
    /// switcher: an opaque tile 824 of 1024 px across with 100 px margins, at
    /// every size, with the system's corners and shadow, and nothing in the
    /// margins more than that soft shadow. Classic
    /// is upstream's own .icns and isn't drawn here.
    #[cfg(target_os = "macos")]
    #[test]
    fn drawn_variants_sit_on_the_macos_icon_grid() {
        let drawn = BUILT_IN.into_iter().filter(|(_, icon)| !icon.is_classic()).map(|(name, icon)| (name.to_owned(), icon.bytes().unwrap().into_owned()));
        let themed = sample_themes().into_iter().map(|(name, colors)| (format!("theme {name}"), theme_icns(&colors)));
        for (name, icns) in drawn.chain(themed) {
            for (px, alpha) in icns_alpha(&icns) {
                let n = px as usize;
                let solid = |x: usize, y: usize| alpha[y * n + x] > 127;
                let rows: Vec<_> = (0..n).filter(|&y| (0..n).any(|x| solid(x, y))).collect();
                let cols: Vec<_> = (0..n).filter(|&x| (0..n).any(|y| solid(x, y))).collect();
                let (x0, y0, x1, y1) = (cols[0], rows[0], cols[cols.len() - 1], rows[rows.len() - 1]);
                let margins = [x0, y0, n - 1 - x1, n - 1 - y1];
                let (want, slack) = (100.0 * px as f64 / 1024.0, 1.0 + 3.0 * px as f64 / 1024.0);
                for margin in margins {
                    assert!((margin as f64 - want).abs() <= slack, "{name} at {px} px: margins {margins:?}, want {want:.1}");
                }
                // Past the edge's one antialiased pixel, so a re-render shifting it by a count doesn't fail.
                let outside = (0..n).flat_map(|y| (0..n).map(move |x| (x, y))).filter(|&(x, y)| x + 1 < x0 || x > x1 + 1 || y + 1 < y0 || y > y1 + 1);
                let densest = outside.map(|(x, y)| alpha[y * n + x]).max().unwrap_or(0);
                assert!(densest < 80, "{name} at {px} px: a margin pixel is {densest}/255 opaque");
                if px == 1024 {
                    // The corners are the system's superellipse, not circular arcs (row 110
                    // starts at x 254 and row 200 at x 133 in macOS's own icons), and the
                    // shadow below the tile is there (44/255 just under it).
                    let first = |y: usize| (0..n).find(|&x| solid(x, y)).unwrap();
                    for (y, want) in [(110, 254), (200, 133)] {
                        assert!(first(y).abs_diff(want) <= 2, "{name}: row {y} starts at x {}, want {want}", first(y));
                    }
                    let under = alpha[(y1 + 2) * n + n / 2];
                    assert!((30..60).contains(&under), "{name}: {under}/255 under the tile, want the system's shadow");
                }
            }
        }
    }

    /// SVG files decode too (AppKit's own SVG support), as the docs promise.
    #[cfg(target_os = "macos")]
    #[test]
    fn appkit_reads_an_svg_file() {
        use objc2::AllocAnyThread as _;
        let svg = AppIcon::File(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/variants/mirage.svg").into());
        let data = objc2_foundation::NSData::with_bytes(&svg.bytes().unwrap());
        assert!(objc2_app_kit::NSImage::initWithData(objc2_app_kit::NSImage::alloc(), &data).is_some());
    }

    /// Off the main thread (where tests run) following an icon changes nothing.
    #[cfg(target_os = "macos")]
    #[test]
    fn following_off_the_main_thread_does_nothing() {
        follow(&AppIcon::Mirage, &crate::settings::Theme::new());
        follow(&AppIcon::Theme, &crate::settings::Theme::new());
        assert_eq!(SHOWN.with_borrow(|shown| shown.clone()), None);
    }

    /// (name, background, terminal foreground, accent) of themes at the
    /// extremes: Herdr's presets, pure black and white, a pastel, a light
    /// theme whose accent barely shows on it, and one whose accent is its
    /// background.
    const THEMES: [(&str, u32, u32, u32); 8] = [
        ("graphite", 0x17191d, 0xd2d4d9, 0x96b2d7),
        ("paper", 0xffffff, 0x25272c, 0x1f62d0),
        ("modus-vivendi", 0x000000, 0xffffff, 0x2fafff),
        ("modus-operandi", 0xffffff, 0x000000, 0x0031a9),
        ("dracula", 0x282a36, 0xf8f8f2, 0xbd93f9),
        ("rose-pine-dawn", 0xfaf4ed, 0x575279, 0x56949f),
        ("pale-accent", 0xfdf6e3, 0x657b83, 0xf5e9c8),
        ("accent-is-background", 0x1f2430, 0xcccac2, 0x1f2430),
    ];

    fn sample_themes() -> Vec<(&'static str, ThemeColors)> {
        THEMES.iter().map(|&(name, bg, fg, accent)| (name, ThemeColors::new(bg, fg, accent))).collect()
    }

    /// The artwork reads on every part of the tile; a dark tile is lifted at
    /// the top and a light one shaded at the bottom, toward its foreground.
    #[test]
    fn theme_colors_keep_the_artwork_readable_on_the_tile() {
        use crate::settings::{contrast, is_light};
        for &(name, bg, fg, accent) in &THEMES {
            let c = ThemeColors::new(bg, fg, accent);
            assert_eq!(c.light, is_light(bg), "{name}");
            assert_eq!(c.edge, fg, "{name}");
            for glyph in [c.glyph_from, c.glyph_to] {
                for tile in [c.tile_top, c.tile_bottom] {
                    assert!(contrast(glyph, tile) >= 3.0, "{name}: {glyph:06x} on {tile:06x}");
                }
            }
            if c.light {
                assert!(contrast(c.tile_bottom, fg) < contrast(bg, fg), "{name}: a light tile shades toward its ink");
            } else {
                assert!(contrast(c.tile_top, fg) < contrast(bg, fg), "{name}: a dark tile lifts toward its ink");
            }
            assert_eq!(ThemeColors::new(bg, fg, accent), c, "{name}: the same palette, the same colours");
        }
        // An accent that already reads is kept as it is.
        assert_eq!(ThemeColors::new(0x282a36, 0xf8f8f2, 0xbd93f9).glyph_from, 0xbd93f9);
    }

    /// Only the background, the terminal's foreground and the accent (a
    /// theme's own `accent` included) decide the icon: focus moving `chrome`
    /// doesn't redraw it.
    #[test]
    fn theme_colors_come_from_the_resolved_theme() {
        let theme = |pairs: &[(&str, &str)]| -> crate::settings::Theme { pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() };
        let base = [("background", "#282a36"), ("terminal_foreground", "#f8f8f2"), ("accent", "#bd93f9ff"), ("chrome", "#111111")];
        let a = ThemeColors::of(&theme(&base));
        assert_eq!(a, ThemeColors::new(0x282a36, 0xf8f8f2, 0xbd93f9));
        let mut refocused = base;
        refocused[3].1 = "#222222";
        assert_eq!(Shown::of(&AppIcon::Theme, &theme(&refocused)), Shown::Theme(a));
        let mut recoloured = base;
        recoloured[2].1 = "#ff79c6";
        assert_ne!(ThemeColors::of(&theme(&recoloured)), a);
        assert_eq!(Shown::of(&AppIcon::Mirage, &theme(&base)), Shown::Icon(AppIcon::Mirage));
    }

    /// An icon is drawn once per palette: asked again while drawing it waits,
    /// and once drawn it is shown from the cache, as is an earlier one
    /// (switching light and dark), until four newer ones push it out.
    #[test]
    fn an_unchanged_palette_is_not_drawn_again() {
        let colors: Vec<_> = sample_themes().into_iter().map(|(_, c)| c).collect();
        let icns = |i: u8| std::sync::Arc::new(vec![i]);
        let mut icons = ThemeIcons::default();
        assert_eq!(icons.ask(colors[0]), Next::Draw);
        assert_eq!(icons.ask(colors[0]), Next::Wait);
        icons.drawn(colors[0], icns(0));
        assert_eq!(icons.ask(colors[0]), Next::Show(icns(0)));
        assert_eq!(icons.ask(colors[1]), Next::Draw);
        icons.drawn(colors[1], icns(1));
        assert_eq!(icons.ask(colors[0]), Next::Show(icns(0)));
        assert_eq!(icons.ask(colors[1]), Next::Show(icns(1)));
        for (i, c) in colors[2..6].iter().enumerate() {
            assert_eq!(icons.ask(*c), Next::Draw);
            icons.drawn(*c, icns(i as u8 + 2));
        }
        assert_eq!(icons.drawn.len(), THEME_ICONS_KEPT);
        assert_eq!(icons.ask(colors[0]), Next::Draw, "the least recently shown went first");
    }

    /// The templates have every field the colours fill, and nothing left unfilled.
    #[test]
    fn theme_templates_are_filled_completely() {
        let c = ThemeColors::new(0x17191d, 0xd2d4d9, 0x96b2d7);
        for template in [THEME_SVG, THEME_SMALL_SVG] {
            for field in ["{tile_top}", "{tile_bottom}", "{edge}", "{edge_opacity}", "{glyph_from}", "{glyph_to}"] {
                assert!(template.contains(field), "{field}");
            }
            let svg = c.svg(template);
            assert!(!svg.contains('{') && svg.contains("#96b2d7"), "filled");
        }
        assert!(THEME_SVG.contains("filter=\"url(#lift)\"") && !THEME_SMALL_SVG.contains("url(#lift)"), "small sizes leave out the shadow");
    }

    /// The Settings preview (frame/settings/app-icons/theme.png) is the theme
    /// icon in Graphite, Herdr's default dark theme, at the other previews'
    /// 96 px. HERDR_WRITE_THEME_PREVIEW=1 writes it again.
    #[test]
    fn theme_preview_is_the_drawn_icon() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/defaults/frame/settings/app-icons/theme.png");
        let png = theme_png(&ThemeColors::new(0x17191d, 0xd2d4d9, 0x96b2d7), 96);
        if std::env::var_os("HERDR_WRITE_THEME_PREVIEW").is_some_and(|v| !v.is_empty()) {
            std::fs::write(path, &png).unwrap();
        }
        let decode = |bytes: &[u8]| image::load_from_memory(bytes).unwrap().to_rgba8();
        let (want, have) = (decode(&png), decode(&std::fs::read(path).unwrap()));
        assert_eq!(have.dimensions(), (96, 96));
        let off = want.pixels().zip(have.pixels()).filter(|(a, b)| a.0.iter().zip(b.0).any(|(x, y)| x.abs_diff(y) > 8)).count();
        assert!(off < 96 * 96 / 100, "{off} preview pixels differ; HERDR_WRITE_THEME_PREVIEW=1 cargo test theme_preview");
    }

    /// How long a theme icon takes to draw (every size), on the thread that
    /// draws it: `cargo test theme_icon_draw_time -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn theme_icon_draw_time() {
        let c = ThemeColors::new(0x17191d, 0xd2d4d9, 0x96b2d7);
        theme_icns(&c);
        let runs = 10;
        let start = std::time::Instant::now();
        for _ in 0..runs {
            std::hint::black_box(theme_icns(&c));
        }
        println!("theme icon .icns: {:?} each", start.elapsed() / runs);
        for px in [16, 32, 256, 1024] {
            let start = std::time::Instant::now();
            std::hint::black_box(theme_png(&c, px));
            println!("  {px} px: {:?}", start.elapsed());
        }
        let theme: crate::settings::Theme = [("background", "#17191d"), ("terminal_foreground", "#d2d4d9"), ("accent", "#96b2d7")].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let start = std::time::Instant::now();
        for _ in 0..1000 {
            std::hint::black_box(Shown::of(&AppIcon::Theme, &theme) == Shown::Theme(c));
        }
        println!("the per-settings check (colours from the theme, compared): {:?}", start.elapsed() / 1000);
    }

    /// Writes the theme icon at 256 px for each theme in THEME_ICON_SHEET
    /// (`name:background:foreground:accent,...`, hex) into THEME_ICON_SHEET_DIR.
    #[test]
    #[ignore]
    fn theme_icon_sheet() {
        let dir = std::path::PathBuf::from(std::env::var("THEME_ICON_SHEET_DIR").unwrap());
        for entry in std::env::var("THEME_ICON_SHEET").unwrap().split(',') {
            let f: Vec<_> = entry.split(':').collect();
            let hex = |s: &str| u32::from_str_radix(s.trim_start_matches('#'), 16).unwrap();
            std::fs::write(dir.join(format!("{}.png", f[0])), theme_png(&ThemeColors::new(hex(f[1]), hex(f[2]), hex(f[3])), 256)).unwrap();
        }
    }
}
