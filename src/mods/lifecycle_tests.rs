use super::*;
use gpui::{IntoElement, Render, TestAppContext, VisualTestContext};
use std::ops::Deref;
use gpui_shell::{ShellRoot, ShellRuntime, policy::Policy};
use std::{cell::Cell, rc::Rc, sync::atomic::{AtomicU64, Ordering}, time::Instant};

struct Fixture(PathBuf);
impl Fixture {
    fn new(source: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = PathBuf::from(format!("/var/tmp/herdr-lifecycle-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("main.js"), source).unwrap();
        Self(dir)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}
struct Empty;
impl Render for Empty {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        gpui::div()
    }
}

struct Mounted(gpui::Entity<ShellRoot>);
impl Render for Mounted {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        self.0.clone()
    }
}

/// Stands in for `AppWindow`: the window's root, rendered on every draw.
struct Counted(gpui::Entity<ShellRoot>, Rc<Cell<usize>>);
impl Render for Counted {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        self.1.set(self.1.get() + 1);
        self.0.clone()
    }
}

// A spinner step presents the frame on screen again with the spinner turned:
// no window render, no script, no rebuild of the mod, in a view whose opacity
// changes on hover too.
#[gpui::test]
fn spinners_turn_without_drawing_the_window(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    gpui_shell::policy::set_default(Policy::new());
    for hover in ["", ".hover(el => el.opacity(0.9))"] {
        let fixture = Fixture::new(&format!(r#"
import {{ View, div, svg }} from "gpui-kit";
export default class extends View {{
  render() {{
    return div().size_full(){hover}
      .children([1, 2, 3].map(() => svg("ring.svg").size(12).spin(900)));
  }}
}}"#));
        std::fs::write(fixture.0.join("ring.svg"), r#"<svg xmlns="http://www.w3.org/2000/svg" width="12" height="12"><path d="M6 1a5 5 0 0 1 5 5"/></svg>"#).unwrap();
        cx.set_asset_source(gpui_shell::AppAssets::new(fixture.0.clone()));
        let runtime = ShellRuntime::new_isolated().unwrap();
        let renders = Rc::new(Cell::new(0));
        let counted = renders.clone();
        let window = cx.add_window(|window, cx| Counted(runtime.try_load(&fixture.0, window, cx).unwrap(), counted));
        let handle: gpui::AnyWindowHandle = window.into();
        for _ in 0..3 {
            cx.simulate_window_frame_request(handle);
            cx.run_until_parked();
        }
        let presented = |cx: &mut TestAppContext| handle.update(cx, |_, window, _| window.presented_spinners()).unwrap();
        let first = presented(cx);
        assert_eq!(first.len(), 3, "three spinners on screen ({hover:?})");
        let (drawn, before) = (renders.get(), runtime.read_metrics());
        let mut last = first;
        for _ in 0..5 {
            std::thread::sleep(std::time::Duration::from_millis(15));
            cx.executor().advance_clock(gpui::SPIN_STEP);
            cx.run_until_parked();
            cx.simulate_window_frame_request(handle);
            let now = presented(cx);
            assert_eq!(now.len(), 3);
            assert_ne!(now, last, "each step turns the spinners ({hover:?})");
            last = now;
        }
        let after = runtime.read_metrics();
        assert_eq!(renders.get(), drawn, "a spinner step rendered the window ({hover:?})");
        assert_eq!(after.script_renders(), before.script_renders());
        assert_eq!(after.materializations(), before.materializations(), "a spinner step rebuilt the mod ({hover:?})");

        // Something outside the mod changes: the window draws, replaying the
        // cached mod, whose spinners keep turning.
        window.update(cx, |_, _, cx| cx.notify()).unwrap();
        cx.simulate_window_frame_request(handle);
        cx.run_until_parked();
        assert_eq!(renders.get(), drawn + 1, "the window drew ({hover:?})");
        for _ in 0..2 {
            std::thread::sleep(std::time::Duration::from_millis(15));
            cx.executor().advance_clock(gpui::SPIN_STEP);
            cx.run_until_parked();
            cx.simulate_window_frame_request(handle);
            let now = presented(cx);
            assert_eq!(now.len(), 3, "replayed spinners are on screen ({hover:?})");
            assert_ne!(now, last, "and still turn ({hover:?})");
            last = now;
        }
        assert_eq!(runtime.read_metrics().materializations(), before.materializations(), "the replay rebuilt the mod ({hover:?})");

        window.update(cx, |_, window, cx| window.replace_root(cx, |_, _| Empty)).unwrap();
        cx.simulate_window_frame_request(handle);
        assert!(presented(cx).is_empty(), "a frame without spinners turns nothing");
        // The step already scheduled finds no spinners: no more present-only frames.
        let presents = handle.update(cx, |_, window, _| window.presents()).unwrap();
        cx.executor().advance_clock(gpui::SPIN_STEP * 2);
        cx.run_until_parked();
        cx.simulate_window_frame_request(handle);
        assert_eq!(handle.update(cx, |_, window, _| window.presents()).unwrap(), presents, "presented with no spinners ({hover:?})");
        let _ = handle.update(cx, |_, window, _| window.remove_window());
        cx.run_until_parked();
    }
}

// A `frames` spinner steps as a ring does: each present shows the frame the
// shared clock is on, with no window render, no script and no rebuild of the
// mod. CoreText shapes it, so its glyphs are real sprites.
#[cfg(target_os = "macos")]
#[test]
fn frames_step_without_drawing_the_window() {
    let cx = &mut TestAppContext::build_with_text_system(gpui::TestDispatcher::new(0), None, std::sync::Arc::new(gpui_macos::MacTextSystem::new()));
    cx.update(gpui_shell::init);
    gpui_shell::policy::set_default(Policy::new());
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
export default class extends View {
  render() {
    return div().size_full().child(div().child("Pi"))
      .child(div().frames(["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"], 80).child("⠋"));
  }
}"#);
    let runtime = ShellRuntime::new_isolated().unwrap();
    let renders = Rc::new(Cell::new(0));
    let counted = renders.clone();
    let window = cx.add_window(|window, cx| Counted(runtime.try_load(&fixture.0, window, cx).unwrap(), counted));
    let handle: gpui::AnyWindowHandle = window.into();
    for _ in 0..3 {
        cx.simulate_window_frame_request(handle);
        cx.run_until_parked();
    }
    let presented = |cx: &mut TestAppContext| handle.update(cx, |_, window, _| window.presented_frames()).unwrap();
    let first = presented(cx);
    assert_eq!(first.iter().map(|c| (c.count, c.period_ms)).collect::<Vec<_>>(), [(10, 80)], "one frame of ten on screen");
    let (drawn, before) = (renders.get(), runtime.read_metrics());
    let presents = handle.update(cx, |_, window, _| window.presents()).unwrap();
    let started = Instant::now();
    // The shared clock is real time and the timers run on the simulated
    // one: move both, 20 ms a display frame.
    for _ in 0..50 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        cx.executor().advance_clock(std::time::Duration::from_millis(20));
        cx.run_until_parked();
        cx.simulate_window_frame_request(handle);
        cx.run_until_parked();
    }
    let after = runtime.read_metrics();
    assert_eq!(renders.get(), drawn, "a simulated second of frames rendered the window");
    assert_eq!(after.script_renders(), before.script_renders());
    assert_eq!(after.materializations(), before.materializations(), "a frame step rebuilt the mod");
    let stepped = handle.update(cx, |_, window, _| window.presents()).unwrap() - presents;
    // A present per frame the real clock reached, however long the loop took.
    let most = started.elapsed().as_millis() as usize / 80 + 2;
    assert!((12..=most).contains(&stepped), "a present per 80 ms frame (12 to {most}), not per display frame: {stepped}");
    // As the clock moves on, so does the frame shown.
    let mut last = first[0].index;
    for _ in 0..3 {
        std::thread::sleep(std::time::Duration::from_millis(85));
        cx.executor().advance_clock(std::time::Duration::from_millis(100));
        cx.run_until_parked();
        cx.simulate_window_frame_request(handle);
        let now = presented(cx);
        assert_eq!(now.len(), 1, "one frame at a time: {now:?}");
        assert_ne!(now[0].index, last, "the frame advances");
        last = now[0].index;
    }
    assert_eq!(renders.get(), drawn, "still without a render");
    let _ = handle.update(cx, |_, window, _| window.remove_window());
    cx.run_until_parked();
}

// A sooner step replaces a later one already scheduled: a slow cycle on
// screen doesn't hold back a fast one drawn after it.
#[cfg(target_os = "macos")]
#[test]
fn a_faster_spinner_drawn_later_steps_at_its_own_pace() {
    let cx = &mut TestAppContext::build_with_text_system(gpui::TestDispatcher::new(0), None, std::sync::Arc::new(gpui_macos::MacTextSystem::new()));
    cx.update(gpui_shell::init);
    gpui_shell::policy::set_default(Policy::new());
    let slow = r#"div().frames(["◐", "◓"], 5000).child("◐")"#;
    let only_slow = Fixture::new(&format!(r#"import {{ View, div }} from "gpui-kit"; export default class extends View {{ render() {{ return div().size_full().child({slow}); }} }}"#));
    let both = Fixture::new(&format!(r#"import {{ View, div }} from "gpui-kit"; export default class extends View {{ render() {{ return div().size_full().child({slow}).child(div().frames(["⠋", "⠙", "⠹", "⠸"], 80).child("⠋")); }} }}"#));
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|window, cx| Mounted(runtime.try_load(&only_slow.0, window, cx).unwrap()));
    let handle: gpui::AnyWindowHandle = window.into();
    for _ in 0..3 {
        cx.simulate_window_frame_request(handle);
        cx.run_until_parked();
    }
    window.update(cx, |_, window, cx| window.replace_root(cx, |window, cx| Mounted(runtime.try_load(&both.0, window, cx).unwrap()))).unwrap();
    cx.simulate_window_frame_request(handle);
    cx.run_until_parked();
    let presents = handle.update(cx, |_, window, _| window.presents()).unwrap();
    for _ in 0..12 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        cx.executor().advance_clock(std::time::Duration::from_millis(20));
        cx.run_until_parked();
        cx.simulate_window_frame_request(handle);
        cx.run_until_parked();
    }
    let stepped = handle.update(cx, |_, window, _| window.presents()).unwrap() - presents;
    assert!(stepped >= 2, "240 ms of an 80 ms spinner beside a 5 s one: {stepped} presents");
    let _ = handle.update(cx, |_, window, _| window.remove_window());
    cx.run_until_parked();
}

// Colour emoji take turns as glyphs do, and frames nobody can see (faded out,
// or drawn in a clear colour) and a list of one frame are left out of the
// cycles, so they don't keep the window presenting.
#[cfg(target_os = "macos")]
#[test]
fn emoji_frames_cycle_and_invisible_frames_are_left_out() {
    let cx = &mut TestAppContext::build_with_text_system(gpui::TestDispatcher::new(0), None, std::sync::Arc::new(gpui_macos::MacTextSystem::new()));
    cx.update(gpui_shell::init);
    gpui_shell::policy::set_default(Policy::new());
    let fixture = Fixture::new(r##"
import { View, div } from "gpui-kit";
export default class extends View {
  render() {
    return div().size_full()
      .child(div().frames(["🌑", "🌓", "🌕"], 200).child("🌑"))
      .child(div().opacity(0).child(div().frames(["⠋", "⠙"], 80).child("⠋")))
      .child(div().opacity(0).child(div().frames(["🌑", "🌕"], 150).child("🌑")))
      .child(div().text_color("#00000000").frames(["◐", "◓"], 90).child("◐"))
      .child(div().frames(["●"], 80).child("●"));
  }
}"##);
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|window, cx| Mounted(runtime.try_load(&fixture.0, window, cx).unwrap()));
    let handle: gpui::AnyWindowHandle = window.into();
    for _ in 0..3 {
        cx.simulate_window_frame_request(handle);
        cx.run_until_parked();
    }
    let presented = handle.update(cx, |_, window, _| window.presented_frames()).unwrap();
    assert_eq!(presented.iter().map(|c| (c.count, c.period_ms)).collect::<Vec<_>>(), [(3, 200)], "one emoji frame: nothing invisible, and one frame alone draws plainly");
    let _ = handle.update(cx, |_, window, _| window.remove_window());
    cx.run_until_parked();
}

// A spinner nobody can see (faded out, or drawn in a clear colour) is left
// out of the frame, so it doesn't keep the window presenting ~10 times a second.
#[gpui::test]
fn invisible_spinners_do_not_keep_the_window_presenting(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    gpui_shell::policy::set_default(Policy::new());
    let fixture = Fixture::new(r##"
import { View, div, svg } from "gpui-kit";
export default class extends View {
  render() {
    return div().size_full()
      .child(svg("ring.svg").size(12).spin(900))
      .child(div().opacity(0).child(svg("ring.svg").size(12).spin(900)))
      .child(svg("ring.svg").size(12).text_color("#00000000").spin(900));
  }
}"##);
    std::fs::write(fixture.0.join("ring.svg"), r#"<svg xmlns="http://www.w3.org/2000/svg" width="12" height="12"><path d="M6 1a5 5 0 0 1 5 5"/></svg>"#).unwrap();
    cx.set_asset_source(gpui_shell::AppAssets::new(fixture.0.clone()));
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|window, cx| Mounted(runtime.try_load(&fixture.0, window, cx).unwrap()));
    let handle: gpui::AnyWindowHandle = window.into();
    for _ in 0..3 {
        cx.simulate_window_frame_request(handle);
        cx.run_until_parked();
    }
    let presented = handle.update(cx, |_, window, _| window.presented_spinners()).unwrap();
    assert_eq!(presented.len(), 1, "only the visible spinner is on screen");
    let _ = handle.update(cx, |_, window, _| window.remove_window());
    cx.run_until_parked();
}

// `spin` takes any number a script hands the bridge: a non-finite period is
// ignored and a huge one clamped, so none of them panics the UI thread while it
// draws. `.spin()` rejects these in JavaScript, but a script can call the global
// `__apply` itself, which is the path the clamp guards.
#[gpui::test]
fn spin_with_an_absurd_period_draws_without_panicking(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    gpui_shell::policy::set_default(Policy::new());
    let fixture = Fixture::new(r##"
import { View, div, svg } from "gpui-kit";
const spun = (ms) => {
  const ring = svg("ring.svg").size(12);
  let accepted = true;
  try { ring.spin(ms); } catch { accepted = false; }
  if (accepted) throw new Error("spin(" + ms + ") was accepted");
  __apply(ring.__id, "spin", [ms]);
  return ring;
};
export default class extends View {
  render() {
    return div().size_full().children([Infinity, NaN, 1e300].map(spun));
  }
}"##);
    std::fs::write(fixture.0.join("ring.svg"), r#"<svg xmlns="http://www.w3.org/2000/svg" width="12" height="12"><path d="M6 1a5 5 0 0 1 5 5"/></svg>"#).unwrap();
    cx.set_asset_source(gpui_shell::AppAssets::new(fixture.0.clone()));
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|window, cx| Mounted(runtime.try_load(&fixture.0, window, cx).unwrap()));
    let handle: gpui::AnyWindowHandle = window.into();
    for _ in 0..3 {
        cx.simulate_window_frame_request(handle);
        cx.run_until_parked();
    }
    let presented = handle.update(cx, |_, window, _| window.presented_spinners()).unwrap();
    assert_eq!(presented.len(), 1, "only the clamped period spins");
    let _ = handle.update(cx, |_, window, _| window.remove_window());
    cx.run_until_parked();
}

#[gpui::test]
fn size_change_reports_initial_and_resized_layout_without_pointer_input(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
import { note } from "fixture";
export default class extends View {
  render() {
    return div().id("root").size_full().border(3).p(7)
      .on_size_change((size, cx) => { note(size.width, size.height); cx.notify(); });
  }
}"#);
    let sizes = Rc::new(RefCell::new(Vec::new()));
    let calls = sizes.clone();
    gpui_shell::policy::set_default(Policy::new().with_host_module(
        HostModule::new("fixture").function("note", move |a| {
            calls.borrow_mut().push((a.number(0)?, a.number(1)?));
            Ok(HostValue::Null)
        })
    ).unwrap());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|window, cx| Mounted(runtime.try_load(&fixture.0, window, cx).unwrap()));
    let mut visual = VisualTestContext::from_window(*window.deref(), cx);
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.run_until_parked();
    assert_eq!(sizes.borrow().len(), 1);
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.run_until_parked();
    assert_eq!(sizes.borrow().len(), 1, "unchanged layout must not repeat the callback");
    visual.simulate_resize(gpui::size(gpui::px(720.), gpui::px(480.)));
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.run_until_parked();
    assert_eq!(sizes.borrow().last().copied(), Some((720., 480.)));
    assert_eq!(sizes.borrow().len(), 2);
}


#[gpui::test]
fn plain_mod_scroll_handles_reveal_children_once_without_moving_visible_items(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    for (overflow, horizontal) in [("overflow_x_scroll", true), ("overflow_x_scrollbar", true), ("overflow_y_scroll", false), ("overflow_y_scrollbar", false)] {
        let source = r#"
import { View, div } from "gpui-kit";
import { ScrollHandle } from "gpui-base";
import { note } from "fixture";
export default class extends View {
  init(_, cx) {
    this.scroll = ScrollHandle.new();
    this.scroll.scroll_to_item(4);
    this.focus = cx.focus_handle();
    this.focus.focus();
  }
  render() {
    return div().size_full().track_focus(this.focus)
      .on_key_down((e, cx) => {
        this.scroll.scroll_to_item(Number(e.key));
        cx.notify();
      })
      .child(div().id("area").flex().DIRECTION().w(WIDTH).h(HEIGHT).OVERFLOW().track_scroll(this.scroll).on_scroll_wheel(() => {})
        .children([0, 1, 2, 3, 4].map(i => div().id("item-" + i).w(40).h(40).flex_none()
          .on_click(() => note(i)).child(String(i)))));
  }
}"#.replace("OVERFLOW", overflow)
            .replace("DIRECTION", if horizontal { "flex_row" } else { "flex_col" })
            .replace("WIDTH", if horizontal { "120" } else { "40" })
            .replace("HEIGHT", if horizontal { "40" } else { "120" });
        let fixture = Fixture::new(&source);
        let notes = Rc::new(RefCell::new(Vec::new()));
        let calls = notes.clone();
        gpui_shell::policy::set_default(Policy::new().with_host_module(
            HostModule::new("fixture").function("note", move |a| {
                calls.borrow_mut().push(a.number(0)? as usize);
                Ok(HostValue::Null)
            })
        ).unwrap());
        let runtime = ShellRuntime::new_isolated().unwrap();
        let window = cx.add_window(|window, cx| Mounted(runtime.try_load(&fixture.0, window, cx).unwrap()));
        let mut visual = VisualTestContext::from_window(*window.deref(), cx);
        for _ in 0..3 {
            visual.update(|window, cx| {
                window.simulate_next_frame(cx);
                window.draw(cx).clear(cx);
            });
            visual.run_until_parked();
        }
        let position = |along| if horizontal { gpui::point(gpui::px(along), gpui::px(20.)) } else { gpui::point(gpui::px(20.), gpui::px(along)) };
        visual.simulate_click(position(100.), Default::default());
        assert_eq!(*notes.borrow(), vec![4], "{overflow}: the last child must be visible");
        visual.simulate_keystrokes("3");
        for _ in 0..3 {
            visual.update(|window, cx| {
                window.simulate_next_frame(cx);
                window.draw(cx).clear(cx);
            });
            visual.run_until_parked();
        }
        visual.simulate_click(position(10.), Default::default());
        assert_eq!(*notes.borrow(), vec![4, 2], "{overflow}: revealing a visible child must keep the offset");
        visual.simulate_keystrokes("0");
        for _ in 0..3 {
            visual.update(|window, cx| {
                window.simulate_next_frame(cx);
                window.draw(cx).clear(cx);
            });
            visual.run_until_parked();
        }
        visual.simulate_click(position(10.), Default::default());
        assert_eq!(*notes.borrow(), vec![4, 2, 0], "{overflow}: the first child must be visible again");
        let delta = if horizontal { gpui::point(gpui::px(-80.), gpui::px(0.)) } else { gpui::point(gpui::px(0.), gpui::px(-80.)) };
        visual.simulate_mouse_move(position(20.), None, Default::default());
        visual.simulate_event(gpui::ScrollWheelEvent {
            position: position(20.),
            delta: gpui::ScrollDelta::Pixels(delta),
            ..Default::default()
        });
        for _ in 0..3 {
            visual.update(|window, cx| {
                window.simulate_next_frame(cx);
                window.draw(cx).clear(cx);
            });
            visual.run_until_parked();
        }
        visual.simulate_click(position(100.), Default::default());
        assert_eq!(*notes.borrow(), vec![4, 2, 0, 4], "{overflow}: manual scrolling must work after a consumed request");
        visual.update(|window, _| window.remove_window());
        visual.run_until_parked();
    }
}

#[gpui::test]
fn external_file_drops_reach_mod_elements(cx: &mut TestAppContext) {
    use gpui::{ExternalPaths, FileDropEvent, InputEvent};
    cx.update(gpui_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
import { note } from "fixture";
export default class extends View {
  render() {
    return div().size_full().on_drop((event, cx) => { note(event.paths); cx.stop_propagation(); });
  }
}"#);
    let paths = Rc::new(RefCell::new(Vec::new()));
    let calls = paths.clone();
    gpui_shell::policy::set_default(Policy::new().with_host_module(
        HostModule::new("fixture").function("note", move |a| {
            calls.borrow_mut().push(from_host(a.value(0)?));
            Ok(HostValue::Null)
        })
    ).unwrap());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|window, cx| Mounted(runtime.try_load(&fixture.0, window, cx).unwrap()));
    let mut visual = VisualTestContext::from_window(*window.deref(), cx);
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.run_until_parked();
    visual.update(|window, cx| {
        let position = gpui::point(gpui::px(20.), gpui::px(20.));
        // Typed last, as before dragging a screenshot in: no mouse move reaches the window.
        window.dispatch_event(gpui::KeyDownEvent {
            keystroke: gpui::Keystroke::parse("a").unwrap(),
            is_held: false,
            prefer_character_input: false,
        }.to_platform_input(), cx);
        window.dispatch_event(FileDropEvent::Entered {
            position,
            paths: ExternalPaths([PathBuf::from("/var/tmp/screenshot.png"), PathBuf::from("/var/tmp/a file.jpg")].into_iter().collect()),
        }.to_platform_input(), cx);
        window.draw(cx).clear(cx);
        window.dispatch_event(FileDropEvent::Submit { position }.to_platform_input(), cx);
    });
    visual.run_until_parked();
    assert_eq!(*paths.borrow(), vec![json!(["/var/tmp/screenshot.png", "/var/tmp/a file.jpg"])]);
}

#[gpui::test]
fn mod_run_rejects_unknown_arguments_before_queueing_a_command(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
import { run } from "herdr";
import { note } from "fixture";
export default class extends View {
  init() {
    for (const [name, args] of [["space.close", {space: "X", confirmed: true}], ["terminal.kill", {id: "X"}]]) {
      try { run(name, args); note("accepted invalid command"); }
      catch (error) { note(String(error)); }
    }
  }
  render() { return div(); }
}"#);
    let seen = Rc::new(RefCell::new(Vec::new()));
    let calls = seen.clone();
    gpui_shell::policy::set_default(Policy::new()
        .with_host_module(module(Instance { name: "test".into(), props: Value::Null }, 0)).unwrap()
        .with_host_module(HostModule::new("fixture").function("note", move |a| {
            calls.borrow_mut().push(a.string(0)?.to_owned());
            Ok(HostValue::Null)
        })).unwrap());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|_, _| Empty);
    let _root = window.update(cx, |_, window, cx| runtime.try_load(&fixture.0, window, cx).unwrap()).unwrap();
    let seen = seen.borrow();
    assert_eq!(seen.len(), 2);
    assert!(seen[0].contains("space.close: unknown argument(s): space; valid arguments: id, confirmed"), "{}", seen[0]);
    assert!(seen[1].contains("terminal.kill: unknown argument(s): id; valid arguments: terminal"), "{}", seen[1]);
}

#[gpui::test]
fn focus_command_lets_a_mod_take_arrow_keys(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
import { nextCommand } from "herdr";
import { note } from "fixture";
export default class extends View {
  init(_props, cx) {
    this.handle = cx.focus_handle();
    cx.spawn(async (cx) => {
      for (;;) {
        const command = await nextCommand();
        if (command === "focus") { this.handle.focus(); note(command); }
        cx.notify();
      }
    });
  }
  render() {
    return div().size_full().track_focus(this.handle)
      .on_key_down((e, cx) => { note(e.key); cx.stop_propagation(); });
  }
}"#);
    let notes = Rc::new(RefCell::new(Vec::new()));
    let calls = notes.clone();
    gpui_shell::policy::set_default(Policy::new()
        .with_host_module(module(Instance { name: "focus-test".into(), props: Value::Null }, 42)).unwrap()
        .with_host_module(HostModule::new("fixture").function("note", move |a| {
            calls.borrow_mut().push(a.string(0)?.to_owned());
            Ok(HostValue::Null)
        })).unwrap());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|window, cx| Mounted(runtime.try_load(&fixture.0, window, cx).unwrap()));
    let mut visual = VisualTestContext::from_window(*window.deref(), cx);
    visual.run_until_parked();
    set_shown_copies([42].into());
    assert!(send_command_to(42, "focus"));
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.simulate_keystrokes("down up");
    visual.run_until_parked();
    assert_eq!(*notes.borrow(), vec!["focus", "down", "up"]);
    set_shown_copies(Default::default());
}

#[gpui::test]
fn close_is_once_and_awaits_its_promise(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
import { note } from "fixture";
export default class extends View {
  close(cx) {
    note("start");
    return Promise.resolve().then(() => { note("done"); cx.notify(); });
  }
  render() { return div(); }
}"#);
    let seen = Rc::new(RefCell::new(Vec::new()));
    let calls = seen.clone();
    gpui_shell::policy::set_default(Policy::new().with_host_module(
        HostModule::new("fixture").function("note", move |a| {
            calls.borrow_mut().push(a.string(0)?.to_owned());
            Ok(HostValue::Null)
        })
    ).unwrap());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|_, _| Empty);
    let root = window.update(cx, |_, window, cx| runtime.try_load(&fixture.0, window, cx).unwrap()).unwrap();
    window.update(cx, |_, window, cx| {
        let deadline = Instant::now() + Duration::from_millis(250);
        runtime.close(&root, deadline, window, cx).detach();
        runtime.close(&root, deadline, window, cx).detach();
    }).unwrap();
    cx.run_until_parked();
    assert_eq!(*seen.borrow(), ["start", "done"]);
    window.update(cx, |_, window, _| window.remove_window()).unwrap();
}

#[gpui::test]
fn close_starts_native_work_before_releasing_the_copy(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
import { start } from "fixture";
export default class extends View {
  close() { start(); }
  render() { return div(); }
}"#);
    let started = Rc::new(Cell::new(false));
    let call = started.clone();
    gpui_shell::policy::set_default(Policy::new().with_host_module(
        HostModule::new("fixture").async_function("start", move |_| {
            call.set(true);
            // Starting native work must not depend on its promise settling.
            Ok(std::future::pending::<gpui_shell::HostResult>())
        })
    ).unwrap());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|_, _| Empty);
    let root = window.update(cx, |_, window, cx| runtime.try_load(&fixture.0, window, cx).unwrap()).unwrap();
    window.update(cx, |_, window, cx| runtime.close(&root, Instant::now() + Duration::from_millis(250), window, cx).detach()).unwrap();
    drop(root);
    cx.run_until_parked();
    assert!(started.get());
    window.update(cx, |_, window, _| window.remove_window()).unwrap();
}

#[gpui::test]
fn final_close_pumps_native_awaits_before_the_window_goes(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
import { wait, note } from "fixture";
export default class extends View {
  async close(cx) { await cx.sleep(1); await wait(); note(); await wait(); note(); }
  render() { return div(); }
}"#);
    let completed = Rc::new(Cell::new(0));
    let calls = completed.clone();
    gpui_shell::policy::set_default(Policy::new().with_host_module(
        HostModule::new("fixture")
            .async_function("wait", |_| Ok(async {
                std::thread::sleep(Duration::from_millis(10));
                Ok(HostValue::Null)
            }))
            .function("note", move |_| { calls.set(calls.get() + 1); Ok(HostValue::Null) })
    ).unwrap());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|_, _| Empty);
    let root = window.update(cx, |_, window, cx| runtime.try_load(&fixture.0, window, cx).unwrap()).unwrap();
    window.update(cx, |_, window, cx| {
        // Each native await runs on its own thread; on a loaded runner that took up to 205ms for the
        // whole close. The deadline only bounds a hang, and the pump returns as soon as both finish.
        runtime.close_now(&root, Instant::now() + Duration::from_secs(5), window, cx);
        assert_eq!(completed.get(), 2, "both native awaits must finish before window destruction");
        window.remove_window();
    }).unwrap();
}

#[gpui::test]
fn final_close_stops_waiting_at_its_deadline(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
import { wait } from "fixture";
export default class extends View {
  async close() { await wait(); }
  render() { return div(); }
}"#);
    gpui_shell::policy::set_default(Policy::new().with_host_module(
        HostModule::new("fixture").async_function("wait", |_| Ok(std::future::pending::<gpui_shell::HostResult>()))
    ).unwrap());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|_, _| Empty);
    let root = window.update(cx, |_, window, cx| runtime.try_load(&fixture.0, window, cx).unwrap()).unwrap();
    let start = Instant::now();
    window.update(cx, |_, window, cx| {
        runtime.close_now(&root, start + Duration::from_millis(15), window, cx);
        window.remove_window();
    }).unwrap();
    assert!(start.elapsed() < Duration::from_millis(200));
}

#[gpui::test]
fn close_interrupts_a_synchronous_loop_at_the_shared_deadline(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
export default class extends View {
  close() { while (true) {} }
  render() { return div(); }
}"#);
    gpui_shell::policy::set_default(Policy::new());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|_, _| Empty);
    let root = window.update(cx, |_, window, cx| runtime.try_load(&fixture.0, window, cx).unwrap()).unwrap();
    let start = Instant::now();
    window.update(cx, |_, window, cx| runtime.close(&root, start + Duration::from_millis(15), window, cx).detach()).unwrap();
    assert!(start.elapsed() < Duration::from_millis(200));
    cx.run_until_parked();
    window.update(cx, |_, window, _| window.remove_window()).unwrap();
}

#[gpui::test]
fn virtual_list_rows_carry_their_own_handlers(cx: &mut TestAppContext) {
    use gpui::{Modifiers, point, px};
    cx.update(gpui_shell::init);
    // A card per row, as the agents sidebar draws them: the card notes a
    // focus, its pin stops propagation, and hover swaps what the card shows.
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
import { v_flex, h_flex, v_virtual_list } from "gpui-base";
import { note } from "fixture";
export default class extends View {
  init() { this.hovered = null; }
  render() {
    return v_flex().w(300).h(200).child(v_virtual_list("cards", 100, 40, (i) => `a${i}`, (range) => {
      const rows = [];
      for (let i = range.start; i < range.end; i++) {
        const key = `a${i}`;
        rows.push(h_flex().id(`card-${key}`).h(40)
          .on_hover((on, cx) => { if (on) { this.hovered = key; note(`hover ${key}`); cx.notify(); } })
          .on_click(() => note(`focus ${key}`))
          .child(div().id(`pin-${key}`).w(20).h(20).on_click((_e, cx) => { cx.stop_propagation(); note(`pin ${key}`); }))
          .child(this.hovered === key ? "pin" : key));
      }
      return rows;
    }).on_item_click((key) => note(`item ${key}`)));
  }
}"#);
    let notes = Rc::new(RefCell::new(Vec::new()));
    let calls = notes.clone();
    gpui_shell::policy::set_default(Policy::new().with_host_module(
        HostModule::new("fixture").function("note", move |a| {
            calls.borrow_mut().push(a.string(0)?.to_owned());
            Ok(HostValue::Null)
        })
    ).unwrap());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|window, cx| Mounted(runtime.try_load(&fixture.0, window, cx).unwrap()));
    let mut visual = VisualTestContext::from_window(*window.deref(), cx);
    let frame = |visual: &mut VisualTestContext| {
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));
    };
    frame(&mut visual);
    frame(&mut visual);

    // The second card spans 40..80; its pin is the 20px square at its top left.
    visual.simulate_mouse_move(point(px(150.), px(60.)), None, Modifiers::default());
    frame(&mut visual);
    visual.simulate_click(point(px(150.), px(60.)), Modifiers::default());
    frame(&mut visual);
    visual.simulate_click(point(px(10.), px(50.)), Modifiers::default());
    frame(&mut visual);
    // The test pointer starts at the window's origin, over the first card.
    assert_eq!(*notes.borrow(), vec!["hover a0", "hover a1", "focus a1", "item a1", "pin a1"]);
}
