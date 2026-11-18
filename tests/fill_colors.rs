#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]
//! Solid fills reach the frame as the exact sRGB bytes they were given: the
//! ayu-mirage ANSI colours, background and a neutral, each painted as a
//! cell-sized quad, read back from the GPU. Headless frames have no layer, so
//! this guards GPUI's colors before the compositor, not the layer's sRGB tag
//! (`window_layers_are_tagged_srgb` in gpui-pre-apple covers that).

use gpui::{prelude::*, *};

const COLORS: [(&str, u32); 6] = [
    ("ansi1 red", 0xed8274),
    ("ansi2 green", 0x87d96c),
    ("ansi4 blue", 0x6dcbfa),
    ("ansi7 neutral", 0xc7c7c7),
    ("background", 0x1f2430),
    ("selection", 0x409fff),
];
const CELL: (f32, f32) = (9., 20.);

struct Cells;

impl Render for Cells {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        canvas(
            |_, _, _| {},
            |_, _, window, _| {
                for (i, (_, c)) in COLORS.iter().enumerate() {
                    let origin = point(px(CELL.0 * i as f32), px(0.));
                    window.paint_quad(fill(Bounds::new(origin, size(px(CELL.0), px(CELL.1))), rgb(*c)));
                }
            },
        )
        .size_full()
    }
}

#[cfg(target_os = "macos")]
fn main() {
    let text_system = gpui_platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(text_system, std::sync::Arc::new(()), gpui_platform::current_headless_renderer);
    let window = cx.open_window(size(px(CELL.0 * COLORS.len() as f32), px(CELL.1)), |_, cx| cx.new(|_| Cells)).unwrap();
    for scale in [2.0, 1.0] {
        cx.simulate_scale_factor_change(window.into(), scale);
        let image = cx
            .update_window(window.into(), |_, window, cx| {
                let _ = window.draw(cx);
                window.render_to_image().expect("real GPU readback")
            })
            .unwrap();
        let mut wrong = Vec::new();
        for (i, (name, c)) in COLORS.iter().enumerate() {
            let x = ((CELL.0 * (i as f32 + 0.5)) * scale) as u32;
            let p = image.get_pixel(x, (CELL.1 / 2. * scale) as u32).0;
            let got = (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32;
            println!("{scale}x {name}: #{c:06x} -> #{got:06x}");
            if got != *c {
                wrong.push(format!("{name} #{c:06x} drew #{got:06x}"));
            }
        }
        assert!(wrong.is_empty(), "fills changed at {scale}x: {wrong:?}");
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {}
