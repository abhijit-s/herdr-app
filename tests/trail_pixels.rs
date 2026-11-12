//! The cursor trail's paint, really rendered (headless, read back from the
//! GPU): the comet's streak brightens along its gradient toward the head, and
//! the head's glow is one soft round blur with nothing cut from its middle,
//! gradient stops sit on their points at any angle, and the comet as drawn
//! brightens to a pale nose.
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

struct Scene(Vec<Layer>, Hsla);
impl Render for Scene {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let (layers, color) = (self.0.clone(), self.1);
        div().size_full().bg(rgb(0)).child(
            canvas(
                |_, _, _| {},
                move |_, _, window, _| trail_paint::paint_layers(&layers, color, window),
            )
            .size_full(),
        )
    }
}

/// `layers` in white on black: each pixel's brightness at px (x, y).
fn rendered(cx: &mut HeadlessAppContext, layers: Vec<Layer>) -> impl Fn(f32, f32) -> u8 {
    let at = rendered_in(
        cx,
        layers,
        hsla(0.0, 0.0, 1.0, 1.0),
        size(px(400.), px(80.)),
    );
    move |x, y| at(x, y)[0]
}

/// `layers` in `color` on black in a window `area` big: each pixel's RGBA.
fn rendered_in(
    cx: &mut HeadlessAppContext,
    layers: Vec<Layer>,
    color: Hsla,
    area: Size<Pixels>,
) -> impl Fn(f32, f32) -> [u8; 4] {
    let handle = cx
        .open_window(area, |_, cx| cx.new(|_| Scene(layers, color)))
        .unwrap();
    let (image, scale) = cx
        .update_window(handle.into(), |_, window, cx| {
            let _ = window.draw(cx);
            (window.render_to_image().unwrap(), window.scale_factor())
        })
        .unwrap();
    move |x: f32, y: f32| image.get_pixel((x * scale) as u32, (y * scale) as u32).0
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

/// A stop placed with `trail_paint::along` sits on its point at any angle
/// and length: a short streak is full at its head, a diagonal one too.
fn gradient_stops_sit_on_their_points(cx: &mut HeadlessAppContext) {
    for (tail, head) in [
        ((180.0, 40.0), (200.0, 40.0)),
        ((100.0, 10.0), (160.0, 70.0)),
        ((300.0, 70.0), (240.0, 10.0)),
    ] {
        let at = rendered(
            cx,
            vec![layer(
                cursor_shapes::streak(tail, head, 4.0),
                Some((tail, head)),
                None,
            )],
        );
        let point = |k: f32| {
            at(
                tail.0 + (head.0 - tail.0) * k,
                tail.1 + (head.1 - tail.1) * k,
            )
        };
        let samples: Vec<u8> = [0.1, 0.3, 0.5, 0.7, 0.9]
            .iter()
            .map(|k| point(*k))
            .collect();
        assert!(
            samples.windows(2).all(|w| w[0] < w[1]),
            "{tail:?} to {head:?}: {samples:?}"
        );
        let near = |v: u8, want: f32| (f32::from(v) - want * 255.0).abs() < 30.0;
        assert!(
            near(samples[0], 0.1) && near(samples[2], 0.5) && near(samples[4], 0.9),
            "{tail:?} to {head:?}: {samples:?} not about 0.1, 0.5, 0.9 of full"
        );
        println!("trail pixels: gradient {tail:?} to {head:?}: {samples:?}");
    }
}

/// The comet as drawn, in an amber cursor color, at any angle: none at its
/// tail, rising to full strength well before the head and staying there, its
/// nose whiter than the glow beside it.
fn the_comet_brightens_to_a_pale_nose(cx: &mut HeadlessAppContext) {
    let amber = hsla(0.11, 1.0, 0.6, 1.0);
    let cell =
        |x: f32, y: f32| Bounds::new(point(px(x - 4.0), px(y - 8.0)), size(px(8.0), px(16.0)));
    for (from, to) in [
        ((20.0, 100.0), (360.0, 100.0)),
        ((40.0, 20.0), (320.0, 300.0)),
    ] {
        let layers = cursor_shapes::trail(
            settings::TrailStyle::Comet,
            cursor_shapes::quad(cell(from.0, from.1)),
            cell(to.0, to.1),
            16.0,
            0.0,
        )
        .0;
        let at = rendered_in(cx, layers, amber, size(px(400.), px(340.)));
        let point = |k: f32| at(from.0 + (to.0 - from.0) * k, from.1 + (to.1 - from.1) * k);
        let red: Vec<u8> = [0.05, 0.3, 0.5, 0.8, 0.95]
            .iter()
            .map(|k| point(*k)[0])
            .collect();
        assert!(
            red.windows(2).all(|w| w[0] <= w[1]),
            "{from:?}: brightening toward the head {red:?}"
        );
        assert!(
            red[0] < 60 && red[3] > 200 && red[4] > 200,
            "{from:?}: faint at the tail, full from about two thirds on: {red:?}"
        );
        let nose = point(1.0);
        let n = ((to.1 - from.1), -(to.0 - from.0));
        let len = n.0.hypot(n.1);
        let beside = at(
            to.0 - 20.0 * (to.0 - from.0) / len.max(1.0) + 3.2 * n.0 / len,
            to.1 - 20.0 * (to.1 - from.1) / len.max(1.0) + 3.2 * n.1 / len,
        );
        assert!(
            nose[2] > beside[2] + 20,
            "{from:?}: the nose paler (bluer, toward white) than the streak's edge: {nose:?} vs {beside:?}"
        );
        println!("trail pixels: comet {from:?} red {red:?}, nose {nose:?}, edge {beside:?}");
    }
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
        gradient_stops_sit_on_their_points(&mut cx);
        the_comet_brightens_to_a_pale_nose(&mut cx);
        println!("trail pixel tests passed");
    }
}
