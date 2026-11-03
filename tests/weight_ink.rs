#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]
//! Real CoreText rasterising and GPU readback: the window's weight shift
//! (`set_font_weight_shift`, ui_font_weight) makes the panels' regular text
//! heavier or lighter, keeps bold text as much bolder, and leaves a terminal
//! grid shaped inside `with_font_weight_shift(0)` at its own weight.
//! Prints each row's ink: the mean of (pixel - background) / (text -
//! background) over the text's box.
use gpui::{prelude::*, *};

const TEXT: &str = "the quick brown fox jumps over the lazy dog 0123456789";
const DARK: u32 = 0x1b1b1e;
const LIGHT: u32 = 0xe4e4e8;
// Rows: (font, weight the text asks for, shaped as the terminal grid). The
// bundled Geist is one static Regular face; Geist Mono is variable, as a
// ui_font must be to draw every step.
const ROWS: [(&str, Option<FontWeight>, bool); 3] = [("Geist Mono", None, false), ("Geist Mono", Some(FontWeight::SEMIBOLD), false), ("JetBrains Mono", Some(FontWeight::MEDIUM), true)];
const ROW: f32 = 18.;
const WIDTH: f32 = 220.;

struct Scene {
    font_size: f32,
    shift: f32,
}

impl Render for Scene {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        window.set_font_weight_shift(self.shift);
        let font_size = self.font_size;
        canvas(
            |_, _, _| {},
            move |_, _, window, cx| {
                for (i, (family, weight, terminal)) in ROWS.into_iter().enumerate() {
                    let top = point(px(0.), px(ROW * i as f32));
                    window.paint_quad(fill(Bounds::new(top, size(px(WIDTH), px(ROW))), rgb(LIGHT)));
                    let mut font = font(family);
                    font.weight = weight.unwrap_or_default();
                    let run = TextRun { len: TEXT.len(), font, color: rgb(DARK).into(), background_color: None, underline: None, strikethrough: None };
                    let shape = |window: &mut Window| window.text_system().shape_line(TEXT.into(), px(font_size), &[run], None);
                    let line = if terminal { window.with_font_weight_shift(0., shape) } else { shape(window) };
                    line.paint(top + point(px(2.), px(0.)), px(ROW), TextAlign::Left, None, window, cx).unwrap();
                }
            },
        )
        .size_full()
    }
}

/// (regular, semibold, terminal) ink at this shift.
fn inks(window: &mut Window, cx: &mut App, shift: f32) -> [f32; 3] {
    let _ = window.draw(cx);
    let image = window.render_to_image().expect("real GPU screenshot");
    let scale = window.scale_factor();
    let inks = [0, 1, 2].map(|i| ink(&image, scale, i));
    println!("scale {scale} shift {shift}: regular ink {:.4}, semibold ink {:.4}, terminal ink {:.4}", inks[0], inks[1], inks[2]);
    inks
}

fn check(by_shift: &[(f32, [f32; 3])]) {
    let at = |shift: f32| by_shift.iter().find(|(s, _)| *s == shift).unwrap().1;
    let (light, normal, heavy) = (at(-200.), at(0.), at(300.));
    assert!(heavy[0] > normal[0] * 1.1, "a higher ui_font_weight inks regular text more: {} to {}", normal[0], heavy[0]);
    assert!(light[0] < normal[0] * 0.9, "a lower one inks it less: {} to {}", normal[0], light[0]);
    for (shift, [regular, semibold, terminal]) in by_shift {
        assert!(semibold > regular, "shift {shift}: bold stays bolder ({semibold} vs {regular})");
        assert!((terminal - normal[2]).abs() < normal[2] * 0.001, "shift {shift}: the terminal keeps its weight ({terminal} vs {})", normal[2]);
    }
}

fn ink(image: &image::RgbaImage, scale: f32, i: usize) -> f32 {
    let channel = |c: u32, k: u32| ((c >> (16 - 8 * k)) & 0xff) as f32;
    let (y0, y1) = ((ROW * i as f32 * scale) as u32, (ROW * (i + 1) as f32 * scale) as u32);
    let (mut sum, mut n) = (0.0, 0);
    for y in y0..y1 {
        for x in 0..(WIDTH * scale) as u32 {
            let p = image.get_pixel(x, y).0;
            let mean = (0..3).map(|k| (p[k as usize] as f32 - channel(LIGHT, k)) / (channel(DARK, k) - channel(LIGHT, k))).sum::<f32>() / 3.0;
            sum += mean.clamp(0.0, 1.0);
            n += 1;
        }
    }
    sum / n as f32
}

fn add_fonts(cx: &mut App) {
    let geist: &'static [u8] = include_bytes!("../assets/fonts/GeistMono-Variable.ttf");
    let mono: &'static [u8] = include_bytes!("../assets/fonts/JetBrainsMono-Variable.ttf");
    cx.text_system().add_fonts(vec![geist.into(), mono.into()]).unwrap();
}

const SHIFTS: [f32; 3] = [-200., 0., 300.];

#[cfg(target_os = "macos")]
fn main() {
    let mut cx = HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        std::sync::Arc::new(()),
        gpui_platform::current_headless_renderer,
    );
    cx.update(add_fonts);
    // The test window is 2x: font_size 6 draws as 12 device pixels.
    let by_shift: Vec<(f32, [f32; 3])> = SHIFTS
        .into_iter()
        .map(|shift| {
            let window = cx.open_window(size(px(WIDTH), px(ROW * ROWS.len() as f32)), |_, cx| cx.new(|_| Scene { font_size: 6., shift })).unwrap();
            (shift, cx.update_window(window.into(), |_, window, cx| inks(window, cx, shift)).unwrap())
        })
        .collect();
    check(&by_shift);
}

#[cfg(not(target_os = "macos"))]
fn main() {}
