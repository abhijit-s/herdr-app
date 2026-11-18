//! How symbols, and Nerd Font icons from the bundled Symbols Nerd Font, fit
//! their cells: Ghostty's symbol set and constraint width
//! (`src/renderer/cell.zig`) and its constraints (`src/font/Glyph.zig` `constrain`), with its
//! per-range rules from `src/font/nerd_font_tables.zig`, which
//! `src/font/nerd_font_codegen.py` generates from Nerd Fonts' font-patcher.

use std::ops::RangeInclusive;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Size {
    /// Shrink to fit the box if it's larger, otherwise keep the icon's size.
    Fit,
    /// Shrink to fit the box, grow to cover one cell, otherwise keep the
    /// icon's own size (Ghostty's `fit_cover1`).
    FitCover1,
    /// Scale up or down to just fit the box.
    Cover,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Height {
    /// The face's height.
    Cell,
    /// Ghostty's icon height: the face's for two cells, (2 cap height +
    /// face height) / 3 for one.
    Icon,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Align {
    Start,
    End,
    /// Centred on the first cell, however many cells it may use.
    Center1,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rule {
    pub size: Size,
    pub height: Height,
    pub max_cells: u8,
    pub align: Align,
    /// Left, right, top and bottom, in face widths and heights.
    pub pad: [f32; 4],
}

const fn rule(size: Size, height: Height, max_cells: u8, align: Align, pad: f32) -> Rule {
    Rule { size, height, max_cells, align, pad: [pad; 4] }
}

/// A private-use character outside the icon sets: shrunk to fit if it's too
/// large, as Ghostty fits any symbol. Ghostty leaves its position alone;
/// here it's centred like the icons.
pub const DEFAULT: Rule = rule(Size::Fit, Height::Cell, 2, Align::Center1, 0.0);

/// The icon sets' rule.
const ICON: Rule = rule(Size::FitCover1, Height::Icon, 2, Align::Center1, 0.0);

use {Align::*, Height::*, Size::*};

/// The private-use ranges and their rules, from Ghostty
/// `nerd_font_tables.zig` (vendor/ghostty `f39ed1bd`; its lookup is
/// `stage3[stage2[stage1[cp >> 8] + (cp & 0xff)]]`), its scale groups
/// (`relative_*`, which size a few multi-glyph groups alike) left out.
/// E0B0–E0BF are Herdr's sprites and aren't here. Ghostty stretches the powerline extras (E0C0–E0D7, EE00–EE05) to their box
/// without keeping their aspect; an icon is drawn at one font size, so they
/// cover the box instead, and stretch's own negative padding (which Ghostty
/// clamps to 0) is 0 here.
const RULES: &[(RangeInclusive<u32>, Rule)] = &[
    (0xE000..=0xE00A, ICON),
    (0xE200..=0xE2A9, ICON),
    (0xE300..=0xE3E3, ICON),
    (0xE5FA..=0xE6B8, ICON),
    (0xE700..=0xE8EF, ICON),
    (0xEA60..=0xEA88, ICON),
    (0xEA8A..=0xEA8C, ICON),
    (0xEA8F..=0xEAC7, ICON),
    (0xEAC9..=0xEAC9, ICON),
    (0xEACC..=0xEB09, ICON),
    (0xEB0B..=0xEB4E, ICON),
    (0xEB50..=0xEC1E, ICON),
    (0xED00..=0xEDFF, ICON),
    (0xEE0C..=0xEFCE, ICON),
    (0xF000..=0xF381, ICON),
    (0xF400..=0xF533, ICON),
    (0xF0001..=0xF1AF0, ICON),
    (0xE0A0..=0xE0A3, rule(FitCover1, Cell, 2, Center1, 0.0)),
    (0xE0C0..=0xE0C1, rule(Cover, Cell, 2, Start, 0.0)),
    (0xE0C2..=0xE0C3, rule(Cover, Cell, 2, End, 0.0)),
    (0xE0C4..=0xE0C4, rule(Cover, Cell, 2, Start, 0.015)),
    (0xE0C5..=0xE0C5, rule(Cover, Cell, 2, End, 0.015)),
    (0xE0C6..=0xE0C6, rule(Cover, Cell, 2, Start, 0.015)),
    (0xE0C7..=0xE0C7, rule(Cover, Cell, 2, End, 0.015)),
    (0xE0C8..=0xE0C8, rule(Cover, Cell, 2, Start, 0.0)),
    (0xE0CA..=0xE0CA, rule(Cover, Cell, 2, End, 0.0)),
    (0xE0CC..=0xE0CD, rule(Cover, Cell, 2, Start, 0.0)),
    (0xE0CE..=0xE0CE, rule(FitCover1, Cell, 2, Start, 0.0)),
    (0xE0CF..=0xE0CF, rule(FitCover1, Cell, 2, Center1, 0.0)),
    (0xE0D0..=0xE0D1, rule(FitCover1, Cell, 2, Start, 0.0)),
    (0xE0D2..=0xE0D2, rule(Cover, Cell, 1, Start, 0.0)),
    (0xE0D4..=0xE0D4, rule(Cover, Cell, 1, End, 0.0)),
    (0xE0D6..=0xE0D6, rule(Cover, Cell, 1, Start, 0.0)),
    (0xE0D7..=0xE0D7, rule(Cover, Cell, 1, End, 0.0)),
    (0xEE00..=0xEE00, rule(Cover, Cell, 1, End, 0.0)),
    (0xEE01..=0xEE01, rule(Cover, Cell, 1, Center1, 0.0)),
    (0xEE02..=0xEE02, rule(Cover, Cell, 1, Start, 0.0)),
    (0xEE03..=0xEE03, rule(Cover, Cell, 1, End, 0.0)),
    (0xEE04..=0xEE04, rule(Cover, Cell, 1, Center1, 0.0)),
    (0xEE05..=0xEE05, rule(Cover, Cell, 1, Start, 0.0)),
    (0xEE06..=0xEE0B, rule(Cover, Cell, 1, Center1, 0.015)),
];

pub fn rule_for(c: char) -> Rule {
    let c = c as u32;
    RULES.iter().find(|(range, _)| range.contains(&c)).map_or(DEFAULT, |(_, rule)| *rule)
}

/// The face the icon is sized against, in px: the terminal font's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Face {
    pub width: f32,
    pub height: f32,
    pub cap_height: f32,
    /// The face's top, from the cell's.
    pub top: f32,
}

/// An icon fitted to its cells: its scale from its own size, and its ink's
/// top left from the face's left edge and the cell's top.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fitted {
    pub scale: f32,
    pub x: f32,
    pub y: f32,
}

/// Fits an icon whose ink is `ink` (width, height) px at the text size into
/// `cells` cells (1 or 2, Ghostty's constraint width).
pub fn fit(rule: Rule, ink: (f32, f32), face: Face, cell_width: f32, cells: u8) -> Fitted {
    let cells = cells.min(rule.max_cells).max(1);
    let [left, right, top, bottom] = rule.pad;
    let scale_in = |cells: u8| {
        let height = match rule.height {
            Height::Cell => face.height,
            Height::Icon if cells > 1 => face.height,
            Height::Icon => (2.0 * face.cap_height + face.height) / 3.0,
        };
        let width = (f32::from(cells) - left - right) * face.width;
        (width / ink.0).min((1.0 - top - bottom) * height / ink.1)
    };
    let scale = match rule.size {
        Size::Fit => scale_in(cells).min(1.0),
        Size::FitCover1 if cells > 1 && scale_in(cells) > 1.0 => scale_in(1).max(1.0),
        Size::FitCover1 | Size::Cover => scale_in(cells),
    };
    let (w, h) = (ink.0 * scale, ink.1 * scale);
    // Never left of the cell: an icon wider than its first cell starts at
    // the cell and spills into the blank after it.
    let start = left * face.width;
    let end = |span: f32| (span - w - right * face.width).max(start);
    let x = match rule.align {
        Align::Start => start,
        Align::End => end(face.width + f32::from(cells - 1) * cell_width),
        Align::Center1 => (start + end(face.width)) / 2.0,
    };
    let y = face.top + top * face.height + ((1.0 - top - bottom) * face.height - h) / 2.0;
    Fitted { scale, x, y }
}

/// Ghostty's constraint width: two cells when the next is blank, unless the
/// cell is the row's last or follows another symbol, so runs of them line up.
pub fn cells(next_blank: bool, last: bool, after_symbol: bool) -> u8 {
    if next_blank && !last && !after_symbol { 2 } else { 1 }
}

/// The blocks Ghostty counts as symbols besides private use
/// (`src/build/uucode_config.zig` `IsSymbolComponent`): Arrows, Enclosed
/// Alphanumerics, Miscellaneous Symbols, Dingbats, Enclosed Alphanumeric
/// Supplement, Miscellaneous Symbols and Pictographs, Emoticons, Transport
/// and Map Symbols.
const SYMBOL_BLOCKS: &[RangeInclusive<u32>] = &[
    0x2190..=0x21FF,
    0x2460..=0x24FF,
    0x2600..=0x26FF,
    0x2700..=0x27BF,
    0x1F100..=0x1F1FF,
    0x1F300..=0x1F5FF,
    0x1F600..=0x1F64F,
    0x1F680..=0x1F6FF,
];

/// Whether Ghostty fits `c` to its cells (and lets it take two before a
/// blank): private use and the symbol blocks. Anything else draws as its
/// font draws it.
pub fn is_symbol(c: char) -> bool {
    let c = c as u32;
    matches!(c, 0xE000..=0xF8FF | 0xF0000..=0xFFFFD | 0x100000..=0x10FFFD) || SYMBOL_BLOCKS.iter().any(|block| block.contains(&c))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// JetBrains Mono at the font card's two measured cells (8.98 and 7.98 px
    /// wide): face height 1.32 em, cap height 0.73 em, advance 0.6 em.
    fn face(cell_width: f32) -> Face {
        let em = cell_width / 0.6;
        Face { width: cell_width, height: 1.32 * em, cap_height: 0.73 * em, top: 0.0 }
    }

    /// Ink size in px at the text size, from the bundled Symbols Nerd Font
    /// (units per em 2048).
    fn ink(c: char, cell_width: f32) -> (f32, f32) {
        let face = ttf_parser::Face::parse(crate::settings::SYMBOLS_FONT_FILE, 0).unwrap();
        let b = face.glyph_bounding_box(face.glyph_index(c).unwrap()).unwrap();
        let px = cell_width / 0.6 / f32::from(face.units_per_em());
        (f32::from(b.x_max - b.x_min) * px, f32::from(b.y_max - b.y_min) * px)
    }

    fn fitted_size(c: char, cell_width: f32, cells: u8) -> (f32, f32) {
        let ink = ink(c, cell_width);
        let f = fit(rule_for(c), ink, face(cell_width), cell_width, cells);
        (ink.0 * f.scale, ink.1 * f.scale)
    }

    #[test]
    fn icons_take_ghosttys_sizes_on_the_font_card() {
        // Ghostty's sizes, from its rule on the same fonts (the font card's
        // measured boxes are these plus about 1 px of anti-aliased edge).
        let cases = [
            ('\u{f015}', 8.98, (15.6, 13.8)),
            ('\u{f07b}', 8.98, (13.8, 12.1)),
            ('\u{e718}', 8.98, (10.4, 12.0)),
            ('\u{e7a8}', 8.98, (12.0, 12.0)),
            ('\u{e73c}', 8.98, (11.3, 11.5)),
            ('\u{f418}', 8.98, (11.2, 13.1)),
            ('\u{f02a2}', 8.98, (12.5, 12.5)),
            ('\u{f0219}', 8.98, (10.0, 12.5)),
            ('\u{e718}', 7.98, (9.3, 10.6)),
            ('\u{e7a8}', 7.98, (10.7, 10.7)),
            ('\u{e73c}', 7.98, (10.1, 10.2)),
            ('\u{f418}', 7.98, (10.0, 11.6)),
        ];
        for (c, cell, (w, h)) in cases {
            let got = fitted_size(c, cell, 2);
            assert!((got.0 - w).abs() < 0.1 && (got.1 - h).abs() < 0.1, "U+{:X} at {cell}: {got:?}, want {w}x{h}", c as u32);
        }
    }

    #[test]
    fn one_cell_caps_an_icon_at_the_face_width_and_icon_height() {
        let f = face(8.98);
        let (w, h) = fitted_size('\u{f015}', 8.98, 1);
        assert!(w <= f.width + 1e-3 && h <= (2.0 * f.cap_height + f.height) / 3.0 + 1e-3, "{w}x{h}");
        assert!((w - f.width).abs() < 1e-3 || (h - (2.0 * f.cap_height + f.height) / 3.0).abs() < 1e-3, "covers one cell");
    }

    #[test]
    fn a_small_icon_grows_to_cover_one_cell_and_no_more_with_two() {
        let f = face(8.98);
        let rule = rule_for('\u{f015}');
        let small = (4.0, 4.0);
        let one = fit(rule, small, f, 8.98, 1);
        let two = fit(rule, small, f, 8.98, 2);
        assert_eq!(one.scale, two.scale);
        assert!((small.0 * one.scale - f.width).abs() < 1e-3);
        let big = fit(rule, (30.0, 10.0), f, 8.98, 2);
        assert!((30.0 * big.scale - 2.0 * f.width).abs() < 1e-3, "shrinks to two cells");
        let between = fit(rule, (12.0, 12.0), f, 8.98, 2);
        assert_eq!(between.scale, 1.0, "between one and two cells it keeps its size");
    }

    #[test]
    fn cover_fills_its_box_and_aligns_to_its_edge() {
        let f = face(9.0);
        let r = rule_for('\u{e0d2}');
        assert_eq!((r.size, r.max_cells, r.align), (Size::Cover, 1, Align::Start));
        let fitted = fit(r, (2.0, 8.0), f, 9.0, 2);
        assert!((8.0 * fitted.scale - f.height).abs() < 1e-3);
        assert_eq!(fitted.x, 0.0);
        let end = fit(rule_for('\u{e0d4}'), (20.0, 2.0), f, 9.0, 2);
        assert!((end.x + 20.0 * end.scale - 9.0).abs() < 1e-3, "ends at its one cell's right edge");
        let padded = fit(rule_for('\u{ee06}'), (10.0, 10.0), f, 9.0, 1);
        assert!((10.0 * padded.scale - 0.97 * f.width).abs() < 1e-3);
    }

    #[test]
    fn icons_centre_on_their_first_cell_and_the_face() {
        let f = Face { top: 1.5, ..face(9.0) };
        let narrow = fit(ICON, (6.0, 12.0), f, 9.0, 2);
        let (w, h) = (6.0 * narrow.scale, 12.0 * narrow.scale);
        assert!((narrow.x - (9.0 - w) / 2.0).abs() < 1e-3, "centred on its first cell");
        assert!((narrow.y + h / 2.0 - (1.5 + f.height / 2.0)).abs() < 1e-3, "centred on the face");
        let wide = fit(ICON, (12.0, 12.0), f, 9.0, 2);
        assert_eq!(wide.x, 0.0, "wider than its first cell, it starts there and never left of it");
    }

    #[test]
    fn symbols_are_ghosttys_blocks_and_private_use() {
        for c in ['❯', '✻', '★', '☀', '→', '①', '🚀', '\u{f015}', '\u{f0219}'] {
            assert!(is_symbol(c), "{c}");
        }
        for c in ['⏺', '◐', '∑', 'a', '界', '─'] {
            assert!(!is_symbol(c), "{c}");
        }
    }

    #[test]
    fn two_cells_only_before_a_blank_and_not_after_an_icon_or_at_the_end() {
        assert_eq!(cells(true, false, false), 2);
        assert_eq!(cells(false, false, false), 1);
        assert_eq!(cells(true, true, false), 1);
        assert_eq!(cells(true, false, true), 1);
    }

    #[test]
    fn icon_sets_cover_a_cell_and_other_private_use_only_shrinks() {
        assert_eq!(rule_for('\u{f015}'), ICON);
        assert_eq!(rule_for('\u{f0219}'), ICON);
        assert_eq!(rule_for('\u{e0a0}').height, Height::Cell);
        assert_eq!(rule_for('\u{e0c9}'), DEFAULT);
        let f = face(9.0);
        assert_eq!(fit(DEFAULT, (4.0, 4.0), f, 9.0, 2).scale, 1.0);
        assert!((fit(DEFAULT, (40.0, 4.0), f, 9.0, 2).scale - 18.0 / 40.0).abs() < 1e-3);
    }
}
