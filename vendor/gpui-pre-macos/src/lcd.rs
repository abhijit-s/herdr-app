//! HERDR: subpixel (LCD) glyph coverage. CoreGraphics no longer makes LCD
//! masks, so swash draws the outline at three times the horizontal resolution
//! and FreeType's default five-tap filter turns each device pixel's three
//! subpixels into a red, a green and a blue coverage. CoreText still shapes and
//! places every glyph; only the rasterising moves here.

use collections::HashMap;
use core_foundation::{
    base::{CFType, TCFType},
    dictionary::CFDictionary,
    number::CFNumber,
};
use core_text::font::CTFont;
use gpui::FontId;
use std::sync::Arc;
use swash::{
    CacheKey, FontRef, Setting,
    scale::{ScaleContext, outline::Outline},
    zeno::{self, Format, Transform, Vector},
};

/// FreeType's `FT_LCD_FILTER_DEFAULT` weights, out of 256.
pub const FIR: [u32; 5] = [8, 77, 86, 77, 8];

/// The thicken strengths at which CoreGraphics' smoothing steps one dilation
/// level up (Ghostty's gray fill, in linear gray). tests/subpixel_text.rs
/// sweeps every strength against them.
const SMOOTHING_STEPS: [f32; 4] = [100., 165., 208., 241.];

/// The smoothing level, 0 to 4, that a thicken strength draws at, made
/// continuous: 0 at strength 0, then straight through the middle of each of
/// CoreGraphics' levels 1 to 3, to 4 at 255. Every strength adds weight, and
/// no strength lands more than three quarters of a level from grayscale's.
pub fn thicken_level(strength: u8) -> f32 {
    let strength = strength as f32;
    let mut knots = vec![(0.0, 0.0)];
    knots.extend(SMOOTHING_STEPS.windows(2).enumerate().map(|(k, pair)| ((pair[0] + pair[1]) / 2.0, k as f32 + 1.0)));
    knots.push((255.0, 4.0));
    knots
        .windows(2)
        .find(|pair| strength <= pair[1].0)
        .map_or(4.0, |pair| {
            let ((s0, l0), (s1, l1)) = (pair[0], pair[1]);
            l0 + (strength - s0) / (s1 - s0) * (l1 - l0)
        })
}

/// How far a font's strokes grow, in device pixels, across and along the
/// rows, for swash to draw it as CoreGraphics' grayscale does through
/// thicken's smoothing levels: `base` at level 0, `per_level` more for each
/// level (`fit` at levels 0 and 4).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Weight {
    pub base: [f32; 2],
    pub per_level: [f32; 2],
}

impl Weight {
    /// For a font swash can't calibrate: JetBrains Mono's at 12 px.
    pub const FALLBACK: Weight = Weight { base: [0.04, 0.12], per_level: [0.015, 0.015] };

    /// From the growths that draw a font as CoreGraphics does at smoothing
    /// levels 0 and 4.
    pub fn between(low: [f32; 2], high: [f32; 2]) -> Weight {
        Weight { base: low, per_level: [(high[0] - low[0]) / 4.0, (high[1] - low[1]) / 4.0] }
    }

    /// The same growth at every level.
    pub fn steady(growth: [f32; 2]) -> Weight {
        Weight { base: growth, per_level: [0.0; 2] }
    }

    /// The growth across and along the rows at this smoothing level.
    pub fn at(self, level: f32) -> [f32; 2] {
        [self.base[0] + self.per_level[0] * level, self.base[1] + self.per_level[1] * level]
    }
}

/// A glyph's swash coverage, 0 to 1 per pixel, in CoreGraphics' box of
/// `width` pixels a row: plain, and grown by `Samples::PROBE` across the rows
/// and along them.
pub struct Samples {
    pub width: usize,
    pub plain: Vec<f32>,
    pub grown: [Vec<f32>; 2],
}

impl Samples {
    /// How far the calibration's swash passes grow the glyph, each one way:
    /// small enough that coverage grows about linearly.
    pub const PROBE: f32 = 0.1;
    /// How much more a row's summed coverage counts than one pixel's when
    /// fitting: the glyphs' vertical profiles come first, their pixels break
    /// ties. Anywhere from 10 to 100 fits alike.
    const ROW_WEIGHT: f64 = 30.0;

    /// 1, or -1 for a glyph whose winding swash reads backwards (JetBrains
    /// Mono's `i`): a positive growth shrinks it.
    pub fn outward(&self) -> f32 {
        let ink: f32 = self.grown.iter().flat_map(|grown| grown.iter().zip(&self.plain).map(|(g, p)| g - p)).sum();
        if ink < 0.0 { -1.0 } else { 1.0 }
    }
}

/// One growth across and along the rows for a whole font: the one that inks
/// the glyphs (each `Samples` beside CoreGraphics' grayscale of it) exactly as
/// much as CoreGraphics does in all, and draws them closest to it, row by row
/// first, then pixel by pixel, taking coverage as linear in the growth. Each
/// glyph grows outward, by its `Samples::outward` times the growth. One
/// growth for every glyph keeps stems alike: fitted per glyph, neighbouring
/// glyphs' stems grew by different amounts. CoreGraphics grows glyphs more
/// along the rows than across them, so one isotropic growth matched to its
/// ink draws glyphs short with heavy stems. Glyphs whose two ways can't be
/// told apart (dots) grow alike in both. None when every glyph draws empty.
pub fn fit(glyphs: &[(&Samples, &[f32])]) -> Option<[f32; 2]> {
    let h = Samples::PROBE as f64;
    let (mut xx, mut xy, mut yy, mut bx, mut by) = (0.0, 0.0, 0.0, 0.0, 0.0);
    let (mut inks, mut ink) = ([0.0f64; 2], 0.0f64);
    for &(samples, gray) in glyphs {
        if !gray.iter().any(|&c| c > 0.0) {
            continue;
        }
        let sign = samples.outward() as f64;
        let gradients = samples.grown.each_ref().map(|grown| grown.iter().zip(&samples.plain).map(|(&g, &p)| sign * (g as f64 - p as f64) / h).collect::<Vec<f64>>());
        let residual: Vec<f64> = gray.iter().zip(&samples.plain).map(|(&c, &p)| c as f64 - p as f64).collect();
        let rows = |v: &[f64]| -> Vec<f64> { v.chunks(samples.width.max(1)).map(|row| row.iter().sum()).collect() };
        let dot = |a: &[f64], b: &[f64]| {
            let pixels: f64 = a.iter().zip(b).map(|(a, b)| a * b).sum();
            let rows: f64 = rows(a).iter().zip(rows(b)).map(|(a, b)| a * b).sum();
            pixels + Samples::ROW_WEIGHT * rows
        };
        let [gx, gy] = &gradients;
        xx += dot(gx, gx);
        xy += dot(gx, gy);
        yy += dot(gy, gy);
        bx += dot(gx, &residual);
        by += dot(gy, &residual);
        inks[0] += gx.iter().sum::<f64>();
        inks[1] += gy.iter().sum::<f64>();
        ink += residual.iter().sum::<f64>();
    }
    if inks[0] + inks[1] == 0.0 {
        return None;
    }
    // Least squares under the ink constraint, by Lagrange: the gradients'
    // Gram matrix bordered by their inks.
    let m = vec![vec![xx, xy, inks[0]], vec![xy, yy, inks[1]], vec![inks[0], inks[1], 0.0]];
    match solve(m, vec![bx, by, ink], 1e-6 * (xx + yy)) {
        Some(x) => Some([x[0] as f32, x[1] as f32]),
        None => {
            let both = (ink / (inks[0] + inks[1])) as f32;
            both.is_finite().then_some([both; 2])
        }
    }
}

/// `m x = b` by Gaussian elimination with partial pivoting, or None when a
/// pivot is within `tolerance` of 0.
fn solve(mut m: Vec<Vec<f64>>, mut b: Vec<f64>, tolerance: f64) -> Option<Vec<f64>> {
    let n = b.len();
    for k in 0..n {
        let pivot = (k..n).max_by(|&i, &j| m[i][k].abs().total_cmp(&m[j][k].abs()))?;
        if m[pivot][k].abs() <= tolerance {
            return None;
        }
        m.swap(k, pivot);
        b.swap(k, pivot);
        for i in k + 1..n {
            let f = m[i][k] / m[k][k];
            for j in k..n {
                m[i][j] -= f * m[k][j];
            }
            b[i] -= f * b[k];
        }
    }
    let mut x = vec![0.0; n];
    for k in (0..n).rev() {
        x[k] = (b[k] - (k + 1..n).map(|j| m[k][j] * x[j]).sum::<f64>()) / m[k][k];
    }
    Some(x)
}

/// A 3x-wide alpha mask: `width` subpixels by `height` rows, its first column
/// at subpixel `left` and first row at row `top` of the box it lands in.
pub struct Mask<'a> {
    pub data: &'a [u8],
    pub width: usize,
    pub height: usize,
    pub left: i32,
    pub top: i32,
}

/// Filters `mask` into a `width` x `height` box of BGRA texels: each color
/// channel the coverage of its own subpixel (red the left one), alpha the
/// largest of the three.
pub fn filter(mask: &Mask, width: usize, height: usize) -> Vec<u8> {
    let mut out = vec![0u8; width * height * 4];
    let coverage = |row: &[u8], s: i32| -> u32 {
        let column = s - mask.left;
        if column < 0 || column as usize >= mask.width { 0 } else { row[column as usize] as u32 }
    };
    for y in 0..height {
        let mask_row = y as i32 - mask.top;
        if mask_row < 0 || mask_row as usize >= mask.height {
            continue;
        }
        let row = &mask.data[mask_row as usize * mask.width..][..mask.width];
        for x in 0..width {
            let texel = &mut out[(y * width + x) * 4..][..4];
            for channel in 0..3 {
                let s = 3 * x as i32 + channel;
                let sum: u32 = FIR.iter().enumerate().map(|(k, w)| w * coverage(row, s + k as i32 - 2)).sum();
                texel[2 - channel as usize] = (sum / 256) as u8;
            }
            texel[3] = texel[0].max(texel[1]).max(texel[2]);
        }
    }
    out
}

/// A face's bytes, kept for as long as the rasteriser draws it.
enum Bytes {
    Shared(Arc<Vec<u8>>),
    Static(&'static [u8]),
    Mapped(memmap2::Mmap),
    Owned(Vec<u8>),
}

impl Bytes {
    fn get(&self) -> &[u8] {
        match self {
            Bytes::Shared(bytes) => bytes,
            Bytes::Static(bytes) => bytes,
            Bytes::Mapped(map) => map,
            Bytes::Owned(bytes) => bytes,
        }
    }
}

struct Face {
    bytes: Bytes,
    offset: u32,
    key: CacheKey,
    variations: Vec<Setting<f32>>,
}

impl Face {
    fn font(&self) -> FontRef<'_> {
        FontRef { data: self.bytes.get(), offset: self.offset, key: self.key }
    }
}

/// Swash's state and the faces it has opened, by GPUI font.
#[derive(Default)]
pub struct Rasterizer {
    context: ScaleContext,
    faces: HashMap<FontId, Option<Face>>,
    outline: Outline,
}

/// What `Rasterizer::render` needs to know about a glyph.
pub struct Glyph {
    pub glyph_id: u16,
    /// The size in device pixels.
    pub ppem: f32,
    /// Where the pen starts right of the box's pixel grid, 0 to 1.
    pub shift: f32,
    /// How far the strokes grow across and along the rows, in device pixels
    /// (`fit`, by `Samples::outward`).
    pub embolden: [f32; 2],
    /// `Samples::outward`: -1 for a glyph whose winding swash reads backwards.
    pub outward: f32,
    /// The box: its origin relative to the pen (y down) and size.
    pub origin: (i32, i32),
    pub size: (usize, usize),
}

impl Rasterizer {
    /// The glyph's BGRA coverage in its box, or None when swash can't open
    /// the face or draw the glyph (the caller falls back to grayscale).
    pub fn render(&mut self, font_id: FontId, font: impl FnOnce() -> FaceSource, glyph: &Glyph) -> Option<Vec<u8>> {
        let (data, mask) = self.mask(font_id, font, glyph, 3)?;
        Some(filter(&Mask { data: &data, ..mask }, glyph.size.0, glyph.size.1))
    }

    /// The glyph's plain alpha coverage, 0 to 1, in its box at one sample per
    /// pixel, as CoreGraphics' grayscale draws it: what `Weight` calibrates
    /// against.
    pub fn coverage(&mut self, font_id: FontId, font: impl FnOnce() -> FaceSource, glyph: &Glyph) -> Option<Vec<f32>> {
        let (data, mask) = self.mask(font_id, font, glyph, 1)?;
        let (width, height) = glyph.size;
        let mut out = vec![0.0; width * height];
        for (y, row) in out.chunks_exact_mut(width).enumerate() {
            let mask_row = y as i32 - mask.top;
            if mask_row < 0 || mask_row as usize >= mask.height {
                continue;
            }
            for (x, texel) in row.iter_mut().enumerate() {
                let column = x as i32 - mask.left;
                if column >= 0 && (column as usize) < mask.width {
                    *texel = data[mask_row as usize * mask.width + column as usize] as f32 / 255.0;
                }
            }
        }
        Some(out)
    }

    /// The glyph's outline grown by `embolden`, stretched `columns` times
    /// horizontally, as an alpha mask placed in its box (`data` empty: the
    /// bytes come first).
    fn mask(&mut self, font_id: FontId, font: impl FnOnce() -> FaceSource, glyph: &Glyph, columns: i32) -> Option<(Vec<u8>, Mask<'static>)> {
        let face = self.faces.entry(font_id).or_insert_with(|| open(font())).as_ref()?;
        let mut scaler = self
            .context
            .builder(face.font())
            .size(glyph.ppem)
            .hint(false)
            .variations(face.variations.iter().copied())
            .build();
        if !scaler.scale_outline_into(glyph.glyph_id, &mut self.outline) {
            return None;
        }
        // swash moves an outline's right and top edges by twice the strength,
        // its left and bottom edges fixed, as CoreGraphics' smoothing does; a
        // backwards-wound glyph moves its left and bottom edges instead,
        // whichever way the strength grows it. Moved back, it keeps its place
        // among the rest. The growth's sign doesn't say which: a font can
        // shrink (smoothing off fits a negative growth).
        let [x, y] = glyph.embolden;
        if x != 0.0 || y != 0.0 {
            self.outline.embolden(x, y);
        }
        let shift = if glyph.outward < 0.0 { Transform::translation(-2.0 * x, -2.0 * y) } else { Transform::translation(0.0, 0.0) };
        self.outline.transform(&shift.then_scale(columns as f32, 1.0));
        let offset = Vector::new(glyph.shift * columns as f32, 0.0);
        let mut data = Vec::new();
        let placement = zeno::Mask::new(self.outline.path())
            .format(Format::Alpha)
            .origin(zeno::Origin::BottomLeft)
            .offset(offset)
            .render_offset(offset)
            .inspect(|format, width, height| data.resize(format.buffer_size(width, height), 0))
            .render_into(&mut data, None);
        let mask = Mask {
            data: &[],
            width: placement.width as usize,
            height: placement.height as usize,
            left: placement.left - columns * glyph.origin.0,
            top: -placement.top - glyph.origin.1,
        };
        Some((data, mask))
    }
}

/// Where a face's bytes may come from, best first.
pub struct FaceSource {
    pub native: CTFont,
    pub shared: Option<Arc<Vec<u8>>>,
    pub embedded: Option<&'static [u8]>,
    pub postscript_name: Option<String>,
}

fn open(source: FaceSource) -> Option<Face> {
    let variations = variations(&source.native);
    let (bytes, index) = if let Some(bytes) = source.shared {
        let index = face_index(&bytes, source.postscript_name.as_deref());
        (Bytes::Shared(bytes), index)
    } else if let Some(bytes) = source.embedded {
        (Bytes::Static(bytes), face_index(bytes, source.postscript_name.as_deref()))
    } else if let Some(map) = mapped_file(&source.native) {
        let index = face_index(&map, source.postscript_name.as_deref());
        (Bytes::Mapped(map), index)
    } else {
        (Bytes::Owned(sfnt_from_tables(&source.native)?), 0)
    };
    let font = FontRef::from_index(bytes.get(), index)?;
    let (offset, key) = (font.offset, font.key);
    Some(Face { bytes, offset, key, variations })
}

/// The face in a collection whose PostScript name matches, else the first.
fn face_index(bytes: &[u8], postscript_name: Option<&str>) -> usize {
    let Some(name) = postscript_name else { return 0 };
    swash::FontDataRef::new(bytes)
        .and_then(|data| {
            data.fonts().position(|font| {
                font.localized_strings()
                    .find_by_id(swash::StringId::PostScript, None)
                    .is_some_and(|s| s.chars().eq(name.chars()))
            })
        })
        .unwrap_or(0)
}

/// The face's file mapped, when it is a single-face file: a collection's
/// faces can't be told apart from here by every name CoreText reports.
fn mapped_file(native: &CTFont) -> Option<memmap2::Mmap> {
    let path = native.url()?.to_path()?;
    let file = std::fs::File::open(path).ok()?;
    let map = unsafe { memmap2::Mmap::map(&file) }.ok()?;
    (map.get(..4)? != b"ttcf").then_some(map)
}

/// The face rebuilt from the tables CoreText hands out.
fn sfnt_from_tables(native: &CTFont) -> Option<Vec<u8>> {
    let tags: Vec<u32> = native.get_available_font_tables()?.iter().map(|tag| *tag).collect();
    let tables: Vec<(u32, Vec<u8>)> =
        tags.into_iter().filter_map(|tag| Some((tag, native.get_font_table(tag)?.bytes().to_vec()))).collect();
    let version: u32 = if tables.iter().any(|(tag, _)| tag == &u32::from_be_bytes(*b"CFF ")) { 0x4F54_544F } else { 0x0001_0000 };
    let count = tables.len() as u16;
    let mut out = Vec::new();
    out.extend_from_slice(&version.to_be_bytes());
    let entry_selector = 15 - count.max(1).leading_zeros() as u16;
    let search_range = 16u16 << entry_selector;
    out.extend_from_slice(&count.to_be_bytes());
    out.extend_from_slice(&search_range.to_be_bytes());
    out.extend_from_slice(&entry_selector.to_be_bytes());
    out.extend_from_slice(&(count * 16 - search_range).to_be_bytes());
    let mut offset = 12 + 16 * tables.len();
    let mut body = Vec::new();
    for (tag, data) in &tables {
        out.extend_from_slice(&tag.to_be_bytes());
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        body.extend_from_slice(data);
        let padded = data.len().next_multiple_of(4);
        body.resize(body.len() + padded - data.len(), 0);
        offset += padded;
    }
    out.extend_from_slice(&body);
    Some(out)
}

#[link(name = "CoreText", kind = "framework")]
unsafe extern "C" {
    fn CTFontCopyVariation(font: core_text::font::CTFontRef) -> core_foundation::dictionary::CFDictionaryRef;
}

/// The variation coordinates CoreText draws the face at (a variable face at
/// a weight, or a named instance).
fn variations(native: &CTFont) -> Vec<Setting<f32>> {
    let dictionary = unsafe { CTFontCopyVariation(native.as_concrete_TypeRef()) };
    if dictionary.is_null() {
        return Vec::new();
    }
    let dictionary: CFDictionary<CFType, CFType> = unsafe { CFDictionary::wrap_under_create_rule(dictionary) };
    let (keys, values) = dictionary.get_keys_and_values();
    keys.into_iter()
        .zip(values)
        .filter_map(|(key, value)| {
            let key = unsafe { CFType::wrap_under_get_rule(key) }.downcast::<CFNumber>()?.to_i64()?;
            let value = unsafe { CFType::wrap_under_get_rule(value) }.downcast::<CFNumber>()?.to_f64()?;
            Some(Setting { tag: key as u32, value: value as f32 })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thicken_level_rises_through_the_middle_of_each_level() {
        assert_eq!(thicken_level(0), 0.0);
        assert_eq!(thicken_level(255), 4.0);
        for (k, pair) in SMOOTHING_STEPS.windows(2).enumerate() {
            let middle = (pair[0] + pair[1]) / 2.0;
            let (below, above) = (thicken_level(middle.floor() as u8), thicken_level(middle.ceil() as u8));
            assert!(below <= k as f32 + 1.0 && k as f32 + 1.0 <= above, "level {} mid-step", k + 1);
        }
        for strength in 0..=255u8 {
            let grayscale = SMOOTHING_STEPS.iter().filter(|&&step| strength as f32 >= step).count() as f32;
            assert!((thicken_level(strength) - grayscale).abs() <= 0.76, "{strength}: near grayscale's level");
        }
        assert!(thicken_level(30) > 0.0, "no strength is weightless");
        for strength in 1..=255u8 {
            let (below, here) = (thicken_level(strength - 1), thicken_level(strength));
            assert!(here > below, "{strength}: rises");
            assert!(here - below < 0.08, "{strength}: no jump");
        }
    }

    fn close(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5
    }

    fn samples(plain: &[f32], across: &[f32], along: &[f32], width: usize) -> Samples {
        Samples { width, plain: plain.to_vec(), grown: [across.to_vec(), along.to_vec()] }
    }

    /// Two rows of three pixels whose coverage grows linearly: across the
    /// rows on a stem's sides, along them at its end. `sign` -1 winds it
    /// backwards.
    fn stem(sign: f32) -> (Samples, impl Fn([f32; 2]) -> Vec<f32>) {
        let plain = [0.5, 1.0, 0.5, 0.0, 0.2, 0.0];
        let gx = [1.0, 0.0, 1.0, 0.0, 0.0, 0.0];
        let gy = [0.0, 0.0, 0.0, 0.3, 1.0, 0.3];
        let at = move |[x, y]: [f32; 2]| -> Vec<f32> { (0..plain.len()).map(|k| plain[k] + x * gx[k] + y * gy[k]).collect() };
        let probe = Samples::PROBE * sign;
        (samples(&plain, &at([probe, 0.0]), &at([0.0, probe]), 3), at)
    }

    #[test]
    fn fit_recovers_each_way_and_interpolates_by_level() {
        let (samples, at) = stem(1.0);
        let (low, high) = (fit(&[(&samples, &at([0.04, 0.12]))]).unwrap(), fit(&[(&samples, &at([0.10, 0.18]))]).unwrap());
        assert!(close(low, [0.04, 0.12]), "{low:?}");
        assert!(close(high, [0.10, 0.18]), "{high:?}");
        let weight = Weight::between(low, high);
        assert!(close(weight.at(2.0), [0.07, 0.15]), "{weight:?}");
        assert!(close(Weight::steady(low).at(3.0), low));
    }

    #[test]
    fn fit_gives_one_growth_for_the_whole_font() {
        // Two glyphs CoreGraphics grows differently: the font's growth sits
        // between theirs and inks as much as both together.
        let (a, at_a) = stem(1.0);
        let (b, at_b) = stem(1.0);
        let (gray_a, gray_b) = (at_a([0.02, 0.10]), at_b([0.06, 0.14]));
        let [x, y] = fit(&[(&a, &gray_a), (&b, &gray_b)]).unwrap();
        assert!(0.02 < x && x < 0.06 && 0.10 < y && y < 0.14, "{x} {y}");
        let ink = |v: &[f32]| v.iter().sum::<f32>();
        let fitted = ink(&at_a([x, y])) + ink(&at_b([x, y]));
        assert!((fitted - ink(&gray_a) - ink(&gray_b)).abs() < 1e-4);
    }

    #[test]
    fn fit_matches_ink_when_the_ways_look_alike() {
        // A dot: growing either way adds the same coverage, so it grows
        // alike both ways, as far as matches CoreGraphics' ink.
        let dot = samples(&[0.5, 0.5], &[0.6, 0.6], &[0.6, 0.6], 2);
        let [x, y] = fit(&[(&dot, &[0.62, 0.62])]).unwrap();
        assert!((x - y).abs() < 1e-6 && (x - 0.06).abs() < 1e-5, "{x} {y}");
    }

    #[test]
    fn backwards_wound_glyphs_grow_outward_by_the_fonts_growth() {
        // swash reads one glyph's winding backwards: a positive growth
        // shrinks it. It still fits the font's growth, grown by -1 times it.
        let (forwards, at_forwards) = stem(1.0);
        let (backwards, at_backwards) = stem(-1.0);
        assert_eq!((forwards.outward(), backwards.outward()), (1.0, -1.0));
        let growth = [0.04, 0.12];
        let fitted = fit(&[(&forwards, &at_forwards(growth)), (&backwards, &at_backwards(growth))]).unwrap();
        assert!(close(fitted, growth), "{fitted:?}");
    }

    #[test]
    fn fit_skips_empty_glyphs() {
        let empty = samples(&[0.0; 3], &[0.0; 3], &[0.0; 3], 3);
        assert_eq!(fit(&[(&empty, &[0.0; 3])]), None);
        assert_eq!(fit(&[]), None);
    }

    #[test]
    fn filter_spreads_one_subpixel_by_the_fir_weights() {
        // One fully covered subpixel: the green one of pixel 1 (subpixel 4).
        let data = [255u8];
        let mask = Mask { data: &data, width: 1, height: 1, left: 4, top: 0 };
        let out = filter(&mask, 3, 1);
        // BGRA per pixel; channel subpixels 3x, 3x+1, 3x+2 for R, G, B.
        let rgb = |x: usize| [out[x * 4 + 2], out[x * 4 + 1], out[x * 4]];
        let w = |k: u32| (k * 255 / 256) as u8;
        assert_eq!(rgb(0), [0, 0, w(8)]);
        assert_eq!(rgb(1), [w(77), w(86), w(77)]);
        assert_eq!(rgb(2), [w(8), 0, 0]);
        assert_eq!(out[4 + 3], w(86), "alpha is the largest channel");
    }

    #[test]
    fn filter_of_full_coverage_is_full_and_gray() {
        let data = [255u8; 30];
        let mask = Mask { data: &data, width: 30, height: 1, left: -6, top: 0 };
        let out = filter(&mask, 6, 1);
        // Every pixel's five taps are covered, and the weights sum to 256.
        for x in 0..6 {
            let texel = &out[x * 4..][..4];
            assert_eq!(texel, [255, 255, 255, 255], "pixel {x}");
        }
    }

    #[test]
    fn filter_skips_rows_outside_the_mask() {
        let data = [255u8; 3];
        let mask = Mask { data: &data, width: 3, height: 1, left: 0, top: 1 };
        let out = filter(&mask, 1, 3);
        assert!(out[..4].iter().all(|&b| b == 0));
        assert!(out[4..8].iter().any(|&b| b > 0));
        assert!(out[8..].iter().all(|&b| b == 0));
    }
}
