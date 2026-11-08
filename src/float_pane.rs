//! Floating panes: a pane lifted out of its tab's tiled tree to float above
//! it, and the way back (docking). One pane per float; its rect is fractions
//! of the tab area, so it fits every screen that shares the machine's document.
//!
//! Docking splits what it was split with (`DockHint::split`, else `beside`)
//! again, on the same side at the same share, else the tab's tiled pane
//! focused last. An arranged tab puts it beside that pane in reading order and
//! lets its mode lay it out. Neither tuios nor Rex restores more; this gets the
//! common case (float, glance, dock) back as it was. It runs only on float and
//! dock, never while drawing.

use crate::layout::{Axis, Edge, Node, Size};
use serde::{Deserialize, Serialize};

/// `[x, y, w, h]`, fractions of the tab area.
pub type Rect = [f64; 4];

/// Where a float lands when nobody says: centred, 60% × 60%.
pub const DEFAULT_RECT: Rect = [0.2, 0.2, 0.6, 0.6];

/// The smallest share of the tab area a float keeps on either axis. The
/// window also keeps it at least `MIN_COLS` × `MIN_ROWS` cells when drawn.
const MIN_SHARE: f64 = 0.05;
pub const MIN_COLS: f64 = 20.0;
pub const MIN_ROWS: f64 = 4.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FloatPane {
    /// The pane block, as the tiled tree holds one (id, terminal or mod, name).
    pub pane: Node,
    pub rect: Rect,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dock: Option<DockHint>,
}

/// Where a floating pane came from, to dock it back there.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DockHint {
    /// The tiled pane it was split with, its side of that pane, and its share of the two.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beside: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge: Option<Edge>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f32>,
    /// When it was split with a split of several panes (`beside` is the
    /// nearest of them), that split: docking splits it whole again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split: Option<String>,
}

/// A rect kept inside the tab area and at least `MIN_SHARE` of it; one that
/// isn't a number at all becomes `DEFAULT_RECT`.
pub fn clamp(r: Rect) -> Rect {
    if r.iter().any(|v| !v.is_finite()) {
        return DEFAULT_RECT;
    }
    let w = r[2].clamp(MIN_SHARE, 1.0);
    let h = r[3].clamp(MIN_SHARE, 1.0);
    [r[0].clamp(0.0, 1.0 - w), r[1].clamp(0.0, 1.0 - h), w, h]
}

/// Takes `pane` out of `root` (as `Node::remove` does) and records the pane
/// it was split with. None: it isn't a pane of `root`.
pub fn lift(root: &mut Node, pane: &str) -> Option<(Node, DockHint)> {
    let parent = root.parent_of(pane)?;
    let siblings = parent.children();
    let index = siblings.iter().position(|n| n.id == pane)?;
    let before = index > 0;
    // The sibling `Node::remove` gives its share to: the one before it, else the next.
    let neighbour = siblings.get(if before { index - 1 } else { 1 });
    let beside = neighbour.and_then(|n| {
        let panes = n.panes();
        if before { panes.last() } else { panes.first() }.map(|p| p.id.clone())
    });
    let edge = parent.axis().zip(neighbour).map(|(axis, _)| match (axis, before) {
        (Axis::Row, true) => Edge::Right,
        (Axis::Row, false) => Edge::Left,
        (Axis::Column, true) => Edge::Bottom,
        (Axis::Column, false) => Edge::Top,
    });
    let size = match (weight(siblings[index].size), neighbour.and_then(|n| weight(n.size))) {
        (Some(a), Some(b)) if a + b > 0.0 => Some(a / (a + b)),
        _ => None,
    };
    let split = neighbour.filter(|n| !n.is_pane()).map(|n| n.id.clone());
    let node = root.remove(pane)?;
    Some((node, DockHint { beside, edge, size, split }))
}

impl DockHint {
    /// `beside`, if it's still a pane of `root`.
    pub fn anchor<'a>(&'a self, root: &Node) -> Option<&'a str> {
        self.beside.as_deref().filter(|b| root.find(b).is_some_and(Node::is_pane))
    }

    /// Whether it docks before its anchor (on its left or above it).
    pub fn before(&self) -> bool {
        matches!(self.edge, Some(Edge::Left | Edge::Top))
    }
}

/// Puts `pane` back into `root`: beside `hint.split` or `hint.beside` on its
/// side at its share, else beside `fallback` along its axis, else as the
/// root's only pane.
/// `pair`: splits make pairs (`bsp`). `container` names a split if one is needed.
pub fn place(root: &mut Node, pane: Node, hint: &DockHint, fallback: Option<(&str, Axis)>, pair: bool, container: String) {
    let split = hint.split.as_deref().filter(|s| root.find(s).is_some_and(|n| n.axis().is_some()));
    if let Some(beside) = split.or(hint.anchor(root)).map(str::to_owned) {
        return beside_of(root, pane, &beside, hint.edge.unwrap_or(Edge::Right), hint.size, pair, container);
    }
    if let Some((target, axis)) = fallback.filter(|(t, _)| root.find(t).is_some_and(Node::is_pane)) {
        let edge = if axis == Axis::Row { Edge::Right } else { Edge::Bottom };
        return beside_of(root, pane, target, edge, None, pair, container);
    }
    if let Some(children) = root.children_mut() {
        children.push(pane);
    }
}

/// Splits `target` (a pane or a split) with `pane` on `edge`, `pane` taking `share` of the two.
fn beside_of(root: &mut Node, mut pane: Node, target: &str, edge: Edge, share: Option<f32>, pair: bool, container: String) {
    let id = pane.id.clone();
    pane.size = None;
    let axis = if edge.vertical() { Axis::Row } else { Axis::Column };
    let split = if pair { Node::split_pair } else { Node::split };
    if !split(root, target, axis, pane, container) {
        return;
    }
    if matches!(edge, Edge::Left | Edge::Top) {
        root.swap(&id, target);
    }
    let Some(share) = share.filter(|s| (0.0..=1.0).contains(s)) else { return };
    let total = [id.as_str(), target].iter().filter_map(|n| root.find(n).and_then(|n| weight(n.size))).sum::<f32>();
    for (n, part) in [(id.as_str(), share), (target, 1.0 - share)] {
        if let Some(node) = root.find_mut(n) {
            node.size = Some(Size::Weight { weight: total * part });
        }
    }
}

fn weight(size: Option<Size>) -> Option<f32> {
    match size {
        None => Some(1.0),
        Some(Size::Weight { weight }) => Some(weight),
        Some(Size::Px { .. }) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Content;

    fn pane(id: &str) -> Node {
        Node::block(id, Content::Terminal { terminal: format!("t-{id}") })
    }

    /// Each pane's rect, rounded: what the person sees, whatever the container ids.
    pub(crate) fn drawn(root: &Node) -> Vec<(String, [i32; 4])> {
        let mut v: Vec<_> = crate::arrange::rects(root).into_iter().map(|(id, r)| (id, r.map(|x| (x * 1000.0).round() as i32))).collect();
        v.sort();
        v
    }

    #[test]
    fn rects_stay_inside_the_area_and_keep_a_minimum() {
        assert_eq!(clamp([0.9, 0.9, 0.5, 0.5]), [0.5, 0.5, 0.5, 0.5]);
        assert_eq!(clamp([-1.0, -1.0, 2.0, 2.0]), [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(clamp([0.1, 0.1, 0.0, 0.01]), [0.1, 0.1, MIN_SHARE, MIN_SHARE]);
        assert_eq!(clamp([f64::NAN, 0.0, 0.5, 0.5]), DEFAULT_RECT);
    }

    #[test]
    fn a_lifted_pane_docks_back_where_it_was_drawn() {
        let mut inner = Node::container("inner", Axis::Column, vec![pane("b"), pane("c")]);
        inner.size = Some(Size::Weight { weight: 2.0 });
        let trees = [
            Node::container("t", Axis::Row, vec![pane("a"), pane("b"), pane("c")]),
            Node::container("t", Axis::Row, vec![pane("a"), inner]),
        ];
        for tree in trees {
            for id in ["a", "b", "c"] {
                let mut root = tree.clone();
                let (node, hint) = lift(&mut root, id).unwrap();
                assert!(root.find(id).is_none());
                place(&mut root, node, &hint, None, false, "fresh".into());
                assert_eq!(drawn(&root), drawn(&tree), "{id}");
            }
        }
    }

    #[test]
    fn a_gone_neighbour_falls_back_to_the_given_pane_then_to_the_root() {
        let mut root = Node::container("t", Axis::Row, vec![pane("a"), pane("b"), pane("c")]);
        let (node, hint) = lift(&mut root, "b").unwrap();
        assert_eq!((hint.beside.as_deref(), hint.edge), (Some("a"), Some(Edge::Right)));
        let mut root = Node::container("t", Axis::Row, vec![pane("c")]);
        place(&mut root, node.clone(), &hint, Some(("c", Axis::Column)), false, "s".into());
        assert_eq!(root.children()[0].axis(), Some(Axis::Column));
        assert_eq!(root.children()[0].children().iter().map(|n| n.id.as_str()).collect::<Vec<_>>(), ["c", "b"]);
        let mut root = Node::container("t", Axis::Row, vec![]);
        place(&mut root, node, &hint, None, false, "s".into());
        assert_eq!(root.panes().len(), 1);
    }

    #[test]
    fn a_left_side_hint_docks_on_the_left_at_its_share() {
        let mut a = pane("a");
        a.size = Some(Size::Weight { weight: 1.0 });
        let mut b = pane("b");
        b.size = Some(Size::Weight { weight: 3.0 });
        let mut root = Node::container("t", Axis::Row, vec![a, b, pane("c")]);
        let (node, hint) = lift(&mut root, "a").unwrap();
        assert_eq!((hint.beside.as_deref(), hint.edge, hint.size), (Some("b"), Some(Edge::Left), Some(0.25)));
        root.children_mut().unwrap()[0].size = Some(Size::Weight { weight: 2.0 });
        place(&mut root, node, &hint, None, false, "s".into());
        let c = root.children();
        assert_eq!(c.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);
        let w = |n: &Node| weight(n.size).unwrap();
        assert!((w(&c[0]) / (w(&c[0]) + w(&c[1])) - 0.25).abs() < 1e-5);
    }

    use crate::arrange::Mode;
    use crate::model::{Model, Tab};

    /// A tab "t" in space "s": a | (b over c) | d.
    fn model() -> Model {
        let mut m = Model::default();
        m.add_space("s", "work", "/w", Tab::new("t", "", "a", "ta"));
        let p = |id: &str| crate::model::pane(id, &format!("t{id}"));
        let bc = Node::container("bc", Axis::Column, vec![p("b"), p("c")]);
        m.tab_mut("t").unwrap().root = Node::container("t", Axis::Row, vec![p("a"), bc, p("d")]);
        m
    }

    fn tiled(m: &Model) -> &Node {
        &m.tab("t").unwrap().root
    }

    fn order(m: &Model) -> Vec<String> {
        crate::arrange::reading_order(tiled(m)).iter().map(|n| n.id.clone()).collect()
    }

    #[test]
    fn floating_and_docking_keep_the_pane_and_its_place() {
        use crate::pane_icons::{Choice, Kind};
        let mut m = model();
        m.rename_pane("c", "build");
        let icon = Choice { kind: Kind::Letter, value: None };
        m.set_icon("c", Some(icon.clone()));
        let before = drawn(tiled(&m));
        m.float_pane("c", [0.1, 0.1, 0.5, 0.5]).unwrap();
        let t = m.tab("t").unwrap();
        assert!(t.tiled_pane("c").is_none() && t.float("c").is_some(), "it left the tiled tree");
        assert_eq!(t.focused.as_deref(), Some("c"));
        assert_eq!((m.pane_of("tc").as_deref(), m.address_of("c").as_deref(), m.pane_name("c")), (Some("c"), Some("tc"), Some("build")));
        assert!(m.float_pane("c", DEFAULT_RECT).unwrap_err().contains("already floats"));
        m.dock_pane("c", None).unwrap();
        assert_eq!(drawn(tiled(&m)), before, "back where it was");
        assert!(m.tab("t").unwrap().floating.is_empty());
        assert_eq!((m.pane_name("c"), m.icons.get("c")), (Some("build"), Some(&icon)));
        assert!(m.dock_pane("c", None).unwrap_err().contains("doesn't float"));
    }

    /// Float then dock, with the neighbour still there and the tree untouched
    /// meanwhile: every arrange mode gives the same tab back, the master
    /// pane included, and a pane in a nested split.
    #[test]
    fn every_arrange_mode_docks_a_pane_back_where_it_was() {
        let modes = [None, Some(Mode::Bsp), Some(Mode::Tiled), Some(Mode::Master), Some(Mode::Columns), Some(Mode::Rows)];
        for mode in modes {
            for pane in ["a", "b", "c", "d"] {
                let mut m = model();
                m.focus_pane("a");
                if let Some(mode) = mode {
                    m.arrange_tab("t", mode).unwrap();
                }
                let (shape, read) = (drawn(tiled(&m)), order(&m));
                m.float_pane(pane, DEFAULT_RECT).unwrap();
                assert!(m.tab("t").unwrap().tiled_pane(pane).is_none());
                m.dock_pane(pane, None).unwrap();
                assert_eq!(order(&m), read, "{mode:?} {pane}: reading order");
                assert_eq!(drawn(tiled(&m)), shape, "{mode:?} {pane}: the same tab");
            }
        }
    }

    #[test]
    fn a_pane_whose_neighbour_closed_docks_beside_the_tiled_pane_focused_last() {
        let mut m = model();
        m.float_pane("b", DEFAULT_RECT).unwrap();
        assert_eq!(m.tab("t").unwrap().float("b").unwrap().dock.as_ref().unwrap().beside.as_deref(), Some("c"));
        m.remove_pane("c");
        m.focus_pane("d");
        m.focus_pane("b");
        m.dock_pane("b", None).unwrap();
        let parent = tiled(&m).parent_of("b").unwrap();
        assert!(parent.children().iter().any(|n| n.id == "d"), "beside d: {:?}", drawn(tiled(&m)));
        // An explicit target and edge win over the hint; the target must be tiled.
        m.float_pane("b", DEFAULT_RECT).unwrap();
        assert!(m.dock_pane("b", Some(("b", Edge::Left))).is_err());
        m.dock_pane("b", Some(("a", Edge::Left))).unwrap();
        assert_eq!(order(&m)[0], "b");
    }

    #[test]
    fn the_last_tiled_pane_can_float_and_the_tab_closes_with_its_last_pane() {
        let mut m = Model::default();
        m.add_space("s", "", "/w", Tab::new("t", "", "a", "ta"));
        m.split("a", Axis::Row, "b", "tb");
        m.float_pane("a", DEFAULT_RECT).unwrap();
        m.float_pane("b", DEFAULT_RECT).unwrap();
        let t = m.tab("t").unwrap();
        assert_eq!((t.tiled_count(), t.panes().len()), (0, 2));
        // A split of a float lands in the tiled tree, empty or not.
        m.split("b", Axis::Row, "c", "tc");
        assert_eq!(m.tab("t").unwrap().tiled_pane("c").map(|n| n.id.as_str()), Some("c"));
        m.dock_pane("a", None).unwrap();
        assert_eq!(m.tab("t").unwrap().tiled_count(), 2);
        m.remove_pane("a");
        m.remove_pane("c");
        assert_eq!(m.remove_pane("b"), (true, true), "its last pane, floating, closes the tab");
    }

    #[test]
    fn focus_raises_and_closing_a_focused_float_goes_back() {
        let mut m = model();
        m.float_pane("b", DEFAULT_RECT).unwrap();
        m.float_pane("c", DEFAULT_RECT).unwrap();
        let fronts = |m: &Model| m.tab("t").unwrap().floating.iter().map(|f| f.pane.id.clone()).collect::<Vec<_>>();
        assert_eq!(fronts(&m), ["b", "c"]);
        m.focus_pane("b");
        assert_eq!(fronts(&m), ["c", "b"], "focus brings it to the front");
        assert!(m.restack_pane("b", -1));
        assert_eq!(fronts(&m), ["b", "c"]);
        assert!(!m.restack_pane("a", 1), "a tiled pane has no z-order");
        m.focus_pane("a");
        m.focus_pane("c");
        m.remove_pane("c");
        assert_eq!(m.focused_pane().as_deref(), Some("a"), "back to the pane before it");
    }

    #[test]
    fn arranging_leaves_floats_and_a_swap_by_target_trades_layers() {
        let mut m = model();
        m.float_pane("b", [0.5, 0.0, 0.5, 0.5]).unwrap();
        m.arrange_tab("t", Mode::Rows).unwrap();
        assert_eq!(m.tab("t").unwrap().float("b").unwrap().rect, [0.5, 0.0, 0.5, 0.5]);
        m.swap_panes("b", "c").unwrap();
        let t = m.tab("t").unwrap();
        assert!(t.tiled_pane("b").is_some() && t.float("c").is_some(), "c floats where b was, b took c's slot");
        assert_eq!(t.float("c").unwrap().rect, [0.5, 0.0, 0.5, 0.5]);
        assert_eq!(m.pane_of("tb").as_deref(), Some("b"));
    }

    #[test]
    fn a_zoomed_pane_unzooms_when_it_floats_and_a_zoomed_float_draws_alone() {
        let mut m = model();
        m.tab_mut("t").unwrap().zoomed = Some("b".into());
        m.float_pane("b", DEFAULT_RECT).unwrap();
        m.float_pane("c", DEFAULT_RECT).unwrap();
        let t = m.tab("t").unwrap();
        assert_eq!(t.zoomed, None);
        assert_eq!(t.shown_floats().count(), 2);
        m.tab_mut("t").unwrap().zoomed = Some("b".into());
        let t = m.tab("t").unwrap();
        assert_eq!(t.shown_floats().map(|f| f.pane.id.as_str()).collect::<Vec<_>>(), ["b"]);
        assert_eq!(t.shown().id, "t", "the tiled tree stays as it is");
        m.tab_mut("t").unwrap().zoomed = Some("a".into());
        assert_eq!(m.tab("t").unwrap().shown_floats().count(), 2, "a zoomed tiled pane keeps the floats above it");
    }

    /// A walk of `Tab::root` alone misses floating panes: a float's terminal
    /// then has no pane, and the orphan scan adopts it again as a second,
    /// tiled pane. Outside `impl Tab`, walks go through `Tab`'s own.
    #[test]
    fn tiled_only_walks_stay_inside_tab() {
        let forbidden = [".root.panes()", ".root.terminals()", ".root.walk()", ".root.find(", ".root.find_mut(", ".root.path_to("];
        let mut found = Vec::new();
        let mut files = vec![std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/src"))];
        while let Some(path) = files.pop() {
            if path.is_dir() {
                files.extend(std::fs::read_dir(&path).unwrap().map(|e| e.unwrap().path()));
                continue;
            }
            let name = path.to_string_lossy().into_owned();
            if !name.ends_with(".rs") || name.ends_with("_tests.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let code = text.split("\n#[cfg(test)]\nmod tests").next().unwrap();
            let mut in_tab = false;
            for (i, line) in code.lines().enumerate() {
                if name.ends_with("src/model.rs") {
                    in_tab = if line == "impl Tab {" { true } else { in_tab && line != "}" };
                }
                if !in_tab && forbidden.iter().any(|f| line.contains(f)) {
                    found.push(format!("{name}:{}: {}", i + 1, line.trim()));
                }
            }
        }
        assert!(found.is_empty(), "use Tab::panes, terminals, find or path_to:\n{}", found.join("\n"));
    }
}
