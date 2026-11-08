//! `pane.arrange`: rebuilds a tab's existing panes into a preset shape with
//! even sizes. Pane blocks move whole (id, terminal or mod, name), so nothing
//! restarts; only the row/column containers around them are new.

use crate::layout::{Axis, Kind, Node, Size};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// A balanced grid, row by row; a short last row's panes widen (tmux's tiled).
    Tiled,
    /// Binary space partition: each pane takes half of what's left, splitting
    /// right, then down, then right… (bspwm's and tuios's dwindle).
    Bsp,
    /// The master pane on the left half, the rest stacked on the right.
    Master,
    /// All side by side.
    Columns,
    /// All stacked.
    Rows,
}

/// The order `next` cycles through.
pub const MODES: [Mode; 5] = [Mode::Tiled, Mode::Bsp, Mode::Master, Mode::Columns, Mode::Rows];

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Tiled => "tiled",
            Mode::Bsp => "bsp",
            Mode::Master => "master",
            Mode::Columns => "columns",
            Mode::Rows => "rows",
        }
    }

    pub fn parse(name: &str) -> Option<Mode> {
        MODES.into_iter().find(|m| m.name() == name)
    }

    /// Whether a pane coming or going rebuilds the tab. `bsp` keeps its tree,
    /// as tuios and bspwm do: a split halves the pane it splits, in the
    /// direction asked, and a closed pane's space goes to its neighbour.
    pub fn rebuilds_on_change(self) -> bool {
        self != Mode::Bsp
    }

    /// The mode after `last` (the first when there's none).
    pub fn next(last: Option<Mode>) -> Mode {
        let i = last.and_then(|m| MODES.iter().position(|x| *x == m)).map_or(0, |i| (i + 1) % MODES.len());
        MODES[i]
    }
}

/// A tab's panes as a person reads them: by their top edge, then their left
/// edge, with the tab's current sizes (hidden panes last, in tree order).
pub fn reading_order(root: &Node) -> Vec<&Node> {
    let mut placed = Vec::new();
    place(root, (0.0, 0.0, 1.0, 1.0), &mut placed);
    // Rounded so float noise in nested shares doesn't reorder equal edges.
    let key = |v: f32| (v * 10_000.0).round() as i64;
    placed.sort_by_key(|(n, (x, y, _, _))| (n.hidden, key(*y), key(*x)));
    placed.into_iter().map(|(n, _)| n).collect()
}

type Rect = (f32, f32, f32, f32);

/// Each pane's rect in the tab, as fractions `[x, y, w, h]`.
#[cfg(test)]
pub fn rects(root: &Node) -> Vec<(String, [f32; 4])> {
    let mut placed = Vec::new();
    place(root, (0.0, 0.0, 1.0, 1.0), &mut placed);
    placed.into_iter().map(|(n, (x, y, w, h))| (n.id.clone(), [x, y, w, h])).collect()
}

fn place<'a>(node: &'a Node, rect: Rect, out: &mut Vec<(&'a Node, Rect)>) {
    if node.is_pane() {
        out.push((node, rect));
        return;
    }
    let children = node.children();
    let share = |n: &Node| if n.hidden { 0.0 } else { match n.size { Some(Size::Weight { weight }) => weight.max(0.0), _ => 1.0 } };
    let total: f32 = children.iter().map(share).sum::<f32>().max(f32::EPSILON);
    let (x, y, w, h) = rect;
    let mut at = 0.0;
    for c in children {
        let part = share(c) / total;
        let r = match node.axis() {
            Some(Axis::Row) => (x + at * w, y, part * w, h),
            Some(Axis::Column) => (x, y + at * h, w, part * h),
            None => rect,
        };
        at += part;
        place(c, r, out);
    }
}

/// Why a tab can't be arranged: it holds blocks other than panes.
pub fn check(root: &Node) -> Result<(), String> {
    let stray = root.walk().into_iter().skip(1).any(|n| !n.is_pane() && n.axis().is_none());
    if stray { Err("this tab holds blocks that aren't panes; pane.arrange leaves it alone".into()) } else { Ok(()) }
}

/// The tab's tree rebuilt in `mode`. `master` (a pane id) leads in master mode;
/// the others keep reading order. The root keeps its id and look, as a row;
/// `fresh` names the new containers.
pub fn arrange(root: &Node, mode: Mode, master: Option<&str>, fresh: impl FnMut() -> String) -> Node {
    let order: Vec<String> = reading_order(root).iter().map(|n| n.id.clone()).collect();
    arrange_in(root, &order, mode, master, fresh)
}

/// `arrange`, its panes in `order` (pane ids) rather than reading order; panes
/// `order` leaves out follow in reading order.
pub fn arrange_in(root: &Node, order: &[String], mode: Mode, master: Option<&str>, mut fresh: impl FnMut() -> String) -> Node {
    let rank = |n: &Node| order.iter().position(|id| *id == n.id).unwrap_or(order.len());
    let mut panes: Vec<Node> = reading_order(root).into_iter().cloned().collect();
    panes.sort_by_key(|p| (p.hidden, rank(p)));
    for p in &mut panes {
        p.size = None;
    }
    let hidden: Vec<Node> = panes.iter().filter(|p| p.hidden).cloned().collect();
    panes.retain(|p| !p.hidden);
    if mode == Mode::Master
        && let Some(i) = master.and_then(|m| panes.iter().position(|p| p.id == m))
    {
        let m = panes.remove(i);
        panes.insert(0, m);
    }
    let shape = match mode {
        Mode::Tiled => tiled(panes, &mut fresh),
        Mode::Bsp => bsp(panes, Axis::Row, &mut fresh),
        Mode::Master => {
            let stack = panes.split_off(panes.len().min(1));
            let stack = line(Axis::Column, stack, &mut fresh);
            line(Axis::Row, panes.into_iter().chain(stack).collect(), &mut fresh)
        }
        Mode::Columns => line(Axis::Row, panes, &mut fresh),
        Mode::Rows => line(Axis::Column, panes, &mut fresh),
    };
    let mut children = match shape {
        Some(n) if n.axis() == Some(Axis::Row) => n.children().to_vec(),
        Some(n) => vec![n],
        None => vec![],
    };
    children.extend(hidden);
    let mut out = root.clone();
    out.kind = Kind::Row { children };
    out
}

/// One pane alone, else a container of them along `axis`, evenly sized.
fn line(axis: Axis, mut nodes: Vec<Node>, fresh: &mut impl FnMut() -> String) -> Option<Node> {
    match nodes.len() {
        0 => None,
        1 => nodes.pop(),
        _ => Some(Node::container(fresh(), axis, nodes)),
    }
}

fn tiled(panes: Vec<Node>, fresh: &mut impl FnMut() -> String) -> Option<Node> {
    let n = panes.len();
    let cols = (1..=n.max(1)).find(|c| c * c >= n).unwrap_or(1);
    let mut rows = Vec::new();
    let mut panes = panes.into_iter().peekable();
    while panes.peek().is_some() {
        let row: Vec<Node> = panes.by_ref().take(cols).collect();
        rows.extend(line(Axis::Row, row, fresh));
    }
    line(Axis::Column, rows, fresh)
}

fn bsp(mut panes: Vec<Node>, axis: Axis, fresh: &mut impl FnMut() -> String) -> Option<Node> {
    if panes.len() <= 1 {
        return panes.pop();
    }
    let first = panes.remove(0);
    let other = if axis == Axis::Row { Axis::Column } else { Axis::Row };
    let rest = bsp(panes, other, fresh);
    line(axis, std::iter::once(first).chain(rest).collect(), fresh)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Content;
    use serde_json::json;

    fn pane(id: &str) -> Node {
        Node::block(id, Content::Terminal { terminal: format!("t-{id}") })
    }

    fn ids(n: usize) -> Vec<String> {
        (1..=n).map(|i| format!("p{i}")).collect()
    }

    /// A tab whose panes are all side by side, p1 first.
    fn tab(n: usize) -> Node {
        Node::container("tab", Axis::Row, ids(n).iter().map(|id| pane(id)).collect())
    }

    /// The tree as a compact string: R[..] row, C[..] column, pane ids.
    fn shape(n: &Node) -> String {
        match n.axis() {
            Some(axis) => {
                let inner: Vec<String> = n.children().iter().map(shape).collect();
                format!("{}[{}]", if axis == Axis::Row { "R" } else { "C" }, inner.join(" "))
            }
            None => n.id.clone(),
        }
    }

    fn arranged(n: usize, mode: Mode, master: Option<&str>) -> Node {
        let mut k = 0;
        arrange(&tab(n), mode, master, || {
            k += 1;
            format!("s{k}")
        })
    }

    #[test]
    fn every_mode_builds_its_shape() {
        let cases: &[(Mode, &[&str])] = &[
            (Mode::Tiled, &[
                "R[p1]",
                "R[p1 p2]",
                "R[C[R[p1 p2] p3]]",
                "R[C[R[p1 p2] R[p3 p4]]]",
                "R[C[R[p1 p2 p3] R[p4 p5]]]",
                "R[C[R[p1 p2 p3] R[p4 p5 p6] p7]]",
            ]),
            (Mode::Bsp, &[
                "R[p1]",
                "R[p1 p2]",
                "R[p1 C[p2 p3]]",
                "R[p1 C[p2 R[p3 p4]]]",
                "R[p1 C[p2 R[p3 C[p4 p5]]]]",
                "R[p1 C[p2 R[p3 C[p4 R[p5 C[p6 p7]]]]]]",
            ]),
            (Mode::Master, &[
                "R[p1]",
                "R[p1 p2]",
                "R[p1 C[p2 p3]]",
                "R[p1 C[p2 p3 p4]]",
                "R[p1 C[p2 p3 p4 p5]]",
                "R[p1 C[p2 p3 p4 p5 p6 p7]]",
            ]),
            (Mode::Columns, &["R[p1]", "R[p1 p2]", "R[p1 p2 p3]", "R[p1 p2 p3 p4]", "R[p1 p2 p3 p4 p5]", "R[p1 p2 p3 p4 p5 p6 p7]"]),
            (Mode::Rows, &["R[p1]", "R[C[p1 p2]]", "R[C[p1 p2 p3]]", "R[C[p1 p2 p3 p4]]", "R[C[p1 p2 p3 p4 p5]]", "R[C[p1 p2 p3 p4 p5 p6 p7]]"]),
        ];
        for (mode, want) in cases {
            for (n, want) in [1, 2, 3, 4, 5, 7].into_iter().zip(want.iter()) {
                let got = arranged(n, *mode, None);
                assert_eq!(shape(&got), *want, "{mode:?} with {n} panes");
                assert_eq!(got.id, "tab");
            }
        }
    }

    #[test]
    fn sizes_are_even_and_terminals_stay_with_their_panes() {
        let mut before = tab(5);
        for (i, c) in before.children_mut().unwrap().iter_mut().enumerate() {
            c.size = Some(Size::Weight { weight: 0.1 + i as f32 });
        }
        for mode in MODES {
            let mut k = 0;
            let after = arrange(&before, mode, None, || {
                k += 1;
                format!("s{k}")
            });
            // No sizes left over: every sibling shares its container equally.
            assert!(after.walk().iter().all(|n| n.size.is_none()), "{mode:?}");
            let terminals = |n: &Node| {
                let mut t: Vec<(String, String)> = n.panes().iter().map(|p| (p.id.clone(), p.terminal().unwrap().to_owned())).collect();
                t.sort();
                t
            };
            assert_eq!(terminals(&after), terminals(&before), "{mode:?}");
            // Fresh container ids: none shared with a pane or each other.
            let mut all: Vec<&str> = after.walk().iter().map(|n| n.id.as_str()).collect();
            all.sort();
            all.dedup();
            assert_eq!(all.len(), after.walk().len(), "{mode:?}");
        }
    }

    #[test]
    fn order_is_reading_order_and_master_mode_leads_with_the_master() {
        // [p1 | (p2 / p3)] with p4 under everything: reading order p1 p2 p3 p4.
        let side = Node::container("a", Axis::Column, vec![pane("p2"), pane("p3")]);
        let top = Node::container("b", Axis::Row, vec![pane("p1"), side]);
        let root = Node::container("tab", Axis::Row, vec![Node::container("c", Axis::Column, vec![top, pane("p4")])]);
        let order: Vec<&str> = reading_order(&root).iter().map(|n| n.id.as_str()).collect();
        assert_eq!(order, ["p1", "p2", "p3", "p4"]);
        // [(p1 / p2) | p3]: p3's top edge comes before p2's.
        let left = Node::container("a", Axis::Column, vec![pane("p1"), pane("p2")]);
        let root = Node::container("tab", Axis::Row, vec![left, pane("p3")]);
        let order: Vec<&str> = reading_order(&root).iter().map(|n| n.id.as_str()).collect();
        assert_eq!(order, ["p1", "p3", "p2"]);
        // Sizes count: [(p1 / p2) | (p3 / p4)] with a short p3 puts p4's top above p2's.
        let left = Node::container("a", Axis::Column, vec![pane("p1"), pane("p2")]);
        let mut short = pane("p3");
        short.size = Some(Size::Weight { weight: 0.1 });
        let right = Node::container("b", Axis::Column, vec![short, pane("p4")]);
        let root = Node::container("tab", Axis::Row, vec![left, right]);
        let order: Vec<&str> = reading_order(&root).iter().map(|n| n.id.as_str()).collect();
        assert_eq!(order, ["p1", "p3", "p4", "p2"]);

        let got = arranged(4, Mode::Master, Some("p3"));
        assert_eq!(shape(&got), "R[p3 C[p1 p2 p4]]");
        // An unknown master: the first pane leads.
        assert_eq!(shape(&arranged(3, Mode::Master, Some("nope"))), "R[p1 C[p2 p3]]");
        // Other modes ignore it.
        assert_eq!(shape(&arranged(3, Mode::Columns, Some("p3"))), "R[p1 p2 p3]");
    }

    #[test]
    fn arranging_twice_is_stable_and_round_trips_through_every_mode() {
        for n in [1, 2, 3, 4, 5, 7] {
            let start = arranged(n, Mode::Tiled, None);
            let mut tree = start.clone();
            for mode in MODES.into_iter().chain([Mode::Tiled]) {
                let mut k = 100;
                tree = arrange(&tree, mode, None, || {
                    k += 1;
                    format!("s{k}")
                });
                let order: Vec<String> = reading_order(&tree).iter().map(|n| n.id.clone()).collect();
                assert_eq!(order, ids(n), "{mode:?} with {n} panes keeps reading order");
            }
            assert_eq!(shape(&tree), shape(&start));
        }
    }

    #[test]
    fn a_given_order_wins_over_reading_order_and_missing_panes_follow() {
        let order = |ids: &[&str]| ids.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let got = arrange_in(&tab(4), &order(&["p3", "p1"]), Mode::Columns, None, || "s".into());
        assert_eq!(shape(&got), "R[p3 p1 p2 p4]");
        let got = arrange_in(&tab(4), &order(&["p1", "p4", "p2", "p3"]), Mode::Bsp, None, || "s".into());
        assert_eq!(shape(&got), "R[p1 C[p4 R[p2 p3]]]");
        // The master still leads in master mode.
        let got = arrange_in(&tab(3), &order(&["p2", "p1", "p3"]), Mode::Master, Some("p3"), || "s".into());
        assert_eq!(shape(&got), "R[p3 C[p2 p1]]");
    }

    #[test]
    fn mod_panes_move_like_terminals_and_hidden_panes_stay_hidden_at_the_end() {
        let viewer = Node::block("m", Content::Mod { name: "markdown".into(), props: json!({ "file": "README.md" }) });
        let mut ghost = pane("g");
        ghost.hidden = true;
        let mut named = pane("p1");
        named.title = Some("build".into());
        let root = Node::container("tab", Axis::Row, vec![named, ghost, viewer, pane("p2")]);
        assert!(check(&root).is_ok());
        let got = arrange(&root, Mode::Rows, None, || "s".into());
        assert_eq!(shape(&got), "R[C[p1 m p2] g]");
        assert!(got.find("g").unwrap().hidden);
        assert_eq!(got.find("p1").unwrap().title.as_deref(), Some("build"));
        assert_eq!(got.find("m").unwrap().content(), root.find("m").unwrap().content());
    }

    #[test]
    fn tabs_with_other_blocks_are_refused() {
        let stray = Node::new("x", Kind::Stack { children: vec![], active: None, header: None });
        assert!(check(&Node::container("tab", Axis::Row, vec![pane("p1"), stray])).is_err());
        assert!(check(&tab(3)).is_ok());
    }

    #[test]
    fn next_cycles_through_every_mode_and_starts_tiled() {
        assert_eq!(Mode::next(None), Mode::Tiled);
        let mut seen = vec![];
        let mut m = None;
        for _ in 0..MODES.len() {
            let n = Mode::next(m);
            seen.push(n);
            m = Some(n);
        }
        assert_eq!(seen, MODES);
        assert_eq!(Mode::next(m), Mode::Tiled);
        for mode in MODES {
            assert_eq!(Mode::parse(mode.name()), Some(mode));
        }
        assert_eq!(Mode::parse("next"), None);
    }
}
