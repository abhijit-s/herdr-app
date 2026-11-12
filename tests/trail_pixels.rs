//! The cursor trail's paint, really rendered (headless, read back from the
//! GPU): the comet's streak brightens along its gradient toward the head, and
//! the head's glow is one soft round blur with nothing cut from its middle.
use gpui::{prelude::*, *};

// Only the shapes and their paint, without the app: what they need of settings.
#[allow(dead_code)]
mod settings {
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub enum Arrival {
        Off,
        Ripple,
        Boom,
        RippleRect,
        BoomRect,
    }
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub enum TrailStyle {
        Comet,
        Tail,
        Sweep,
        Warp,
    }
}

#[path = "../src/window/cursor_shapes.rs"]
#[allow(dead_code, unused_imports)]
mod cursor_shapes;
#[path = "../src/window/trail_paint.rs"]
#[allow(dead_code)]
mod trail_paint;

use cursor_shapes::Layer;

struct Scene(Vec<Layer>);
impl Render for Scene {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let layers = self.0.clone();
        div().size_full().bg(rgb(0)).child(
            canvas(
                |_, _, _| {},
                move |_, _, window, _| {
                    trail_paint::paint_layers(&layers, hsla(0.0, 0.0, 1.0, 1.0), window)
                },
            )
            .size_full(),
        )
    }
}

/// `layers` in white on black: each pixel's brightness at px (x, y).
fn rendered(cx: &mut HeadlessAppContext, layers: Vec<Layer>) -> impl Fn(f32, f32) -> u8 {
    let handle = cx
        .open_window(size(px(400.), px(80.)), |_, cx| cx.new(|_| Scene(layers)))
        .unwrap();
    let (image, scale) = cx
        .update_window(handle.into(), |_, window, cx| {
            let _ = window.draw(cx);
            (window.render_to_image().unwrap(), window.scale_factor())
        })
        .unwrap();
    move |x: f32, y: f32| image.get_pixel((x * scale) as u32, (y * scale) as u32).0[0]
}

fn layer(
    outline: Vec<Point<Pixels>>,
    fade: Option<((f32, f32), (f32, f32))>,
    glow: Option<((f32, f32), f32, f32)>,
) -> Layer {
    Layer {
        outline,
        hole: None,
        stroke: None,
        alpha: 1.0,
        core: false,
        fade,
        glow,
    }
}

fn streak_brightens_toward_its_head(cx: &mut HeadlessAppContext) {
    let streak = cursor_shapes::streak((20.0, 40.0), (320.0, 40.0), 8.0);
    let at = rendered(
        cx,
        vec![layer(streak, Some(((20.0, 40.0), (320.0, 40.0))), None)],
    );
    let along: Vec<u8> = [30.0, 100.0, 170.0, 240.0, 310.0]
        .iter()
        .map(|x| at(*x, 40.0))
        .collect();
    assert!(
        along.windows(2).all(|w| w[0] < w[1]),
        "brighter toward the head: {along:?}"
    );
    assert!(
        along[0] < 40 && along[4] > 200,
        "none at the tail, full at the head: {along:?}"
    );
    assert_eq!(at(200.0, 60.0), 0, "nothing off the streak");
    println!("trail pixels: streak {along:?}");
}

fn glow_is_soft_and_round(cx: &mut HeadlessAppContext) {
    // The comet's: spread and blurred 0.3 of a 16 px cell.
    let at = rendered(
        cx,
        vec![layer(Vec::new(), None, Some(((200.0, 40.0), 4.8, 4.8)))],
    );
    let (middle, near, far) = (at(200.0, 40.0), at(208.0, 40.0), at(230.0, 40.0));
    assert!(middle > 90, "bright in the middle, not a hole: {middle}");
    assert!(
        at(202.0, 40.0) <= middle && at(204.0, 40.0) < at(202.0, 40.0),
        "fading from the middle out"
    );
    assert!(0 < near && near < middle, "soft past its radius: {near}");
    assert_eq!(far, 0, "and gone beyond its blur");
    assert!(
        (i16::from(at(200.0, 48.0)) - i16::from(near)).abs() <= 8,
        "round: as bright below as beside"
    );
    println!("trail pixels: glow middle {middle}, near {near}, far {far}");
}

fn main() {
    #[cfg(not(windows))]
    {
        let mut cx = HeadlessAppContext::with_platform(
            gpui_platform::current_platform(true).text_system(),
            std::sync::Arc::new(()),
            gpui_platform::current_headless_renderer,
        );
        streak_brightens_toward_its_head(&mut cx);
        glow_is_soft_and_round(&mut cx);
        println!("trail pixel tests passed");
    }
}
