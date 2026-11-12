//! How the cursor effects' shapes (window/cursor_shapes.rs) are painted: in
//! the effect's color, the comet's core paler, a streak brightening along a
//! GPUI gradient and a glow as one blurred shadow. Apart from the window so
//! tests/trail_pixels.rs renders it headless and reads the pixels back.

use super::cursor_shapes::Layer;
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

/// Paints `layers` back to front in `color`: filled (even-odd, so a hole
/// stays clear) or stroked outlines, brightening along a `fade`, and soft
/// round glows (a blurred shadow with nothing under it).
pub(crate) fn paint_layers(layers: &[Layer], color: Hsla, window: &mut Window) {
    for layer in layers {
        let tint = if layer.core { paler(color) } else { color };
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
        path.add_polygon(&layer.outline, true);
        if let Some(hole) = &layer.hole {
            path.add_polygon(hole, true);
        }
        let Ok(path) = path.build() else { continue };
        match layer.fade {
            // CSS angles: 0 points up, 90 right; from the tail toward the head.
            Some(((x0, y0), (x1, y1))) if (x1 - x0).hypot(y1 - y0) >= 1.0 => {
                let angle = (y1 - y0).atan2(x1 - x0).to_degrees() + 90.0;
                let fade = linear_gradient(
                    angle,
                    linear_color_stop(tint.opacity(0.0), 0.0),
                    linear_color_stop(tint.opacity(layer.alpha), 1.0),
                );
                window.paint_path(path, fade);
            }
            _ => window.paint_path(path, tint.opacity(layer.alpha)),
        }
    }
}
