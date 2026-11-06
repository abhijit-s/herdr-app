//! Layout clicks live apart from the layout file in layouts/.state/.

use crate::layout::{Align, Background, Content, Edge, Kind, Line, Node, Padding, PaneFrame, Size, ThemeChoice};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

// ---- clicks ------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub nodes: BTreeMap<String, NodeState>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub floats: BTreeMap<String, FloatState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<ThemeChoice>,
    /// The layout file as it was when these clicks were saved: a field the
    /// file changed since (an edit while the app was closed, an update)
    /// drops its click on load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NodeState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<Size>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    /// A stack's shown child.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<Background>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<PaneClick>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub padding: Option<Padding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_padding: Option<[f32; 2]>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PaneClick {
    pub header_height: Option<f32>,
    pub border: Option<Line>,
    pub focus: Option<Line>,
    pub gap: Option<f32>,
}

fn pane(n: &Node) -> Option<&PaneFrame> {
    match n.content() {
        Some(Content::Context { pane, .. }) => pane.as_ref(),
        _ => None,
    }
}

fn pane_mut(n: &mut Node) -> Option<&mut PaneFrame> {
    match &mut n.kind {
        Kind::Block { content: Content::Context { pane, .. } } => pane.as_mut(),
        _ => None,
    }
}

impl PaneClick {
    fn of(n: &Node) -> Option<Self> {
        let p = pane(n)?;
        Some(Self { header_height: p.header.as_ref().and_then(|h| h.height), border: p.border.clone(), focus: p.focus.clone(), gap: p.gap })
    }

    fn apply(&self, n: &mut Node) {
        if let Some(p) = pane_mut(n) {
            if let Some(h) = &mut p.header {
                h.height = self.header_height;
            }
            p.border = self.border.clone();
            p.focus = self.focus.clone();
            p.gap = self.gap;
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FloatState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge: Option<Edge>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<Align>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
}

impl NodeState {
    fn is_empty(&self) -> bool {
        self.size.is_none() && self.hidden.is_none() && self.active.is_none() && self.background.is_none() && self.font_size.is_none() && self.pane.is_none()
            && self.padding.is_none() && self.gap.is_none() && self.terminal_padding.is_none()
    }
}

impl FloatState {
    fn is_empty(&self) -> bool {
        self.edge.is_none() && self.align.is_none() && self.hidden.is_none()
    }
}

fn active(n: &Node) -> Option<&String> {
    match &n.kind {
        crate::layout::Kind::Stack { active, .. } => active.as_ref(),
        _ => None,
    }
}

fn set_active(n: &mut Node, to: Option<String>) {
    if let crate::layout::Kind::Stack { active, .. } = &mut n.kind {
        *active = to;
    }
}

impl State {
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.floats.is_empty() && self.theme.is_none()
    }

    /// Drops the clicks on fields `file` changed since they were saved.
    pub fn forget_since_saved(&mut self, file: &Node) {
        if let Some(then) = self.file.take().and_then(|v| serde_json::from_value::<Node>(v).ok()) {
            self.forget_edited(&then, file);
        }
    }

    /// The clicks: where `now` differs from `file` in the fields clicks change.
    pub fn between(file: &Node, now: &Node) -> State {
        let mut s = State::default();
        for n in now.walk() {
            let Some(f) = file.find(&n.id) else { continue };
            let mut ns = NodeState::default();
            if n.size != f.size {
                ns.size = n.size;
            }
            if n.hidden != f.hidden {
                ns.hidden = Some(n.hidden);
            }
            if active(n) != active(f) {
                ns.active = active(n).cloned();
            }
            if n.background != f.background {
                ns.background = n.background.clone();
            }
            if n.look.font_size != f.look.font_size {
                ns.font_size = n.look.font_size;
            }
            if PaneClick::of(n) != PaneClick::of(f) {
                ns.pane = PaneClick::of(n);
            }
            if n.padding != f.padding {
                ns.padding = n.padding;
            }
            if n.gap != f.gap {
                ns.gap = n.gap;
            }
            if n.look.terminal_padding != f.look.terminal_padding {
                ns.terminal_padding = n.look.terminal_padding;
            }
            if !ns.is_empty() {
                s.nodes.insert(n.id.clone(), ns);
            }
            for fl in &n.floating {
                let Some(ff) = f.floating.iter().find(|x| x.id == fl.id) else { continue };
                let fs = FloatState {
                    edge: (fl.edge != ff.edge).then_some(fl.edge),
                    align: (fl.align != ff.align).then_some(fl.align),
                    hidden: (fl.hidden != ff.hidden).then_some(fl.hidden),
                };
                if !fs.is_empty() {
                    s.floats.insert(fl.id.clone(), fs);
                }
            }
        }
        if now.look.theme != file.look.theme {
            s.theme = now.look.theme.clone();
        }
        s
    }

    /// Puts the clicks onto a tree (ids it doesn't have are left alone).
    pub fn apply(&self, root: &mut Node) {
        for (id, ns) in &self.nodes {
            let Some(n) = root.find_mut(id) else { continue };
            if let Some(sz) = ns.size {
                n.size = Some(sz);
            }
            if let Some(h) = ns.hidden {
                n.hidden = h;
            }
            if let Some(background) = &ns.background {
                n.background = Some(background.clone());
            }
            if let Some(font_size) = ns.font_size {
                n.look.font_size = Some(font_size);
            }
            if let Some(pane) = &ns.pane {
                pane.apply(n);
            }
            if let Some(padding) = ns.padding {
                n.padding = Some(padding);
            }
            if let Some(gap) = ns.gap {
                n.gap = Some(gap);
            }
            if let Some(terminal_padding) = ns.terminal_padding {
                n.look.terminal_padding = Some(terminal_padding);
            }
            if let Some(a) = &ns.active
                && n.children().iter().any(|c| &c.id == a)
            {
                set_active(n, Some(a.clone()));
            }
        }
        for (id, fs) in &self.floats {
            let Some(f) = root.find_float_mut(id) else { continue };
            if let Some(e) = fs.edge {
                f.edge = e;
            }
            if let Some(a) = fs.align {
                f.align = a;
            }
            if let Some(h) = fs.hidden {
                f.hidden = h;
            }
        }
        if let Some(t) = &self.theme {
            root.look.theme = Some(t.clone());
        }
    }

    /// The file changed from `old` to `new`: a field it now sets on purpose
    /// wins over the click on that field.
    pub fn forget_edited(&mut self, old: &Node, new: &Node) {
        for (id, ns) in self.nodes.iter_mut() {
            let (Some(o), Some(n)) = (old.find(id), new.find(id)) else { continue };
            if o.size != n.size {
                ns.size = None;
            }
            if o.hidden != n.hidden {
                ns.hidden = None;
            }
            if active(o) != active(n) {
                ns.active = None;
            }
            if o.background != n.background {
                ns.background = None;
            }
            if o.look.font_size != n.look.font_size {
                ns.font_size = None;
            }
            if PaneClick::of(o) != PaneClick::of(n) {
                ns.pane = None;
            }
            if o.padding != n.padding {
                ns.padding = None;
            }
            if o.gap != n.gap {
                ns.gap = None;
            }
            if o.look.terminal_padding != n.look.terminal_padding {
                ns.terminal_padding = None;
            }
        }
        self.nodes.retain(|_, ns| !ns.is_empty());
        let floats = |root: &Node| root.walk().into_iter().flat_map(|n| n.floating.clone()).collect::<Vec<_>>();
        let (of, nf) = (floats(old), floats(new));
        for (id, fs) in self.floats.iter_mut() {
            let (Some(o), Some(n)) = (of.iter().find(|f| &f.id == id), nf.iter().find(|f| &f.id == id)) else { continue };
            if o.edge != n.edge {
                fs.edge = None;
            }
            if o.align != n.align {
                fs.align = None;
            }
            if o.hidden != n.hidden {
                fs.hidden = None;
            }
        }
        self.floats.retain(|_, fs| !fs.is_empty());
        if old.look.theme != new.look.theme {
            self.theme = None;
        }
    }
}

/// `now` with the click fields put back as `file` has them: what's left
/// differs from `file` only by deliberate changes (a mod docked, `layout.set`).
pub fn without_clicks(file: &Node, now: &Node) -> Node {
    let mut out = now.clone();
    let ids: Vec<String> = out.walk().iter().map(|n| n.id.clone()).collect();
    for id in ids {
        let (Some(f), Some(n)) = (file.find(&id), out.find_mut(&id)) else { continue };
        n.size = f.size;
        n.hidden = f.hidden;
        n.background = f.background.clone();
        n.look.font_size = f.look.font_size;
        n.padding = f.padding;
        n.gap = f.gap;
        n.look.terminal_padding = f.look.terminal_padding;
        if let Some(pane) = PaneClick::of(f) {
            pane.apply(n);
        }
        let a = active(f).cloned();
        set_active(n, a);
        let ff = f.floating.clone();
        for fl in n.floating.iter_mut() {
            if let Some(o) = ff.iter().find(|x| x.id == fl.id) {
                fl.edge = o.edge;
                fl.align = o.align;
                fl.hidden = o.hidden;
            }
        }
    }
    out.look.theme = file.look.theme.clone();
    out
}

// ---- spacing -----------------------------------------------------------

/// The largest spacing `layout.spacing` sets, px at zoom 1.
pub const SPACING_MAX: f32 = 40.0;

/// `layout.spacing`: space around the shown tab's panes, as clicks on the
/// layout. `pane_gap` is the tab block's `pane.gap`; `area_padding` and
/// `area_gap` are the `padding` and `gap` of the row or column holding the
/// tab block; `terminal_padding` is the root's. `reset` names fields to put
/// back as the layout file has them.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spacing {
    pub pane_gap: Option<f32>,
    pub area_padding: Option<Padding>,
    pub area_gap: Option<f32>,
    pub terminal_padding: Option<[f32; 2]>,
    #[serde(default)]
    pub reset: Vec<String>,
}

const SPACING_FIELDS: [&str; 4] = ["pane_gap", "area_padding", "area_gap", "terminal_padding"];

/// The first block showing a tab's panes, and the row or column holding it.
pub fn tab_area(root: &Node) -> Option<(&Node, Option<&Node>)> {
    fn find<'a>(n: &'a Node, parent: Option<&'a Node>) -> Option<(&'a Node, Option<&'a Node>)> {
        if matches!(n.content(), Some(Content::Context { .. })) {
            let area = parent.filter(|p| matches!(p.kind, Kind::Row { .. } | Kind::Column { .. }));
            return Some((n, area));
        }
        n.children().iter().find_map(|c| find(c, Some(n)))
    }
    find(root, None)
}

fn clamp_spacing(v: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, SPACING_MAX) } else { 0.0 }
}

impl Spacing {
    /// Sets these on `now`, the shown tree; a reset takes `file`'s value.
    pub fn apply(&self, now: &mut Node, file: &Node) -> Result<(), String> {
        if let Some(bad) = self.reset.iter().find(|r| !SPACING_FIELDS.contains(&r.as_str())) {
            return Err(format!("reset: {bad:?} isn't one of {}", SPACING_FIELDS.join(", ")));
        }
        let reset = |field: &str| self.reset.iter().any(|r| r == field);
        let (block, area) = tab_area(now).ok_or("this layout shows no tab's panes")?;
        let (block, area) = (block.id.clone(), area.map(|a| a.id.clone()));
        if self.pane_gap.is_some() || reset("pane_gap") {
            let was = file.find(&block).and_then(pane).and_then(|p| p.gap);
            let p = now.find_mut(&block).and_then(pane_mut).ok_or("this layout doesn't frame its panes (no pane on its tab block)")?;
            p.gap = if reset("pane_gap") { was } else { self.pane_gap.map(clamp_spacing) };
        }
        if self.area_padding.is_some() || self.area_gap.is_some() || reset("area_padding") || reset("area_gap") {
            let area = area.ok_or("the tab block isn't in a row or column")?;
            let was = file.find(&area);
            let n = now.find_mut(&area).ok_or("the tab block isn't in a row or column")?;
            if reset("area_padding") {
                n.padding = was.and_then(|w| w.padding);
            } else if let Some(p) = self.area_padding {
                n.padding = Some(match p {
                    Padding::All(v) => Padding::All(clamp_spacing(v)),
                    Padding::Axes([y, x]) => Padding::Axes([clamp_spacing(y), clamp_spacing(x)]),
                });
            }
            if reset("area_gap") {
                n.gap = was.and_then(|w| w.gap);
            } else if let Some(g) = self.area_gap {
                n.gap = Some(clamp_spacing(g));
            }
        }
        if reset("terminal_padding") {
            now.look.terminal_padding = file.look.terminal_padding;
        } else if let Some([x, y]) = self.terminal_padding {
            now.look.terminal_padding = Some([clamp_spacing(x), clamp_spacing(y)]);
        }
        Ok(())
    }
}

// ---- updates -----------------------------------------------------------

/// Whole numbers written as `268.0` (the app's own saves) read as `268`, so
/// a number nobody changed never counts as a change.
pub fn normalize(v: &mut Value) {
    match v {
        Value::Number(n) => {
            if let Some(f) = n.as_f64().filter(|f| f.fract() == 0.0 && f.abs() < 1e15)
                && n.as_i64().is_none()
            {
                *v = Value::from(f as i64);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(normalize),
        Value::Object(m) => m.values_mut().for_each(normalize),
        _ => {}
    }
}

/// A layout as its file holds it: pretty, whole numbers without `.0`.
pub fn text_of(n: &Node) -> String {
    let mut v = serde_json::to_value(n).unwrap_or_default();
    normalize(&mut v);
    serde_json::to_string_pretty(&v).unwrap_or_default() + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node(j: Value) -> Node {
        serde_json::from_value(j).unwrap()
    }

    #[test]
    fn desktop_wallpaper_and_pixel_fit_are_clicks_not_structure_edits() {
        let file = node(json!({
            "id":"root", "type":"column", "font_size":16, "background":"#008080",
            "children":[{"id":"main", "type":"block", "content":{"tab":"active","pane":{
                "header":{"content":{"mod":"win98"},"height":18},
                "border":{"width":3,"color":"#c0c0c0"}, "focus":{"width":3,"color":"#c0c0c0"}
            }}}]
        }));
        let mut now = file.clone();
        now.background = Some(Background::Color("#123456".into()));
        now.look.font_size = Some(20.0);
        let p = pane_mut(now.find_mut("main").unwrap()).unwrap();
        p.header.as_mut().unwrap().height = Some(25.0);
        p.border.as_mut().unwrap().width = 4.0;
        p.focus.as_mut().unwrap().width = 4.0;
        assert_eq!(without_clicks(&file, &now), file);
        let state = State::between(&file, &now);
        let mut shown = file.clone();
        state.apply(&mut shown);
        assert_eq!(shown, now);
        shown.find_mut("main").unwrap().kind = Kind::Block { content: Content::Mod { name: "other".into(), props: Value::Null } };
        assert_ne!(without_clicks(&file, &shown), file);
    }

    #[test]
    fn clicks_are_kept_apart_and_an_edit_wins() {
        let file = node(json!({ "id": "root", "type": "row", "children": [
            { "id": "side", "type": "block", "size": { "px": 240 }, "content": { "mod": "spaces" } },
            { "id": "panel", "type": "stack", "hidden": true, "active": "a", "children": [
                { "id": "a", "type": "block", "content": { "mod": "files" } },
                { "id": "b", "type": "block", "content": { "mod": "changes" } } ] } ] }));
        let mut now = file.clone();
        now.find_mut("side").unwrap().size = Some(Size::Px { px: 300.0 });
        now.find_mut("panel").unwrap().hidden = false;
        assert!(now.select("panel", "b"));
        let s = State::between(&file, &now);
        assert_eq!(s.nodes.len(), 2);
        assert_eq!(without_clicks(&file, &now), file);
        let mut again = file.clone();
        s.apply(&mut again);
        assert_eq!(again, now);
        // The file now sets the sidebar's width itself: that wins.
        let mut edited = file.clone();
        edited.find_mut("side").unwrap().size = Some(Size::Px { px: 200.0 });
        let mut s2 = s.clone();
        s2.forget_edited(&file, &edited);
        assert!(s2.nodes.get("side").is_none());
        assert!(s2.nodes.get("panel").is_some());
    }

    #[test]
    fn an_edit_made_while_closed_beats_an_older_click() {
        let file = node(json!({ "id": "root", "type": "row", "children": [
            { "id": "side", "type": "block", "size": { "px": 240 }, "content": { "mod": "spaces" } },
            { "id": "main", "type": "block", "size": { "px": 500 }, "content": { "tab": "active" } } ] }));
        let mut now = file.clone();
        now.find_mut("side").unwrap().size = Some(Size::Px { px: 300.0 });
        now.find_mut("main").unwrap().size = Some(Size::Px { px: 600.0 });
        let mut s = State::between(&file, &now);
        s.file = serde_json::to_value(&file).ok();
        // Quit; someone sets the sidebar to 200 in the file; start.
        let mut edited = file.clone();
        edited.find_mut("side").unwrap().size = Some(Size::Px { px: 200.0 });
        s.forget_since_saved(&edited);
        let mut shown = edited.clone();
        s.apply(&mut shown);
        assert_eq!(shown.find("side").unwrap().size, Some(Size::Px { px: 200.0 }));
        assert_eq!(shown.find("main").unwrap().size, Some(Size::Px { px: 600.0 }));
    }

    fn deck() -> Node {
        node(json!({ "id": "root", "type": "row", "terminal_padding": [14, 12], "children": [
            { "id": "side", "type": "block", "content": { "mod": "spaces" } },
            { "id": "work-body", "type": "row", "padding": 4, "gap": 8, "children": [
                { "id": "main", "type": "block", "content": { "tab": "active", "pane": { "radius": 10, "gap": 6, "lone": true } } },
                { "id": "panel", "type": "block", "content": { "mod": "files" } } ] } ] }))
    }

    fn spacing(j: Value) -> Spacing {
        serde_json::from_value(j).unwrap()
    }

    fn pane_gap(n: &Node) -> Option<f32> {
        pane(n.find("main").unwrap()).and_then(|p| p.gap)
    }

    #[test]
    fn spacing_is_a_click_the_file_never_takes() {
        let file = deck();
        let mut now = file.clone();
        spacing(json!({ "pane_gap": 9, "area_padding": 3, "area_gap": 12, "terminal_padding": [8, 4] })).apply(&mut now, &file).unwrap();
        let body = now.find("work-body").unwrap();
        assert_eq!((pane_gap(&now), body.padding, body.gap, now.look.terminal_padding), (Some(9.0), Some(Padding::All(3.0)), Some(12.0), Some([8.0, 4.0])));
        // save() writes the file only when something besides clicks changed.
        assert_eq!(without_clicks(&file, &now), file);
        let state = State::between(&file, &now);
        let saved: State = serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
        let mut shown = file.clone();
        saved.apply(&mut shown);
        assert_eq!(shown, now);
    }

    #[test]
    fn spacing_stays_within_0_to_40() {
        let file = deck();
        let mut now = file.clone();
        spacing(json!({ "pane_gap": 99, "area_padding": [-3, 50], "area_gap": -1, "terminal_padding": [41, 2.5] })).apply(&mut now, &file).unwrap();
        let body = now.find("work-body").unwrap();
        assert_eq!((pane_gap(&now), body.padding, body.gap, now.look.terminal_padding), (Some(SPACING_MAX), Some(Padding::Axes([0.0, 40.0])), Some(0.0), Some([40.0, 2.5])));
    }

    #[test]
    fn spacing_reset_shows_the_files_values_again() {
        let file = deck();
        let mut now = file.clone();
        spacing(json!({ "pane_gap": 9, "area_padding": 3, "area_gap": 12, "terminal_padding": [8, 4] })).apply(&mut now, &file).unwrap();
        spacing(json!({ "reset": ["pane_gap", "area_gap"] })).apply(&mut now, &file).unwrap();
        let state = State::between(&file, &now);
        assert_eq!(state.nodes.keys().collect::<Vec<_>>(), ["root", "work-body"]);
        assert_eq!(state.nodes["work-body"].gap, None);
        spacing(json!({ "reset": ["area_padding", "terminal_padding"] })).apply(&mut now, &file).unwrap();
        assert_eq!(now, file);
        assert!(State::between(&file, &now).is_empty());
        // Unset in the file, a reset unsets it again: Herdr's default shows.
        let mut bare = file.clone();
        bare.look.terminal_padding = None;
        let mut shown = bare.clone();
        spacing(json!({ "terminal_padding": [8, 4] })).apply(&mut shown, &bare).unwrap();
        spacing(json!({ "reset": ["terminal_padding"] })).apply(&mut shown, &bare).unwrap();
        assert_eq!(shown.look.terminal_padding, None);
    }

    #[test]
    fn a_file_edit_of_a_spacing_field_beats_its_click() {
        let file = deck();
        let mut now = file.clone();
        spacing(json!({ "pane_gap": 9, "area_padding": 3, "area_gap": 12, "terminal_padding": [8, 4] })).apply(&mut now, &file).unwrap();
        let mut s = State::between(&file, &now);
        s.file = serde_json::to_value(&file).ok();
        // Quit; someone sets work-body's gap and the terminal padding in the file; start.
        let mut edited = file.clone();
        edited.find_mut("work-body").unwrap().gap = Some(2.0);
        edited.look.terminal_padding = Some([1.0, 1.0]);
        s.forget_since_saved(&edited);
        let mut shown = edited.clone();
        s.apply(&mut shown);
        let body = shown.find("work-body").unwrap();
        assert_eq!((pane_gap(&shown), body.padding, body.gap, shown.look.terminal_padding), (Some(9.0), Some(Padding::All(3.0)), Some(2.0), Some([1.0, 1.0])));
    }

    #[test]
    fn spacing_says_why_it_cannot() {
        let file = deck();
        let try_on = |file: &Node, j: Value| spacing(j).apply(&mut file.clone(), file);
        assert!(try_on(&file, json!({ "reset": ["gap"] })).unwrap_err().contains("isn't one of"));
        assert!(serde_json::from_value::<Spacing>(json!({ "pane_gaps": 1 })).is_err());
        let no_tab = node(json!({ "id": "root", "type": "row", "children": [{ "id": "a", "type": "block", "content": { "mod": "x" } }] }));
        assert_eq!(try_on(&no_tab, json!({ "pane_gap": 1 })).unwrap_err(), "this layout shows no tab's panes");
        let unframed = node(json!({ "id": "root", "type": "block", "content": { "tab": "active" } }));
        assert!(try_on(&unframed, json!({ "pane_gap": 1 })).unwrap_err().contains("doesn't frame its panes"));
        assert_eq!(try_on(&unframed, json!({ "area_gap": 1 })).unwrap_err(), "the tab block isn't in a row or column");
        // A tab block alone still takes terminal padding.
        assert!(try_on(&unframed, json!({ "terminal_padding": [4, 4] })).is_ok());
    }

    #[test]
    fn click_files_from_before_pane_gap_still_load() {
        let s: State = serde_json::from_value(json!({ "nodes": { "main": { "pane": { "header_height": 20, "border": null, "focus": null } } } })).unwrap();
        assert_eq!(s.nodes["main"].pane.as_ref().unwrap().gap, None);
    }

    #[test]
    fn shipped_layouts_take_spacing_clicks() {
        for name in ["main", "terminals", "win98"] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("defaults/layouts/{name}.json"));
            let file: Node = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
            let mut now = file.clone();
            spacing(json!({ "pane_gap": 3, "terminal_padding": [5, 5] })).apply(&mut now, &file).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(without_clicks(&file, &now), file, "{name}");
            assert!(!State::between(&file, &now).is_empty(), "{name}");
        }
    }

    #[test]
    fn terminal_padding_clicks_scale_with_zoom() {
        let file = deck();
        let mut now = file.clone();
        spacing(json!({ "terminal_padding": [8, 4] })).apply(&mut now, &file).unwrap();
        for zoom in [1.0, 2.0] {
            let person = crate::settings::Person { zoom, ..crate::settings::Person::default() };
            let s = crate::settings::Settings::compose(&person, &now.look, true);
            assert_eq!(s.terminal_padding_px(), (8.0 * zoom, 4.0 * zoom));
        }
    }

    #[test]
    fn shipped_layouts_name_every_node() {
        fn missing(v: &Value, at: &str, out: &mut Vec<String>) {
            if v.get("id").and_then(Value::as_str).is_none_or(str::is_empty) {
                out.push(at.to_owned());
            }
            for (k, list) in [("children", v.get("children")), ("floating", v.get("floating"))] {
                for (i, c) in list.and_then(Value::as_array).into_iter().flatten().enumerate() {
                    missing(c, &format!("{at}/{k}[{i}]"), out);
                }
            }
        }
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("defaults/layouts");
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let mut out = vec![];
            missing(&v, "root", &mut out);
            assert!(out.is_empty(), "{}: nodes without an id: {out:?}", path.display());
        }
    }

}
