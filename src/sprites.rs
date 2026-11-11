//! Box drawing, block and braille characters drawn as shapes that fit their cell
//! exactly, as Ghostty does. Font glyphs for these rarely span the whole
//! cell, so tables get gaps between rows and block art gets seams.

use gpui::{Bounds, Hsla, PathBuilder, Pixels, Window, fill, point, px, size};

/// Per direction (up, right, down, left): 0 none, 1 light, 2 heavy, 3 double.
fn lines(c: char) -> Option<[u8; 4]> {
    const LIGHT_HEAVY: &str = "\
        0101 0202 1010 2020 0101 0202 1010 2020 0101 0202 1010 2020 \
        0110 0210 0120 0220 0011 0012 0021 0022 1100 1200 2100 2200 \
        1001 1002 2001 2002 1110 1210 2110 1120 2120 2210 1220 2220 \
        1011 1012 2011 1021 2021 2012 1022 2022 0111 0112 0211 0212 \
        0121 0122 0221 0222 1101 1102 1201 1202 2101 2102 2201 2202 \
        1111 1112 1211 1212 2111 1121 2121 2112 2211 1122 1221 2212 \
        1222 2122 2221 2222 0101 0202 1010 2020";
    const DOUBLE: &str = "\
        0303 3030 0310 0130 0330 0013 0031 0033 1300 3100 3300 1003 \
        3001 3003 1310 3130 3330 1013 3031 3033 0313 0131 0333 1303 \
        3101 3303 1313 3131 3333 0110 0011 1001 1100";
    const SINGLES: &str = "0001 1000 0100 0010 0002 2000 0200 0020 0201 1020 0102 2010";
    let code = c as u32;
    let (table, base) = match code {
        0x2500..=0x254F => (LIGHT_HEAVY, 0x2500),
        0x2550..=0x2570 => (DOUBLE, 0x2550),
        0x2574..=0x257F => (SINGLES, 0x2574),
        _ => return None,
    };
    let word = table.split_whitespace().nth((code - base) as usize)?;
    let mut out = [0u8; 4];
    for (o, b) in out.iter_mut().zip(word.bytes()) {
        *o = b - b'0';
    }
    Some(out)
}

/// Rounded corners (╭╮╯╰): which way their two arms leave the center,
/// (right or left, down or up). Drawn as arcs, as Ghostty does.
fn arc(c: char) -> Option<(f32, f32)> {
    match c {
        '╭' => Some((1.0, 1.0)),
        '╮' => Some((-1.0, 1.0)),
        '╯' => Some((-1.0, -1.0)),
        '╰' => Some((1.0, -1.0)),
        _ => None,
    }
}

/// Dashed lines: (dashes per cell, heavy, horizontal), as Ghostty draws them.
fn dashes(c: char) -> Option<(u8, bool, bool)> {
    Some(match c {
        '┄' => (3, false, true),
        '┅' => (3, true, true),
        '┆' => (3, false, false),
        '┇' => (3, true, false),
        '┈' => (4, false, true),
        '┉' => (4, true, true),
        '┊' => (4, false, false),
        '┋' => (4, true, false),
        '╌' => (2, false, true),
        '╍' => (2, true, true),
        '╎' => (2, false, false),
        '╏' => (2, true, false),
        _ => return None,
    })
}

/// Rectangles in cell fractions (x0, y0, x1, y1) and an alpha, for block elements.
fn blocks(c: char) -> Option<(Vec<[f32; 4]>, f32)> {
    let e = 1.0 / 8.0;
    let r = |x0: f32, y0: f32, x1: f32, y1: f32| [x0, y0, x1, y1];
    let quads = |ul: bool, ur: bool, ll: bool, lr: bool| {
        let mut v = Vec::new();
        if ul { v.push(r(0.0, 0.0, 0.5, 0.5)) }
        if ur { v.push(r(0.5, 0.0, 1.0, 0.5)) }
        if ll { v.push(r(0.0, 0.5, 0.5, 1.0)) }
        if lr { v.push(r(0.5, 0.5, 1.0, 1.0)) }
        v
    };
    Some(match c {
        '▀' => (vec![r(0.0, 0.0, 1.0, 0.5)], 1.0),
        '▁'..='█' => {
            let n = (c as u32 - '▁' as u32 + 1) as f32;
            (vec![r(0.0, 1.0 - n * e, 1.0, 1.0)], 1.0)
        }
        '▉'..='▏' => {
            let n = (8 - (c as u32 - '▉' as u32 + 1)) as f32;
            (vec![r(0.0, 0.0, n * e, 1.0)], 1.0)
        }
        '▐' => (vec![r(0.5, 0.0, 1.0, 1.0)], 1.0),
        '░' => (vec![r(0.0, 0.0, 1.0, 1.0)], 0.25),
        '▒' => (vec![r(0.0, 0.0, 1.0, 1.0)], 0.5),
        '▓' => (vec![r(0.0, 0.0, 1.0, 1.0)], 0.75),
        '▔' => (vec![r(0.0, 0.0, 1.0, e)], 1.0),
        '▕' => (vec![r(1.0 - e, 0.0, 1.0, 1.0)], 1.0),
        '▖' => (quads(false, false, true, false), 1.0),
        '▗' => (quads(false, false, false, true), 1.0),
        '▘' => (quads(true, false, false, false), 1.0),
        '▙' => (quads(true, false, true, true), 1.0),
        '▚' => (quads(true, false, false, true), 1.0),
        '▛' => (quads(true, true, true, false), 1.0),
        '▜' => (quads(true, true, false, true), 1.0),
        '▝' => (quads(false, true, false, false), 1.0),
        '▞' => (quads(false, true, true, false), 1.0),
        '▟' => (quads(false, true, true, true), 1.0),
        _ => return None,
    })
}

/// Ghostty's dot sizing, with vertical spacing even across adjacent cells.
/// Bit order is 1,2,3,7 down the left, 4,5,6,8 down the right.
fn braille(c: char, width: i32, height: i32) -> Option<Vec<[i32; 4]>> {
    if !('\u{2800}'..='\u{28FF}').contains(&c) {
        return None;
    }
    let mut dot = (width / 4).min(height / 8);
    let (mut xs, ys) = (width / 4, height / 8);
    let (mut xm, ym) = (xs / 2, ys / 2);
    let mut xr = width - 2 * xm - xs - 2 * dot;
    let mut yr = height - 2 * ym - 3 * ys - 4 * dot;
    if xr >= 2 && yr >= 4 && dot == 0 {
        dot += 1;
        xr -= 2;
        yr -= 4;
    }
    if xr >= 2 && xm == 0 {
        xm = 1;
        xr -= 2;
    }
    if yr >= 2 && ym == 0 {
        yr -= 2;
    }
    if xr >= 1 {
        xs += 1;
        xr -= 1;
    }
    if yr >= 3 {
        yr -= 3;
    }
    if xr >= 2 {
        xm += 1;
        xr -= 2;
    }
    if yr >= 2 {
        yr -= 2;
    }
    if xr >= 2 && yr >= 4 {
        dot += 1;
    }
    let mut rects = Vec::new();
    for (bit, col, row) in [(0, 0, 0), (1, 0, 1), (2, 0, 2), (3, 1, 0), (4, 1, 1), (5, 1, 2), (6, 0, 3), (7, 1, 3)] {
        if (c as u32) & (1 << bit) != 0 {
            let x = xm + col * (dot + xs);
            let y = ((2 * row + 1) * height - 4 * dot + 4) / 8;
            rects.push([x, y, x + dot, y + dot]);
        }
    }
    Some(rects)
}

/// ⏺ (Claude Code marks every step with it): few terminal fonts have it, and
/// off macOS the fallback may draw nothing. macOS always has a font for it,
/// so it draws from the fonts, fitted to its cell as other fallback symbols
/// are, matching Ghostty.
const RECORD: char = '\u{23FA}';

pub fn is_sprite(text: &str) -> bool {
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => lines(c).is_some() || blocks(c).is_some() || ('\u{2800}'..='\u{28FF}').contains(&c) || ('\u{E0B0}'..='\u{E0BF}').contains(&c) || (c == RECORD && !cfg!(target_os = "macos")),
        _ => false,
    }
}

/// The cell on whole device pixels: origin and size.
fn device_cell(cell: Bounds<Pixels>, scale: f32) -> (f32, f32, f32, f32) {
    let (x, y, w, h) = (f32::from(cell.origin.x), f32::from(cell.origin.y), f32::from(cell.size.width), f32::from(cell.size.height));
    let (dx, dy) = ((x * scale).round(), (y * scale).round());
    (dx, dy, ((x + w) * scale).round() - dx, ((y + h) * scale).round() - dy)
}

/// The centre of a stroke `t` px thick across `len` device pixels from
/// `origin`, as Ghostty places it: its near edge on device pixel
/// floor((len - t) / 2), so a light line sits above or left of the middle
/// when the two don't share parity.
fn centre(origin: f32, len: f32, t: f32, scale: f32) -> f32 {
    (origin + ((len - t * scale).max(0.0) / 2.0).floor()) / scale + t / 2.0
}

/// Paints `text` (one sprite character) into `cell`, snapped to device
/// pixels. `thickness` is a light line's, Ghostty's box thickness: the
/// font's underline thickness on whole device pixels.
pub fn paint(text: &str, cell: Bounds<Pixels>, thickness: Pixels, color: Hsla, window: &mut Window) {
    let Some(c) = text.chars().next() else { return };
    let scale = window.scale_factor();
    let snap = |v: f32| (v * scale).round() / scale;
    let (x, y, w, h) = (f32::from(cell.origin.x), f32::from(cell.origin.y), f32::from(cell.size.width), f32::from(cell.size.height));
    let mut rect = |x0: f32, y0: f32, x1: f32, y1: f32, color: Hsla| {
        let (x0, y0, x1, y1) = (snap(x0), snap(y0), snap(x1), snap(y1));
        if x1 > x0 && y1 > y0 {
            window.paint_quad(fill(Bounds::new(point(px(x0), px(y0)), size(px(x1 - x0), px(y1 - y0))), color));
        }
    };

    let (dx, dy, dw, dh) = device_cell(cell, scale);
    if let Some(dots) = braille(c, dw as i32, dh as i32) {
        for [x0, y0, x1, y1] in dots {
            rect((dx + x0 as f32) / scale, (dy + y0 as f32) / scale,
                (dx + x1 as f32) / scale, (dy + y1 as f32) / scale, color);
        }
        return;
    }
    if c == RECORD {
        let d = snap((w * 0.62).min(h * 0.5));
        let (x0, y0) = (snap(x + (w - d) / 2.0), snap(y + (h - d) / 2.0));
        window.paint_quad(fill(Bounds::new(point(px(x0), px(y0)), size(px(d), px(d))), color).corner_radii(px(d / 2.0)));
        return;
    }
    if ('\u{E0B0}'..='\u{E0BF}').contains(&c) {
        let line = (w / 8.0).round().max(1.0 / scale);
        powerline(c, (x, y, w, h), line, color, window);
        return;
    }
    if let Some((rects, alpha)) = blocks(c) {
        let mut color = color;
        color.a *= alpha;
        for [x0, y0, x1, y1] in rects {
            rect(x + x0 * w, y + y0 * h, x + x1 * w, y + y1 * h, color);
        }
        return;
    }
    let light = light_thickness(thickness, scale);
    if let Some((sx, sy)) = arc(c) {
        // Centred as the solid arms are, so the stroke meets them exactly.
        let (cx, cy) = (centre(dx, dw, light, scale), centre(dy, dh, light, scale));
        let ex = snap(if sx > 0.0 { x + w } else { x });
        let ey = snap(if sy > 0.0 { y + h } else { y });
        // Keep the turn inside the snapped cell, without doubling back.
        let (r, k) = ((ex - cx).abs().min((ey - cy).abs()), 0.25);
        let mut path = PathBuilder::stroke(px(light));
        path.move_to(point(px(cx), px(ey)));
        path.line_to(point(px(cx), px(cy + sy * r)));
        path.cubic_bezier_to(point(px(cx + sx * r), px(cy)), point(px(cx), px(cy + sy * k * r)), point(px(cx + sx * k * r), px(cy)));
        path.line_to(point(px(ex), px(cy)));
        if let Ok(path) = path.build() {
            window.paint_path(path, color);
        }
        return;
    }
    strokes(c, cell, scale, light, |x0, y0, x1, y1| rect(x0, y0, x1, y1, color));
}

/// A light line's thickness: `thickness` on whole device pixels, at least one.
fn light_thickness(thickness: Pixels, scale: f32) -> f32 {
    (f32::from(thickness) * scale).round().max(1.0) / scale
}

/// The rectangles (x0, y0, x1, y1) of a straight or dashed box-drawing
/// character, before snapping. Each stroke is centred as Ghostty centres it
/// (`centre`), so a stroke's edges fall on device pixels.
fn strokes(c: char, cell: Bounds<Pixels>, scale: f32, light: f32, mut rect: impl FnMut(f32, f32, f32, f32)) {
    let (dx, dy, dw, dh) = device_cell(cell, scale);
    let (left, top, right, bottom) = (dx / scale, dy / scale, (dx + dw) / scale, (dy + dh) / scale);
    // Centres of a vertical (cx) or horizontal (cy) stroke `t` thick.
    let cx = |t: f32| centre(dx, dw, t, scale);
    let cy = |t: f32| centre(dy, dh, t, scale);
    if let Some((n, heavy, horizontal)) = dashes(c) {
        // Ghostty's layout, on whole device pixels: gaps of 4px (or a light
        // line, if wider) but at most half the cell, and leftover pixels added
        // to the first dashes. A horizontal line keeps half a gap at its left
        // end and a vertical one its gap below, so neighbors tile evenly. A
        // cell too small for them gets a solid light line.
        let (origin, len, n) = if horizontal { (dx, dw, n as f32) } else { (dy, dh, n as f32) };
        let (t, count, gap) = if len < 2.0 * n {
            (light, 1.0, 0.0)
        } else {
            (if heavy { light * 2.0 } else { light }, n, (light * scale).max(4.0).min((len / (2.0 * n)).floor()))
        };
        let dash = ((len - count * gap) / count).floor();
        let mut extra = (len - count * gap) - dash * count;
        let mut at = if horizontal { (gap / 2.0).floor() } else { 0.0 };
        for _ in 0..count as usize {
            let end = at + dash + if extra > 0.0 { 1.0 } else { 0.0 };
            extra -= 1.0;
            let (a0, a1) = ((origin + at) / scale, (origin + end) / scale);
            if horizontal {
                rect(a0, cy(t) - t / 2.0, a1, cy(t) + t / 2.0);
            } else {
                rect(cx(t) - t / 2.0, a0, cx(t) + t / 2.0, a1);
            }
            at = end + gap;
        }
        return;
    }
    let Some(dirs) = lines(c) else { return };
    let thick = |k: u8| if k == 2 { light * 2.0 } else { light };
    // Where a line along an arm starts, measured toward the arm from the
    // centre of the crossing stroke it reaches (the second value's thickness):
    // `before` and `after` are the crossing arms on this line's side and the
    // other side. Negative reaches past the centre so joins close.
    let cover = |k: u8| if k == 3 { light * 1.5 } else { thick(k) / 2.0 };
    let start = |before: u8, after: u8, double: bool, opposite_differs: bool| -> (f32, f32) {
        match (before, after) {
            // A double line stops at a double crossing on its side (inner corner).
            (3, _) if double => (light / 2.0, light),
            // With nothing crossing, arms overlap by a light line's width, as in Ghostty.
            (0, 0) => (-light / 2.0, light),
            // A T onto a matching pair of crossing lines stops at their near
            // edge, as in Ghostty, so ╧ and ╟ keep the double line's gap open.
            (k, j) if k == j && k != 2 && opposite_differs => (light / 2.0, light),
            (0, k) | (k, _) => (-cover(k), thick(k)),
        }
    };
    // Arms: (direction, sign along the axis, horizontal?).
    for (i, sign, horizontal) in [(0usize, -1.0f32, false), (1, 1.0, true), (2, 1.0, false), (3, -1.0, true)] {
        let k = dirs[i];
        if k == 0 {
            continue;
        }
        // The crossing arms on each side of this arm (up/down for a horizontal arm).
        let (side_a, side_b) = if horizontal { (dirs[0], dirs[2]) } else { (dirs[3], dirs[1]) };
        let (a, b) = if side_a >= side_b { (side_a, side_b) } else { (side_b, side_a) };
        let doubled = [(-light, light, side_a, side_b), (light, light, side_b, side_a)];
        let single = [(0.0, thick(k), a, b)];
        let lines: &[(f32, f32, u8, u8)] = if k == 3 { &doubled } else { &single };
        for &(off, t, before, after) in lines {
            let (near, crossing) = start(before, after, k == 3, dirs[(i + 2) % 4] != k);
            let (c, edge) = if horizontal { (cx(crossing), if sign > 0.0 { right } else { left }) } else { (cy(crossing), if sign > 0.0 { bottom } else { top }) };
            let from = c + sign * near;
            let (a0, a1) = if sign > 0.0 { (from, edge) } else { (edge, from) };
            if horizontal {
                rect(a0, cy(t) + off - t / 2.0, a1, cy(t) + off + t / 2.0);
            } else {
                rect(cx(t) + off - t / 2.0, a0, cx(t) + off + t / 2.0, a1);
            }
        }
    }
}

/// Nerd Font powerline separators (U+E0B0..E0BF): arrows, half circles and
/// slants, drawn to fill the cell exactly as Ghostty does. Fonts that have
/// them rarely match the cell, and fonts that don't fall back to whatever
/// font has those codes (on Linux, sometimes digits).
fn powerline(c: char, (x, y, w, h): (f32, f32, f32, f32), line: f32, color: Hsla, window: &mut Window) {
    let solid = (c as u32).is_multiple_of(2);
    // A solid shape reaches a device pixel past its cell's sides, so its flat
    // edge meets the neighbor's color without an anti-aliased seam.
    let ext = if solid { 1.0 / window.scale_factor() } else { 0.0 };
    let p = |px_: f32, py_: f32| point(px(x + px_ * w + (px_ - 0.5) * 2.0 * ext), px(y + py_ * h));
    let mut path = if solid { PathBuilder::fill() } else { PathBuilder::stroke(px(line)) };
    // Half an ellipse filling the cell: four quarter arcs as cubics.
    const K: f32 = 0.552_284_8;
    match c {
        '\u{E0B0}' | '\u{E0B1}' => path.add_polygon(&[p(0.0, 0.0), p(1.0, 0.5), p(0.0, 1.0)], solid),
        '\u{E0B2}' | '\u{E0B3}' => path.add_polygon(&[p(1.0, 0.0), p(0.0, 0.5), p(1.0, 1.0)], solid),
        '\u{E0B4}' | '\u{E0B5}' => {
            path.move_to(p(0.0, 0.0));
            path.cubic_bezier_to(p(1.0, 0.5), p(K, 0.0), p(1.0, 0.5 - K / 2.0));
            path.cubic_bezier_to(p(0.0, 1.0), p(1.0, 0.5 + K / 2.0), p(K, 1.0));
            if solid {
                path.close();
            }
        }
        '\u{E0B6}' | '\u{E0B7}' => {
            path.move_to(p(1.0, 0.0));
            path.cubic_bezier_to(p(0.0, 0.5), p(1.0 - K, 0.0), p(0.0, 0.5 - K / 2.0));
            path.cubic_bezier_to(p(1.0, 1.0), p(0.0, 0.5 + K / 2.0), p(1.0 - K, 1.0));
            if solid {
                path.close();
            }
        }
        '\u{E0B8}' => path.add_polygon(&[p(0.0, 0.0), p(1.0, 1.0), p(0.0, 1.0)], true),
        '\u{E0BA}' => path.add_polygon(&[p(1.0, 0.0), p(1.0, 1.0), p(0.0, 1.0)], true),
        '\u{E0BC}' => path.add_polygon(&[p(0.0, 0.0), p(1.0, 0.0), p(0.0, 1.0)], true),
        '\u{E0BE}' => path.add_polygon(&[p(0.0, 0.0), p(1.0, 0.0), p(1.0, 1.0)], true),
        '\u{E0B9}' | '\u{E0BF}' => path.add_polygon(&[p(0.0, 0.0), p(1.0, 1.0)], false),
        _ => path.add_polygon(&[p(0.0, 1.0), p(1.0, 0.0)], false), // E0BB, E0BD
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn braille_patterns_use_all_eight_bits_and_stay_in_the_cell() {
        assert!(!is_sprite("\u{27FF}") && !is_sprite("\u{2900}"));
        assert!(!is_sprite("⠁⠂") && !is_sprite("⠁\u{FE0F}"));
        assert_eq!(braille('A', 8, 22), None);
        let positions = [(1, 2), (1, 7), (1, 13), (5, 2), (5, 7), (5, 13), (1, 18), (5, 18)];
        for pattern in 0u32..256 {
            let c = char::from_u32(0x2800 + pattern).unwrap();
            assert!(is_sprite(&c.to_string()));
            let expected: Vec<_> = positions.iter().enumerate()
                .filter(|(bit, _)| pattern & (1 << bit) != 0)
                .map(|(_, &(x, y))| [x, y, x + 2, y + 2]).collect();
            assert_eq!(braille(c, 8, 22).unwrap(), expected);
        }
        for w in 0..80 {
            for h in 0..160 {
                for [x0, y0, x1, y1] in braille('⣿', w, h).unwrap() {
                    assert!(0 <= x0 && x0 <= x1 && x1 <= w);
                    assert!(0 <= y0 && y0 <= y1 && y1 <= h);
                    assert_eq!(x1 - x0, y1 - y0);
                }
                let dots = braille('⡇', w, h).unwrap();
                let gaps = [dots[1][1] - dots[0][1], dots[2][1] - dots[1][1], dots[3][1] - dots[2][1], h + dots[0][1] - dots[3][1]];
                assert!(gaps.iter().max().unwrap() - gaps.iter().min().unwrap() <= 1, "{w}x{h}: {gaps:?}");
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rendered_braille_matches_device_pixel_layout() {
        use gpui::{prelude::*, *};

        struct BrailleScene { w: f32, h: f32 }
        impl Render for BrailleScene {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let (w, h) = (self.w, self.h);
                div().size_full().bg(rgb(0)).child(canvas(|_, _, _| {}, move |_, _, window, _| {
                    for (col, c) in ['⠀', '⠁', '⠂', '⠄', '⠈', '⠐', '⠠', '⡀', '⢀', '⣿'].into_iter().enumerate() {
                        paint(&c.to_string(), Bounds::new(
                            point(px(7.3 + col as f32 * w), px(5.7)),
                            size(px(w), px(h)),
                        ), px(1.0), rgb(0xffffff).into(), window);
                    }
                }).size_full())
            }
        }
        let mut cx = HeadlessAppContext::with_platform(
            gpui_platform::current_platform(true).text_system(),
            std::sync::Arc::new(()),
            gpui_platform::current_headless_renderer,
        );
        for (w, h) in [(4.2, 11.), (5.04, 13.2), (8.4, 22.), (10.08, 26.4)] {
            let handle = cx.open_window(size(px(120.), px(40.)), |_, cx| cx.new(|_| BrailleScene { w, h })).unwrap();
            cx.update_window(handle.into(), |_, window, cx| {
                let scale = window.scale_factor();
                let _ = window.draw(cx);
                let image = window.render_to_image().unwrap();
                let snap = |v: f32| (v * scale).round() as i32;
                for (col, c) in ['⠀', '⠁', '⠂', '⠄', '⠈', '⠐', '⠠', '⡀', '⢀', '⣿'].into_iter().enumerate() {
                    let (x, y) = (snap(7.3 + col as f32 * w), snap(5.7));
                    let (dw, dh) = (snap(7.3 + (col + 1) as f32 * w) - x, snap(5.7 + h) - y);
                    let dots = braille(c, dw, dh).unwrap();
                    for py in 0..dh {
                        for px in 0..dw {
                            let on = dots.iter().any(|&[x0, y0, x1, y1]| x0 <= px && px < x1 && y0 <= py && py < y1);
                            let actual = image.get_pixel((x + px) as u32, (y + py) as u32).0[0];
                            assert_eq!(actual, if on { 255 } else { 0 }, "{c}: {w}x{h} at {px},{py}");
                        }
                    }
                }
            }).unwrap();
        }
    }

    #[test]
    fn tables_cover_their_ranges() {
        assert_eq!(lines('─'), Some([0, 1, 0, 1]));
        assert_eq!(lines('┼'), Some([1, 1, 1, 1]));
        assert_eq!(lines('╋'), Some([2, 2, 2, 2]));
        assert_eq!(lines('┌'), Some([0, 1, 1, 0]));
        assert_eq!(lines('┘'), Some([1, 0, 0, 1]));
        assert_eq!(lines('╏'), Some([2, 0, 2, 0]));
        assert_eq!(lines('╔'), Some([0, 3, 3, 0]));
        assert_eq!(lines('╬'), Some([3, 3, 3, 3]));
        assert_eq!(lines('╭'), Some([0, 1, 1, 0]));
        assert_eq!(lines('╰'), Some([1, 1, 0, 0]));
        assert_eq!(lines('╱'), None);
        assert_eq!(lines('╿'), Some([2, 0, 1, 0]));
        for c in ('\u{2500}'..='\u{2570}').chain('\u{2574}'..='\u{257F}') {
            assert!(lines(c).is_some(), "{c} has no entry");
        }
        for c in '\u{2580}'..='\u{259F}' {
            assert!(blocks(c).is_some(), "{c} has no entry");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rendered_rounded_corners_join_straight_lines() {
        use gpui::{prelude::*, *};

        struct BoxScene { w: f32, h: f32, t: f32 }
        impl Render for BoxScene {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let (w, h, t) = (self.w, self.h, self.t);
                div().size_full().bg(rgb(0)).child(canvas(|_, _, _| {}, move |_, _, window, _| {
                    for (row, text) in ["╭─╮", "│ │", "╰─╯"].iter().enumerate() {
                        for (col, c) in text.chars().enumerate() {
                            paint(&c.to_string(), Bounds::new(
                                point(px(7.3 + col as f32 * w), px(5.7 + row as f32 * h)),
                                size(px(w), px(h)),
                            ), px(t), rgb(0xffffff).into(), window);
                        }
                    }
                }).size_full())
            }
        }

        let mut cx = HeadlessAppContext::with_platform(
            gpui_platform::current_platform(true).text_system(),
            std::sync::Arc::new(()),
            gpui_platform::current_headless_renderer,
        );
        // The headless platform renders at 2x: a 0.5px line is one device pixel.
        for (w, h, t) in [0.5, 1.0].into_iter().flat_map(|t| [(4.2, 11., t), (5.46, 14.3, t), (8.4, 22., t), (10.92, 28.6, t), (16., 32., t)]) {
            let handle = cx.open_window(size(px(80.), px(120.)), |_, cx| cx.new(|_| BoxScene { w, h, t })).unwrap();
            cx.update_window(handle.into(), |_, window, cx| {
                let scale = window.scale_factor();
                let _ = window.draw(cx);
                let image = window.render_to_image().unwrap();
                let snap = |v: f32| (v * scale).round() as u32;
                // Compare each arc endpoint with the neighboring solid arm,
                // including the background on either side of the stroke.
                for row in [0, 2] {
                    let cy = snap(5.7 + row as f32 * h + h / 2.);
                    for boundary in [1, 2] {
                        let edge = snap(7.3 + boundary as f32 * w);
                        for y in cy - 3..=cy + 3 {
                            let a = image.get_pixel(edge - 1, y).0[0];
                            let b = image.get_pixel(edge, y).0[0];
                            assert!(a.abs_diff(b) <= 24, "horizontal join: {w}x{h} scale {scale}, row {row}, edge {boundary}, y {y}: {a} != {b}");
                        }
                    }
                }
                for col in [0, 2] {
                    let cx = snap(7.3 + col as f32 * w + w / 2.);
                    for boundary in [1, 2] {
                        let edge = snap(5.7 + boundary as f32 * h);
                        for x in cx - 3..=cx + 3 {
                            let a = image.get_pixel(x, edge - 1).0[0];
                            let b = image.get_pixel(x, edge).0[0];
                            assert!(a.abs_diff(b) <= 24, "vertical join: {w}x{h} scale {scale}, col {col}, edge {boundary}, x {x}: {a} != {b}");
                        }
                    }
                }
            }).unwrap();
        }
    }

    /// The device pixels `c`'s strokes cover in a `w` x `h` cell at `scale`,
    /// row by row, with a light line `thickness` px thick.
    fn stroke_mask(c: char, (w, h): (f32, f32), scale: f32, thickness: f32) -> Vec<Vec<bool>> {
        let cell = Bounds::new(point(px(0.0), px(0.0)), size(px(w), px(h)));
        let (_, _, dw, dh) = device_cell(cell, scale);
        let mut mask = vec![vec![false; dw as usize]; dh as usize];
        let at = |v: f32| (v * scale).round() as usize;
        strokes(c, cell, scale, light_thickness(px(thickness), scale), |x0, y0, x1, y1| {
            for row in &mut mask[at(y0)..at(y1)] {
                row[at(x0)..at(x1)].fill(true);
            }
        });
        mask
    }

    /// A mask of full-width rows `rows` and full-height columns `cols`.
    fn cross_mask((dw, dh): (usize, usize), rows: std::ops::Range<usize>, cols: std::ops::Range<usize>) -> Vec<Vec<bool>> {
        (0..dh).map(|y| (0..dw).map(|x| rows.contains(&y) || cols.contains(&x)).collect()).collect()
    }

    #[test]
    fn lines_sit_on_ghosttys_device_pixels() {
        // (cell, scale, light thickness px, light rows, light cols, heavy rows, heavy cols),
        // from Ghostty's linesChar: a stroke's top/left edge at floor((cell - stroke) / 2).
        let cases = [
            ((7.0, 16.0), 1.0, 1.0, 7..8, 3..4, 7..9, 2..4),
            ((7.0, 16.0), 2.0, 1.0, 15..17, 6..8, 14..18, 5..9),
            ((14.0, 32.0), 1.0, 2.0, 15..17, 6..8, 14..18, 5..9),
            ((14.0, 32.0), 2.0, 2.0, 30..34, 12..16, 28..36, 10..18),
        ];
        for (cell, scale, thickness, light_rows, light_cols, heavy_rows, heavy_cols) in cases {
            let device = ((cell.0 * scale) as usize, (cell.1 * scale) as usize);
            let check = |c: char, rows: std::ops::Range<usize>, cols: std::ops::Range<usize>| {
                assert_eq!(stroke_mask(c, cell, scale, thickness), cross_mask(device, rows, cols), "{c} in {cell:?} at {scale}x");
            };
            check('─', light_rows.clone(), 0..0);
            check('│', 0..0, light_cols.clone());
            check('┼', light_rows.clone(), light_cols.clone());
            check('━', heavy_rows.clone(), 0..0);
            check('┃', 0..0, heavy_cols.clone());
            check('╋', heavy_rows, heavy_cols);
        }
    }

    /// A `dw` x `dh` mask lit where `on(x, y)`.
    fn mask_of((dw, dh): (usize, usize), on: impl Fn(usize, usize) -> bool) -> Vec<Vec<bool>> {
        (0..dh).map(|y| (0..dw).map(|x| on(x, y)).collect()).collect()
    }

    #[test]
    fn dashes_follow_ghosttys_pixel_layout() {
        // From Ghostty's dashHorizontal / dashVertical: whole-pixel gaps of
        // min(max(4, light), cell / 2n), leftover pixels on the first dashes.
        assert_eq!(stroke_mask('┄', (7.0, 16.0), 1.0, 1.0), mask_of((7, 16), |x, y| y == 7 && [0, 1, 3, 5].contains(&x)));
        assert_eq!(stroke_mask('┄', (7.0, 16.0), 2.0, 1.0), mask_of((14, 32), |x, y| (15..17).contains(&y) && [1, 2, 3, 6, 7, 8, 11, 12].contains(&x)));
        assert_eq!(stroke_mask('┆', (7.0, 16.0), 1.0, 1.0), mask_of((7, 16), |x, y| x == 3 && [0, 1, 2, 3, 6, 7, 8, 11, 12, 13].contains(&y)));
        assert_eq!(stroke_mask('┅', (7.0, 16.0), 1.0, 1.0), mask_of((7, 16), |x, y| (7..9).contains(&y) && [0, 1, 3, 5].contains(&x)));
        // Too narrow for four dashes and their gaps: a solid light line.
        assert_eq!(stroke_mask('┉', (7.0, 16.0), 1.0, 1.0), mask_of((7, 16), |_, y| y == 7));
    }

    #[test]
    fn a_t_onto_a_double_line_keeps_its_gap_open() {
        // Ghostty stops ╧'s up arm at the upper of the two lines and ╟'s
        // right arm at the right one. Only a crossing (╪ ╫) runs through.
        let double_rows = |y: usize| y == 6 || y == 8;
        assert_eq!(stroke_mask('╧', (7.0, 16.0), 1.0, 1.0), mask_of((7, 16), |x, y| double_rows(y) || (x == 3 && y < 7)));
        assert_eq!(stroke_mask('╤', (7.0, 16.0), 1.0, 1.0), mask_of((7, 16), |x, y| double_rows(y) || (x == 3 && y > 8)));
        assert_eq!(stroke_mask('╪', (7.0, 16.0), 1.0, 1.0), mask_of((7, 16), |x, y| double_rows(y) || x == 3));
        let double_cols = |x: usize| x == 2 || x == 4;
        assert_eq!(stroke_mask('╟', (7.0, 16.0), 1.0, 1.0), mask_of((7, 16), |x, y| double_cols(x) || (y == 7 && x > 4)));
        assert_eq!(stroke_mask('╫', (7.0, 16.0), 1.0, 1.0), mask_of((7, 16), |x, y| double_cols(x) || y == 7));
    }

    #[test]
    fn line_thickness_follows_the_fonts_underline() {
        // A 12x24 cell's font can have a 1px underline, where an eighth of
        // the cell would round to 2px: Ghostty draws its box lines 1px.
        assert_eq!(stroke_mask('─', (12.0, 24.0), 1.0, 1.0), cross_mask((12, 24), 11..12, 0..0));
        assert_eq!(stroke_mask('━', (12.0, 24.0), 1.0, 1.0), cross_mask((12, 24), 11..13, 0..0));
        assert_eq!(stroke_mask('│', (12.0, 24.0), 1.0, 1.0), cross_mask((12, 24), 0..0, 5..6));
        // Never thinner than a device pixel.
        assert_eq!(stroke_mask('─', (12.0, 24.0), 2.0, 0.1), cross_mask((24, 48), 23..24, 0..0));
    }

    #[test]
    fn half_lines_reach_past_the_centre_as_in_ghostty() {
        // Ghostty ends a lone arm at the far edge of a light vertical line.
        assert_eq!(stroke_mask('╴', (7.0, 16.0), 1.0, 1.0), (0..16).map(|y| (0..7).map(|x| y == 7 && x < 4).collect()).collect::<Vec<Vec<bool>>>());
        assert_eq!(stroke_mask('╶', (7.0, 16.0), 1.0, 1.0), (0..16).map(|y| (0..7).map(|x| y == 7 && x >= 3).collect()).collect::<Vec<Vec<bool>>>());
        assert_eq!(stroke_mask('╵', (7.0, 16.0), 1.0, 1.0), (0..16).map(|y| (0..7).map(|x| x == 3 && y < 8).collect()).collect::<Vec<Vec<bool>>>());
    }

    #[test]
    fn rounded_corners_and_dashes_have_their_own_shapes() {
        for c in ['╭', '╮', '╯', '╰'] {
            assert!(arc(c).is_some() && is_sprite(&c.to_string()), "{c}");
        }
        for c in ['┄', '┅', '┆', '┇', '┈', '┉', '┊', '┋', '╌', '╍', '╎', '╏'] {
            assert!(dashes(c).is_some() && is_sprite(&c.to_string()), "{c}");
        }
        assert_eq!(arc('┌'), None);
        assert_eq!(dashes('─'), None);
    }
}
