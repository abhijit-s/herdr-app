//! Machine-owned layout documents and this screen's private view.
use crate::{
    layout::{Content, Kind, Node},
    float_pane::FloatPane,
    model::{Agent, Model, Recent, Space, Tab},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Document {
    #[serde(default)]
    pub spaces: Vec<SharedSpace>,
    /// Icons someone chose for this machine's panes and tabs, by id: only
    /// ones still there, so an icon goes with its pane or tab. Read leniently
    /// (`Choice::parse`): an entry this build can't read draws the automatic
    /// icon and is written back as it was (`Model::icons_read`), never the layout's loss.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub icons: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SharedSpace {
    id: String,
    name: String,
    folder: String,
    tabs: Vec<SharedTab>,
    /// Fields this build doesn't read, kept so a newer build's survive this one's write.
    #[serde(flatten)]
    rest: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct SharedTab {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default = "empty_root")]
    root: Node,
    /// Panes floating above `root`, back to front (`Tab::floating`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    floating: Vec<FloatPane>,
    /// Fields this build doesn't read, kept so a newer build's survive this one's write.
    #[serde(flatten)]
    rest: serde_json::Map<String, serde_json::Value>,
}

fn empty_root() -> Node {
    Node::container("", crate::layout::Axis::Row, vec![])
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct View {
    active: Option<String>,
    spaces: BTreeMap<String, Option<String>>,
    tabs: BTreeMap<String, TabView>,
    recent: Vec<Recent>,
    attention: BTreeMap<String, Seen>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct TabView {
    focused: Option<String>,
    before: Vec<String>,
    zoomed: Option<String>,
    settled: Option<u64>,
    #[serde(default)]
    arrange: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Seen {
    word: Option<String>,
    seen: bool,
}

pub fn qualify(id: &str, machine: Option<&str>) -> String {
    machine.map_or_else(|| id.to_owned(), |m| format!("{m}/{id}"))
}

/// The machine a qualified id belongs to: None for this one's.
pub fn machine_of(id: &str) -> Option<&str> {
    id.split_once('/').map(|(m, _)| m)
}

pub fn local<'a>(id: &'a str, machine: Option<&str>) -> &'a str {
    machine.and_then(|m| id.strip_prefix(m).and_then(|s| s.strip_prefix('/'))).unwrap_or(id)
}

fn map_node(node: &mut Node, map: &impl Fn(&str) -> String) {
    node.id = map(&node.id);
    match &mut node.kind {
        Kind::Block { content: Content::Terminal { terminal } } => *terminal = map(terminal),
        Kind::Stack { active: Some(active), .. } => *active = map(active),
        _ => {}
    }
    for float in &mut node.floating {
        float.id = map(&float.id);
        if let Content::Terminal { terminal } = &mut float.content {
            *terminal = map(terminal);
        }
    }
    if let Some(children) = node.children_mut() {
        for child in children {
            map_node(child, map);
        }
    }
}

/// A tab's floating panes with their ids (and the ones each docks beside)
/// mapped, as `map_node` maps the tree; read ones' rects are kept inside the tab.
fn map_floats(floating: &[FloatPane], map: &impl Fn(&str) -> String) -> Vec<FloatPane> {
    floating
        .iter()
        .map(|f| {
            let mut f = f.clone();
            map_node(&mut f.pane, map);
            if let Some(dock) = f.dock.as_mut() {
                for id in [&mut dock.beside, &mut dock.split].into_iter().flatten() {
                    *id = map(id);
                }
            }
            f.rect = crate::float_pane::clamp(f.rect);
            f
        })
        .collect()
}

/// A build without floats keeps a tab's `floating` (unknown to it) but sees
/// no pane for a float's terminal, so its orphan scan adopts the terminal
/// into the tiled tree too. Read back, the float wins: the tiled copy goes.
fn drop_tiled_copies(root: &mut Node, floating: &[FloatPane]) {
    let floated: Vec<&str> = floating.iter().flat_map(|f| f.pane.terminals()).filter_map(Node::terminal).collect();
    let copies: Vec<String> = root.terminals().iter().filter(|n| n.terminal().is_some_and(|a| floated.contains(&a))).map(|n| n.id.clone()).collect();
    for id in copies {
        root.remove(&id);
    }
}

/// Every tab and pane id of a machine's spaces.
fn ids_on(model: &Model, machine: Option<&str>) -> std::collections::HashSet<String> {
    let tabs = model.spaces.iter().filter(|s| s.machine.as_deref() == machine).flat_map(|s| &s.tabs);
    tabs.flat_map(|t| std::iter::once(t.id.clone()).chain(t.panes().into_iter().map(|p| p.id.clone()))).collect()
}

/// A pane's or tab's icon entry in its document: as read while nobody changed
/// it, else this build's choice.
fn icon_entry(model: &Model, id: &str) -> Option<serde_json::Value> {
    model.icons_read.get(id).cloned().or_else(|| model.icons.get(id).map(|c| serde_json::to_value(c).unwrap_or_default()))
}

impl Document {
    pub fn capture(model: &Model, machine: Option<&str>) -> Self {
        let map = |s: &str| local(s, machine).to_owned();
        let ids = ids_on(model, machine);
        Self {
            icons: ids.iter().filter_map(|id| Some((map(id), icon_entry(model, id)?))).collect(),
            spaces: model
                .spaces
                .iter()
                .filter(|s| s.machine.as_deref() == machine)
                .map(|s| SharedSpace {
                    id: map(&s.id),
                    name: s.name.clone(),
                    folder: s.folder.clone(),
                    tabs: s
                        .tabs
                        .iter()
                        .map(|t| {
                            let mut root = t.root.clone();
                            map_node(&mut root, &map);
                            SharedTab { id: map(&t.id), name: t.name.clone(), root, floating: map_floats(&t.floating, &map), rest: t.rest.clone() }
                        })
                        .collect(),
                    rest: s.rest.clone(),
                })
                .collect(),
        }
    }

    pub fn replace(&self, model: &mut Model, machine: Option<&str>) {
        let view = View::capture(model);
        // All of the machine's, not only its spaces' now: ones kept while its spaces were away are stale.
        model.icons.retain(|id, _| machine_of(id) != machine);
        model.icons_read.retain(|id, _| machine_of(id) != machine);
        let at = model.spaces.iter().position(|s| s.machine.as_deref() == machine).unwrap_or(model.spaces.len());
        model.spaces.retain(|s| s.machine.as_deref() != machine);
        let map = |s: &str| qualify(s, machine);
        let spaces = self.spaces.iter().map(|s| Space {
            id: map(&s.id),
            name: s.name.clone(),
            folder: s.folder.clone(),
            machine: machine.map(str::to_owned),
            active: None,
            tabs: s
                .tabs
                .iter()
                .map(|t| {
                    let mut root = t.root.clone();
                    map_node(&mut root, &map);
                    let floating = map_floats(&t.floating, &map);
                    drop_tiled_copies(&mut root, &floating);
                    Tab { id: map(&t.id), name: t.name.clone(), root, floating, focused: None, before: vec![], zoomed: None, settled: None, arrange: None, rest: t.rest.clone() }
                })
                .collect(),
            rest: s.rest.clone(),
        });
        model.spaces.splice(at..at, spaces);
        // Only for panes and tabs the document holds: anything else would linger.
        let here = ids_on(model, machine);
        for (id, v) in &self.icons {
            let id = qualify(id, machine);
            if !here.contains(&id) {
                continue;
            }
            if let Ok(Some(choice)) = crate::pane_icons::Choice::parse(v) {
                model.icons.insert(id.clone(), choice);
            }
            model.icons_read.insert(id, v.clone());
        }
        model.order_spaces();
        view.apply(model);
    }
}

impl View {
    pub fn capture(model: &Model) -> Self {
        Self {
            active: model.active.clone(),
            recent: model.recent.clone(),
            spaces: model.spaces.iter().map(|s| (s.id.clone(), s.active.clone())).collect(),
            tabs: model.tabs().map(|t| (t.id.clone(), TabView { focused: t.focused.clone(), before: t.before.clone(), zoomed: t.zoomed.clone(), settled: t.settled, arrange: t.arrange.map(|m| m.name().to_owned()) })).collect(),
            attention: model.agents.iter().map(|(a, s)| (a.clone(), Seen { word: s.word.clone(), seen: s.seen })).collect(),
        }
    }

    /// Restore saved choices when a machine's first document arrives, without
    /// resetting choices already made on other machines during startup.
    pub fn apply_machine(&self, model: &mut Model, machine: Option<&str>) {
        let mut merged = Self::capture(model);
        for space in model.spaces.iter().filter(|s| s.machine.as_deref() == machine) {
            if let Some(active) = self.spaces.get(&space.id) {
                merged.spaces.insert(space.id.clone(), active.clone());
            }
            for tab in &space.tabs {
                if let Some(view) = self.tabs.get(&tab.id) {
                    merged.tabs.insert(tab.id.clone(), view.clone());
                }
            }
            if self.active.as_deref() == Some(&space.id) {
                merged.active = self.active.clone();
            }
        }
        for (address, seen) in &self.attention {
            if address.split_once('/').map(|(m, _)| m) == machine {
                merged.attention.insert(address.clone(), seen.clone());
            }
        }
        if machine.is_none() {
            merged.recent = self.recent.clone();
        }
        merged.apply(model);
        for (address, seen) in &self.attention {
            if address.split_once('/').map(|(m, _)| m) == machine
                && let Some(agent) = model.agents.get_mut(address)
                && agent.word == seen.word
            {
                agent.seen = seen.seen;
            }
        }
    }

    pub fn preserve_unloaded(&mut self, saved: &Self, loaded: impl Fn(Option<&str>) -> bool) {
        let unloaded = |id: &str| !loaded(id.split_once('/').map(|(m, _)| m));
        self.spaces.extend(saved.spaces.iter().filter(|(id, _)| unloaded(id)).map(|(id, v)| (id.clone(), v.clone())));
        self.tabs.extend(saved.tabs.iter().filter(|(id, _)| unloaded(id)).map(|(id, v)| (id.clone(), v.clone())));
        self.attention.extend(saved.attention.iter().filter(|(id, _)| unloaded(id)).map(|(id, v)| (id.clone(), v.clone())));
    }

    pub fn apply(&self, model: &mut Model) {
        for s in &mut model.spaces {
            s.active = self.spaces.get(&s.id).cloned().flatten().filter(|id| s.tab(id).is_some()).or_else(|| s.tabs.first().map(|t| t.id.clone()));
            for t in &mut s.tabs {
                if let Some(v) = self.tabs.get(&t.id) {
                    t.focused = v.focused.clone().filter(|id| t.find(id).is_some());
                    t.zoomed = v.zoomed.clone().filter(|id| t.find(id).is_some());
                    t.before = v.before.iter().filter(|id| t.find(id).is_some()).cloned().collect();
                    t.settled = v.settled;
                    t.arrange = v.arrange.as_deref().and_then(crate::arrange::Mode::parse);
                }
                if t.focused.is_none() {
                    t.focused = t.panes().first().map(|n| n.id.clone());
                }
            }
        }
        model.active = self.active.clone().filter(|id| model.space(id).is_some()).or_else(|| model.spaces.first().map(|s| s.id.clone()));
        model.recent = self.recent.clone();
        for (a, s) in &self.attention {
            model.agents.entry(a.clone()).or_insert_with(|| Agent { word: s.word.clone(), seen: s.seen, ..Default::default() });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Axis;
    fn model() -> Model {
        let mut m = Model::default();
        m.add_space("s", "work", "/work", Tab::new("t", "task", "p", "t1"));
        m
    }
    #[test]
    fn focus_zoom_and_settle_are_private() {
        let mut m = model();
        let before = Document::capture(&m, None);
        m.tab_mut("t").unwrap().zoomed = Some("p".into());
        m.settle("t", 12);
        assert_eq!(before, Document::capture(&m, None));
        let view = View::capture(&m);
        let mut other = Model::default();
        before.replace(&mut other, None);
        assert_eq!(other.tab("t").unwrap().zoomed, None);
        view.apply(&mut other);
        assert_eq!(other.tab("t").unwrap().zoomed.as_deref(), Some("p"));
        assert_eq!(other.tab("t").unwrap().settled, Some(12));
    }

    /// Two screens of one machine: one moves a tab to another space. The
    /// other reads it there with its own focus and zoom on it, a newer build's
    /// fields on it kept, and the space it emptied gone.
    #[test]
    fn a_tab_moved_on_one_screen_keeps_the_other_screens_view_of_it() {
        let mut here = model();
        here.split("p", Axis::Row, "q", "t2");
        here.tab_mut("t").unwrap().rest.insert("newer".into(), serde_json::json!({"x": 1}));
        here.add_space("s2", "", "/b", Tab::new("u", "", "r", "t3"));
        let mut there = Model::default();
        Document::capture(&here, None).replace(&mut there, None);
        there.focus_pane("q");
        there.tab_mut("t").unwrap().zoomed = Some("q".into());
        there.select_tab("t");

        here.move_tab_to_space("t", "s2", Some(1)).unwrap();
        let doc = Document::capture(&here, None);
        doc.replace(&mut there, None);
        assert_eq!(there.spaces.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["s2"]);
        assert_eq!(there.space("s2").unwrap().tabs.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["t", "u"]);
        let t = there.tab("t").unwrap();
        assert_eq!((t.focused.as_deref(), t.zoomed.as_deref()), (Some("q"), Some("q")), "this screen's view goes with the tab");
        assert_eq!(t.rest.get("newer"), Some(&serde_json::json!({"x": 1})));
        assert_eq!(there.active.as_deref(), Some("s2"), "its shown space closed: the first one left");
        assert_eq!(Document::capture(&there, None), doc, "read alone, it writes the same document back");
    }

    #[test]
    fn a_tabs_arrange_mode_is_this_screens_and_survives_a_restart() {
        let mut m = model();
        m.split("p", Axis::Row, "q", "t2");
        m.arrange_tab("t", crate::arrange::Mode::Rows).unwrap();
        let text = serde_json::to_string(&View::capture(&m)).unwrap();
        let mut other = Model::default();
        Document::capture(&m, None).replace(&mut other, None);
        assert_eq!(other.tab("t").unwrap().arrange, None, "not in the shared document");
        serde_json::from_str::<View>(&text).unwrap().apply(&mut other);
        assert_eq!(other.tab("t").unwrap().arrange, Some(crate::arrange::Mode::Rows));
        // Views saved before the field existed still load.
        let old = text.replace(",\"arrange\":\"rows\"", "");
        assert!(!old.contains("arrange"));
        assert!(serde_json::from_str::<View>(&old).is_ok());
    }
    #[test]
    fn remote_roundtrip_qualifies_only_identity_not_mod_props() {
        let mut m = model();
        m.split_with("p", Axis::Row, Node::block("mod", Content::Mod { name: "test".into(), props: serde_json::json!({"terminal":"t1"}) }));
        let doc = Document::capture(&m, None);
        doc.replace(&mut m, Some("mac"));
        assert_eq!(m.address_of("mac/p").as_deref(), Some("mac/t1"));
        assert_eq!(doc, Document::capture(&m, Some("mac")));
        assert!(m.space("s").is_some());
        assert!(m.space("mac/s").is_some());
    }
    #[test]
    fn replacement_preserves_valid_view_and_falls_back_when_deleted() {
        let mut m = model();
        m.split("p", Axis::Row, "p2", "t2");
        let doc = Document::capture(&m, None);
        m.focus_pane("p");
        doc.replace(&mut m, None);
        assert_eq!(m.focused_pane().as_deref(), Some("p"));
        let mut other = model();
        other.remove_pane("p");
        Document::capture(&other, None).replace(&mut m, None);
        assert!(m.active_space().is_none());
    }
    #[test]
    fn late_remote_load_restores_only_that_machines_view() {
        let mut m = model();
        Document::capture(&m, None).replace(&mut m, Some("mac"));
        m.tab_mut("mac/t").unwrap().zoomed = Some("mac/p".into());
        m.active = Some("mac/s".into());
        let saved = View::capture(&m);
        let doc = Document::capture(&m, Some("mac"));
        m.spaces.retain(|s| s.machine.is_none());
        m.active = Some("s".into());
        m.tab_mut("t").unwrap().settled = Some(42);
        doc.replace(&mut m, Some("mac"));
        saved.apply_machine(&mut m, Some("mac"));
        assert_eq!(m.tab("t").unwrap().settled, Some(42));
        assert_eq!(m.tab("mac/t").unwrap().zoomed.as_deref(), Some("mac/p"));
        assert_eq!(m.active.as_deref(), Some("mac/s"));
    }
    #[test]
    fn seen_attention_survives_a_screen_restart_but_not_a_new_word() {
        let mut m = model();
        m.set_word("t1", Some("done".into()), None, false);
        m.agents.get_mut("t1").unwrap().seen = false;
        let saved = View::capture(&m);
        m.agents.clear();
        saved.apply_machine(&mut m, None);
        assert!(!m.agents["t1"].seen);
        m.set_word("t1", Some("working".into()), None, false);
        saved.apply_machine(&mut m, None);
        assert!(m.agents["t1"].seen);
    }
    /// Chosen icons live in the machine's document beside its spaces: a restart
    /// (a new model from the document) keeps them, by id, on that machine only.
    #[test]
    fn chosen_icons_are_kept_with_their_machines_document() {
        use crate::pane_icons::{Choice, Kind};
        let emoji = Choice { kind: Kind::Emoji, value: Some("🌾".into()) };
        let letter = Choice { kind: Kind::Letter, value: None };
        let mut m = model();
        assert!(m.set_icon("p", Some(emoji.clone())));
        assert!(m.set_icon("t", Some(letter.clone())));
        assert!(!m.set_icon("nope", Some(letter.clone())), "no such pane or tab");
        let doc = Document::capture(&m, None);
        let text = serde_json::to_string(&doc).unwrap();
        assert!(text.contains(r#""icons":{"p":{"kind":"emoji","value":"🌾"},"t":{"kind":"letter"}}"#), "{text}");
        let mut restarted = Model::default();
        serde_json::from_str::<Document>(&text).unwrap().replace(&mut restarted, None);
        assert_eq!(restarted.icons, m.icons, "a restart keeps them");
        // Another machine's copy is qualified, and its own document holds only its own.
        doc.replace(&mut restarted, Some("mac"));
        assert_eq!(restarted.icons.get("mac/p"), Some(&emoji));
        assert_eq!(Document::capture(&restarted, Some("mac")).icons, doc.icons);
        assert_eq!(Document::capture(&restarted, None).icons, doc.icons, "the local ones stay unqualified");
        // Clearing one: gone from the document.
        assert!(m.set_icon("p", None));
        assert_eq!(Document::capture(&m, None).icons.keys().collect::<Vec<_>>(), ["t"]);
        // Documents from before icons, and entries this build can't read, load.
        let old: Document = serde_json::from_value(serde_json::json!({"spaces":[{"id":"s","tabs":[{"id":"t"}]}]})).unwrap();
        assert!(old.icons.is_empty());
        assert!(!serde_json::to_string(&Document::capture(&model(), None)).unwrap().contains("icons"), "none chosen: no field");
        let odd: Document = serde_json::from_value(serde_json::json!({"spaces":[{"id":"s","tabs":[{"id":"t"}]}], "icons": {"t": {"kind": "letter", "value": "x"}, "p": {"kind": "sticker"}, "gone": {"kind": "letter"}}})).unwrap();
        let mut m2 = Model::default();
        odd.replace(&mut m2, None);
        assert_eq!(m2.icons.keys().collect::<Vec<_>>(), ["t"], "the unreadable entry and one for no pane or tab are dropped, the rest kept");
        assert_eq!(m2.icons["t"], Choice { kind: Kind::Letter, value: None }, "kept as it would be written");
    }

    /// An icon goes with its pane or tab: the next document written for that
    /// machine leaves it out. A machine whose document isn't loaded keeps its own.
    #[test]
    fn a_closed_pane_or_tabs_icon_goes_with_it() {
        use crate::pane_icons::{Choice, Kind};
        let letter = Choice { kind: Kind::Letter, value: None };
        let mut m = model();
        m.split("p", Axis::Row, "q", "t2");
        m.add_tab("s", Tab::new("u", "", "r", "t3"));
        for id in ["p", "q", "t", "u"] {
            assert!(m.set_icon(id, Some(letter.clone())));
        }
        Document::capture(&m, None).replace(&mut m, Some("mac"));
        assert!(m.set_icon("mac/q", Some(letter.clone())));
        m.remove_pane("q");
        m.remove_tab("u");
        assert_eq!(Document::capture(&m, None).icons.keys().collect::<Vec<_>>(), ["p", "t"]);
        assert_eq!(Document::capture(&m, Some("mac")).icons.keys().collect::<Vec<_>>(), ["p", "q", "t", "u"], "mac's are its own");
    }

    /// A machine's icons are its own: a document replaces all of them, even
    /// ones kept while its spaces were away (it left the machine list), so an
    /// icon cleared elsewhere meanwhile isn't written back when it returns.
    #[test]
    fn a_machine_back_in_the_list_takes_its_icons_from_its_document() {
        use crate::pane_icons::{Choice, Kind};
        let letter = Choice { kind: Kind::Letter, value: None };
        let mut m = model();
        assert!(m.set_icon("p", Some(letter.clone())));
        let chosen = Document::capture(&m, None);
        assert!(m.set_icon("p", None));
        let cleared = Document::capture(&m, None);
        let mut screen = model();
        assert!(screen.set_icon("p", Some(letter.clone())));
        chosen.replace(&mut screen, Some("mac"));
        assert_eq!(screen.icons.get("mac/p"), Some(&letter));
        // Its spaces left; the document read on its return has no icon.
        screen.spaces.retain(|s| s.machine.is_none());
        cleared.replace(&mut screen, Some("mac"));
        assert_eq!(screen.icons.get("mac/p"), None, "the stale icon is gone");
        assert_eq!(Document::capture(&screen, Some("mac")), cleared, "nothing to write back");
        assert_eq!(screen.icons.get("p"), Some(&letter), "the local one stays");
        // Leaving the list drops its icons with its spaces; mac2, a name mac
        // prefixes, keeps its own through both.
        chosen.replace(&mut screen, Some("mac2"));
        chosen.replace(&mut screen, Some("mac"));
        cleared.replace(&mut screen, Some("mac"));
        assert_eq!(screen.icons.get("mac2/p"), Some(&letter), "replacing mac leaves mac2's");
        chosen.replace(&mut screen, Some("mac"));
        screen.retain_machines(|m| m != "mac");
        assert!(screen.space("mac/s").is_none());
        assert!(screen.space("mac2/s").is_some());
        assert_eq!(screen.icons.keys().collect::<Vec<_>>(), ["mac2/p", "p"]);
    }

    /// Reading a document writes nothing back by itself: entries this build
    /// can't read (a newer build's kind) or would write differently are kept
    /// as read, until someone chooses that pane's or tab's icon again.
    #[test]
    fn icons_this_build_cant_read_survive_a_read() {
        use crate::pane_icons::{Choice, Kind};
        let read: Document = serde_json::from_value(serde_json::json!({"spaces":[{"id":"s","tabs":[{"id":"t","root":
            {"id":"t","type":"row","children":[{"id":"p","type":"block","content":{"terminal":"t1"}},{"id":"q","type":"block","content":{"terminal":"t2"}}]}}]}],
            "icons": {"t": {"kind": "letter", "value": "x"}, "p": {"kind": "sticker", "value": "cat.png"}, "q": {"kind": "emoji", "value": "🌾", "tone": 2}}})).unwrap();
        for machine in [None, Some("mac")] {
            let mut m = Model::default();
            read.replace(&mut m, machine);
            assert_eq!(Document::capture(&m, machine), read, "{machine:?}: read alone, nothing to write");
            let id = |s: &str| qualify(s, machine);
            assert_eq!(m.icons.get(&id("t")), Some(&Choice { kind: Kind::Letter, value: None }), "drawn as this build reads it");
            assert_eq!(m.icons.get(&id("p")), None, "an unknown kind draws the automatic icon");
            // Choosing again writes this build's form; clearing removes even an unknown one.
            assert!(m.set_icon(&id("t"), Some(Choice { kind: Kind::Letter, value: None })));
            assert!(m.set_icon(&id("p"), None));
            let icons = Document::capture(&m, machine).icons;
            assert_eq!(serde_json::to_value(&icons).unwrap(), serde_json::json!({"t": {"kind": "letter"}, "q": {"kind": "emoji", "value": "🌾", "tone": 2}}));
        }
    }

    /// A newer build may add fields to a space or tab (#718's `floating`): this
    /// build writes them back as they were, so a mixed-version machine keeps them.
    #[test]
    fn unknown_space_and_tab_fields_survive_a_read_and_write() {
        let text = serde_json::json!({"spaces":[{"id":"s","name":"work","folder":"/w","color":"teal","tabs":[{"id":"t","name":"","root":
            {"id":"t","type":"row","children":[{"id":"p","type":"block","content":{"terminal":"t1"}}]},
            "floating":[{"pane":{"id":"q","type":"block","content":{"terminal":"t2"}},"rect":[0.2,0.2,0.6,0.6]}]}]}]});
        let read: Document = serde_json::from_value(text.clone()).unwrap();
        for machine in [None, Some("mac")] {
            let mut m = Model::default();
            read.replace(&mut m, machine);
            let id = |s: &str| qualify(s, machine);
            m.rename_pane(&id("p"), "build");
            m.split(&id("p"), Axis::Row, &id("p2"), &id("t3"));
            let written = serde_json::to_value(Document::capture(&m, machine)).unwrap();
            assert_eq!(written["spaces"][0]["color"], "teal", "{machine:?}");
            assert_eq!(written["spaces"][0]["tabs"][0]["floating"], text["spaces"][0]["tabs"][0]["floating"], "{machine:?}");
        }
        // Read and written alone: the same document.
        let mut m = Model::default();
        read.replace(&mut m, None);
        assert_eq!(serde_json::to_value(Document::capture(&m, None)).unwrap(), text);
        assert!(!serde_json::to_string(&Document::capture(&model(), None)).unwrap().contains("rest"), "nothing extra: no field");
    }

    /// Floats are structure: in the machine's document with their ids (and
    /// the pane each docks beside) qualified like the tree's, their order and
    /// rects shared; focus and zoom on a float stay this screen's. A document
    /// from before floats has none.
    #[test]
    fn floats_go_in_the_machines_document_and_focus_stays_on_the_screen() {
        use crate::pane_icons::{Choice, Kind};
        let mut m = model();
        m.split("p", Axis::Row, "q", "t2");
        m.split("q", Axis::Row, "r", "t3");
        m.float_pane("q", [0.1, 0.2, 0.5, 0.5]).unwrap();
        m.float_pane("r", [0.4, 0.4, 0.3, 0.3]).unwrap();
        assert!(m.set_icon("q", Some(Choice { kind: Kind::Letter, value: None })));
        let doc = Document::capture(&m, None);
        let text = serde_json::to_value(&doc).unwrap();
        let floating = &text["spaces"][0]["tabs"][0]["floating"];
        assert_eq!(floating[0]["pane"]["id"], "q");
        assert_eq!(floating[0]["rect"], serde_json::json!([0.1, 0.2, 0.5, 0.5]));
        assert_eq!(floating[0]["dock"]["beside"], "p");
        assert_eq!(text["icons"]["q"]["kind"], "letter", "a float keeps its icon");
        m.tab_mut("t").unwrap().zoomed = Some("r".into());
        let view = View::capture(&m);
        let mut other = Model::default();
        doc.replace(&mut other, Some("mac"));
        let t = other.tab("mac/t").unwrap();
        assert_eq!(t.floating.iter().map(|f| f.pane.id.as_str()).collect::<Vec<_>>(), ["mac/q", "mac/r"]);
        assert_eq!(t.float("mac/q").unwrap().dock.as_ref().unwrap().beside.as_deref(), Some("mac/p"));
        assert_eq!(other.address_of("mac/r").as_deref(), Some("mac/t3"));
        assert_eq!(other.pane_of("mac/t2").as_deref(), Some("mac/q"));
        assert_eq!(t.zoomed, None, "zoom is this screen's");
        assert_eq!(Document::capture(&other, Some("mac")), doc, "written back as read");
        // This screen's view, restarted: focus and zoom on a float come back,
        // and ones on a float gone since are dropped.
        let mut restarted = Model::default();
        doc.replace(&mut restarted, None);
        view.apply(&mut restarted);
        assert_eq!((restarted.tab("t").unwrap().zoomed.as_deref(), restarted.focused_pane().as_deref()), (Some("r"), Some("r")));
        let mut gone = Model::default();
        doc.replace(&mut gone, None);
        gone.remove_pane("r");
        Document::capture(&gone, None).replace(&mut restarted, None);
        view.apply(&mut restarted);
        assert_eq!(restarted.tab("t").unwrap().zoomed, None);
        assert_eq!(restarted.focused_pane().as_deref(), Some("p"), "the first pane, as for a tiled pane gone since");
        // A rect from a bigger screen (or a bad hand edit) is kept inside the tab.
        let mut odd = text.clone();
        odd["spaces"][0]["tabs"][0]["floating"][0]["rect"] = serde_json::json!([0.9, 0.9, 0.5, 0.5]);
        serde_json::from_value::<Document>(odd).unwrap().replace(&mut restarted, None);
        assert_eq!(restarted.tab("t").unwrap().float("q").unwrap().rect, [0.5, 0.5, 0.5, 0.5]);
        // Before floats: no field read, none written.
        assert!(!serde_json::to_string(&Document::capture(&model(), None)).unwrap().contains("floating"));
    }

    /// An older build kept the tab's `floating` and adopted the float's
    /// terminal as a tiled pane too (a new id): read back, it floats once.
    #[test]
    fn a_floats_terminal_adopted_as_a_tiled_pane_by_an_older_build_floats_once() {
        let doc: Document = serde_json::from_value(serde_json::json!({"spaces":[{"id":"s","tabs":[{"id":"t","root":
            {"id":"t","type":"row","children":[{"id":"p","type":"block","content":{"terminal":"t1"}},{"id":"adopted","type":"block","content":{"terminal":"t2"}}]},
            "floating":[{"pane":{"id":"q","type":"block","content":{"terminal":"t2"}},"rect":[0.2,0.2,0.6,0.6]}]}]}]})).unwrap();
        for machine in [None, Some("mac")] {
            let id = |s: &str| qualify(s, machine);
            let mut m = Model::default();
            doc.replace(&mut m, machine);
            let t = m.tab(&id("t")).unwrap();
            assert_eq!(t.panes().iter().map(|n| n.id.clone()).collect::<Vec<_>>(), [id("p"), id("q")], "{machine:?}");
            assert_eq!(m.pane_of(&id("t2")), Some(id("q")));
        }
    }

    #[test]
    fn shared_fields_can_be_omitted() {
        let doc: Document = serde_json::from_value(serde_json::json!({"spaces":[{"id":"s","tabs":[{"id":"t"}]}]})).unwrap();
        assert_eq!(doc.spaces[0].name, "");
        assert_eq!(doc.spaces[0].folder, "");
        assert_eq!(doc.spaces[0].tabs[0].name, "");
        assert!(doc.spaces[0].tabs[0].root.children().is_empty());
        assert_eq!(serde_json::from_value::<Document>(serde_json::json!({})).unwrap(), Document::default());
    }
    #[test]
    fn pane_names_are_shared_and_older_documents_have_none() {
        let mut m = model();
        m.rename_pane("p", "build");
        let doc: Document = serde_json::from_str(&serde_json::to_string(&Document::capture(&m, None)).unwrap()).unwrap();
        let mut other = Model::default();
        doc.replace(&mut other, Some("mac"));
        assert_eq!(other.pane_name("mac/p"), Some("build"));
        let old: Document = serde_json::from_value(serde_json::json!({"spaces":[{"id":"s","tabs":[{"id":"t","root":
            {"id":"t","type":"row","children":[{"id":"p","type":"block","content":{"terminal":"t1"}}]}}]}]})).unwrap();
        old.replace(&mut other, None);
        assert_eq!(other.address_of("p").as_deref(), Some("t1"));
        assert_eq!(other.pane_name("p"), None);
    }
    #[test]
    fn machine_blocks_are_deterministic_and_moves_stay_inside_them() {
        let doc = Document::capture(&model(), None);
        let mut m = Model::default();
        doc.replace(&mut m, Some("z"));
        doc.replace(&mut m, Some("a"));
        doc.replace(&mut m, None);
        assert_eq!(m.spaces.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["s", "a/s", "z/s"]);
        m.add_space("s2", "", "", Tab::new("t2", "", "p2", "t2"));
        m.order_spaces();
        assert!(m.move_space("s", usize::MAX));
        assert_eq!(m.spaces.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["s2", "s", "a/s", "z/s"]);
        assert!(m.move_space("z/s", 1));
        assert_eq!(m.spaces.last().unwrap().id, "z/s");
        doc.replace(&mut m, Some("a"));
        assert_eq!(m.spaces.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["s2", "s", "a/s", "z/s"]);
    }
    #[test]
    fn fresh_ids_are_not_a_per_screen_counter() {
        assert_ne!(Model::default().fresh_id("p"), Model::default().fresh_id("p"));
    }
}
