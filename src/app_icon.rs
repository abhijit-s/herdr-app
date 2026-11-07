//! The app icon (settings.json `app_icon`): a built-in variant of Herdr's
//! artwork (scripts/app-icons.py) or an image file. macOS shows it in the Dock
//! from start-up and whenever it changes; installs from a checkout also write
//! it into the bundle or launcher (scripts/app-icon).

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
        let ext = std::path::Path::new(&v).extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
        if (v.starts_with('/') || v.starts_with("~/")) && ext.is_some_and(|e| FILE_TYPES.contains(&e.as_str())) {
            return Ok(AppIcon::File(v));
        }
        let names: Vec<_> = BUILT_IN.iter().map(|(name, _)| *name).collect();
        Err(format!("app_icon {v:?} is not one of {} or an absolute or ~/ path to an .icns, .png or .svg file", names.join(", ")))
    }
}

impl From<AppIcon> for String {
    fn from(icon: AppIcon) -> String {
        match icon {
            AppIcon::File(path) => path,
            built_in => BUILT_IN.iter().find(|(_, i)| *i == built_in).map(|(name, _)| name.to_string()).unwrap_or_default(),
        }
    }
}

#[cfg(target_os = "macos")]
thread_local! {
    /// What the Dock shows, once set: None until an icon loads.
    static SHOWN: std::cell::RefCell<Option<AppIcon>> = const { std::cell::RefCell::new(None) };
}

/// Shows `icon` in the Dock unless it already is. Settings arrive on the main
/// thread; anywhere else (tests) this does nothing. A file that can't be read
/// or decoded is tried again with the next settings, so one made afterwards
/// still shows.
#[cfg(target_os = "macos")]
pub fn follow(icon: &AppIcon) {
    use objc2::AllocAnyThread as _;
    let Some(mtm) = objc2::MainThreadMarker::new() else { return };
    if SHOWN.with_borrow(|shown| shown.as_ref() == Some(icon)) {
        return;
    }
    let bytes = match icon.bytes() {
        Ok(bytes) => bytes,
        Err(e) => return tracing::warn!("app_icon {}: {e}", String::from(icon.clone())),
    };
    let data = objc2_foundation::NSData::with_bytes(&bytes);
    let Some(image) = objc2_app_kit::NSImage::initWithData(objc2_app_kit::NSImage::alloc(), &data) else {
        return tracing::warn!("app_icon {}: not an image macOS can read", String::from(icon.clone()));
    };
    // SAFETY: on the main thread (`mtm`), with an image AppKit itself decoded.
    unsafe { objc2_app_kit::NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&image)) };
    SHOWN.set(Some(icon.clone()));
}

#[cfg(not(target_os = "macos"))]
pub fn follow(_icon: &AppIcon) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_icon_reads_names_and_image_paths() {
        for (name, icon) in BUILT_IN {
            assert_eq!(AppIcon::try_from(name.to_string()), Ok(icon.clone()));
            assert_eq!(String::from(icon), name);
        }
        for path in ["~/icons/mine.icns", "/tmp/a.PNG", "/x y/z.svg"] {
            assert_eq!(AppIcon::try_from(path.to_string()), Ok(AppIcon::File(path.into())));
        }
        for bad in ["blueprint", "Mirage", "custom", "mine.icns", "./x.svg", "icons/x.png", "~user/x.png", "/tmp/a.jpg", ""] {
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
    fn icns_alpha(icon: &AppIcon) -> std::collections::BTreeMap<isize, Vec<u8>> {
        use objc2_app_kit::{NSBitmapFormat, NSBitmapImageRep};
        let data = objc2_foundation::NSData::with_bytes(&icon.bytes().unwrap());
        let mut sizes = std::collections::BTreeMap::new();
        for rep in NSBitmapImageRep::imageRepsWithData(&data) {
            let Ok(rep) = rep.downcast::<NSBitmapImageRep>() else { continue };
            let (px, row, step) = (rep.pixelsWide(), rep.bytesPerRow() as usize, rep.bitsPerPixel() as usize / 8);
            assert!(!rep.isPlanar() && rep.bitsPerSample() == 8 && rep.hasAlpha(), "{px} px: an 8-bit RGBA bitmap");
            let alpha_at = if rep.bitmapFormat().contains(NSBitmapFormat::AlphaFirst) { 0 } else { step - 1 };
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
            let sizes: Vec<_> = icns_alpha(&icon).into_keys().collect();
            assert_eq!(sizes, [16, 32, 64, 128, 256, 512, 1024], "{name}");
        }
    }

    /// Each drawn variant (scripts/app-icons.py) sits on macOS's icon grid, as
    /// the system draws other apps' icons beside it in the Dock and the app
    /// switcher: an opaque tile 824 of 1024 px across with 100 px margins, at
    /// every size, and nothing in the margins more than a soft shadow. Classic
    /// is upstream's own .icns and isn't drawn here.
    #[cfg(target_os = "macos")]
    #[test]
    fn drawn_variants_sit_on_the_macos_icon_grid() {
        for (name, icon) in BUILT_IN.into_iter().filter(|(_, icon)| !icon.is_classic()) {
            for (px, alpha) in icns_alpha(&icon) {
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
                let outside = (0..n).flat_map(|y| (0..n).map(move |x| (x, y))).filter(|&(x, y)| x < x0 || x > x1 || y < y0 || y > y1);
                let densest = outside.map(|(x, y)| alpha[y * n + x]).max().unwrap_or(0);
                assert!(densest < 80, "{name} at {px} px: a margin pixel is {densest}/255 opaque");
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
        follow(&AppIcon::Mirage);
        assert_eq!(SHOWN.with_borrow(|shown| shown.clone()), None);
    }
}
