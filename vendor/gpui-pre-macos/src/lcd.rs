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
    scale::{Render, ScaleContext, Source},
    zeno::{Format, Transform, Vector},
};

/// FreeType's `FT_LCD_FILTER_DEFAULT` weights, out of 256.
pub const FIR: [u32; 5] = [8, 77, 86, 77, 8];

/// How far each side of a stroke grows, in device pixels, to land subpixel
/// ink near CoreGraphics' grayscale at the same smoothing level (0 to 4, see
/// `smoothing_level`): swash's plain coverage is lighter, and each level of
/// CoreGraphics' smoothing dilation adds about the same again. Tuned with
/// tests/subpixel_text.rs, which prints both inks.
pub fn embolden(level: u8) -> f32 {
    const BASE: f32 = 0.09;
    const PER_LEVEL: f32 = 0.032;
    BASE + PER_LEVEL * level.min(4) as f32
}

/// The dilation level CoreGraphics smoothing draws a thicken strength at:
/// strengths below the first step look the same as none.
pub fn smoothing_level(strength: u8) -> u8 {
    const STEPS: [u8; 4] = [112, 192, 224, 255];
    STEPS.iter().filter(|&&step| strength >= step).count() as u8
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
}

/// What `Rasterizer::render` needs to know about a glyph.
pub struct Glyph {
    pub glyph_id: u16,
    /// The size in device pixels.
    pub ppem: f32,
    /// Where the pen starts right of the box's pixel grid, 0 to 1.
    pub shift: f32,
    /// How far each stroke side grows, in device pixels.
    pub embolden: f32,
    /// The box: its origin relative to the pen (y down) and size.
    pub origin: (i32, i32),
    pub size: (usize, usize),
}

impl Rasterizer {
    /// The glyph's BGRA coverage in its box, or None when swash can't open
    /// the face or draw the glyph (the caller falls back to grayscale).
    pub fn render(&mut self, font_id: FontId, font: impl FnOnce() -> FaceSource, glyph: &Glyph) -> Option<Vec<u8>> {
        let face = self.faces.entry(font_id).or_insert_with(|| open(font())).as_ref()?;
        let mut scaler = self
            .context
            .builder(face.font())
            .size(glyph.ppem)
            .hint(false)
            .variations(face.variations.iter().copied())
            .build();
        let image = Render::new(&[Source::Outline])
            .format(Format::Alpha)
            .embolden(glyph.embolden)
            .transform(Some(Transform::scale(3.0, 1.0)))
            .offset(Vector::new(glyph.shift * 3.0, 0.0))
            .render(&mut scaler, glyph.glyph_id)?;
        let placement = image.placement;
        let mask = Mask {
            data: &image.data,
            width: placement.width as usize,
            height: placement.height as usize,
            left: placement.left - 3 * glyph.origin.0,
            top: -placement.top - glyph.origin.1,
        };
        Some(filter(&mask, glyph.size.0, glyph.size.1))
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
