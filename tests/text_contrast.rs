#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]
//! Real CoreText rasterising and GPU readback: text_contrast sharpens glyph
//! edges at about the same ink, where font thickening adds ink.
//! `with_glyph_contrast` (the terminal grid's text_contrast) and
//! `set_glyph_contrast` (the window's ui_text_contrast) stay apart.
//! Prints each case's edge share, the share of inked pixels whose coverage is
//! between 0.2 and 0.8 (lower is crisper), and its ink, the mean coverage over
//! the text's box; coverage is (pixel - background) / (text - background).
use gpui::{prelude::*, *};

const TEXT: &str = "the quick brown fox jumps over the lazy dog 0123456789";
const DARK: u32 = 0x1b1b1e;
const LIGHT: u32 = 0xe4e4e8;
// Rows: (contrast, thicken, text, background).
const CASES: [(f32, Option<u8>, u32, u32); 8] = [
    (0.0, None, LIGHT, DARK),
    (0.5, None, LIGHT, DARK),
    (1.0, None, LIGHT, DARK),
    (0.0, Some(255), LIGHT, DARK),
    (0.0, None, DARK, LIGHT),
    (0.5, None, DARK, LIGHT),
    (1.0, None, DARK, LIGHT),
    (0.0, Some(255), DARK, LIGHT),
];
const ROW: f32 = 18.;
const WIDTH: f32 = 220.;

fn paint_row(i: usize, fg: u32, bg: u32, font_size: f32, window: &mut Window, cx: &mut App) {
    let top = point(px(0.), px(ROW * i as f32));
    window.paint_quad(fill(Bounds::new(top, size(px(WIDTH), px(ROW))), rgb(bg)));
    let run = TextRun { len: TEXT.len(), font: font("JetBrains Mono"), color: rgb(fg).into(), background_color: None, underline: None, strikethrough: None };
    let line = window.text_system().shape_line(TEXT.into(), px(font_size), &[run], None);
    line.paint(top + point(px(2.), px(0.)), px(ROW), TextAlign::Left, None, window, cx).unwrap();
}

struct Scene {
    font_size: f32,
}

impl Render for Scene {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let font_size = self.font_size;
        canvas(
            |_, _, _| {},
            move |_, _, window, cx| {
                for (i, (contrast, thicken, fg, bg)) in CASES.into_iter().enumerate() {
                    window.with_glyph_thicken(thicken, |window| window.with_glyph_contrast(contrast, |window| paint_row(i, fg, bg, font_size, window, cx)));
                }
            },
        )
        .size_full()
    }
}

// The panels at the window's contrast, then a terminal grid inside them at 0.
struct UiScene {
    font_size: f32,
    ui_contrast: f32,
}

impl Render for UiScene {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        window.set_glyph_contrast(self.ui_contrast);
        let font_size = self.font_size;
        canvas(
            |_, _, _| {},
            move |_, _, window, cx| {
                paint_row(0, LIGHT, DARK, font_size, window, cx);
                window.with_glyph_contrast(0., |window| paint_row(1, LIGHT, DARK, font_size, window, cx));
            },
        )
        .size_full()
    }
}

/// (edge share, ink) of row `i`.
fn measure(image: &image::RgbaImage, scale: f32, i: usize, fg: u32, bg: u32) -> (f32, f32) {
    let channel = |c: u32, k: u32| ((c >> (16 - 8 * k)) & 0xff) as f32;
    let (y0, y1) = ((ROW * i as f32 * scale) as u32, (ROW * (i + 1) as f32 * scale) as u32);
    let (mut sum, mut n, mut inked, mut edge) = (0.0, 0, 0, 0);
    for y in y0..y1 {
        for x in 0..(WIDTH * scale) as u32 {
            let p = image.get_pixel(x, y).0;
            let a = ((0..3).map(|k| (p[k as usize] as f32 - channel(bg, k)) / (channel(fg, k) - channel(bg, k))).sum::<f32>() / 3.0).clamp(0.0, 1.0);
            sum += a;
            n += 1;
            if a > 0.02 {
                inked += 1;
                edge += (0.2..=0.8).contains(&a) as usize;
            }
        }
    }
    (edge as f32 / inked as f32, sum / n as f32)
}

fn check(window: &mut Window, cx: &mut App) {
    let _ = window.draw(cx);
    let image = window.render_to_image().expect("real GPU screenshot");
    let scale = window.scale_factor();
    let m: Vec<(f32, f32)> = CASES.iter().enumerate().map(|(i, (_, _, fg, bg))| measure(&image, scale, i, *fg, *bg)).collect();
    for ((contrast, thicken, fg, _), (edge, ink)) in CASES.iter().zip(&m) {
        let polarity = if *fg == DARK { "dark on light" } else { "light on dark" };
        println!("scale {scale} contrast {contrast} thicken {thicken:?} {polarity}: edge share {edge:.4}, ink {ink:.4}");
    }
    for polarity in m.chunks(4) {
        let [(off_edge, off_ink), (half_edge, _), (on_edge, on_ink), (_, thick_ink)] = polarity[..] else { unreachable!() };
        assert!(half_edge < off_edge * 0.97 && on_edge < half_edge * 0.97, "contrast sharpens edges: {off_edge} to {half_edge} to {on_edge}");
        assert!(on_edge < off_edge * 0.85, "contrast 1 cuts the edge share by at least 15%: {off_edge} to {on_edge}");
        assert!((on_ink - off_ink).abs() < off_ink * 0.03, "contrast keeps the ink: {off_ink} to {on_ink}");
        // Light text on dark already draws at full strength unthickened.
        if polarity[0] == m[4] {
            assert!(thick_ink - off_ink > (on_ink - off_ink).abs() * 5.0, "thickening adds far more ink than contrast: {thick_ink} vs {on_ink}");
        }
    }
}

fn check_ui(window: &mut Window, cx: &mut App, ui_contrast: f32) -> ((f32, f32), (f32, f32)) {
    let _ = window.draw(cx);
    let image = window.render_to_image().expect("real GPU screenshot");
    let scale = window.scale_factor();
    let (ui, terminal) = (measure(&image, scale, 0, LIGHT, DARK), measure(&image, scale, 1, LIGHT, DARK));
    println!("scale {scale} ui_text_contrast {ui_contrast}: interface edge share {:.4}, terminal edge share {:.4}", ui.0, terminal.0);
    (ui, terminal)
}

fn add_font(cx: &mut App) {
    let bytes: &'static [u8] = include_bytes!("../assets/fonts/JetBrainsMono-Variable.ttf");
    cx.text_system().add_fonts(vec![bytes.into()]).unwrap();
}

#[cfg(target_os = "macos")]
fn main() {
    let mut cx = HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        std::sync::Arc::new(()),
        gpui_platform::current_headless_renderer,
    );
    cx.update(add_font);
    // The test window is 2x: half of font_size 12 draws as 12 device pixels,
    // as on a 1x display.
    let window = cx.open_window(size(px(WIDTH), px(ROW * CASES.len() as f32)), |_, cx| cx.new(|_| Scene { font_size: 6. })).unwrap();
    cx.update_window(window.into(), |_, window, cx| check(window, cx)).unwrap();
    let [off, on] = [0., 1.].map(|ui_contrast| {
        let window = cx.open_window(size(px(WIDTH), px(ROW * 2.)), |_, cx| cx.new(|_| UiScene { font_size: 6., ui_contrast })).unwrap();
        cx.update_window(window.into(), |_, window, cx| check_ui(window, cx, ui_contrast)).unwrap()
    });
    assert!(on.0.0 < off.0.0 * 0.85, "ui_text_contrast sharpens the panels: {} to {}", off.0.0, on.0.0);
    assert_eq!(on.1, off.1, "the terminal grid keeps its own");
    assert_eq!(off.0, off.1, "off, both draw as GPUI does");
}

#[cfg(not(target_os = "macos"))]
fn main() {}
