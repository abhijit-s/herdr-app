//! How the cursor effects' shapes (window/cursor_shapes.rs) are painted: in
//! the effect's color, the comet's core paler, a streak brightening along a
//! GPUI gradient and a glow as one blurred shadow. Apart from the window so
//! tests/trail_pixels.rs renders it headless and reads the pixels back.

use super::cursor_shapes::{Layer, Tone};
use gpui::{
    Bounds, BoxShadow, Corners, FillOptions, FillRule, Hsla, PathBuilder, PathStyle, Window,
    linear_color_stop, linear_gradient, point, px, size,
};

/// How far the comet's core color is mixed toward white, as Rex mixes it.
const CORE_WHITE: f32 = 0.25;

/// The core's color: the trail's mixed `CORE_WHITE` toward white.
pub(crate) fn paler(c: Hsla) -> Hsla {
    Hsla {
        l: c.l + (1.0 - c.l) * CORE_WHITE,
        ..c
    }
}

/// The blaze's own flame: amber, with a red edge.
/// sRGB (1.0, 0.725, 0.161).
const AMBER: Hsla = Hsla { h: 40.33 / 360.0, s: 1.0, l: 0.5805, a: 1.0 };
const RED: Hsla = Hsla { h: 0.0, s: 1.0, l: 0.5, a: 1.0 };

/// The colors an effect is painted in: its color (the theme's cursor or
/// `cursor_trail_color`), and the blaze's flame and edge: amber and red, or,
/// when `cursor_trail_color` is set, that color and a deeper one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Tint {
    pub(crate) color: Hsla,
    pub(crate) flame: (Hsla, Hsla),
}

impl Tint {
    /// The tint for the effect `color`, `chosen` when `cursor_trail_color` set it.
    pub(crate) fn new(color: Hsla, chosen: bool) -> Self {
        let flame = if chosen { (color, deeper(color)) } else { (AMBER, RED) };
        Tint { color, flame }
    }

    fn of(&self, tone: Tone) -> Hsla {
        match tone {
            Tone::Effect => self.color,
            Tone::Core => paler(self.color),
            Tone::Flame => self.flame.0,
            Tone::FlameEdge => self.flame.1,
        }
    }
}

/// `c` darker and more saturated: a chosen flame color's edge.
pub(crate) fn deeper(c: Hsla) -> Hsla {
    Hsla {
        s: (c.s + 0.25).min(1.0),
        l: c.l * 0.6,
        ..c
    }
}

/// Paints `layers` back to front in `tint`: filled (even-odd, so a hole
/// stays clear) or stroked outlines, closed or open, brightening along a
/// `fade`, and soft round glows (a blurred shadow with nothing under it).
pub(crate) fn paint_layers(layers: &[Layer], tint: &Tint, window: &mut Window) {
    for layer in layers {
        let tint = tint.of(layer.tone);
        if let Some(((x, y), radius, blur)) = layer.glow {
            let at = Bounds::new(point(px(x), px(y)), size(px(0.0), px(0.0)));
            let shadow = BoxShadow {
                color: tint.opacity(layer.alpha),
                offset: point(px(0.0), px(0.0)),
                blur_radius: px(blur),
                spread_radius: px(radius),
                inset: false,
            };
            window.paint_drop_shadows(at, Corners::all(px(radius)), &[shadow]);
            continue;
        }
        let mut path = match layer.stroke {
            Some(w) => PathBuilder::stroke(px(w)),
            None => PathBuilder::fill().with_style(PathStyle::Fill(
                FillOptions::default().with_fill_rule(FillRule::EvenOdd),
            )),
        };
        path.add_polygon(&layer.outline, !layer.open);
        if let Some(hole) = &layer.hole {
            path.add_polygon(hole, true);
        }
        let Ok(path) = path.build() else { continue };
        match layer.fade {
            // CSS angles: 0 points up, 90 right; from the tail toward the head.
            Some((from, to)) if (to.0 - from.0).hypot(to.1 - from.1) >= 1.0 => {
                let angle = (to.1 - from.1).atan2(to.0 - from.0).to_degrees() + 90.0;
                let (start, end) = (
                    along(&layer.outline, angle, from),
                    along(&layer.outline, angle, to),
                );
                let fade = linear_gradient(
                    angle,
                    linear_color_stop(tint.opacity(0.0), start),
                    linear_color_stop(tint.opacity(layer.alpha), end),
                );
                window.paint_path(path, fade);
            }
            _ => window.paint_path(path, tint.opacity(layer.alpha)),
        }
    }
}

/// Where `p` falls in a GPUI linear gradient at `angle` over the bounds of
/// `outline` (0 one edge, 1 the other), as the GPU's `fill_color` measures
/// it: the direction squashed to the bounds' shape and the projection taken
/// over its longer side, so a stop placed here sits on `p` at any angle.
pub(crate) fn along(outline: &[gpui::Point<gpui::Pixels>], angle: f32, p: (f32, f32)) -> f32 {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for q in outline {
        let (x, y) = (f32::from(q.x), f32::from(q.y));
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
    }
    let (w, h) = ((x1 - x0).max(1e-3), (y1 - y0).max(1e-3));
    let r = (angle.rem_euclid(360.0) - 90.0).to_radians();
    let (mut dx, mut dy) = (r.cos(), r.sin());
    if w > h {
        dy *= h / w;
    } else {
        dx *= w / h;
    }
    let (cx, cy) = (x0 + w / 2.0, y0 + h / 2.0);
    let t = ((p.0 - cx) * dx + (p.1 - cy) * dy) / dx.hypot(dy);
    if dx.abs() > dy.abs() {
        (t + w / 2.0) / w
    } else {
        (t + h / 2.0) / h
    }
}
