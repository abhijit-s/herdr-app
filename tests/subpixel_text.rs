#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]
//! Trial: subpixel text smoothing, with real rasterising and GPU readback.
//! At 1x a paint that asks for subpixel glyphs (`with_glyph_subpixel`, the
//! text_rendering settings) draws a coverage per color channel; at 2x it
//! draws exactly the grayscale glyphs, and a window moving between the two
//! switches on its next frame.
//! Subpixel text weighs what grayscale text weighs at the same settings: the
//! person's terminal and panel fonts and contrasts, every thicken strength the
//! Settings window reaches to (0, 30, 112, 192, 255) and thicken off, both
//! polarities, painted as the app paints them, read back from the GPU. Ink is
//! the mean over the text's box of (pixel - background) / (text -
//! background), per channel; the linear-light column decodes sRGB first.
//! Below the GPU check, a sweep of every strength 0 to 255 compares the
//! glyphs' summed coverage straight from the rasteriser.
//! With HERDR_SUBPIXEL_PNG set to a folder, saves the rows there as PNGs,
//! named with HERDR_SUBPIXEL_PNG_TAG (default "1x").
use gpui::{prelude::*, *};

const TERMINAL_TEXT: &str = "fn main() { let ok = x != 0x1f; } // quick brown fox @ 0O lI1";
const PANEL_TEXT: &str = "Settings  Workspaces  Agents running: 3  Last build passed 2m ago";
const DARK: u32 = 0x1b1b1e;
const LIGHT: u32 = 0xe4e4e8;
const ROW: f32 = 18.;
const WIDTH: f32 = 520.;

#[derive(Clone, Copy, PartialEq)]
struct Text {
    name: &'static str,
    family: &'static str,
    text: &'static str,
    size: f32,
    contrast: f32,
}

// The person's: terminals JetBrains Mono 12 at contrast 0.2 (font_thicken 30),
// panels Geist 13 at contrast 0.3 (ui_font_thicken 192).
const TERMINAL: Text = Text { name: "terminal", family: "JetBrains Mono", text: TERMINAL_TEXT, size: 12., contrast: 0.2 };
const PANEL: Text = Text { name: "panel", family: "Geist", text: PANEL_TEXT, size: 13., contrast: 0.3 };
const STRENGTHS: [u8; 5] = [0, 30, 112, 192, 255];
const POLARITIES: [(u32, u32); 2] = [(LIGHT, DARK), (DARK, LIGHT)];
/// How far subpixel ink may stray from grayscale's, thickened and plain.
const THICKENED_TOLERANCE: f32 = 2.0;
const PLAIN_TOLERANCE: f32 = 5.0;

#[derive(Clone, Copy)]
struct Row {
    text: Text,
    thicken: Option<u8>,
    fg: u32,
    bg: u32,
    subpixel: bool,
}

/// Rows: each text at each strength then thicken off, both polarities,
/// grayscale then subpixel.
fn rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for text in [TERMINAL, PANEL] {
        for thicken in STRENGTHS.map(Some).into_iter().chain([None]) {
            for (fg, bg) in POLARITIES {
                for subpixel in [false, true] {
                    rows.push(Row { text, thicken, fg, bg, subpixel });
                }
            }
        }
    }
    rows
}

fn find_row(text: Text, thicken: Option<u8>, fg: u32, subpixel: bool) -> usize {
    rows().iter().position(|r| r.text == text && r.thicken == thicken && r.fg == fg && r.subpixel == subpixel).unwrap()
}

/// Paints as the app does: the terminal grid inside `with_font_weight_shift(0)`,
/// `with_glyph_thicken`, `with_glyph_contrast` and `with_glyph_subpixel`
/// (src/terminal.rs); panels under the window's same settings.
fn paint_row(i: usize, row: Row, window: &mut Window, cx: &mut App) {
    let top = point(px(0.), px(ROW * i as f32));
    window.paint_quad(fill(Bounds::new(top, size(px(WIDTH), px(ROW))), rgb(row.bg)));
    let run = TextRun { len: row.text.text.len(), font: font(row.text.family), color: rgb(row.fg).into(), background_color: None, underline: None, strikethrough: None };
    window.with_font_weight_shift(0.0, |window| {
        let line = window.text_system().shape_line(row.text.text.into(), px(row.text.size), &[run], None);
        window.with_glyph_thicken(row.thicken, |window| {
            window.with_glyph_contrast(row.text.contrast, |window| {
                window.with_glyph_subpixel(row.subpixel, |window| line.paint(top + point(px(4.), px(0.)), px(ROW), TextAlign::Left, None, window, cx).unwrap())
            })
        })
    });
}

struct Scene;

impl Render for Scene {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        canvas(
            |_, _, _| {},
            |_, _, window, cx| {
                for (i, row) in rows().into_iter().enumerate() {
                    paint_row(i, row, window, cx);
                }
            },
        )
        .size_full()
    }
}

struct Measure {
    ink: f32,
    linear_ink: f32,
    /// Pixels whose channels' coverages differ by more than 0.1.
    colored: usize,
}

fn srgb_to_linear(v: f32) -> f32 {
    let v = v / 255.0;
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

fn measure(image: &image::RgbaImage, scale: f32, i: usize, fg: u32, bg: u32) -> Measure {
    let channel = |c: u32, k: usize| ((c >> (16 - 8 * k)) & 0xff) as f32;
    let share = |p: f32, f: f32, b: f32| ((p - b) / (f - b)).clamp(0.0, 1.0);
    let (y0, y1) = ((ROW * i as f32 * scale) as u32, (ROW * (i + 1) as f32 * scale) as u32);
    let (mut sum, mut linear_sum, mut n, mut colored) = (0.0, 0.0, 0, 0);
    for y in y0..y1 {
        for x in 0..(WIDTH * scale) as u32 {
            let p = image.get_pixel(x, y).0;
            let a: [f32; 3] = std::array::from_fn(|k| share(p[k] as f32, channel(fg, k), channel(bg, k)));
            let linear: [f32; 3] = std::array::from_fn(|k| share(srgb_to_linear(p[k] as f32), srgb_to_linear(channel(fg, k)), srgb_to_linear(channel(bg, k))));
            sum += a.iter().sum::<f32>() / 3.0;
            linear_sum += linear.iter().sum::<f32>() / 3.0;
            n += 1;
            let spread = a.iter().cloned().fold(0.0f32, f32::max) - a.iter().cloned().fold(1.0f32, f32::min);
            colored += (spread > 0.1) as usize;
        }
    }
    Measure { ink: sum / n as f32, linear_ink: linear_sum / n as f32, colored }
}

fn draw(window: &mut Window, cx: &mut App) -> image::RgbaImage {
    let _ = window.draw(cx);
    window.render_to_image().expect("real GPU screenshot")
}

fn row_pixels(image: &image::RgbaImage, scale: f32, i: usize) -> Vec<u8> {
    let (y0, h) = ((ROW * i as f32 * scale) as u32, (ROW * scale) as u32);
    image::imageops::crop_imm(image, 0, y0, image.width(), h).to_image().into_raw()
}

fn save(image: &image::RgbaImage, dir: &std::path::Path, rows: &[(usize, String)], name: &str) {
    let (w, h) = (image.width(), ROW as u32);
    let mut sheet = image::RgbaImage::new(w, h * rows.len() as u32);
    for (k, (i, _)) in rows.iter().enumerate() {
        let crop = image::imageops::crop_imm(image, 0, h * *i as u32, w, h).to_image();
        image::imageops::replace(&mut sheet, &crop, 0, (h * k as u32) as i64);
    }
    sheet.save(dir.join(format!("{name}.png"))).unwrap();
    let zoom = image::imageops::resize(&image::imageops::crop_imm(&sheet, 0, 0, 260, sheet.height()).to_image(), 260 * 6, sheet.height() * 6, image::imageops::FilterType::Nearest);
    zoom.save(dir.join(format!("{name}_zoom6x.png"))).unwrap();
    let labels: Vec<&str> = rows.iter().map(|(_, l)| l.as_str()).collect();
    std::fs::write(dir.join(format!("{name}.txt")), labels.join("\n") + "\n").unwrap();
}

fn save_pngs(image: &image::RgbaImage) {
    let Some(dir) = std::env::var_os("HERDR_SUBPIXEL_PNG") else { return };
    let dir = std::path::PathBuf::from(dir);
    let tag = std::env::var("HERDR_SUBPIXEL_PNG_TAG").unwrap_or_else(|_| "1x".into());
    std::fs::create_dir_all(&dir).unwrap();
    for (text, strength, name) in [(TERMINAL, 30, "terminal_thicken30"), (TERMINAL, 112, "terminal_thicken112"), (PANEL, 192, "panel_thicken192")] {
        let mut sheet = Vec::new();
        for (fg, _) in POLARITIES {
            let polarity = if fg == DARK { "dark on light" } else { "light on dark" };
            for (subpixel, mode) in [(false, "grayscale"), (true, "subpixel")] {
                sheet.push((find_row(text, Some(strength), fg, subpixel), format!("{} {polarity}, thicken {strength}, {mode}", text.name)));
            }
        }
        save(image, &dir, &sheet, &format!("{name}_{tag}"));
    }
    image.save(dir.join(format!("all_rows_{tag}.png"))).unwrap();
}

fn check(window: &mut Window, cx: &mut App) {
    let image = draw(window, cx);
    let scale = window.scale_factor();
    assert_eq!(scale, 1.0);
    save_pngs(&image);
    let rows = rows();
    let m: Vec<Measure> = rows.iter().enumerate().map(|(i, row)| measure(&image, scale, i, row.fg, row.bg)).collect();
    println!("| text | thicken | polarity | grayscale ink | subpixel ink | difference | linear-light difference | colored px (subpixel) |");
    println!("|---|---|---|---|---|---|---|---|");
    let mut failures = Vec::new();
    for (pair, rows) in m.chunks(2).zip(rows.chunks(2)) {
        let row = rows[0];
        let polarity = if row.fg == DARK { "dark on light" } else { "light on dark" };
        let thicken = row.thicken.map_or("off".to_string(), |s| s.to_string());
        let (gray, sub) = (&pair[0], &pair[1]);
        let difference = (sub.ink - gray.ink) / gray.ink * 100.0;
        let linear = (sub.linear_ink - gray.linear_ink) / gray.linear_ink * 100.0;
        println!("| {} | {thicken} | {polarity} | {:.4} | {:.4} | {difference:+.1}% | {linear:+.1}% | {} |", row.text.name, gray.ink, sub.ink, sub.colored);
        let label = format!("{} thicken {thicken} {polarity}", row.text.name);
        if gray.colored != 0 {
            failures.push(format!("{label}: grayscale draws color ({} px)", gray.colored));
        }
        if sub.colored <= 200 {
            failures.push(format!("{label}: subpixel colors too few edges ({} px)", sub.colored));
        }
        let tolerance = if row.thicken.is_some() { THICKENED_TOLERANCE } else { PLAIN_TOLERANCE };
        if difference.abs() >= tolerance {
            failures.push(format!("{label}: subpixel ink {difference:+.1}% from grayscale (within {tolerance}%)"));
        }
    }
    // Thicken adds weight at every strength, as a stronger fill always does.
    for text in [TERMINAL, PANEL] {
        for (fg, bg) in POLARITIES {
            let inks: Vec<f32> = STRENGTHS.iter().map(|&s| m[find_row(text, Some(s), fg, true)].ink).collect();
            for (k, pair) in inks.windows(2).enumerate() {
                if pair[1] <= pair[0] {
                    let polarity = if bg == LIGHT { "dark on light" } else { "light on dark" };
                    failures.push(format!("{} {polarity}: subpixel thicken {} inks no more than {} ({:.4} vs {:.4})", text.name, STRENGTHS[k + 1], STRENGTHS[k], pair[1], pair[0]));
                }
            }
        }
    }
    assert!(failures.is_empty(), "subpixel ink against grayscale:\n{}", failures.join("\n"));
}

/// Summed glyph coverage, CoreGraphics' grayscale against swash's subpixel
/// (the three channels' mean), at every thicken strength: subpixel follows
/// grayscale's smoothing steps within three quarters of a step's ink (a
/// step is about 5.5%), rising smoothly where grayscale steps.
fn check_strength_sweep(text_system: &std::sync::Arc<dyn PlatformTextSystem>) {
    const SWEEP_TOLERANCE: f64 = 5.0;
    for text in [TERMINAL, PANEL] {
        let font_id = text_system.font_id(&font(text.family)).unwrap();
        let glyphs: Vec<GlyphId> = text.text.chars().filter_map(|c| text_system.glyph_for_char(font_id, c)).collect();
        let coverage = |thicken: u8, subpixel: bool| -> f64 {
            let mut sum = 0.0;
            for &glyph_id in &glyphs {
                let params = RenderGlyphParams {
                    font_id,
                    glyph_id,
                    font_size: px(text.size),
                    subpixel_variant: point(0, 0),
                    scale_factor: 1.0,
                    is_emoji: false,
                    subpixel_rendering: subpixel,
                    dilation: 0,
                    thicken: Some(thicken),
                    foreground: [0, 0, 0, 255],
                };
                let bounds = text_system.glyph_raster_bounds(&params).unwrap();
                if bounds.is_zero() {
                    continue;
                }
                let (_, bytes) = text_system.rasterize_glyph(&params, bounds).unwrap();
                sum += if subpixel {
                    bytes.chunks_exact(4).map(|t| (t[0] as f64 + t[1] as f64 + t[2] as f64) / 3.0).sum::<f64>()
                } else {
                    bytes.iter().map(|&a| a as f64).sum::<f64>()
                };
            }
            sum
        };
        let (mut worst, mut worst_at, mut previous) = (0.0f64, 0, 0.0);
        let (mut steps, mut at_settings) = (Vec::new(), Vec::new());
        let mut last_gray = 0.0;
        for strength in 0..=255u8 {
            let (gray, sub) = (coverage(strength, false), coverage(strength, true));
            if strength > 0 && gray != last_gray {
                steps.push(strength);
            }
            last_gray = gray;
            let difference = (sub - gray) / gray * 100.0;
            if STRENGTHS.contains(&strength) {
                at_settings.push(format!("{strength}: {difference:+.1}%"));
            }
            if difference.abs() > worst.abs() {
                (worst, worst_at) = (difference, strength);
            }
            if strength > 0 {
                let change = (sub - previous) / previous * 100.0;
                assert!(change > -0.1, "{}: subpixel coverage falls {change:.2}% at strength {strength}", text.name);
                assert!(change < 1.0, "{}: subpixel coverage steps {change:.2}% at strength {strength}", text.name);
            }
            previous = sub;
        }
        println!("{} strength sweep: grayscale steps at {steps:?}; subpixel within {worst:+.1}% (at {worst_at}); {}", text.name, at_settings.join(", "));
        assert!(worst.abs() < SWEEP_TOLERANCE, "{}: subpixel coverage {worst:+.1}% from grayscale at strength {worst_at}", text.name);
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
    rows().iter().enumerate().map(|(i, row)| measure(&image, scale, i, row.fg, row.bg).colored).sum()
}

fn add_fonts(cx: &mut App) {
    let fonts: [&'static [u8]; 2] = [include_bytes!("../assets/fonts/JetBrainsMono-Variable.ttf"), include_bytes!("../assets/fonts/Geist.ttf")];
    cx.text_system().add_fonts(fonts.into_iter().map(Into::into).collect()).unwrap();
}

/// Every printable ASCII glyph inks more at thicken 255 than at 0, subpixel
/// as grayscale: swash's emboldening grows each one, `i` included.
fn check_every_glyph_grows(text_system: &std::sync::Arc<dyn PlatformTextSystem>) {
    for text in [TERMINAL, PANEL] {
        let font_id = text_system.font_id(&font(text.family)).unwrap();
        let coverage = |glyph_id, thicken| {
            let params = RenderGlyphParams {
                font_id,
                glyph_id,
                font_size: px(text.size),
                subpixel_variant: point(0, 0),
                scale_factor: 1.0,
                is_emoji: false,
                subpixel_rendering: true,
                dilation: 0,
                thicken: Some(thicken),
                foreground: [0, 0, 0, 255],
            };
            let bounds = text_system.glyph_raster_bounds(&params).unwrap();
            let (_, bytes) = text_system.rasterize_glyph(&params, bounds).unwrap();
            bytes.chunks_exact(4).map(|t| t[0] as u32 + t[1] as u32 + t[2] as u32).sum::<u32>()
        };
        let shrinking: Vec<char> = ('!'..='~')
            .filter(|&ch| text_system.glyph_for_char(font_id, ch).is_some_and(|glyph| coverage(glyph, 255) <= coverage(glyph, 0)))
            .collect();
        assert!(shrinking.is_empty(), "{}: thicken 255 inks no more than 0 on {shrinking:?}", text.family);
    }
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

// A full terminal screen at 1x: 220 columns by 50 rows of 12 px text.
struct Screen {
    subpixel: bool,
}

impl Render for Screen {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let subpixel = self.subpixel;
        canvas(
            |_, _, _| {},
            move |_, _, window, cx| {
                let printable: Vec<char> = (' '..='~').collect();
                for row in 0..50 {
                    let text: String = (0..220).map(|col| printable[(row * 7 + col * 13) % printable.len()]).collect();
                    let top = point(px(0.), px(15. * row as f32));
                    let run = TextRun { len: text.len(), font: font("JetBrains Mono"), color: rgb(LIGHT).into(), background_color: None, underline: None, strikethrough: None };
                    let line = window.text_system().shape_line(text.into(), px(12.), &[run], None);
                    window.with_glyph_thicken(Some(30), |window| window.with_glyph_subpixel(subpixel, |window| line.paint(top, px(15.), TextAlign::Left, None, window, cx).unwrap()));
                }
            },
        )
        .size_full()
    }
}

/// The first 1x frame of a full screen, which rasterises every glyph (the
/// window opened at 2x, whose tiles don't serve 1x), against the next.
fn time_first_frame(cx: &mut HeadlessAppContext) {
    for subpixel in [false, true] {
        let window = cx.open_window(size(px(1600.), px(760.)), |_, cx| cx.new(|_| Screen { subpixel })).unwrap();
        cx.simulate_scale_factor_change(window.into(), 1.0);
        let [first, second] = [0, 1].map(|_| {
            cx.update_window(window.into(), |_, window, cx| {
                window.refresh();
                let start = std::time::Instant::now();
                let _ = window.draw(cx);
                start.elapsed().as_secs_f64() * 1e3
            })
            .unwrap()
        });
        let mode = if subpixel { "subpixel" } else { "grayscale" };
        let image = cx.update_window(window.into(), |_, window, _| window.render_to_image().unwrap()).unwrap();
        let inked = image.pixels().filter(|p| p.0[0] > 0x40).count();
        println!("full screen 220x50, {mode}: first frame {first:.1} ms, next {second:.1} ms ({inked} inked px)");
    }
}


/// The person's three test lines, painted as the terminal grid paints them:
/// each glyph at its cell's whole-pixel x (cell_width "pixel": 7 px at 12 px)
/// on a whole-pixel baseline, line_height 1 (16 px rows).
const SHAPE_LINES: [&str; 3] = [
    "The quick brown fox jumps over the lazy dog 0123456789",
    "abcdefghijklmnopqrstuvwxyz ABCDEFGHIJKLMNOPQRSTUVWXYZ",
    "{}[]()<>=!:;.,-_+*/\\|@#$%^&~`\"' il1I| 0Oo rnm",
];
const SHAPE_CELL: (f32, f32) = (7., 16.);
const SHAPE_BASELINE: f32 = 12.;
const SHAPE_LEFT: f32 = 4.;
const SHAPE_STRENGTHS: [Option<u8>; 3] = [Some(30), Some(0), None];

#[derive(Clone, Copy)]
struct ShapeRow {
    line: usize,
    thicken: Option<u8>,
    fg: u32,
    bg: u32,
    subpixel: bool,
}

/// Each strength, each polarity, each line, grayscale then subpixel.
fn shape_rows() -> Vec<ShapeRow> {
    let mut rows = Vec::new();
    for thicken in SHAPE_STRENGTHS {
        for (fg, bg) in POLARITIES {
            for line in 0..SHAPE_LINES.len() {
                for subpixel in [false, true] {
                    rows.push(ShapeRow { line, thicken, fg, bg, subpixel });
                }
            }
        }
    }
    rows
}

struct ShapeScene;

impl Render for ShapeScene {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        canvas(
            |_, _, _| {},
            |_, _, window, cx| {
                for (i, row) in shape_rows().into_iter().enumerate() {
                    let top = px(SHAPE_CELL.1 * i as f32);
                    window.paint_quad(fill(Bounds::new(point(px(0.), top), size(px(WIDTH), px(SHAPE_CELL.1))), rgb(row.bg)));
                    let text = SHAPE_LINES[row.line];
                    let run = TextRun { len: text.len(), font: font("JetBrains Mono"), color: rgb(row.fg).into(), background_color: None, underline: None, strikethrough: None };
                    window.with_font_weight_shift(0.0, |window| {
                        let line = window.text_system().shape_line(text.into(), px(TERMINAL.size), &[run], None);
                        window.with_glyph_thicken(row.thicken, |window| {
                            window.with_glyph_contrast(TERMINAL.contrast, |window| {
                                window.with_glyph_subpixel(row.subpixel, |window| {
                                    for run in &line.runs {
                                        for glyph in &run.glyphs {
                                            let origin = point(px(SHAPE_LEFT + SHAPE_CELL.0 * glyph.index as f32), top + px(SHAPE_BASELINE));
                                            window.paint_glyph(origin, run.font_id, glyph.id, px(TERMINAL.size), rgb(row.fg).into()).unwrap();
                                        }
                                    }
                                })
                            })
                        })
                    });
                }
                let _ = cx;
            },
        )
        .size_full()
    }
}

/// Ink share (0 to 1, the channels' mean) of each pixel of shape row `i`.
fn shape_ink(image: &image::RgbaImage, i: usize, row: ShapeRow) -> Vec<Vec<f32>> {
    let channel = |c: u32, k: usize| ((c >> (16 - 8 * k)) & 0xff) as f32;
    let y0 = (SHAPE_CELL.1 * i as f32) as u32;
    (y0..y0 + SHAPE_CELL.1 as u32)
        .map(|y| {
            (0..WIDTH as u32)
                .map(|x| {
                    let p = image.get_pixel(x, y).0;
                    (0..3).map(|k| ((p[k] as f32 - channel(row.bg, k)) / (channel(row.fg, k) - channel(row.bg, k))).clamp(0.0, 1.0)).sum::<f32>() / 3.0
                })
                .collect()
        })
        .collect()
}

/// A glyph's ink-weighted vertical centre and spread (standard deviation),
/// in rows, over its cell.
fn vertical_moments(ink: &[Vec<f32>], column: usize) -> Option<(f32, f32)> {
    let x0 = (SHAPE_LEFT + SHAPE_CELL.0 * column as f32) as usize;
    let rows: Vec<f32> = ink.iter().map(|row| row[x0..x0 + SHAPE_CELL.0 as usize].iter().sum()).collect();
    let total: f32 = rows.iter().sum();
    (total > 1.0).then(|| {
        let mean = rows.iter().enumerate().map(|(y, v)| y as f32 * v).sum::<f32>() / total;
        let variance = rows.iter().enumerate().map(|(y, v)| (y as f32 - mean).powi(2) * v).sum::<f32>() / total;
        (mean, variance.sqrt())
    })
}

/// Subpixel glyphs keep grayscale's shape, not only its ink: the person's
/// lines painted as the terminal paints them, read back from the GPU. Per
/// strength and polarity, the ink of each pixel row (summed over the three
/// lines) stays within `ROW_TOLERANCE` of grayscale's, relative to the
/// heaviest row (`PLAIN_ROW_TOLERANCE` with thicken off); the ascender rows
/// (above the x-height) and the x-height band each within `BAND_TOLERANCE`;
/// and the glyphs' mean vertical spread within `SPREAD_TOLERANCE` rows.
/// swash emboldened isotropically to grayscale's ink drew glyphs short with
/// heavy stems: the x-height's top row 5 to 7% of the peak row light, the row
/// above it 2 to 3% heavy.
fn check_shape(cx: &mut HeadlessAppContext) {
    const ROW_TOLERANCE: f32 = 0.03;
    const PLAIN_ROW_TOLERANCE: f32 = 0.05;
    const BAND_TOLERANCE: f32 = 3.0;
    const SPREAD_TOLERANCE: f32 = 0.04;
    let rows = shape_rows();
    let window = cx.open_window(size(px(WIDTH), px(SHAPE_CELL.1 * rows.len() as f32)), |_, cx| cx.new(|_| ShapeScene)).unwrap();
    cx.simulate_scale_factor_change(window.into(), 1.0);
    let image = cx.update_window(window.into(), |_, window, cx| draw(window, cx)).unwrap();
    save_shape_pngs(&image, &rows);
    let inks: Vec<Vec<Vec<f32>>> = rows.iter().enumerate().map(|(i, &row)| shape_ink(&image, i, row)).collect();
    let mut failures = Vec::new();
    // x-height of JetBrains Mono at 12 px is 6.6 px: its top lands in the
    // row 7 above the baseline; ascenders and capitals reach row 9.
    let baseline = SHAPE_BASELINE as usize;
    let (ascender, x_band) = (baseline - 10..baseline - 7, baseline - 6..baseline);
    for chunk in rows.chunks(2 * SHAPE_LINES.len()).zip(inks.chunks(2 * SHAPE_LINES.len())) {
        let (rows, inks) = chunk;
        let profile = |subpixel: usize| -> Vec<f32> {
            (0..SHAPE_CELL.1 as usize).map(|y| (0..SHAPE_LINES.len()).map(|l| inks[2 * l + subpixel][y].iter().sum::<f32>()).sum()).collect()
        };
        let (gray, sub) = (profile(0), profile(1));
        let peak = gray.iter().cloned().fold(0.0, f32::max);
        let row = rows[0];
        let label = format!("thicken {} {}", row.thicken.map_or("off".into(), |s| s.to_string()), if row.fg == DARK { "dark on light" } else { "light on dark" });
        let band = |p: &[f32], r: &std::ops::Range<usize>| p[r.clone()].iter().sum::<f32>();
        let ascender_diff = (band(&sub, &ascender) - band(&gray, &ascender)) / band(&gray, &ascender) * 100.0;
        let x_diff = (band(&sub, &x_band) - band(&gray, &x_band)) / band(&gray, &x_band) * 100.0;
        let total_diff = (sub.iter().sum::<f32>() - gray.iter().sum::<f32>()) / gray.iter().sum::<f32>() * 100.0;
        let worst_row = (0..gray.len()).max_by(|&a, &b| (sub[a] - gray[a]).abs().total_cmp(&(sub[b] - gray[b]).abs())).unwrap();
        let worst = (sub[worst_row] - gray[worst_row]) / peak;
        let mut spreads = Vec::new();
        for line in 0..SHAPE_LINES.len() {
            for (column, ch) in SHAPE_LINES[line].chars().enumerate() {
                if let (Some((_, g)), Some((_, s))) = (vertical_moments(&inks[2 * line], column), vertical_moments(&inks[2 * line + 1], column)) {
                    spreads.push((s - g, ch));
                }
            }
        }
        let mean_spread = spreads.iter().map(|(d, _)| d).sum::<f32>() / spreads.len() as f32;
        let worst_spread = spreads.iter().cloned().max_by(|a, b| a.0.abs().total_cmp(&b.0.abs())).unwrap();
        println!(
            "shape, {label}: total {total_diff:+.1}%, ascender rows {ascender_diff:+.1}%, x-height band {x_diff:+.1}%, worst row {:+} {:+.1}% of peak, glyph spread {mean_spread:+.3} rows (worst {:?} {:+.3})",
            worst_row as i32 - baseline as i32,
            worst * 100.0,
            worst_spread.1,
            worst_spread.0
        );
        println!("  rows from baseline: {}", (0..gray.len()).filter(|&y| gray[y] + sub[y] > 0.5).map(|y| format!("{:+}: {:.1}/{:.1}", y as i32 - baseline as i32, gray[y], sub[y])).collect::<Vec<_>>().join(", "));
        let row_tolerance = if row.thicken.is_some() { ROW_TOLERANCE } else { PLAIN_ROW_TOLERANCE };
        if worst.abs() > row_tolerance {
            failures.push(format!("{label}: row {:+} inks {:+.1}% of the peak row from grayscale (within {:.1}%)", worst_row as i32 - baseline as i32, worst * 100.0, row_tolerance * 100.0));
        }
        for (name, diff) in [("ascender rows", ascender_diff), ("x-height band", x_diff)] {
            if diff.abs() > BAND_TOLERANCE {
                failures.push(format!("{label}: {name} ink {diff:+.1}% from grayscale (within {BAND_TOLERANCE}%)"));
            }
        }
        if mean_spread.abs() > SPREAD_TOLERANCE {
            failures.push(format!("{label}: glyphs' vertical spread {mean_spread:+.3} rows from grayscale (within {SPREAD_TOLERANCE})"));
        }
    }
    assert!(failures.is_empty(), "subpixel glyph shape against grayscale:\n{}", failures.join("\n"));
}

fn save_shape_pngs(image: &image::RgbaImage, rows: &[ShapeRow]) {
    let Some(dir) = std::env::var_os("HERDR_SUBPIXEL_PNG") else { return };
    let dir = std::path::PathBuf::from(dir);
    let tag = std::env::var("HERDR_SUBPIXEL_PNG_TAG").unwrap_or_else(|_| "1x".into());
    std::fs::create_dir_all(&dir).unwrap();
    let h = SHAPE_CELL.1 as u32;
    for thicken in SHAPE_STRENGTHS {
        for (fg, _) in POLARITIES {
            let polarity = if fg == DARK { "dark_on_light" } else { "light_on_dark" };
            // Grayscale's three lines, then subpixel's.
            let picks: Vec<usize> = [false, true]
                .iter()
                .flat_map(|&sub| (0..SHAPE_LINES.len()).map(move |line| (line, sub)))
                .map(|(line, sub)| rows.iter().position(|r| r.thicken == thicken && r.fg == fg && r.line == line && r.subpixel == sub).unwrap())
                .collect();
            let mut sheet = image::RgbaImage::new(image.width(), h * picks.len() as u32);
            for (k, &i) in picks.iter().enumerate() {
                let crop = image::imageops::crop_imm(image, 0, h * i as u32, image.width(), h).to_image();
                image::imageops::replace(&mut sheet, &crop, 0, (h * k as u32) as i64);
            }
            let name = format!("shape_thicken{}_{polarity}_{tag}", thicken.map_or("off".into(), |s| s.to_string()));
            sheet.save(dir.join(format!("{name}.png"))).unwrap();
            let zoom = image::imageops::crop_imm(&sheet, 0, 0, 200, sheet.height()).to_image();
            image::imageops::resize(&zoom, 200 * 6, sheet.height() * 6, image::imageops::FilterType::Nearest).save(dir.join(format!("{name}_zoom6x.png"))).unwrap();
        }
    }
}

#[cfg(target_os = "macos")]
fn main() {
    let platform_text_system = gpui_platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(platform_text_system.clone(), std::sync::Arc::new(()), gpui_platform::current_headless_renderer);
    cx.update(add_fonts);
    check_every_glyph_grows(&platform_text_system);
    check_strength_sweep(&platform_text_system);
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
    check_shape(&mut cx);
    check_advances(&platform_text_system);
    check_faces(&platform_text_system);
    time_first_frame(&mut cx);
    time_rasterising(&platform_text_system);
}

#[cfg(not(target_os = "macos"))]
fn main() {}
