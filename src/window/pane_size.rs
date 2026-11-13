//! Geometry for panes that aren't drawn: programs starting, and tabs that are hidden.
//! Layout metrics are shared with the renderer; normal resize still handles pixel rounding.

use super::*;
use crate::layout::Header;

pub(super) fn node_border(n: &Node, focused: bool) -> Option<&Line> {
    n.focus.as_ref().filter(|_| focused).and_then(|f| f.border.as_ref()).or(n.border.as_ref())
}

pub(super) fn padding(n: &Node) -> (f32, f32) {
    n.padding.map_or((0.0, 0.0), |p| p.axes())
}

/// A framed pane's border, width and color: its `focus` line while it has
/// the keyboard (settings.json's `focus_border_width` over the layout's
/// width), else its `border`.
pub(super) fn frame_border(frame: &PaneFrame, focused: bool) -> Option<(f32, &str)> {
    let border = frame.border.as_ref().map(|l| (l.width, l.color.as_str()));
    if !focused { return border; }
    let width = settings::get().focus_width();
    frame.focus.as_ref().map(|l| (width.unwrap_or(l.width), l.color.as_str())).or(border)
}

/// How far a framed pane's content sits in from its edge: the wider of its
/// two borders, so moving the focus never resizes a terminal.
pub(super) fn frame_inset(frame: &PaneFrame) -> f32 {
    let width = |focused| frame_border(frame, focused).map_or(0.0, |(width, _)| width);
    width(false).max(width(true))
}

pub(super) fn header_height(header: &Header) -> f32 {
    header.height.unwrap_or(HEADER_HEIGHT)
}

pub(super) fn gap(n: &Node, frame: Option<&PaneFrame>) -> f32 {
    n.gap.or(frame.and_then(|f| f.gap)).unwrap_or(0.0)
}

pub(super) fn separator_width(n: &Node, gap: f32) -> f32 {
    gap + n.divider.as_ref().map_or(0.0, |line| line.width)
}

pub(super) fn weight(n: &Node) -> f32 {
    match n.size {
        Some(Span::Px { .. }) => 0.0,
        Some(Span::Weight { weight }) => weight,
        None => 1.0,
    }
}

fn inset(size: Size<Pixels>, x: f32, y: f32) -> Size<Pixels> {
    gpui::size((size.width - px(x)).max(px(0.0)), (size.height - px(y)).max(px(0.0)))
}

fn content_size(n: &Node, size: Size<Pixels>, focused: bool, zoom: f32) -> Size<Pixels> {
    let border = node_border(n, focused).map_or(0.0, |line| line.width);
    let (y, x) = padding(n);
    inset(size, 2.0 * (x + border) * zoom, 2.0 * (y + border) * zoom)
}

/// Flex shares after fixed siblings, gaps and min/max constraints.
fn shares(children: &[&Node], available: f32, zoom: f32) -> Vec<f32> {
    let mut sizes = vec![0.0; children.len()];
    let mut pending = Vec::new();
    let mut left = available;
    for (i, n) in children.iter().enumerate() {
        match n.size {
            Some(Span::Px { px }) => {
                sizes[i] = (px * zoom).max(n.min.unwrap_or(0.0) * zoom).min(n.max.unwrap_or(f32::INFINITY) * zoom);
                left -= sizes[i];
            }
            _ => pending.push(i),
        }
    }
    let total_weight = pending.iter().map(|&i| weight(children[i])).sum::<f32>().max(f32::EPSILON);
    let initial_free = left.max(0.0);
    while !pending.is_empty() {
        let total = pending.iter().map(|&i| weight(children[i])).sum::<f32>().max(f32::EPSILON);
        // Grow factors stay normalized to the original siblings after an item freezes.
        let free = left.max(0.0).min(initial_free * (total / total_weight));
        let mut violations = Vec::new();
        for &i in &pending {
            let n = children[i];
            let share = free * weight(n) / total;
            sizes[i] = share.min(n.max.unwrap_or(f32::INFINITY) * zoom).max(n.min.unwrap_or(0.0) * zoom);
            violations.push((i, sizes[i] - share));
        }
        let total_violation = violations.iter().map(|(_, violation)| violation).sum::<f32>();
        if total_violation == 0.0 { break; }
        for (i, violation) in violations {
            if (total_violation > 0.0 && violation <= 0.0) || (total_violation < 0.0 && violation >= 0.0) { continue; }
            left -= sizes[i];
            pending.retain(|&j| j != i);
        }
    }
    sizes
}

fn block_bounds(n: &Node, bounds: Bounds<Pixels>, target: &str, frame: Option<&PaneFrame>, focused: &HashSet<String>, zoom: f32) -> Option<Bounds<Pixels>> {
    if n.hidden { return None; }
    let border = node_border(n, focused.contains(&n.id)).map_or(0.0, |line| line.width) * zoom;
    let mut origin = bounds.origin + point(px(border), px(border));
    let size = inset(bounds.size, 2.0 * border, 2.0 * border);
    if n.content().is_some() {
        return (n.id == target).then_some(Bounds::new(origin, size));
    }
    let (y, x) = padding(n);
    origin += point(px(x * zoom), px(y * zoom));
    let size = content_size(n, bounds.size, focused.contains(&n.id), zoom);
    match &n.kind {
        Kind::Row { children } | Kind::Column { children } => {
            let row = n.axis() == Some(Axis::Row);
            let children: Vec<_> = children.iter().filter(|n| !n.hidden).collect();
            let separator = separator_width(n, gap(n, frame)) * zoom;
            let available = f32::from(if row { size.width } else { size.height }) - separator * children.len().saturating_sub(1) as f32;
            for (child, span) in children.iter().zip(shares(&children, available, zoom)) {
                let size = if row { gpui::size(px(span), size.height) } else { gpui::size(size.width, px(span)) };
                if let Some(bounds) = block_bounds(child, Bounds::new(origin, size), target, frame, focused, zoom) { return Some(bounds); }
                if row { origin.x += px(span + separator); } else { origin.y += px(span + separator); }
            }
            None
        }
        Kind::Stack { header, .. } => {
            let height = header.as_ref().map_or(0.0, header_height) * zoom;
            if header.as_ref().is_some_and(|h| !h.bottom()) { origin.y += px(height); }
            block_bounds(n.active_child()?, Bounds::new(origin, inset(size, 0.0, height)), target, frame, focused, zoom)
        }
        _ => None,
    }
}

fn pane_size(n: &Node, size: Size<Pixels>, target: &str, frame: Option<&PaneFrame>, focused: &HashSet<String>, zoom: f32) -> Option<Size<Pixels>> {
    let bounds = block_bounds(n, Bounds::new(Point::default(), size), target, frame, focused, zoom)?;
    let n = n.find(target)?;
    let (y, x) = padding(n);
    let mut size = inset(bounds.size, 2.0 * x * zoom, 2.0 * y * zoom);
    if let Some(frame) = frame.filter(|_| n.is_pane()) {
        let border = frame_inset(frame) * zoom;
        let header = frame.header.as_ref().filter(|h| !h.hover()).map_or(0.0, header_height) * zoom;
        size = inset(size, 2.0 * border, 2.0 * border + header);
    }
    Some(size)
}

/// The tab area, the cell size, and what else sizes a pane's grid: the
/// terminals' padding, the panels' scale (padding, borders and headers
/// follow it), the tab block's own padding and its pane frame.
type Geometry = (Size<Pixels>, Size<Pixels>, (f32, f32), f32, (f32, f32), Option<PaneFrame>, Option<f32>);

/// Whether hidden tabs fit the window: the geometry seen at the last tick, and
/// whether they were sized for it.
#[derive(Default)]
pub(super) enum HiddenFit {
    #[default]
    Unknown,
    Seen(Geometry),
    Done(Geometry),
}

impl AppWindow {
    pub(super) fn initial_size(&self, root: &Node, pane: &str, window: &Window) -> Option<(u16, u16)> {
        let focused = root.path_to(pane)?.into_iter().collect();
        let area = self.displayed_area(root, root.panes().len(), pane, &focused)?;
        Some(crate::terminal::grid_size(area, crate::terminal::cell_size(window)))
    }

    /// The area `pane` gets when `tab` is the one shown (its terminal sizes
    /// its grid in it at its own text size).
    pub(super) fn shown_area(&self, tab: &Tab, pane: &str) -> Option<Size<Pixels>> {
        let focused = tab.focused_pane().and_then(|f| tab.path_to(&f)).unwrap_or_default().into_iter().collect();
        self.displayed_area(tab.shown(), tab.panes().len(), pane, &focused)
    }

    /// `shown`: the tree on screen; `panes`: how many the tab has (its frame needs more than one, or `lone`).
    fn displayed_area(&self, shown: &Node, panes: usize, pane: &str, focused: &HashSet<String>) -> Option<Size<Pixels>> {
        let (context, area) = self.context_area()?;
        let frame = match context.content() {
            Some(Content::Context { pane: Some(frame), .. }) if frame.lone || panes > 1 => Some(frame),
            _ => None,
        };
        let zoom = settings::ui_scale();
        // The absolute bounds canvas already excludes the context border (padding box).
        let (y, x) = padding(context);
        let size = inset(area, 2.0 * x * zoom, 2.0 * y * zoom);
        pane_size(shown, size, pane, frame, focused, zoom)
    }

    /// Use the full split layout even if it has never been drawn unzoomed.
    pub(super) fn unzoomed_neighbor(&self, tab: &Tab, from: &str, dir: &str) -> Option<String> {
        let (context, area) = self.context_area()?;
        let frame = match context.content() {
            Some(Content::Context { pane: Some(frame), .. }) if frame.lone || tab.root.panes().len() > 1 => Some(frame),
            _ => None,
        };
        let zoom = settings::zoom();
        let (y, x) = padding(context);
        let origin = self.bounds.borrow().get(&context.id)?.origin + point(px(x * zoom), px(y * zoom));
        let bounds = Bounds::new(origin, inset(area, 2.0 * x * zoom, 2.0 * y * zoom));
        let focused = tab.root.path_to(from)?.into_iter().collect();
        let geometry = |id: &str| block_bounds(&tab.root, bounds, id, frame, &focused, zoom);
        let candidates = tab.root.visible_blocks().into_iter().filter(|n| n.is_pane() && n.id != from)
            .filter_map(|n| Some((n.id.clone(), geometry(&n.id)?)));
        nearest(geometry(from)?, dir, candidates, &self.pane_history)
    }

    /// The block tabs show in, and the area it was last drawn with.
    fn context_area(&self) -> Option<(&Node, Size<Pixels>)> {
        let context = self.chrome.visible_blocks().into_iter().find(|n| matches!(n.content(), Some(Content::Context { .. })))?;
        let size = self.bounds.borrow().get(&context.id)?.size;
        (size.width > px(0.0) && size.height > px(0.0)).then_some((context, size))
    }

    /// Terminals in hidden tabs keep the size they last had on screen, so a tab shown
    /// after the window changed size draws its old screens, then jumps once the programs
    /// redraw. Once the window settles (the same at two ticks), they are sized as their
    /// panes would be, so the programs redraw out of sight.
    pub(super) fn fit_hidden_tabs(&mut self, window: &Window, cx: &App) {
        let Some((context, area)) = self.context_area() else { return };
        let frame = match context.content() {
            Some(Content::Context { pane, .. }) => pane.clone(),
            _ => None,
        };
        let cell = crate::terminal::cell_size(window);
        let now = (area, cell, settings::get().terminal_padding_px(), settings::ui_scale(), padding(context), frame, settings::get().focus_width());
        match &self.hidden_fit {
            HiddenFit::Done(g) if *g == now => return,
            HiddenFit::Seen(g) if *g == now => self.hidden_fit = HiddenFit::Done(now),
            _ => {
                self.hidden_fit = HiddenFit::Seen(now);
                return;
            }
        }
        let active = self.model.active_tab().map(|t| t.id.clone());
        for tab in self.model.tabs().filter(|t| Some(&t.id) != active.as_ref()) {
            for pane in tab.shown().panes() {
                if let Some(t) = self.terms.get(&pane.id)
                    && let Some(area) = self.shown_area(tab, &pane.id)
                {
                    t.view.read(cx).fit(area, window);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{frame_border, pane_size};
    use crate::layout::{Line, Node, PaneFrame};
    use crate::settings;
    use gpui::px;

    fn frame(border: f32, focus: f32) -> PaneFrame {
        let line = |width: f32, color: &str| Some(Line { width, color: color.into() });
        PaneFrame { border: line(border, "border"), focus: line(focus, "focus"), ..PaneFrame::default() }
    }

    fn with_focus_width(width: Option<f32>, check: impl FnOnce()) {
        let before = settings::get();
        settings::set(settings::Settings { focus_border_width: width, ..(*before).clone() });
        check();
        settings::set((*before).clone());
    }

    #[test]
    fn focus_border_width_replaces_the_layouts_focus_width_within_1_to_3() {
        let f = frame(1.0, 1.0);
        with_focus_width(None, || assert_eq!(frame_border(&f, true), Some((1.0, "focus"))));
        for (asked, drawn) in [(2.0, 2.0), (3.0, 3.0), (0.0, 1.0), (9.0, 3.0)] {
            with_focus_width(Some(asked), || {
                assert_eq!(frame_border(&f, true), Some((drawn, "focus")), "{asked}");
                assert_eq!(frame_border(&f, false), Some((1.0, "border")), "{asked}: other panes keep the border");
            });
        }
        let border_only = PaneFrame { focus: None, ..frame(1.0, 1.0) };
        with_focus_width(Some(3.0), || assert_eq!(frame_border(&border_only, true), Some((1.0, "border")), "no focus line, nothing to widen"));
    }

    #[test]
    fn moving_the_focus_never_resizes_a_framed_pane() {
        let root: Node = serde_json::from_str(r#"{"id":"r","type":"row","children":[{"id":"a","type":"block","content":{"terminal":"a"}},{"id":"b","type":"block","content":{"terminal":"b"}}]}"#).unwrap();
        let area = gpui::size(px(800.0), px(600.0));
        let size = |f: &PaneFrame, focused: &str| pane_size(&root, area, "a", Some(f), &[focused.to_owned()].into(), 1.0).unwrap();
        for (f, width, inset) in [(frame(1.0, 1.0), None, 1.0), (frame(1.0, 1.0), Some(3.0), 3.0), (frame(1.0, 2.0), None, 2.0), (frame(2.0, 1.0), None, 2.0)] {
            with_focus_width(width, || {
                let (on, off) = (size(&f, "a"), size(&f, "b"));
                assert_eq!(on, off, "{width:?}: focused or not, a keeps its size");
                assert_eq!(on.height, px(600.0 - 2.0 * inset), "{width:?}: inset by the wider border");
            });
        }
    }
}
