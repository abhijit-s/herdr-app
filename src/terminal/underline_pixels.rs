//! What the grid paints for underline and strikethrough styles, rendered
//! headless on the GPU and read back. An ignored measurement: run with
//! `HERDR_UNDERLINE_DUMP=<settings.json>` (only read) to compare two builds
//! line by line. Each test line prints its cell band's device-pixel rows,
//! each as a hash of its pixels and how many differ from the background.

use gpui::*;
use std::sync::Arc;

struct Root {
    terminal: Entity<super::TerminalView>,
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.terminal.clone())
    }
}

const LINES: [(&str, &str); 14] = [
    ("plain", ""),
    ("single", "\x1b[4m"),
    ("strike", "\x1b[9m"),
    ("single+strike", "\x1b[4;9m"),
    ("faint single", "\x1b[2;4m"),
    ("red text single", "\x1b[31;4m"),
    ("double", "\x1b[4:2m"),
    ("double SGR 21", "\x1b[21m"),
    ("curly", "\x1b[4:3m"),
    ("dotted", "\x1b[4:4m"),
    ("dashed", "\x1b[4:5m"),
    ("single SGR 58 rgb", "\x1b[4;58;2;255;0;0m"),
    ("curly SGR 58 index", "\x1b[4:3;58;5;196m"),
    ("bold single", "\x1b[1;4m"),
];

const TEXT: &str = "gjpqy Hello_world 0Oo il1 ";

fn fnv(bytes: impl IntoIterator<Item = u8>) -> u64 {
    bytes.into_iter().fold(0xcbf29ce484222325, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    })
}

#[::core::prelude::v1::test]
#[ignore = "a measurement dump: set HERDR_UNDERLINE_DUMP to a settings.json"]
fn underline_pixels() {
    let path = std::env::var("HERDR_UNDERLINE_DUMP").expect("HERDR_UNDERLINE_DUMP=<settings.json>");
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let person: crate::settings::Person = serde_json::from_value(json).unwrap();
    crate::settings::set(crate::settings::Settings::compose(
        &person,
        &crate::layout::Look::default(),
        true,
    ));

    let mut cx = HeadlessAppContext::with_platform(
        Arc::new(gpui_macos::MacTextSystem::new()),
        Arc::new(()),
        gpui_platform::current_headless_renderer,
    );
    let rows = LINES.len() as u16 + 1;
    let mut terminal = None;
    let window = cx
        .open_window(size(px(420.), px(20. * rows as f32)), |_, cx| {
            let view = cx.new(|cx| {
                super::TerminalView::attach(crate::hc::offline(), "t1".into(), (60, rows), cx)
            });
            terminal = Some(view.clone());
            cx.new(|_| Root { terminal: view })
        })
        .unwrap();
    let terminal = terminal.unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        let _ = window.draw(cx);
        terminal.update(cx, |t, cx| {
            let mut out = String::from("\x1b[?25l\x1b[2J");
            for (y, (_, sgr)) in LINES.iter().enumerate() {
                out.push_str(&format!("\x1b[{};1H{sgr}{TEXT}\x1b[0m", y + 1));
            }
            t.grid.borrow_mut().vt.write(out.as_bytes());
            cx.notify();
        });
        let _ = window.draw(cx);
        let image = window.render_to_image().expect("real GPU readback");
        let (origin, cell, scale) = {
            let t = terminal.read(cx);
            let g = t.grid.borrow();
            (g.origin, g.cell, window.scale_factor())
        };
        let bg = image.get_pixel(1, 1).0;
        println!(
            "scale {scale} cell {:?} origin {:?} image {:?} bg {bg:?}",
            cell,
            origin,
            image.dimensions()
        );
        let x0 = (f32::from(origin.x) * scale).round() as u32;
        let x1 = ((f32::from(origin.x) + f32::from(cell.width) * TEXT.len() as f32) * scale).round()
            as u32;
        for (n, (name, _)) in LINES.iter().enumerate() {
            let top = f32::from(origin.y) + f32::from(cell.height) * n as f32;
            let (y0, y1) = (
                (top * scale).round() as u32,
                ((top + f32::from(cell.height)) * scale).round() as u32,
            );
            let mut red = 0;
            for y in y0..y1.min(image.height()) {
                let pixels: Vec<[u8; 4]> = (x0..x1.min(image.width()))
                    .map(|x| image.get_pixel(x, y).0)
                    .collect();
                let inked = pixels.iter().filter(|p| **p != bg).count();
                red += pixels
                    .iter()
                    .filter(|p| p[0] > 180 && p[1] < 90 && p[2] < 90)
                    .count();
                println!(
                    "{name} row {:2}: {:016x} inked {inked}",
                    y - y0,
                    fnv(pixels.iter().flatten().copied())
                );
            }
            println!("{name} red pixels {red}");
        }
    })
    .unwrap();
}
