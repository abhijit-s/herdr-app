#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]
//! Real CoreText rasterising and GPU readback: font_thicken's strength
//! thickens dark text on light as much as light text on dark (Ghostty's
//! `font-thicken`), and smoothing off inks both alike too, where macOS's own
//! smoothing thickens by text color.
//! The window's own thickening (`set_glyph_thicken`, ui_font_thicken)
//! thickens the panels' dark text on light, while a `with_glyph_thicken`
//! inside it (the terminal grid's, here with font_thicken auto) keeps its own.
//! Prints each case's ink: the mean of (pixel - background) / (text -
//! background) over the text's box, so both polarities compare directly.
use gpui::{prelude::*, *};

const TEXT: &str = "the quick brown fox jumps over the lazy dog 0123456789";
const DARK: u32 = 0x1b1b1e;
const LIGHT: u32 = 0xe4e4e8;
// Rows: (font weight, thicken, text, background). Ghostty on the LG display
// draws Regular (400) with font-thicken-strength 30.
const CASES: [(f32, GlyphThicken, u32, u32); 10] = [
    (400., GlyphThicken::Auto, LIGHT, DARK),
    (400., GlyphThicken::Auto, DARK, LIGHT),
    (400., GlyphThicken::Strength(30), LIGHT, DARK),
    (400., GlyphThicken::Strength(30), DARK, LIGHT),
    (400., GlyphThicken::Strength(255), LIGHT, DARK),
    (400., GlyphThicken::Strength(255), DARK, LIGHT),
    (400., GlyphThicken::Off, LIGHT, DARK),
    (400., GlyphThicken::Off, DARK, LIGHT),
    (250., GlyphThicken::Auto, LIGHT, DARK),
    (250., GlyphThicken::Auto, DARK, LIGHT),
];
const ROW: f32 = 18.;
const WIDTH: f32 = 220.;

struct Scene {
    font_size: f32,
}

impl Render for Scene {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let font_size = self.font_size;
        canvas(
            |_, _, _| {},
            move |_, _, window, cx| {
                for (i, (weight, thicken, fg, bg)) in CASES.into_iter().enumerate() {
                    let top = point(px(0.), px(ROW * i as f32));
                    window.paint_quad(fill(Bounds::new(top, size(px(WIDTH), px(ROW))), rgb(bg)));
                    let mut font = font("JetBrains Mono");
                    font.weight = FontWeight(weight);
                    let run = TextRun { len: TEXT.len(), font, color: rgb(fg).into(), background_color: None, underline: None, strikethrough: None };
                    let line = window.text_system().shape_line(TEXT.into(), px(font_size), &[run], None);
                    window.with_glyph_thicken(thicken, |window| {
                        line.paint(top + point(px(2.), px(0.)), px(ROW), TextAlign::Left, None, window, cx).unwrap();
                    });
                }
            },
        )
        .size_full()
    }
}

// The panels, then a terminal grid inside them, both dark text on light.
const UI_ROWS: usize = 2;

struct UiScene {
    font_size: f32,
    ui_thicken: GlyphThicken,
}

impl Render for UiScene {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        window.set_glyph_thicken(self.ui_thicken);
        let font_size = self.font_size;
        canvas(
            |_, _, _| {},
            move |_, _, window, cx| {
                for i in 0..UI_ROWS {
                    let top = point(px(0.), px(ROW * i as f32));
                    window.paint_quad(fill(Bounds::new(top, size(px(WIDTH), px(ROW))), rgb(LIGHT)));
                    let run = TextRun { len: TEXT.len(), font: font("JetBrains Mono"), color: rgb(DARK).into(), background_color: None, underline: None, strikethrough: None };
                    let line = window.text_system().shape_line(TEXT.into(), px(font_size), &[run], None);
                    let at = top + point(px(2.), px(0.));
                    if i == 0 {
                        line.paint(at, px(ROW), TextAlign::Left, None, window, cx).unwrap();
                    } else {
                        window.with_glyph_thicken(GlyphThicken::Auto, |window| line.paint(at, px(ROW), TextAlign::Left, None, window, cx).unwrap());
                    }
                }
            },
        )
        .size_full()
    }
}

/// (interface ink, terminal ink) at this window strength.
fn ui_inks(window: &mut Window, cx: &mut App, ui_thicken: GlyphThicken) -> (f32, f32) {
    let _ = window.draw(cx);
    let image = window.render_to_image().expect("real GPU screenshot");
    let scale = window.scale_factor();
    let (ui, terminal) = (ink(&image, scale, 0, DARK, LIGHT), ink(&image, scale, 1, DARK, LIGHT));
    println!("scale {scale} ui_font_thicken {ui_thicken:?} dark on light: interface ink {ui:.4}, terminal ink {terminal:.4}");
    (ui, terminal)
}

const UI_THICKENS: [GlyphThicken; 2] = [GlyphThicken::Auto, GlyphThicken::Strength(255)];

fn check_ui([auto, on]: [(f32, f32); 2]) {
    assert!(on.0 > auto.0 * 1.05, "ui_font_thicken thickens the panels' dark text: {} to {}", auto.0, on.0);
    assert!((on.1 - auto.1).abs() < auto.1 * 0.001, "the terminal grid keeps its own: {} vs {}", auto.1, on.1);
    assert!((auto.0 - auto.1).abs() < auto.1 * 0.001, "auto, both draw as GPUI does: {} vs {}", auto.0, auto.1);
}

fn ink(image: &image::RgbaImage, scale: f32, i: usize, fg: u32, bg: u32) -> f32 {
    let channel = |c: u32, k: u32| ((c >> (16 - 8 * k)) & 0xff) as f32;
    let (y0, y1) = ((ROW * i as f32 * scale) as u32, (ROW * (i + 1) as f32 * scale) as u32);
    let (mut sum, mut n) = (0.0, 0);
    for y in y0..y1 {
        for x in 0..(WIDTH * scale) as u32 {
            let p = image.get_pixel(x, y).0;
            let mean = (0..3).map(|k| (p[k as usize] as f32 - channel(bg, k)) / (channel(fg, k) - channel(bg, k))).sum::<f32>() / 3.0;
            sum += mean.clamp(0.0, 1.0);
            n += 1;
        }
    }
    sum / n as f32
}

fn check(window: &mut Window, cx: &mut App) {
    let _ = window.draw(cx);
    let image = window.render_to_image().expect("real GPU screenshot");
    let scale = window.scale_factor();
    let inks: Vec<f32> = CASES.iter().enumerate().map(|(i, (_, _, fg, bg))| ink(&image, scale, i, *fg, *bg)).collect();
    for ((weight, thicken, fg, _), ink) in CASES.iter().zip(&inks) {
        let polarity = if *fg == DARK { "dark on light" } else { "light on dark" };
        println!("scale {scale} weight {weight} thicken {thicken:?} {polarity}: ink {ink:.4}");
    }
    let [auto_light, auto_dark, t30_light, t30_dark, t255_light, t255_dark, off_light, off_dark, ..] = inks[..] else { unreachable!() };
    assert!(auto_light > auto_dark * 1.05, "macOS smoothing thickens light text more ({auto_light} vs {auto_dark})");
    // One mask, tinted: thickened or unsmoothed polarities differ only by rounding.
    for (light, dark) in [(t30_light, t30_dark), (t255_light, t255_dark), (off_light, off_dark)] {
        assert!((light - dark).abs() < light * 0.01, "polarities match: {light} vs {dark}");
    }
    assert!(t255_dark > t30_dark * 1.05, "strength thickens: {t30_dark} to {t255_dark}");
    assert!(off_dark < auto_dark, "smoothing off draws lighter than the lightest smoothing: {off_dark} vs {auto_dark}");
}

fn add_font(cx: &mut App) {
    let bytes: &'static [u8] = include_bytes!("../assets/fonts/JetBrainsMono-Variable.ttf");
    cx.text_system().add_fonts(vec![bytes.into()]).unwrap();
}

#[cfg(target_os = "macos")]
fn headless() {
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
    let inks = UI_THICKENS.map(|ui_thicken| {
        let window = cx.open_window(size(px(WIDTH), px(ROW * UI_ROWS as f32)), |_, cx| cx.new(|_| UiScene { font_size: 6., ui_thicken })).unwrap();
        cx.update_window(window.into(), |_, window, cx| ui_inks(window, cx, ui_thicken)).unwrap()
    });
    check_ui(inks);
}

#[cfg(target_os = "macos")]
fn native() {
    gpui_platform::application().run(|cx| {
        add_font(cx);
        let options = |rows: usize| WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(point(px(0.), px(0.)), size(px(WIDTH), px(ROW * rows as f32))))),
            show: false,
            focus: false,
            titlebar: None,
            ..Default::default()
        };
        let window = cx.open_window(options(CASES.len()), |_, cx| cx.new(|_| Scene { font_size: 12. })).unwrap();
        let ui = UI_THICKENS.map(|ui_thicken| (ui_thicken, cx.open_window(options(UI_ROWS), |_, cx| cx.new(|_| UiScene { font_size: 12., ui_thicken })).unwrap()));
        cx.spawn(async move |cx| {
            cx.update_window(window.into(), |_, window, cx| check(window, cx)).unwrap();
            check_ui(ui.map(|(ui_thicken, w)| cx.update_window(w.into(), |_, window, cx| ui_inks(window, cx, ui_thicken)).unwrap()));
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
}

fn main() {
    #[cfg(target_os = "macos")]
    if std::env::var_os("HERDR_SHOT_NATIVE_TEST").is_some() {
        native();
    } else {
        headless();
    }
}
