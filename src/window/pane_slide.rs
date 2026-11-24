//! Pane slide (`pane_motion`): see FORK.md, Pane slide.

use super::*;
use crate::terminal::TerminalView;
use std::time::{Duration, Instant};

/// A pane dropped on another by its grip: not a command, but it slides as one.
pub(super) const DRAG_MOVE: &str = "pane.move";
/// The commands whose layout change slides. Floating a pane slides the rest
/// into its space (the float itself shows at once); docking one slides it in
/// from where it floated.
const SLIDES: [&str; 9] = ["pane.split", "pane.close", "pane.swap", "pane.arrange", "terminal.open", DRAG_MOVE, "pane.float", "pane.dock", "pane.float_toggle"];
/// Commands starting with these land a running slide first.
const LANDS: [&str; 5] = ["pane.", "tab.", "space.", "layout.", "terminal.open"];

/// The shown tab's blocks before a command that may slide them.
pub(super) struct Before {
    tab: String,
    tree: Node,
    slots: HashMap<String, Bounds<Pixels>>,
}

pub(super) struct Slide {
    tab: String,
    tree: Node,
    start: Instant,
    duration: Duration,
    /// The tiled tree's area in window coordinates, and what sized it.
    area: Bounds<Pixels>,
    cell: Size<Pixels>,
    scale: f32,
    zoom: f32,
    /// Each block's slot in window coordinates: from, to.
    blocks: Vec<(String, Bounds<Pixels>, Bounds<Pixels>)>,
    dividers: Vec<Divider>,
    frozen: Vec<Entity<TerminalView>>,
    /// Blocks whose terminal isn't there yet (a split's starts with the core's
    /// answer): frozen once it appears, so it never fits while it grows.
    waiting: Vec<String>,
}

/// A container's line between two of its children, placed each frame from
/// the blocks either side, so it moves with them.
#[derive(Debug, PartialEq)]
struct Divider {
    row: bool,
    before: Vec<String>,
    after: Vec<String>,
    across: Vec<String>,
    width: f32,
    color: String,
}

fn ease(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

/// `a` to `b` by `t`, edge by edge on whole device pixels: blocks that share
/// an edge at both ends share it on every frame.
fn lerp(a: Bounds<Pixels>, b: Bounds<Pixels>, t: f32, scale: f32) -> Bounds<Pixels> {
    if t >= 1.0 {
        return b;
    }
    let edge = |a: Pixels, b: Pixels| {
        let v = f32::from(a) + (f32::from(b) - f32::from(a)) * t;
        px((v * scale).round() / scale)
    };
    Bounds::from_corners(point(edge(a.left(), b.left()), edge(a.top(), b.top())), point(edge(a.right(), b.right()), edge(a.bottom(), b.bottom())))
}

/// A new block grows out of the edge it was split from: the far edge of its
/// slot when it follows a sibling, else the near one.
fn grown_from(tree: &Node, id: &str, to: Bounds<Pixels>) -> Bounds<Pixels> {
    let Some(parent) = tree.parent_of(id) else { return to };
    let first = parent.children().iter().find(|c| !c.hidden).is_some_and(|c| c.id == id);
    let row = parent.axis() == Some(Axis::Row);
    let corner = match (row, first) {
        (true, false) => point(to.right(), to.top()),
        (false, false) => point(to.left(), to.bottom()),
        _ => to.origin,
    };
    let size = if row { size(px(0.0), to.size.height) } else { size(to.size.width, px(0.0)) };
    Bounds::new(corner, size)
}

/// Whether `n` draws nothing of its own but its children and their lines, so
/// drawing only its blocks and lines draws it. The tab's root may paint
/// behind them (the slide draws its look), but not inset them.
fn plain(n: &Node, root: bool) -> bool {
    if n.content().is_some() {
        return true;
    }
    let inset = n.border.is_some() || n.focus.as_ref().is_some_and(|f| f.border.is_some()) || n.padding.is_some();
    let paints = n.background.is_some() || n.focus.is_some() || n.radius.is_some() || n.shadow.is_some();
    let own = inset || n.window_drag || !n.floating.is_empty() || n.titlebar.is_some() || n.look != crate::layout::Look::default() || (!root && paints);
    let lines = matches!(n.kind, Kind::Row { .. } | Kind::Column { .. });
    !own && lines && n.children().iter().filter(|c| !c.hidden).all(|c| plain(c, false))
}

fn dividers(n: &Node, zoom: f32, out: &mut Vec<Divider>) {
    let shown: Vec<&Node> = n.children().iter().filter(|c| !c.hidden).collect();
    let ids = |n: &Node| n.visible_blocks().into_iter().map(|b| b.id.clone()).collect::<Vec<_>>();
    if let Some(line) = &n.divider {
        for pair in shown.windows(2) {
            out.push(Divider { row: n.axis() == Some(Axis::Row), before: ids(pair[0]), after: ids(pair[1]), across: ids(n), width: line.width * zoom, color: line.color.clone() });
        }
    }
    for child in shown {
        dividers(child, zoom, out);
    }
}

impl Divider {
    /// Centred between the blocks either side, across all of the container's,
    /// as `separator` centres it in the gap.
    fn rect(&self, at: &HashMap<&str, Bounds<Pixels>>, scale: f32) -> Option<Bounds<Pixels>> {
        let pick = |ids: &[String]| ids.iter().filter_map(|id| at.get(id.as_str()).copied()).collect::<Vec<_>>();
        let (before, after, across) = (pick(&self.before), pick(&self.after), pick(&self.across));
        let max = |v: &[Bounds<Pixels>], f: fn(&Bounds<Pixels>) -> Pixels| v.iter().map(f).reduce(Pixels::max);
        let min = |v: &[Bounds<Pixels>], f: fn(&Bounds<Pixels>) -> Pixels| v.iter().map(f).reduce(Pixels::min);
        let snap = |v: f32| px((v * scale).round() / scale);
        let (end, start) = if self.row { (max(&before, Bounds::right)?, min(&after, Bounds::left)?) } else { (max(&before, Bounds::bottom)?, min(&after, Bounds::top)?) };
        let at = snap((f32::from(end) + f32::from(start) - self.width) / 2.0);
        let line = px(self.width);
        Some(if self.row {
            let (top, bottom) = (min(&across, Bounds::top)?, max(&across, Bounds::bottom)?);
            Bounds::new(point(at, top), size(line, bottom - top))
        } else {
            let (left, right) = (min(&across, Bounds::left)?, max(&across, Bounds::right)?);
            Bounds::new(point(left, at), size(right - left, line))
        })
    }
}

impl Slide {
    fn progress(&self, now: Instant) -> f32 {
        now.saturating_duration_since(self.start).as_secs_f32() / self.duration.as_secs_f32()
    }

    /// Where `block` is published while the slide runs: its target, in window coordinates.
    pub(super) fn target(&self, block: &str) -> Option<Bounds<Pixels>> {
        self.blocks.iter().find(|(id, ..)| id == block).map(|(_, _, to)| *to)
    }

    pub(super) fn shows(&self, tab: &str) -> bool {
        self.tab == tab
    }

    #[cfg(test)]
    pub(super) fn waiting(&self) -> &[String] {
        &self.waiting
    }

    /// Where block `id` slides from.
    #[cfg(test)]
    pub(super) fn from(&self, id: &str) -> Option<Bounds<Pixels>> {
        self.blocks.iter().find(|(b, ..)| b == id).map(|(_, from, _)| *from)
    }
}

impl AppWindow {
    /// Where the tab's blocks are laid out in `area`: as `block_slots` puts
    /// them, each edge on a whole device pixel as GPUI's layout rounds it.
    pub(super) fn laid_out(tab: &Tab, area: Bounds<Pixels>, frame: Option<&PaneFrame>, scale: f32) -> Vec<(String, Bounds<Pixels>)> {
        let focused = tab.focused_pane().and_then(|f| tab.path_to(&f)).unwrap_or_default().into_iter().collect();
        let snap = |v: Pixels| px((f32::from(v) * scale).round() / scale);
        pane_size::block_slots(tab.shown(), area, frame, &focused, settings::ui_scale())
            .into_iter()
            .map(|(id, b)| (id, Bounds::from_corners(point(snap(b.left()), snap(b.top())), point(snap(b.right()), snap(b.bottom())))))
            .collect()
    }

    /// Ends the slide: the next frame lays the tab out as usual and fits each
    /// terminal once.
    pub(super) fn land(&mut self, cx: &mut Context<Self>) {
        let Some(slide) = self.slide.take() else { return };
        // Each block's terminal now, and each it froze: one `terminal.show`
        // swapped out mid-slide is no longer its block's.
        for (id, ..) in &slide.blocks {
            if let Some(term) = self.pane_term(id) {
                term.view.read(cx).set_slide(None);
            }
        }
        for view in slide.frozen {
            view.read(cx).set_slide(None);
            view.read(cx).set_frozen(false);
            view.update(cx, |_, cx| cx.notify());
        }
        cx.notify();
    }

    /// Before command `name`: lands a slide it would disturb, and notes the
    /// blocks it may slide.
    pub(super) fn slide_before(&mut self, name: &str, window: &Window, cx: &mut Context<Self>) -> Option<Before> {
        if LANDS.iter().any(|p| name.starts_with(p)) {
            self.land(cx);
        }
        if !SLIDES.contains(&name) || settings::get().pane_motion == settings::PaneMotion::None || cx.reduce_motion() {
            return None;
        }
        let tab = self.model.active_tab().filter(|t| t.zoomed.is_none())?;
        let (area, frame) = self.tab_area(tab.panes().len())?;
        let mut slots: HashMap<_, _> = Self::laid_out(tab, area, frame.as_ref(), window.scale_factor()).into_iter().collect();
        // A floating pane's slot is where it was drawn, so one docked slides in from there.
        let drawn = self.bounds.borrow();
        slots.extend(tab.shown_floats().filter_map(|f| Some((f.pane.id.clone(), *drawn.get(&f.pane.id)?))));
        Some(Before { tab: tab.id.clone(), tree: tab.shown().clone(), slots })
    }

    /// After the command: slides the shown tab from `before` to its new layout.
    pub(super) fn slide_after(&mut self, before: Before, window: &Window, cx: &mut Context<Self>) {
        let s = settings::get();
        let Some(tab) = self.model.active_tab().filter(|t| t.id == before.tab && t.zoomed.is_none()) else { return };
        let tree = tab.shown();
        if *tree == before.tree || !plain(tree, true) {
            return;
        }
        let Some((area, frame)) = self.tab_area(tab.panes().len()) else { return };
        let zoom = settings::ui_scale();
        let blocks: Vec<_> = Self::laid_out(tab, area, frame.as_ref(), window.scale_factor())
            .into_iter()
            .map(|(id, to)| {
                let from = before.slots.get(&id).copied().unwrap_or_else(|| grown_from(tree, &id, to));
                (id, from, to)
            })
            .collect();
        let moving = blocks.iter().filter(|(_, from, to)| from != to).count();
        if moving == 0 || moving > s.pane_motion_max() {
            return;
        }
        let mut lines = Vec::new();
        dividers(tree, zoom, &mut lines);
        let (mut frozen, mut waiting) = (Vec::new(), Vec::new());
        for (id, ..) in &blocks {
            match self.freeze(tab, id, !before.slots.contains_key(id), window, cx) {
                Some(view) => frozen.push(view),
                None if tree.find(id).is_some_and(|n| n.terminal().is_some()) => waiting.push(id.clone()),
                None => {}
            }
        }
        self.slide = Some(Slide {
            tab: tab.id.clone(),
            tree: tree.clone(),
            start: Instant::now(),
            duration: s.pane_motion_duration(),
            area,
            cell: crate::terminal::cell_size(window),
            scale: window.scale_factor(),
            zoom,
            blocks,
            dividers: lines,
            frozen,
            waiting,
        });
        cx.notify();
    }

    /// Freezes the terminal `id` shows, if it has one yet; a `new` one is
    /// first fitted where it lands, so it is sized once.
    fn freeze(&self, tab: &Tab, id: &str, new: bool, window: &Window, cx: &App) -> Option<Entity<TerminalView>> {
        let view = self.pane_term(id)?.view.clone();
        if new && let Some(area) = self.shown_area(tab, id) {
            view.read(cx).fit(area, window);
        }
        view.read(cx).set_frozen(true);
        Some(view)
    }

    /// Each draw, before the layout: lands a slide that is done or whose tab,
    /// tree, area, cell or scale changed; else asks for the next frame.
    pub(super) fn slide_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(slide) = &self.slide else { return };
        let still = self.model.active_tab().is_some_and(|t| t.id == slide.tab && *t.shown() == slide.tree)
            && slide.progress(Instant::now()) < 1.0
            && !cx.reduce_motion()
            && window.scale_factor() == slide.scale
            && settings::ui_scale() == slide.zoom
            && crate::terminal::cell_size(window) == slide.cell
            && self.model.active_tab().and_then(|t| self.tab_area(t.panes().len())).is_some_and(|(area, _)| area == slide.area);
        if !still {
            return self.land(cx);
        }
        let waiting = std::mem::take(&mut self.slide.as_mut().unwrap().waiting);
        if let Some(tab) = self.model.active_tab() {
            let (mut frozen, mut left) = (Vec::new(), Vec::new());
            for id in waiting {
                match self.freeze(tab, &id, true, window, cx) {
                    Some(view) => frozen.push(view),
                    None => left.push(id),
                }
            }
            let slide = self.slide.as_mut().unwrap();
            slide.frozen.extend(frozen);
            slide.waiting = left;
        }
        let app = cx.entity().downgrade();
        window.on_next_frame(move |_, cx| {
            let _ = app.update(cx, |_, cx| cx.notify());
        });
    }

    /// The shown tab's tiled tree mid-slide: each block at its eased rect,
    /// each line between them, in the root's look.
    pub(super) fn render_slide(&self, slide: &Slide, focused: &HashSet<String>, frame: Option<&PaneFrame>, rendered_dividers: &mut HashSet<(String, usize)>, cx: &mut Context<Self>) -> AnyElement {
        crate::bench::count(&crate::bench::SLIDE);
        let t = ease(slide.progress(Instant::now()));
        let at: HashMap<&str, Bounds<Pixels>> = slide.blocks.iter().map(|(id, from, to)| (id.as_str(), lerp(*from, *to, t, slide.scale))).collect();
        let mut el = div().relative().size_full().overflow_hidden();
        if slide.tree.content().is_none() {
            el = apply_look(el, &slide.tree, focused.contains(&slide.tree.id));
            let (bounds_map, id) = (self.bounds.clone(), slide.tree.id.clone());
            el = el.child(canvas(move |b, _, _| {
                bounds_map.borrow_mut().insert(id, b);
            }, |_, _, _, _| {}).absolute().size_full());
        }
        let place = |b: Bounds<Pixels>| {
            let at = b.origin - slide.area.origin;
            div().absolute().left(at.x).top(at.y).w(b.size.width).h(b.size.height)
        };
        for (id, _, to) in &slide.blocks {
            let (Some(node), Some(b)) = (slide.tree.find(id), at.get(id.as_str())) else { continue };
            if let Some(term) = self.pane_term(id) {
                term.view.read(cx).set_slide(Some((*b, *to)));
            }
            el = el.child(place(*b).flex().child(self.render_node(node, focused, frame, rendered_dividers, cx)));
        }
        for d in &slide.dividers {
            if let Some(b) = d.rect(&at, slide.scale) {
                el = el.child(place(b).bg(c(&d.color)));
            }
        }
        el.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{dividers, ease, grown_from, lerp, plain};
    use crate::layout::Node;
    use gpui::{Bounds, Pixels, point, px, size};
    use std::collections::HashMap;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(w), px(h)))
    }

    #[test]
    fn neighbours_share_their_edge_on_every_frame_and_end_exactly() {
        // a | b, the split moving from 300 to 517.0, at 1x and 2x.
        let (a0, b0) = (rect(0.0, 0.0, 300.0, 400.0), rect(300.0, 0.0, 500.0, 400.0));
        let (a1, b1) = (rect(0.0, 0.0, 517.0, 400.0), rect(517.0, 0.0, 283.0, 400.0));
        for scale in [1.0, 2.0] {
            for i in 0..=100 {
                let t = ease(i as f32 / 100.0);
                let (a, b) = (lerp(a0, a1, t, scale), lerp(b0, b1, t, scale));
                assert_eq!(a.right(), b.left(), "t {t} at {scale}x: no gap, no overlap");
                assert_eq!((a.left(), b.right()), (px(0.0), px(800.0)), "t {t}: the outer edges stay put");
                let device = f32::from(a.right()) * scale;
                assert_eq!(device, device.round(), "t {t}: on a device pixel");
            }
            assert_eq!((lerp(a0, a1, ease(1.0), scale), lerp(b0, b1, ease(1.0), scale)), (a1, b1), "exactly the target at the end");
        }
        assert_eq!((ease(0.0), ease(1.0), ease(2.0)), (0.0, 1.0, 1.0));
    }

    #[test]
    fn a_new_block_grows_out_of_the_edge_it_was_split_from() {
        let tree: Node = serde_json::from_str(r#"{"id":"r","type":"row","children":[{"id":"a","type":"block","content":{"terminal":"a"}},{"id":"c","type":"column","children":[{"id":"b","type":"block","content":{"terminal":"b"}},{"id":"n","type":"block","content":{"terminal":"n"}}]}]}"#).unwrap();
        let to = rect(400.0, 300.0, 400.0, 300.0);
        assert_eq!(grown_from(&tree, "n", to), rect(400.0, 600.0, 400.0, 0.0), "below b: from the bottom edge");
        assert_eq!(grown_from(&tree, "a", rect(0.0, 0.0, 400.0, 600.0)), rect(0.0, 0.0, 0.0, 600.0), "first: from its near edge");
        assert_eq!(grown_from(&tree, "r", to), to, "the root has nowhere to grow from");
    }

    #[test]
    fn only_trees_without_chrome_between_their_blocks_slide() {
        let parse = |s: &str| serde_json::from_str::<Node>(s).unwrap();
        let split = |inner: &str| parse(&format!(r#"{{"id":"r","type":"row","divider":{{"width":1,"color":"border"}},"gap":8,"children":[{{"id":"a","type":"block","content":{{"terminal":"a"}}}},{{"id":"c","type":"column",{inner}"children":[{{"id":"b","type":"block","content":{{"terminal":"b"}}}}]}}]}}"#));
        assert!(plain(&split(""), true), "lines and gaps are drawn by the slide");
        assert!(plain(&parse(r#"{"id":"r","type":"row","background":"surface","children":[]}"#), true), "the root may paint behind");
        for chrome in [r#""border":{"width":1,"color":"x"},"#, r#""padding":4,"#, r#""background":"surface","#, r#""window_drag":true,"#] {
            assert!(!plain(&split(chrome), true), "{chrome}");
        }
        assert!(!plain(&parse(r#"{"id":"r","type":"row","padding":4,"children":[]}"#), true), "nor inset them");
        assert!(!plain(&parse(r#"{"id":"r","type":"stack","children":[]}"#), true), "stacks draw headers");
    }

    #[test]
    fn a_divider_sits_between_the_blocks_either_side_across_the_container() {
        let tree: Node = serde_json::from_str(r#"{"id":"r","type":"row","divider":{"width":1,"color":"border"},"children":[{"id":"c","type":"column","children":[{"id":"a","type":"block","content":{"terminal":"a"}},{"id":"b","type":"block","content":{"terminal":"b"}}]},{"id":"d","type":"block","content":{"terminal":"d"}}]}"#).unwrap();
        let mut lines = Vec::new();
        dividers(&tree, 1.0, &mut lines);
        assert_eq!(lines.len(), 1, "only the row has a divider");
        let at: HashMap<&str, Bounds<Pixels>> = [("a", rect(0.0, 0.0, 400.0, 300.0)), ("b", rect(0.0, 300.0, 400.0, 300.0)), ("d", rect(409.0, 0.0, 391.0, 600.0))].into();
        assert_eq!(lines[0].rect(&at, 2.0), Some(rect(404.0, 0.0, 1.0, 600.0)), "centred in the 9 px gap, the full height");
    }
}
