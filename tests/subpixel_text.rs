#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]
//! Trial: subpixel text smoothing, with real rasterising and GPU readback.
//! At 1x a paint that asks for subpixel glyphs (`with_glyph_subpixel`, the
//! text_rendering settings) draws a coverage per color channel; at 2x it
//! draws exactly the grayscale glyphs, and a window moving between the two
//! switches on its next frame. Prints the ink of grayscale and subpixel text
//! at the person's settings, both polarities: the mean over the text's box of
//! (pixel - background) / (text - background), per channel.
//! With HERDR_SUBPIXEL_PNG set to a folder, saves the rows there as PNGs.
use gpui::{prelude::*, *};

const TERMINAL_TEXT: &str = "fn main() { let ok = x != 0x1f; } // quick brown fox @ 0O lI1";
const PANEL_TEXT: &str = "Settings  Workspaces  Agents running: 3  Last build passed 2m ago";
const DARK: u32 = 0x1b1b1e;
const LIGHT: u32 = 0xe4e4e8;
const ROW: f32 = 18.;
const WIDTH: f32 = 520.;

#[derive(Clone, Copy)]
struct Text {
    name: &'static str,
    family: &'static str,
    text: &'static str,
    size: f32,
    thicken: Option<u8>,
    contrast: f32,
}

// The person's: terminals JetBrains Mono 12 at strength 30, contrast 0.2;
// panels Geist 13 at strength 192, contrast 0.3. Then plain text, unthickened.
const TEXTS: [Text; 4] = [
    Text { name: "terminal", family: "JetBrains Mono", text: TERMINAL_TEXT, size: 12., thicken: Some(30), contrast: 0.2 },
    Text { name: "panel", family: "Geist", text: PANEL_TEXT, size: 13., thicken: Some(192), contrast: 0.3 },
    Text { name: "terminal, plain", family: "JetBrains Mono", text: TERMINAL_TEXT, size: 12., thicken: None, contrast: 0. },
    Text { name: "panel, plain", family: "Geist", text: PANEL_TEXT, size: 13., thicken: None, contrast: 0. },
];
const POLARITIES: [(u32, u32); 2] = [(LIGHT, DARK), (DARK, LIGHT)];

/// Rows: every text in both polarities, grayscale then subpixel.
fn rows() -> Vec<(Text, u32, u32, bool)> {
    let mut rows = Vec::new();
    for text in TEXTS {
        for (fg, bg) in POLARITIES {
            for subpixel in [false, true] {
                rows.push((text, fg, bg, subpixel));
            }
        }
    }
    rows
}

fn paint_row(i: usize, text: Text, fg: u32, bg: u32, subpixel: bool, window: &mut Window, cx: &mut App) {
    let top = point(px(0.), px(ROW * i as f32));
    window.paint_quad(fill(Bounds::new(top, size(px(WIDTH), px(ROW))), rgb(bg)));
    let run = TextRun { len: text.text.len(), font: font(text.family), color: rgb(fg).into(), background_color: None, underline: None, strikethrough: None };
    let line = window.text_system().shape_line(text.text.into(), px(text.size), &[run], None);
    window.with_glyph_thicken(text.thicken, |window| {
        window.with_glyph_contrast(text.contrast, |window| {
            window.with_glyph_subpixel(subpixel, |window| line.paint(top + point(px(4.), px(0.)), px(ROW), TextAlign::Left, None, window, cx).unwrap())
        })
    });
}

struct Scene;

impl Render for Scene {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        canvas(
            |_, _, _| {},
            |_, _, window, cx| {
                for (i, (text, fg, bg, subpixel)) in rows().into_iter().enumerate() {
                    paint_row(i, text, fg, bg, subpixel, window, cx);
                }
            },
        )
        .size_full()
    }
}

struct Measure {
    ink: f32,
    /// Pixels whose channels' coverages differ by more than 0.1.
    colored: usize,
}

fn measure(image: &image::RgbaImage, scale: f32, i: usize, fg: u32, bg: u32) -> Measure {
    let channel = |c: u32, k: usize| ((c >> (16 - 8 * k)) & 0xff) as f32;
    let (y0, y1) = ((ROW * i as f32 * scale) as u32, (ROW * (i + 1) as f32 * scale) as u32);
    let (mut sum, mut n, mut colored) = (0.0, 0, 0);
    for y in y0..y1 {
        for x in 0..(WIDTH * scale) as u32 {
            let p = image.get_pixel(x, y).0;
            let a: [f32; 3] = std::array::from_fn(|k| ((p[k] as f32 - channel(bg, k)) / (channel(fg, k) - channel(bg, k))).clamp(0.0, 1.0));
            sum += a.iter().sum::<f32>() / 3.0;
            n += 1;
            let spread = a.iter().cloned().fold(0.0f32, f32::max) - a.iter().cloned().fold(1.0f32, f32::min);
            colored += (spread > 0.1) as usize;
        }
    }
    Measure { ink: sum / n as f32, colored }
}

fn draw(window: &mut Window, cx: &mut App) -> image::RgbaImage {
    let _ = window.draw(cx);
    window.render_to_image().expect("real GPU screenshot")
}

fn row_pixels(image: &image::RgbaImage, scale: f32, i: usize) -> Vec<u8> {
    let (y0, h) = ((ROW * i as f32 * scale) as u32, (ROW * scale) as u32);
    image::imageops::crop_imm(image, 0, y0, image.width(), h).to_image().into_raw()
}

fn save(image: &image::RgbaImage, dir: &std::path::Path, rows: &[(usize, &str)], name: &str) {
    let (w, h) = (image.width(), ROW as u32);
    let mut sheet = image::RgbaImage::new(w, h * rows.len() as u32);
    for (k, (i, _)) in rows.iter().enumerate() {
        let crop = image::imageops::crop_imm(image, 0, h * *i as u32, w, h).to_image();
        image::imageops::replace(&mut sheet, &crop, 0, (h * k as u32) as i64);
    }
    sheet.save(dir.join(format!("{name}.png"))).unwrap();
    let zoom = image::imageops::resize(&image::imageops::crop_imm(&sheet, 0, 0, 260, sheet.height()).to_image(), 260 * 6, sheet.height() * 6, image::imageops::FilterType::Nearest);
    zoom.save(dir.join(format!("{name}_zoom6x.png"))).unwrap();
    let labels: Vec<&str> = rows.iter().map(|(_, l)| *l).collect();
    std::fs::write(dir.join(format!("{name}.txt")), labels.join("\n") + "\n").unwrap();
}

fn check(window: &mut Window, cx: &mut App) {
    let image = draw(window, cx);
    let scale = window.scale_factor();
    assert_eq!(scale, 1.0);
    let rows = rows();
    let m: Vec<Measure> = rows.iter().enumerate().map(|(i, (_, fg, bg, _))| measure(&image, scale, i, *fg, *bg)).collect();
    println!("| text | polarity | grayscale ink | subpixel ink | difference | colored px (subpixel) |");
    println!("|---|---|---|---|---|---|");
    for (pair, rows) in m.chunks(2).zip(rows.chunks(2)) {
        let (text, fg, _, _) = rows[0];
        let polarity = if fg == DARK { "dark on light" } else { "light on dark" };
        let (gray, sub) = (&pair[0], &pair[1]);
        let difference = (sub.ink - gray.ink) / gray.ink * 100.0;
        println!("| {} | {polarity} | {:.4} | {:.4} | {difference:+.1}% | {} |", text.name, gray.ink, sub.ink, sub.colored);
        assert_eq!(gray.colored, 0, "{} {polarity}: grayscale draws no color", text.name);
        assert!(sub.colored > 200, "{} {polarity}: subpixel colors its edges ({} px)", text.name, sub.colored);
        assert!(difference.abs() < 5.0, "{} {polarity}: subpixel ink within 5% of grayscale ({difference:+.1}%)", text.name);
    }
    if let Some(dir) = std::env::var_os("HERDR_SUBPIXEL_PNG") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        save(&image, &dir, &[(0, "terminal light on dark, grayscale"), (1, "terminal light on dark, subpixel"), (2, "terminal dark on light, grayscale"), (3, "terminal dark on light, subpixel")], "terminal_before_after_1x");
        save(&image, &dir, &[(4, "panel light on dark, grayscale"), (5, "panel light on dark, subpixel"), (6, "panel dark on light, grayscale"), (7, "panel dark on light, subpixel")], "panel_before_after_1x");
        image.save(dir.join("all_rows_1x.png")).unwrap();
    }
}

/// At 2x the subpixel rows draw exactly as their grayscale twins.
fn check_retina(window: &mut Window, cx: &mut App) {
    let image = draw(window, cx);
    let scale = window.scale_factor();
    assert_eq!(scale, 2.0);
    for i in (0..rows().len()).step_by(2) {
        assert!(row_pixels(&image, scale, i) == row_pixels(&image, scale, i + 1), "row {i}: subpixel asked at 2x draws grayscale");
    }
}

fn colored(window: &mut Window, cx: &mut App) -> usize {
    let image = draw(window, cx);
    let scale = window.scale_factor();
    rows().iter().enumerate().map(|(i, (_, fg, bg, _))| measure(&image, scale, i, *fg, *bg).colored).sum()
}

fn add_fonts(cx: &mut App) {
    let fonts: [&'static [u8]; 2] = [include_bytes!("../assets/fonts/JetBrainsMono-Variable.ttf"), include_bytes!("../assets/fonts/Geist.ttf")];
    cx.text_system().add_fonts(fonts.into_iter().map(Into::into).collect()).unwrap();
}

/// CoreText's advances, which lay the line out, match the outlines swash
/// draws subpixel glyphs from.
fn check_advances(text_system: &std::sync::Arc<dyn PlatformTextSystem>) {
    let bytes: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Variable.ttf");
    let swash_font = swash::FontRef::from_index(bytes, 0).unwrap();
    let font_id = text_system.font_id(&font("JetBrains Mono")).unwrap();
    let metrics = swash_font.glyph_metrics(&[]).scale(12.0);
    let mut worst = 0f32;
    let mut n = 0;
    for ch in TERMINAL_TEXT.chars().chain(PANEL_TEXT.chars()) {
        let Some(glyph) = text_system.glyph_for_char(font_id, ch) else { continue };
        let units_per_em = text_system.font_metrics(font_id).units_per_em as f32;
        let coretext = text_system.advance(font_id, glyph).unwrap().width / units_per_em * 12.0;
        worst = worst.max((coretext - metrics.advance_width(glyph.0 as u16)).abs());
        n += 1;
    }
    println!("advances: {n} glyphs, CoreText vs swash at 12 px differ by at most {worst:.5} px");
    assert!(worst < 0.001, "advances match: {worst}");
}

/// Rasterising cost: every printable ASCII glyph at each subpixel position,
/// grayscale (CoreGraphics) against subpixel (swash), and the atlas bytes.
fn time_rasterising(text_system: &std::sync::Arc<dyn PlatformTextSystem>) {
    let font_id = text_system.font_id(&font("JetBrains Mono")).unwrap();
    let glyphs: Vec<GlyphId> = (' '..='~').filter_map(|c| text_system.glyph_for_char(font_id, c)).collect();
    for thicken in [None, Some(30)] {
        let mut results = Vec::new();
        for subpixel in [false, true] {
            let params: Vec<RenderGlyphParams> = glyphs
                .iter()
                .flat_map(|&glyph_id| {
                    (0..SUBPIXEL_VARIANTS_X).map(move |x| RenderGlyphParams {
                        font_id,
                        glyph_id,
                        font_size: px(12.),
                        subpixel_variant: point(x, 0),
                        scale_factor: 1.0,
                        is_emoji: false,
                        subpixel_rendering: subpixel,
                        dilation: 0,
                        thicken,
                        foreground: [0, 0, 0, 255],
                    })
                })
                .collect();
            // Once to open the face, then timed.
            for p in &params {
                let bounds = text_system.glyph_raster_bounds(p).unwrap();
                if !bounds.is_zero() {
                    text_system.rasterize_glyph(p, bounds).unwrap();
                }
            }
            let start = std::time::Instant::now();
            let mut bytes = 0;
            let mut count = 0;
            for p in &params {
                let bounds = text_system.glyph_raster_bounds(p).unwrap();
                if !bounds.is_zero() {
                    bytes += text_system.rasterize_glyph(p, bounds).unwrap().1.len();
                    count += 1;
                }
            }
            let per_glyph = start.elapsed().as_secs_f64() * 1e6 / count as f64;
            results.push((per_glyph, bytes, count));
        }
        let [(gray_us, gray_bytes, n), (sub_us, sub_bytes, _)] = results[..] else { unreachable!() };
        println!(
            "rasterising, thicken {thicken:?}: {n} glyph variants; grayscale {gray_us:.1} us/glyph, {gray_bytes} atlas bytes; subpixel {sub_us:.1} us/glyph, {sub_bytes} atlas bytes ({:.1}x time, {:.1}x bytes)",
            sub_us / gray_us,
            sub_bytes as f64 / gray_bytes as f64
        );
    }
}

/// Faces swash reads from different places: an embedded font's bytes, an
/// installed single-face file (mapped), a collection (rebuilt from CoreText's
/// tables). Each draws colored coverage, not the grayscale fallback.
fn check_faces(text_system: &std::sync::Arc<dyn PlatformTextSystem>) {
    for family in ["JetBrains Mono", "Geist", "Courier New", "Menlo", "Helvetica Neue"] {
        let font_id = text_system.font_id(&font(family)).unwrap();
        let glyph_id = text_system.glyph_for_char(font_id, 'm').unwrap();
        let params = RenderGlyphParams {
            font_id,
            glyph_id,
            font_size: px(13.),
            subpixel_variant: point(1, 0),
            scale_factor: 1.0,
            is_emoji: false,
            subpixel_rendering: true,
            dilation: 0,
            thicken: None,
            foreground: [0, 0, 0, 255],
        };
        let bounds = text_system.glyph_raster_bounds(&params).unwrap();
        let (_, bytes) = text_system.rasterize_glyph(&params, bounds).unwrap();
        let colored = bytes.chunks_exact(4).filter(|t| t[0].max(t[1]).max(t[2]) - t[0].min(t[1]).min(t[2]) > 25).count();
        println!("{family}: 'm' has {colored} colored texels");
        assert!(colored > 10, "{family} draws subpixel coverage");
    }
}

#[cfg(target_os = "macos")]
fn main() {
    let platform_text_system = gpui_platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(platform_text_system.clone(), std::sync::Arc::new(()), gpui_platform::current_headless_renderer);
    cx.update(add_fonts);
    let rows = rows().len();
    let window = cx.open_window(size(px(WIDTH), px(ROW * rows as f32)), |_, cx| cx.new(|_| Scene)).unwrap();
    // Test windows open at 2x.
    cx.update_window(window.into(), |_, window, cx| check_retina(window, cx)).unwrap();
    cx.simulate_scale_factor_change(window.into(), 1.0);
    cx.update_window(window.into(), |_, window, cx| check(window, cx)).unwrap();
    // Moving back to a 2x display and to 1x again.
    cx.simulate_scale_factor_change(window.into(), 2.0);
    let at_2x = cx.update_window(window.into(), |_, window, cx| colored(window, cx)).unwrap();
    cx.simulate_scale_factor_change(window.into(), 1.0);
    let at_1x = cx.update_window(window.into(), |_, window, cx| colored(window, cx)).unwrap();
    println!("moving displays: {at_2x} colored px at 2x, {at_1x} back at 1x");
    assert_eq!(at_2x, 0, "grayscale at 2x");
    assert!(at_1x > 1000, "subpixel again at 1x");
    check_advances(&platform_text_system);
    check_faces(&platform_text_system);
    time_rasterising(&platform_text_system);
}

#[cfg(not(target_os = "macos"))]
fn main() {}
