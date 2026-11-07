//! An agent's program icon (programs.rs) as a PNG for its system
//! notification: drawn once per program and look, kept under the home's
//! `cache/notification-icons/`, and copied for each notification, since the
//! notification center moves an attached file into its own store.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::Value;

/// Pixels across: notification thumbnails show at most about 80 points.
const PX: u32 = 192;
/// How much of a tile the glyph fills, and of the picture without one.
const GLYPH_ON_TILE: f32 = 0.6;
const GLYPH_ALONE: f32 = 0.85;
/// A tile's corner radius, as a fraction of its side (the agents panel's look).
const TILE_RADIUS: f32 = 0.22;
/// A one-color glyph without a color of its own: legible on light and dark.
const GLYPH_FALLBACK: [u8; 3] = [0x80, 0x80, 0x80];
/// Copies the notification center never took (a failed post) are swept after this.
const OUTGOING_KEEP: Duration = Duration::from_secs(600);
/// Bumped when the drawing changes, so older PNGs aren't reused.
const DRAWING_VERSION: u32 = 1;

#[cfg(not(test))]
pub fn dir() -> PathBuf {
    crate::mods::app_dir().join("cache").join("notification-icons")
}

/// Tests that post agent alerts stay out of the person's home.
#[cfg(test)]
pub fn dir() -> PathBuf {
    std::env::temp_dir().join(format!("herdr-test-notification-icons-{}", std::process::id()))
}

/// The cached PNG of the icon programs.json shows for an agent's 7501 `app`,
/// drawn now if it isn't there yet; `None` when the program has no icon or it
/// can't be read.
pub fn cached(app: &str) -> Option<PathBuf> {
    cached_in(&dir(), &crate::programs::of(Some(app), &[]), &crate::mods::app_dir())
}

/// `shown` is a programs.rs entry as `programs::of` shows it; its `icon` is a
/// path in `home`.
fn cached_in(cache: &Path, shown: &Value, home: &Path) -> Option<PathBuf> {
    let icon = home.join(shown.get("icon")?.as_str().filter(|i| !i.is_empty())?);
    let bytes = std::fs::read(&icon).ok()?;
    let look = Look::of(shown);
    let key = safe(shown.get("key").and_then(Value::as_str).unwrap_or("program"));
    let file = cache.join(format!("{key}-{:016x}.png", fingerprint(&bytes, &look)));
    if file.is_file() {
        return Some(file);
    }
    let png = match icon.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("svg") => draw(&bytes, &look)?,
        Some("png") => bytes,
        _ => return None,
    };
    std::fs::create_dir_all(cache).ok()?;
    crate::persist::write_atomic(&file, &png).ok()?;
    forget_older(cache, &key, &file);
    Some(file)
}

/// An icon drawn again (its file, look or drawing changed) replaces the
/// program's older PNGs.
fn forget_older(cache: &Path, key: &str, keep: &Path) {
    let Ok(entries) = std::fs::read_dir(cache) else { return };
    let prefix = format!("{key}-");
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let ours = name.to_str().and_then(|n| n.strip_prefix(&prefix)).is_some_and(|rest| rest.len() == 20 && rest.ends_with(".png"));
        if ours && path != keep {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// A copy of an image file for one notification to take (and move), under
/// the cache's `outgoing/`.
pub fn copy_for_posting(file: &Path) -> Option<PathBuf> {
    // Only macOS shows an image (GPUI's `SystemNotification::image`).
    if !cfg!(any(target_os = "macos", test)) {
        return None;
    }
    copy_into(&dir().join("outgoing"), file)
}

/// Copies older than `OUTGOING_KEEP` are swept first.
fn copy_into(outgoing: &Path, file: &Path) -> Option<PathBuf> {
    static COUNT: AtomicU64 = AtomicU64::new(0);
    std::fs::create_dir_all(outgoing).ok()?;
    sweep(outgoing, OUTGOING_KEEP);
    let n = COUNT.fetch_add(1, Ordering::Relaxed);
    let name = format!("{}-{}-{n}.{}", std::process::id(), stem(file), file.extension()?.to_str()?);
    let copy = outgoing.join(name);
    if let Err(error) = std::fs::copy(file, &copy) {
        tracing::warn!("notification image {} not copied: {error}", file.display());
        return None;
    }
    Some(copy)
}

fn stem(path: &Path) -> String {
    safe(path.file_stem().and_then(|s| s.to_str()).unwrap_or("icon"))
}

/// Removes files in `dir` last changed more than `keep` ago.
fn sweep(dir: &Path, keep: Duration) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let old = entry.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).is_some_and(|age| age > keep);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// A program key as a file name.
fn safe(key: &str) -> String {
    key.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

/// How the agents panel draws an icon: a one-color glyph or a picture, on a
/// tile or not.
#[derive(Clone, Debug, Hash, PartialEq)]
struct Look {
    tint: bool,
    tile: Option<[u8; 3]>,
    color: Option<[u8; 3]>,
}

impl Look {
    fn of(shown: &Value) -> Look {
        let rgb = |key: &str| shown.get(key).and_then(Value::as_str).and_then(hex);
        Look { tint: shown.get("tint").and_then(Value::as_bool).unwrap_or(true), tile: rgb("tile"), color: rgb("color") }
    }
}

fn hex(s: &str) -> Option<[u8; 3]> {
    let h = s.strip_prefix('#').filter(|h| h.len() == 6 && h.is_ascii())?;
    let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}

fn fingerprint(bytes: &[u8], look: &Look) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    (DRAWING_VERSION, bytes, look).hash(&mut h);
    h.finish()
}

/// An SVG icon drawn as the agents panel draws it, as a PNG.
fn draw(svg: &[u8], look: &Look) -> Option<Vec<u8>> {
    use resvg::{tiny_skia, usvg};
    let tree = usvg::Tree::from_data(svg, &usvg::Options::default()).ok()?;
    let side = PX as f32;
    let mut glyph = tiny_skia::Pixmap::new(PX, PX)?;
    let fill = if look.tile.is_some() { GLYPH_ON_TILE } else { GLYPH_ALONE } * side;
    let size = tree.size();
    let scale = fill / size.width().max(size.height());
    let (dx, dy) = ((side - size.width() * scale) / 2.0, (side - size.height() * scale) / 2.0);
    resvg::render(&tree, tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, dx, dy), &mut glyph.as_mut());
    if look.tint {
        let [r, g, b] = look.color.unwrap_or(if look.tile.is_some() { [0xff; 3] } else { GLYPH_FALLBACK });
        for px in glyph.pixels_mut() {
            let a = px.alpha();
            let premul = |c: u8| (u16::from(c) * u16::from(a) / 255) as u8;
            *px = tiny_skia::PremultipliedColorU8::from_rgba(premul(r), premul(g), premul(b), a)?;
        }
    }
    let mut out = tiny_skia::Pixmap::new(PX, PX)?;
    if let Some([r, g, b]) = look.tile {
        let path = rounded_square(side, side * TILE_RADIUS)?;
        let mut paint = tiny_skia::Paint::default();
        paint.set_color_rgba8(r, g, b, 255);
        paint.anti_alias = true;
        out.fill_path(&path, &paint, tiny_skia::FillRule::Winding, tiny_skia::Transform::identity(), None);
    }
    out.draw_pixmap(0, 0, glyph.as_ref(), &tiny_skia::PixmapPaint::default(), tiny_skia::Transform::identity(), None);
    out.encode_png().ok()
}

fn rounded_square(side: f32, r: f32) -> Option<resvg::tiny_skia::Path> {
    let mut pb = resvg::tiny_skia::PathBuilder::new();
    pb.move_to(r, 0.0);
    pb.line_to(side - r, 0.0);
    pb.quad_to(side, 0.0, side, r);
    pb.line_to(side, side - r);
    pb.quad_to(side, side, side - r, side);
    pb.line_to(r, side);
    pb.quad_to(0.0, side, 0.0, side - r);
    pb.line_to(0.0, r);
    pb.quad_to(0.0, 0.0, r, 0.0);
    pb.close();
    pb.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const GLYPH: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="1em" height="1em"><path d="M4 4h16v16H4z" fill="currentColor"/></svg>"#;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("herdr-notification-icon-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("icons")).unwrap();
        dir
    }

    fn pixel(png: &Path, x: u32, y: u32) -> [u8; 4] {
        let pixmap = resvg::tiny_skia::Pixmap::load_png(png).unwrap();
        let c = pixmap.pixel(x, y).unwrap().demultiply();
        [c.red(), c.green(), c.blue(), c.alpha()]
    }

    #[test]
    fn an_svg_glyph_is_drawn_once_tinted_on_its_tile_and_kept() {
        let home = scratch("glyph");
        std::fs::write(home.join("icons/pi.svg"), GLYPH).unwrap();
        let cache = home.join("cache");
        let shown = json!({ "key": "pi", "icon": "icons/pi.svg", "tint": true, "tile": "#1c1d21", "color": "#ffffff" });
        let png = cached_in(&cache, &shown, &home).expect("drawn");
        assert!(png.starts_with(&cache) && png.extension().unwrap() == "png", "{png:?}");
        assert_eq!(pixel(&png, PX / 2, PX / 2), [0xff, 0xff, 0xff, 0xff], "the glyph in its color");
        assert_eq!(pixel(&png, 6, PX / 2), [0x1c, 0x1d, 0x21, 0xff], "the tile around it");
        assert_eq!(pixel(&png, 0, 0)[3], 0, "the tile's corners are round");
        // Kept: the same file, not drawn again.
        let stamp = std::fs::metadata(&png).unwrap().modified().unwrap();
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(cached_in(&cache, &shown, &home), Some(png.clone()));
        assert_eq!(std::fs::metadata(&png).unwrap().modified().unwrap(), stamp);
        // Another color is another file, which replaces the older one.
        std::fs::write(cache.join("pig-0123456789abcdef.png"), b"another program's").unwrap();
        let red = json!({ "key": "pi", "icon": "icons/pi.svg", "tint": true, "tile": "#1c1d21", "color": "#ff0000" });
        let other = cached_in(&cache, &red, &home).unwrap();
        assert_ne!(other, png);
        assert_eq!(pixel(&other, PX / 2, PX / 2), [0xff, 0, 0, 0xff]);
        assert!(!png.exists(), "the older drawing is gone");
        assert!(cache.join("pig-0123456789abcdef.png").exists(), "another key's stays");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_picture_keeps_its_colors_and_a_png_is_taken_as_is() {
        let home = scratch("picture");
        std::fs::write(home.join("icons/c.svg"), GLYPH.replace("currentColor", "#D97757")).unwrap();
        let cache = home.join("cache");
        let picture = json!({ "key": "claude", "icon": "icons/c.svg", "tint": false });
        let png = cached_in(&cache, &picture, &home).unwrap();
        assert_eq!(pixel(&png, PX / 2, PX / 2), [0xd9, 0x77, 0x57, 0xff]);
        assert_eq!(pixel(&png, 2, 2)[3], 0, "no tile: transparent around it");
        std::fs::write(home.join("icons/x.png"), std::fs::read(&png).unwrap()).unwrap();
        let given = cached_in(&cache, &json!({ "key": "x", "icon": "icons/x.png" }), &home).unwrap();
        assert_eq!(std::fs::read(&given).unwrap(), std::fs::read(&png).unwrap());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn no_icon_or_an_unreadable_one_attaches_nothing() {
        let home = scratch("none");
        let cache = home.join("cache");
        assert_eq!(cached_in(&cache, &json!({ "key": "agent", "icon": null }), &home), None);
        assert_eq!(cached_in(&cache, &json!({ "key": "agent" }), &home), None);
        assert_eq!(cached_in(&cache, &json!({ "key": "gone", "icon": "icons/gone.svg" }), &home), None);
        std::fs::write(home.join("icons/bad.svg"), "not svg").unwrap();
        assert_eq!(cached_in(&cache, &json!({ "key": "bad", "icon": "icons/bad.svg" }), &home), None);
        std::fs::write(home.join("icons/a.ico"), "x").unwrap();
        assert_eq!(cached_in(&cache, &json!({ "key": "a", "icon": "icons/a.ico" }), &home), None);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn each_notification_gets_its_own_copy_and_old_copies_are_swept() {
        let home = scratch("copy");
        std::fs::write(home.join("icons/pi.svg"), GLYPH).unwrap();
        let cache = home.join("cache");
        let png = cached_in(&cache, &json!({ "key": "pi", "icon": "icons/pi.svg" }), &home).unwrap();
        let outgoing = cache.join("outgoing");
        let (a, b) = (copy_into(&outgoing, &png).unwrap(), copy_into(&outgoing, &png).unwrap());
        assert_ne!(a, b);
        assert!(a.starts_with(&outgoing));
        assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&png).unwrap());
        // The notification center takes (moves) the copy: the cached one stays.
        std::fs::remove_file(&a).unwrap();
        assert!(png.is_file());
        sweep(&cache.join("outgoing"), Duration::ZERO);
        assert!(!b.exists(), "an old copy nobody took is swept");
        assert!(png.is_file(), "the cache itself isn't swept");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn every_shipped_agent_icon_draws() {
        for key in ["claude", "codex", "pi", "opencode", "cursor", "copilot", "amp", "gemini"] {
            let shown = crate::programs::of(Some(key), &[]);
            let icon = shown["icon"].as_str().unwrap_or_else(|| panic!("{key}: no icon"));
            let file = icon.strip_prefix("icons/").unwrap();
            let svg = std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("defaults/icons").join(file)).unwrap();
            assert!(draw(&svg, &Look::of(&shown)).is_some(), "{key}: {icon} didn't draw");
        }
    }
}
