//! Shipped JS descriptions and native rendering regressions.
use super::*;
use gpui::TestAppContext;
use gpui_shell::{ShellRuntime, plugin::DescriptionSession, policy::Policy};
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

struct Staged(PathBuf);
impl Staged {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = PathBuf::from(format!(
            "/var/tmp/herdr-render-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        let staged = Self(path);
        for (folder, files) in [("mods", DEFAULT_MODS), ("frame", FRAME_MODS)] {
            for (name, bytes) in files {
                let path = staged.0.join(folder).join(name);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, bytes).unwrap();
            }
        }
        staged
    }
    fn entries(&self) -> Vec<String> {
        let mut entries = Vec::new();
        for folder in ["mods", "frame"] {
            for entry in std::fs::read_dir(self.0.join(folder)).unwrap() {
                let entry = entry.unwrap();
                assert!(entry.path().join("main.js").is_file(), "missing main.js: {:?}", entry.path());
                entries.push(format!("{folder}/{}", entry.file_name().to_str().unwrap()));
            }
        }
        entries.sort();
        entries
    }
    fn wrapper(&self, entry: &str, setup: &str) -> PathBuf {
        let dir = self.0.clone();
        std::fs::write(dir.join("main.js"), format!(
            "import Real from './{entry}/main.js'; export default class Scenario extends Real {{ init(props, cx) {{ super.init(props, cx); {setup} }} }}"
        )).unwrap();
        dir
    }
}
impl Drop for Staged {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[derive(Default)]
struct Calls {
    unexpected: Vec<String>,
    effects: Vec<(String, Vec<Value>)>,
    shared: BTreeMap<String, Value>,
}

fn updates(app: &str, core: &str) -> Value {
    json!({
        "app": {"version":"0.1.0", "updates":app != "source", "ready": (app == "ready").then_some("0.1.1")},
        "core": {"version":"0.1.5", "ready":(core == "ready").then_some("0.1.6"), "managed":(core == "managed").then_some("homebrew"), "error":(core == "error").then_some("offline"), "updating":core == "updating"},
        "checked":1700000000, "checks":app != "source", "checking":false
    })
}

fn fixture(populated: bool, props: Value, update: Value) -> BTreeMap<String, Value> {
    let program =
        json!({"key":"pi", "name":"Pi", "icon":null, "tint":true, "tile":"#334455", "color":"#aabbcc", "shell":false, "agent":true});
    let mut values = BTreeMap::from([
        ("props".into(), props),
        ("machines".into(), json!([{ "name": null, "layout": "ready" }])),
        ("spaces".into(), json!([])), ("agents".into(), json!([])), ("terminals".into(), json!([])), ("panes".into(), json!([])), ("focused".into(), Value::Null), ("recentPlaces".into(), json!([])),
        ("layout".into(), serde_json::from_str(include_str!("../../defaults/layouts/main.json")).unwrap()),
        ("layoutSpacing".into(), crate::layout_sync::spacing_fields(&serde_json::from_str(include_str!("../../defaults/layouts/main.json")).unwrap())),
        ("layouts".into(), json!(crate::window::shipped_layouts())),
        ("modActions".into(), json!([])),
        ("theme".into(), serde_json::to_value(&*crate::settings::theme()).unwrap()),
        ("settings".into(), crate::settings::Settings::default().for_mods()),
        ("themes".into(), json!([{"name":"graphite", "colors": &*crate::settings::theme()}])),  ("fonts".into(), json!(["Monospace", "Sans"])),
        ("updates".into(), update),
        ("telemetry".into(), json!({"enabled":true,"setting":true,"sends":true,"off_because":null,"endpoint":"https://invalid.example", "draft":{},"queued":[],"last_sent":null})),
        ("window".into(), json!({"platform":"linux","titlebar":"none","buttons":true,"maximized":false,"fullscreen":false,"active":true})),
        ("placement".into(), Value::Null),
        ("home".into(), json!("/fixture")), ("appHome".into(), json!("/fixture/app")),
        ("keys".into(), Value::Array(serde_json::from_str::<BTreeMap<String, BTreeMap<String, String>>>(include_str!("../../defaults/keys.json")).unwrap().into_iter().flat_map(|(section, bindings)| bindings.into_iter().map(move |(keys, action)| json!({"section":section,"keys":keys,"action":action}))).collect())),
        ("commands".into(), Value::Array(crate::commands::COMMANDS.iter().map(|c| json!({"name":c.name,"about":c.about,"args":c.args.iter().map(|a| json!({"name":a.name,"required":a.required})).collect::<Vec<_>>()})).collect())),
    ]);
    if populated {
        values.insert("spaces".into(), json!([
            {"id":"s1","index":1,"name":"Local","folder":"/fixture/project","machine":null,"branch":"main","active":true,"status":"working","tabs":[{"id":"tab1","index":1,"name":"Build","active":true,"status":"working","panes":1,"zoomed":false,"settled":false,"settled_at":null,"changed_at":1700000000000i64,"working_since":1700000000000i64}]},
            {"id":"s2","index":2,"name":"Remote","folder":"/remote/project","machine":"remote","branch":null,"active":false,"status":"done","tabs":[{"id":"tab2","index":1,"name":"Done","active":true,"status":"done","panes":1,"zoomed":false,"settled":false,"settled_at":null,"changed_at":null,"working_since":null}]}
        ]));
        values.insert("panes".into(), json!([{"pane":"p1","terminal":"t1","tab":"tab1","title":"Build","label":"Build","session":null,"cwd":"/fixture/project","branch":"main","status":"working","app":"pi","program":program,"focused":true,"zoomed":false}, {"pane":"p2","terminal":"t2","tab":"tab2","title":"Remote task","label":"Remote task","session":null,"cwd":"/remote/project","branch":null,"status":"done","app":"pi","program":program,"focused":false,"zoomed":false}]));
        values.insert("terminals".into(), json!([{"address":"t1","name":"Build","title":"Build","argv":["pi"],"parent":null,"labels":{},"state":"running","status":"working","report":{"app":"pi"},"program":program,"cwd":"/fixture/project"}, {"address":"t2","name":"Remote task","title":"Remote task","argv":["pi"],"parent":null,"labels":{},"state":"running","status":"done","report":{"app":"pi"},"program":program,"cwd":"/remote/project"}]));
        values.insert("agents".into(), json!([{"address":"t1","pane":"p1","space":"s1","space_name":"Local","tab":"tab1","tab_index":1,"tab_name":"Build","status":"working","app":"pi","program":program,"title":"Build","label":"Build","session":null,"seq":1,"focused":true}, {"address":"t2","pane":"p2","space":"s2","space_name":"Remote","tab":"tab2","tab_index":1,"tab_name":"Done","status":"done","app":"pi","program":program,"title":"Remote task","label":"Remote task","session":null,"seq":2,"focused":false}]));
        values.insert(
            "focused".into(),
            json!({"space":"s1","tab":"tab1","pane":"p1","terminal":"t1","agent":"t1","shown":null,"cwd":"/fixture/project"}),
        );
    }
    values
}

fn module(values: BTreeMap<String, Value>, calls: Rc<RefCell<Calls>>) -> HostModule {
    let mut module = HostModule::new("herdr");
    for (name, value) in values {
        module = module.function(name, move |_| Ok(to_host(&value)));
    }
    module = module.function("feedbackPreview", |a| {
        Ok(to_host(
            &json!({"available":true,"body":{"message":a.string(0)?,"email":a.string(1)?}}),
        ))
    });
    let shared = calls.clone();
    module = module.function("shared", move |a| {
        Ok(shared.borrow().shared.get(a.string(0)?).map(to_host).unwrap_or(HostValue::Null))
    });
    let shared = calls.clone();
    module = module.function("setShared", move |a| {
        shared.borrow_mut().shared.insert(a.string(0)?.into(), from_host(a.value(1)?));
        Ok(HostValue::Null)
    });
    for name in ["run", "playSound", "setAgentOrder", "openOverlay", "closeOverlay", "notify", "presentUpdateNotes"] {
        let calls = calls.clone();
        module = module.function(name, move |a| {
            if name == "run" {
                let command = a.string(0)?;
                let command = crate::commands::find(command).unwrap_or_else(|| panic!("unknown shipped mod command {command}"));
                let args = a.get(1).map(from_host).unwrap_or(Value::Null);
                crate::commands::validate_args(command, &args).unwrap();
            }
            calls
                .borrow_mut()
                .effects
                .push((name.into(), (0..2).filter_map(|i| a.get(i).map(from_host)).collect()));
            Ok(if name == "openOverlay" || name == "notify" {
                "fixture-overlay".into()
            } else {
                HostValue::Null
            })
        });
    }
    for name in ["feedbackPasteImage", "feedbackClearImages", "feedbackAttachImages"] {
        module = module.function(name, |_| Ok(HostValue::Null));
    }
    module = module.async_function("feedbackPickImages", |_| Ok(async { Ok(HostValue::Array(vec![])) }));
    module = module.async_function("knownAgents", |_| Ok(async { Ok(HostValue::Array(vec![])) }));
    module = module.async_function("pickFolder", |_| Ok(async { Ok(HostValue::Null) }));
    module = module.function("gitStatus", |_| Ok(HostValue::Null));
    module = module.function("refreshGitStatus", |_| Ok(HostValue::Null));
    for name in ["nextCommand", "watchFolder", "nextGitStatus"] {
        module = module.async_function(name, |_| Ok(std::future::pending::<gpui_shell::HostResult>()));
    }
    for name in ["exec", "hc", "readTextFile", "writeTextFile", "sendFeedback"] {
        let calls = calls.clone();
        module = module.async_function(name, move |a| {
            let first = a.get(0).map(from_host).unwrap_or(Value::Null);
            let reply = match name {
                "readTextFile" if first == "/fixture/app/settings.json" || first == "/fixture/.ssh/config" => Some(Value::Null),
                "readTextFile" if first.as_str().is_some_and(|p| p.starts_with("/fixture/app/mods/")) => {
                    let rel = first.as_str().unwrap().strip_prefix("/fixture/app/mods/").unwrap();
                    DEFAULT_MODS.iter().find(|(p, _)| *p == rel).map(|(_, bytes)| json!(std::str::from_utf8(bytes).unwrap()))
                }
                "readTextFile" => first
                    .as_str()
                    .and_then(|p| p.strip_prefix("/fixture/app/layouts/"))
                    .and_then(|p| p.strip_suffix(".json"))
                    .filter(|n| crate::window::shipped_layouts().contains(n))
                    .map(|_| json!("{}")),
                "hc" if first == json!(["machine", "ls", "--json"]) => {
                    Some(json!({"code":0,"stdout":"{\"machines\":[{\"name\":\"remote\",\"state\":\"online\"}]}","stderr":""}))
                }
                "hc" if first.as_array().and_then(|a| a.first()).and_then(Value::as_str) == Some("run") => {
                    Some(json!({"code":0,"stdout":"{\"address\":\"t-setup\"}","stderr":""}))
                }
                // The files panel lists a folder with sh: kind and name pairs.
                "exec" if first == "sh" && a.get(1).map(from_host).and_then(|v| v.get(2).cloned()) == Some(json!("files")) => {
                    Some(json!({"code":0,"stdout":"d\u{0}src\u{0}f\u{0}README.md\u{0}","stderr":""}))
                }
                "exec"
                    if first == "git"
                        && [
                            json!(["rev-parse", "--show-toplevel"]),
                            json!(["rev-parse", "--path-format=absolute", "--git-common-dir"]),
                            json!(["status", "--porcelain=v2", "--branch"]),
                            json!(["--no-optional-locks", "-c", "core.fsmonitor=false", "status", "--porcelain=v2", "--branch"]),
                            json!(["--no-optional-locks", "-c", "core.fsmonitor=false", "status", "--porcelain"]),
                            json!(["rev-parse", "--abbrev-ref", "HEAD"]),
                        ]
                        .contains(&a.get(1).map(from_host).unwrap_or(Value::Null)) =>
                {
                    Some(json!({"code":128,"stdout":"","stderr":"fixture: not a git repository"}))
                }
                _ => None,
            };
            let result = if let Some(reply) = reply {
                Ok(to_host(&reply))
            } else {
                let message = format!("unexpected {name}: {first}, {:?}", a.get(1));
                calls.borrow_mut().unexpected.push(message.clone());
                Err(HostError::new(message))
            };
            Ok(std::future::ready(result))
        });
    }
    module.declarations(DECLARATIONS)
}

struct Empty;
impl gpui::Render for Empty {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        gpui::div()
    }
}

fn describe(
    cx: &mut TestAppContext,
    staged: &Staged,
    entry: &str,
    populated: bool,
    props: Value,
    update: Value,
    setup: Option<&str>,
) -> anyhow::Result<String> {
    describe_with_values(cx, staged, entry, fixture(populated, props, update), setup)
}

fn describe_with_values(
    cx: &mut TestAppContext,
    staged: &Staged,
    entry: &str,
    values: BTreeMap<String, Value>,
    setup: Option<&str>,
) -> anyhow::Result<String> {
    let calls = Rc::new(RefCell::new(Calls::default()));
    static NEXT_STORE: AtomicU64 = AtomicU64::new(0);
    let policy = Policy::new()
        .with_storage_path(staged.0.join(format!("store-{}.json", NEXT_STORE.fetch_add(1, Ordering::Relaxed))))
        .with_capabilities(Capabilities::new().storage(true).read_roots([staged.0.clone()]))
        .with_host_module(module(values, calls.clone()))?;
    let runtime = ShellRuntime::new_isolated()?;
    let root = setup.map(|s| staged.wrapper(entry, s)).unwrap_or_else(|| staged.0.join(entry));
    let window = cx.add_window(|_, _| Empty);
    let result = window.update(cx, |_, window, cx| {
        let mut session = DescriptionSession::load(runtime, root, policy, window, cx)?;
        let first = session.describe(window, cx)?;
        let second = session.describe(window, cx)?;
        assert!(!first.is_empty() && !second.is_empty(), "empty description: {entry}");
        Ok(second)
    })?;
    window.update(cx, |_, window, _| window.remove_window())?;
    // localStorage writes land on the executor: let them finish while the
    // staged folder is still there, or they recreate it after its drop.
    cx.run_until_parked();
    assert!(calls.borrow().unexpected.is_empty(), "{entry}: {:?}", calls.borrow().unexpected);
    result
}

fn props() -> Value {
    json!({"overlay":"o1","pane":"p1","terminal":"t1","space":"s1","tab":"pane","target":{"pane":"p1","terminal":"t1","id":"s1","space":"s1"},"title":"Fixture","value":"Name","message":"A prefilled message","command":{"name":"space.rename","args":{"id":"s1"}},"notification":{"title":"Finished","body":"Build done","action":{"name":"app.help"}},"action":"new"})
}

#[gpui::test]
fn shipped_initial_descriptions(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let start = Instant::now();
    let staged = Staged::new();
    let entries = staged.entries();
    let mut count = 0;
    for entry in &entries {
        for populated in [false, true] {
            describe(cx, &staged, entry, populated, props(), updates("current", "current"), None)
                .unwrap_or_else(|e| panic!("{entry}/{}: {e:#}", if populated { "populated" } else { "empty" }));
            count += 1;
        }
    }
    eprintln!(
        "{count} initial-description cases ({} entries, each twice): {:?}",
        entries.len(),
        start.elapsed()
    );
}

#[gpui::test]
fn brand_leaves_room_for_macos_traffic_lights_only_outside_full_screen(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    for (fullscreen, inset) in [(false, true), (true, false)] {
        let mut values = fixture(true, props(), updates("current", "current"));
        values.insert("window".into(), json!({"platform":"macos","titlebar":"none","fullscreen":fullscreen,"buttons":false}));
        let tree = describe_with_values(cx, &staged, "mods/workspace-chrome", values, None).unwrap();
        let root = tree.lines().next().unwrap();
        assert_eq!(root.contains(".pl[Number(76.0)]"), inset, "fullscreen={fullscreen}: {tree}");
    }
}

#[gpui::test]
fn tabs_leave_room_for_macos_traffic_lights(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    // The traffic lights are window px: the mod divides by the zoom its px are drawn at (zoom × ui_zoom).
    for (platform, titlebar, fullscreen, width, lights, (zoom, ui_zoom), expected) in [
        ("macos", "none", false, 44, true, (1.0, 1.0), 32),
        ("macos", "none", false, 44, true, (0.5, 1.0), 108),
        ("macos", "none", false, 44, true, (2.0, 1.0), 10),
        ("macos", "none", false, 44, true, (1.0, 0.5), 108),
        ("macos", "none", false, 44, true, (0.5, 2.0), 32),
        ("macos", "none", false, 60, true, (1.0, 1.0), 16),
        ("macos", "none", false, 76, true, (1.0, 1.0), 10),
        ("macos", "none", false, 268, true, (1.0, 1.0), 10),
        ("macos", "none", true, 44, true, (1.0, 1.0), 10),
        ("macos", "system", false, 44, true, (1.0, 1.0), 10),
        ("macos", "none", false, 44, false, (1.0, 1.0), 10),
        ("linux", "none", false, 44, true, (1.0, 1.0), 10),
        ("windows", "none", false, 44, true, (1.0, 1.0), 10),
    ] {
        let mut values = fixture(true, props(), updates("current", "current"));
        values.insert("window".into(), json!({"platform":platform,"titlebar":titlebar,"fullscreen":fullscreen,"buttons":false}));
        values.get_mut("settings").unwrap()["zoom"] = json!(zoom);
        values.get_mut("settings").unwrap()["ui_zoom"] = json!(ui_zoom);
        values.get_mut("settings").unwrap()["ui_scale"] = json!(zoom * ui_zoom);
        let layout = values.get_mut("layout").unwrap();
        layout["children"][0]["size"]["px"] = json!(width);
        if !lights {
            layout.as_object_mut().unwrap().remove("traffic_lights");
        }
        let tree = describe_with_values(cx, &staged, "mods/tabs", values, None).unwrap();
        let root = tree.lines().next().unwrap();
        assert!(root.contains(&format!(".pl[Number({expected}.0)]")), "{platform}/{titlebar}/{fullscreen}/{width}/{lights}: {tree}");
        assert!(tree.contains("tab-tab1"), "first tab must remain visible: {tree}");
    }
}

/// Mid-drag, the tab strip dims the dragged tab, draws a line where it lands
/// and a ghost under the pointer; the native tree takes all three.
#[gpui::test]
fn tabs_draw_a_drag_in_progress(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let mut values = fixture(true, props(), updates("current", "current"));
    let tabs = values.get_mut("spaces").unwrap()[0]["tabs"].as_array_mut().unwrap();
    let mut second = tabs[0].clone();
    second["id"] = json!("tab3");
    second["index"] = json!(2);
    second["name"] = json!("Review");
    second["active"] = json!(false);
    tabs.push(second);
    let idle = describe_with_values(cx, &staged, "mods/tabs", values.clone(), None).unwrap();
    let setup = r#"this.press = { id: "tab1", x: 40, y: 20, grab: 12, width: 90 }; this.drag = { id: "tab1", x: 300, slot: 2, before: null }; this.strip = { x: 0, y: 0, width: 800, height: 40 };"#;
    let dragging = describe_with_values(cx, &staged, "mods/tabs", values, Some(setup)).unwrap();
    assert!(!idle.contains("opacity"), "{idle}");
    assert!(dragging.contains(".opacity[Number(0.4)]"), "the dragged tab dims: {dragging}");
    assert!(dragging.contains(".right[Number(-2.5)]"), "the line after the last tab: {dragging}");
    assert!(dragging.contains(".left[Number(288.0)]") || dragging.contains(".left[Number(288)]"), "the ghost at the pointer less the grab: {dragging}");
    assert_eq!(dragging.matches("\"Build\"").count(), 2, "the ghost draws the tab again: {dragging}");
}

#[gpui::test]
fn windows_tab_buttons_use_matching_vector_icons(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    for platform in ["windows", "linux"] {
        for maximized in [false, true] {
            let mut values = fixture(true, props(), updates("current", "current"));
            values.insert("window".into(), json!({"platform":platform,"buttons":true,"maximized":maximized}));
            let tree = describe_with_values(cx, &staged, "mods/tabs", values, None).unwrap();
            if platform == "windows" {
                for icon in ["win-min.svg", if maximized { "win-restore.svg" } else { "win-max.svg" }, "win-close.svg"] {
                    assert!(tree.contains(icon), "missing {icon}: {tree}");
                }
            } else {
                assert!(!tree.contains("mods/tabs/win-"), "{tree}");
                assert!(tree.contains("mods/pane/close.svg"), "{tree}");
            }
        }
    }
}

#[gpui::test]
fn font_pickers_contain_scroll_on_the_wrapper(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    for which in ["font", "ui_font"] {
        let mut p = props();
        p["section"] = json!("fonts");
        let tree = describe(
            cx, &staged, "frame/settings", true, p, updates("current", "current"),
            Some(&format!("this.picking = '{which}';")),
        ).unwrap();
        let scroll = tree.lines().find(|line| line.contains(&format!("st-fl-{which}"))).unwrap();
        assert!(scroll.contains("overflow_y_scrollbar"), "{tree}");
        assert!(!scroll.contains("on_scroll_wheel"), "the native scroller must receive the event: {tree}");
        assert_eq!(tree.matches("on_scroll_wheel").count(), 1, "the wrapper must stop scroll chaining: {tree}");
    }
}

#[gpui::test]
fn terminal_text_rows_show_whose_value_is_in_use(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let layout = "This layout sets its own; yours shows in the others.";
    for (person, overridden) in [("{}", 0), ("{font_weight: 300, line_height: 1.4, letter_spacing: 1}", 3)] {
        let setup = format!("this.load = async () => {{}}; this.person = {person};");
        let tree: String = ["fonts", "grid", "rendering"].map(|page| {
            let mut p = props();
            p["section"] = json!(page);
            describe(cx, &staged, "frame/settings", true, p, updates("current", "current"), Some(&setup)).unwrap()
        }).concat();
        for id in ["st-font_weight-plus", "st-font_weight_bold-plus", "st-faint_opacity-plus", "st-line_height-plus", "st-letter_spacing-plus", "st-cw-Font", "st-cw-Whole pixels", "st-font_ligatures", "Terminal text"] {
            assert!(tree.contains(id), "missing {id}: {tree}");
        }
        assert_eq!(tree.matches(layout).count(), overridden, "{person}: {tree}");
    }
}

#[gpui::test]
fn interface_zoom_and_text_size_rows_show_their_percent(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let mut p = props();
    p["section"] = json!("zoom");
    for ((ui_zoom, ui_text_scale), (zoom_shown, text_shown)) in [((1.0, 1.0), ("100%", "100%")), ((1.2, 1.1), ("120%", "110%"))] {
        let mut values = fixture(true, p.clone(), updates("current", "current"));
        values.get_mut("settings").unwrap()["ui_zoom"] = json!(ui_zoom);
        values.get_mut("settings").unwrap()["ui_text_scale"] = json!(ui_text_scale);
        let tree = describe_with_values(cx, &staged, "frame/settings", values, Some("this.load = async () => {}; this.person = {};")).unwrap();
        for id in ["st-ui_zoom-minus", "st-ui_zoom-plus", "st-ui_text_scale-minus", "st-ui_text_scale-plus", "Interface zoom", "Interface text size"] {
            assert!(tree.contains(id), "missing {id}: {tree}");
        }
        // The value shown between this row's own − and +.
        let row = |key: &str| {
            let lines = tree.lines().skip_while(|l| !l.contains(&format!("st-{key}-minus"))).skip(1);
            lines.take_while(|l| !l.contains(&format!("st-{key}-plus"))).collect::<Vec<_>>().join("\n")
        };
        assert!(row("ui_zoom").contains(zoom_shown), "ui_zoom {ui_zoom}: {}", row("ui_zoom"));
        assert!(row("ui_text_scale").contains(text_shown), "ui_text_scale {ui_text_scale}: {}", row("ui_text_scale"));
        assert!(zoom_shown == text_shown || !row("ui_zoom").contains(text_shown), "one value per row: {}", row("ui_zoom"));
    }
}

/// The Zoom row shows a percent at tenth steps and the terminals' text in
/// points at whole-point steps, with the "Zoom steps" picker beside it.
#[gpui::test]
fn zoom_row_shows_points_at_whole_point_steps(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let mut p = props();
    p["section"] = json!("zoom");
    for (step, zoom, shown) in [("tenth", 1.1, "110%"), ("point", 13.0 / 12.0, "13 pt"), ("point", 1.1, "13.2 pt")] {
        let mut values = fixture(true, p.clone(), updates("current", "current"));
        let settings = values.get_mut("settings").unwrap();
        settings["zoom_step"] = json!(step);
        settings["font_size"] = json!(12.0);
        settings["zoom"] = json!(zoom);
        let tree = describe_with_values(cx, &staged, "frame/settings", values, Some(&format!("this.load = async () => {{}}; this.person = {{ zoom_step: \"{step}\" }};"))).unwrap();
        for id in ["Zoom steps", "st-zs-10%", "st-zs-Whole points"] {
            assert!(tree.contains(id), "missing {id}: {tree}");
        }
        let lines = tree.lines().skip_while(|l| !l.contains("st-zoom-minus")).skip(1);
        let row = lines.take_while(|l| !l.contains("st-zoom-plus")).collect::<Vec<_>>().join("\n");
        assert!(row.contains(shown), "{step} at {zoom}: {row}");
        // The picked choice is the one drawn raised (with a border).
        let picked: Vec<_> = tree.lines().filter(|l| l.contains("st-zs-") && l.contains(".border_1")).collect();
        let want = if step == "point" { "st-zs-Whole points" } else { "st-zs-10%" };
        assert!(picked.len() == 1 && picked[0].contains(want), "{step}: {picked:?}");
    }
}

/// The shipped pane header shows the pane's own text size (`text_size` in
/// `panes()`) while it has one, and nothing at the default size.
#[gpui::test]
fn pane_header_shows_a_panes_own_text_size(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    for (text_size, shown) in [(None, None), (Some(16.0), Some("16pt")), (Some(15.400001), Some("15.4pt"))] {
        let mut values = fixture(true, json!({"pane":"p1","terminal":"t1"}), updates("current", "current"));
        values.get_mut("panes").unwrap()[0]["text_size"] = json!(text_size);
        let tree = describe_with_values(cx, &staged, "mods/pane", values, None).unwrap();
        assert!(tree.contains("pane-head-p1"), "{tree}");
        match shown {
            Some(label) => {
                let line = tree.lines().skip_while(|l| !l.contains("pane-text-size-p1")).take(3).collect::<Vec<_>>().join("\n");
                assert!(line.contains(label), "{text_size:?}: {line}");
            }
            None => assert!(!tree.contains("pane-text-size") && !tree.contains("pt\""), "{tree}"),
        }
    }
}

/// The Zoom page has a "Pane zoom keys" toggle under Zoom steps, on unless
/// settings.json turns it off.
#[gpui::test]
fn pane_zoom_keys_toggle_shows_the_setting(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let mut p = props();
    p["section"] = json!("zoom");
    for on in [true, false] {
        let mut values = fixture(true, p.clone(), updates("current", "current"));
        values.get_mut("settings").unwrap()["pane_zoom_keys"] = json!(on);
        let person = if on { "{}" } else { "{ pane_zoom_keys: false }" };
        let tree = describe_with_values(cx, &staged, "frame/settings", values, Some(&format!("this.load = async () => {{}}; this.person = {person};"))).unwrap();
        let steps = tree.find("Zoom steps").expect("Zoom steps");
        let keys = tree.find("Pane zoom keys").expect("Pane zoom keys");
        assert!(steps < keys, "under Zoom steps");
        let toggle = tree.lines().skip_while(|l| !l.contains("st-pane_zoom_keys")).take(6).collect::<Vec<_>>().join("\n");
        assert_eq!(toggle.contains("justify_end"), on, "{toggle}");
    }
}

/// The Terminal grid page has a Cell rounding choice under Cell width,
/// showing the person's pick.
#[gpui::test]
fn cell_rounding_row_shows_the_pick(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let mut p = props();
    p["section"] = json!("grid");
    for (person, want) in [("{}", "st-cr-Outward"), (r#"{ cell_rounding: "nearest" }"#, "st-cr-Nearest")] {
        let values = fixture(true, p.clone(), updates("current", "current"));
        let tree = describe_with_values(cx, &staged, "frame/settings", values, Some(&format!("this.load = async () => {{}}; this.person = {person};"))).unwrap();
        assert!(tree.contains("Cell rounding"), "{tree}");
        let picked: Vec<_> = tree.lines().filter(|l| l.contains("st-cr-") && l.contains(".border_1")).collect();
        assert!(picked.len() == 1 && picked[0].contains(want), "{person}: {picked:?}");
    }
}

/// The Panes page has Focus border (From theme, then the
/// theme's colors as swatches) and Focus border width, each showing the
/// person's pick.
#[gpui::test]
fn focus_border_rows_show_the_pick(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let mut p = props();
    p["section"] = json!("panes");
    for (person, colour, width) in [("{}", "st-fc-theme", "st-fw-Layout"), (r##"{ colors: { focus: "#ff8800" }, focus_border_width: 2 }"##, "st-fc-#ff8800", "st-fw-2 px")] {
        let values = fixture(true, p.clone(), updates("current", "current"));
        let tree = describe_with_values(cx, &staged, "frame/settings", values, Some(&format!("this.load = async () => {{}}; this.person = {person};"))).unwrap();
        let (names, border, wide) = (tree.find("Name panes automatically").expect("names"), tree.find("Focus border").expect("Focus border"), tree.find("Focus border width").expect("width"));
        assert!(names < border && border < wide, "under Panes, after Name panes automatically");
        let picked: Vec<_> = tree.lines().filter(|l| l.contains("st-fw-") && l.contains(".border_1")).collect();
        assert!(picked.len() == 1 && picked[0].contains(width), "{person}: {picked:?}");
        let ringed: Vec<_> = tree.lines().filter(|l| l.contains("st-fc-") && l.contains(".border_color") && !l.contains("#00000000")).collect();
        assert!(ringed.len() == 1 && ringed[0].contains(colour), "{person}: {ringed:?}");
        assert!(tree.lines().filter(|l| l.contains("st-fc-#")).count() >= 2, "the theme's colors are offered: {tree}");
    }
}

/// Searching the Settings window: the field sits in the sidebar, every page
/// with a match shows under its own heading with only its matches (controls
/// working), and a search with none says so.
#[gpui::test]
fn settings_search_filters_every_page(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let mut search = |q: &str| {
        let values = fixture(true, props(), updates("current", "current"));
        describe_with_values(cx, &staged, "frame/settings", values, Some(&format!("this.load = async () => {{}}; this.person = {{}}; this.setQuery({q:?});"))).unwrap()
    };
    let tree = search("");
    assert!(tree.contains("st-search"), "{tree}");
    assert!(!tree.contains("st-found-"), "no results with nothing typed: {tree}");

    let tree = search("rounding");
    assert!(tree.contains("st-found-grid") && tree.contains("st-cr-Nearest"), "{tree}");
    assert!(tree.contains("cell_"), "the row's settings.json key: {tree}");
    for hidden in ["st-found-themes", "st-found-alerts", "st-cw-", "st-font_ligatures", "Ligatures"] {
        assert!(!tree.contains(hidden), "{hidden} shows: {tree}");
    }

    let tree = search("sound");
    assert!(tree.contains("st-found-alerts") && tree.contains("st-sounds"), "{tree}");
    assert!(!tree.contains("st-found-grid"), "{tree}");

    let tree = search("qqqzzz");
    assert!(tree.contains("st-none") && tree.contains("No settings match"), "{tree}");
}

/// The Window page's App icon row: a previewed tile per built-in, the
/// person's outlined, and an image file shown as its path.
#[gpui::test]
fn app_icon_picker_outlines_the_pick(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let mut p = props();
    p["section"] = json!("window");
    for (person, picked) in [("{}", Some("classic")), (r#"{ app_icon: "mirage" }"#, Some("mirage")), (r#"{ app_icon: "theme" }"#, Some("theme")), (r#"{ app_icon: "~/mine.icns" }"#, None)] {
        let values = fixture(true, p.clone(), updates("current", "current"));
        let tree = describe_with_values(cx, &staged, "frame/settings", values, Some(&format!("this.load = async () => {{}}; this.person = {person};"))).unwrap();
        assert!(tree.contains("App icon"), "{tree}");
        let tiles: Vec<_> = tree.lines().filter(|l| l.contains("st-icon-") && !l.contains("st-icon-file")).collect();
        assert_eq!(tiles.len(), 7, "{tiles:?}");
        for name in ["classic", "theme", "midnight", "mirage", "glass", "phosphor", "graphite"] {
            assert!(tree.contains(&format!("frame/settings/app-icons/{name}.png")), "{name} preview");
        }
        let outlined: Vec<_> = tiles.iter().filter(|l| !l.contains("#00000000")).collect();
        match picked {
            Some(name) => assert!(outlined.len() == 1 && outlined[0].contains(&format!("st-icon-{name}")), "{person}: {outlined:?}"),
            None => assert!(outlined.is_empty() && tree.contains("Your file: ~/mine.icns"), "{person}: {outlined:?}"),
        }
    }
}

/// The Themes page's cards are a virtual list's rows: the page itself builds
/// none, however many themes there are.
#[gpui::test]
fn theme_cards_are_rows_of_a_virtual_list(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let mut p = props();
    p["section"] = json!("themes");
    let mut values = fixture(true, p, updates("current", "current"));
    let theme = values["theme"].clone();
    let names: Vec<String> = (0..40).map(|i| format!("dark-{i:02}")).collect();
    values.insert("themes".into(), json!(names.iter().map(|n| json!({ "name": n, "light": false, "colors": theme })).collect::<Vec<_>>()));
    let tree = describe_with_values(cx, &staged, "frame/settings", values, None).unwrap();
    assert!(tree.contains("v_virtual_list \"st-themes\" ×8"), "40 themes in rows of five: {tree}");
    assert!(!tree.contains("st-t-dark-"), "the page built a card: {tree}");
}

#[gpui::test]
fn confirm_dialog_background_follows_the_frame_corners(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let mut p = props();
    p["title"] = json!("Close space?");
    p["command"] = json!({"name": "space.close", "args": {"id": "s1"}});
    let tree = describe(cx, &staged, "mods/confirm", true, p, updates("current", "current"), None).unwrap();
    let root = tree.lines().find(|l| l.contains(":id[Str(\"confirm\")]")).unwrap();
    assert!(root.contains(".rounded[Number(8.0)]"), "the dialog background must follow the frame's corners: {root}");
}

#[gpui::test]
fn check_materializes_retained_elements_in_a_rendered_view(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    for body in [
        "v_flex().h(400).child(v_virtual_list('rows', 0, 28, (i) => 'a', () => { throw new Error('deferred rows ran'); }))",
        "v_flex().h(400).child(v_virtual_list('rows', 1, 28, (i) => 'a', () => { throw new Error('deferred rows ran'); }))",
        "div().w(100).h(50).left(10).transition('left', 200)",
        "div().w(100).h(50).transition('width', 200)",
        "div().w(100).h(50).left(10).spring('left')",
        "div().opacity(0.5).transition('opacity', 200)",
    ] {
        std::fs::write(staged.0.join("main.js"), format!(
            "import {{ View, div }} from 'gpui-kit'; import {{ v_flex, v_virtual_list }} from 'gpui-base'; export default class Demo extends View {{ render() {{ return {body}; }} }}"
        )).unwrap();
        let runtime = ShellRuntime::new_isolated().unwrap();
        let window = cx.add_window(|_, _| Empty);
        let description = window.update(cx, |_, window, cx| runtime.check(&staged.0, window, cx)).unwrap().unwrap();
        assert!(!description.is_empty(), "{body}");
        gpui::AnyWindowHandle::from(window).update(cx, |_, window, _| window.remove_window()).unwrap();
    }
}

#[gpui::test]
fn rebuilt_machine_offers_connect_only_for_identity_mismatch(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    for (state, why, offered) in [
        ("unreachable", "a different core answers there (\"new-id\")", true),
        ("unreachable", "connection refused", false),
        ("unreachable", "a different core answers there", false),
        ("connected", "a different core answers there (\"new-id\")", false),
    ] {
        let setup = format!(
            "this.machines = [{{name:'cloud', state:{}, why:{}, target:'host'}}]; this.folded.add('cloud');",
            json!(state), json!(why)
        );
        let tree = describe(cx, &staged, "mods/spaces", false, props(), updates("current", "current"), Some(&setup)).unwrap();
        assert_eq!(tree.contains("machine-connect-cloud"), offered, "{state}/{why}: {tree}");
        assert_eq!(tree.contains("rebuilt ·"), offered, "{tree}");
        assert_eq!(tree.contains("This machine was rebuilt or replaced. Click to connect to the new one."), offered, "{tree}");
        assert!(!tree.contains("machine-reconnect-"), "reconnect belongs in the machine row: {tree}");
    }
}

#[gpui::test]
fn machine_picker_keeps_long_lists_scrollable(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let tree = describe(
        cx, &staged, "mods/machine-add", false, props(), updates("current", "current"),
        Some("this.hosts = Array.from({length: 30}, (_, i) => ({alias: `host-${i}`, aliases: []}));"),
    ).unwrap();
    let scroll = tree.lines().find(|line| line.contains("ma-hosts")).unwrap();
    assert!(scroll.contains("overflow_y_scroll") && !scroll.contains("overflow_y_scrollbar"), "{tree}");
    assert!(scroll.contains(".min_h[Number(0.0)]"), "{tree}");
    assert!(tree.contains("ma-row-host-29"), "{tree}");
    assert_eq!(tree.matches(".flex_none").count(), 60, "rows must not shrink into the viewport: {tree}");
}

#[gpui::test]
fn machine_setup_prompt_stacks_above_the_picker(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let calls = Rc::new(RefCell::new(Calls::default()));
    let policy = Policy::new().with_host_module(module(fixture(false, props(), updates("current", "current")), calls.clone())).unwrap();
    let root = staged.wrapper("mods/machine-add", "cx.spawn(async () => { await this.inTerminal('host'); });");
    let window = cx.add_window(|_, _| Empty);
    let mut session = window.update(cx, |_, window, cx| {
        DescriptionSession::load(ShellRuntime::new_isolated().unwrap(), root, policy, window, cx).unwrap()
    }).unwrap();
    cx.run_until_parked();
    window.update(cx, |_, window, cx| {
        session.describe(window, cx).unwrap();
        window.remove_window();
    }).unwrap();
    let calls = calls.borrow();
    assert!(calls.unexpected.is_empty(), "{:?}", calls.unexpected);
    let spec = &calls.effects.iter().find(|(name, _)| name == "openOverlay").unwrap().1[0];
    assert_eq!(spec["kind"], "dialog");
    assert_eq!(spec["layout"]["children"][1]["content"]["terminal"], "t-setup");
    assert!(!calls.effects.iter().any(|(name, _)| name == "closeOverlay"), "the picker must keep monitoring setup");
}

#[gpui::test]
fn option_key_settings_are_only_shown_on_macos(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    for platform in ["linux", "windows", "macos"] {
        let mut p = props();
        p["section"] = json!("keyboard");
        let mut values = fixture(true, p, updates("current", "current"));
        values.insert("window".into(), json!({"platform":platform}));
        let tree = describe_with_values(cx, &staged, "frame/settings", values, None).unwrap();
        for value in ["auto", "on", "off", "left", "right"] {
            assert_eq!(tree.contains(&format!("st-option-{value}")), platform == "macos", "{platform}: {tree}");
        }
        assert_eq!(tree.contains("st-s-keyboard"), platform == "macos", "{platform}: {tree}");
    }
}

#[gpui::test]
fn settings_sections_and_updates(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let start = Instant::now();
    let staged = Staged::new();
    for section in ["look", "fonts", "alerts", "privacy", "updates"] {
        let mut p = props();
        p["section"] = json!(section);
        let tree = describe(cx, &staged, "frame/settings", true, p, updates("current", "current"), None).unwrap();
        assert_eq!(tree.contains("TERMINAL TEXT"), section == "fonts", "{section}: {tree}");
    }
    for app in ["source", "current", "ready"] {
        for core in ["current", "ready", "error", "managed", "updating"] {
            let mut p = props();
            p["section"] = json!("updates");
            let tree = describe(cx, &staged, "frame/settings", true, p, updates(app, core), None)
                .unwrap_or_else(|e| panic!("updates/{app}/{core}: {e:#}"));
            if app == "current" && core == "current" {
                assert!(
                    tree.contains("check-app") && tree.contains("check-core"),
                    "both update buttons must exist: {tree}"
                );
            }
        }
    }
    for (checking, checked, asked) in [
        (false, None, false), (false, None, true),
        (true, None, false), (true, None, true),
        (true, Some(1700000000u64), false), (true, Some(1700000000), true),
        (false, Some(1700000000), false), (false, Some(1700000000), true),
    ] {
        let mut p = props();
        p["section"] = json!("updates");
        let mut u = updates("current", "current");
        u["checked"] = json!(checked);
        u["checking"] = json!(checking);
        let setup = asked.then_some("this.asked = Date.now();");
        let tree = describe(cx, &staged, "frame/settings", true, p, u, setup).unwrap();
        let succeeded = !checking && checked.is_some();
        assert_eq!(tree.contains("Up to date"), succeeded, "success requires a completed, successful check: {tree}");
        if !checking && checked.is_none() {
            assert!(tree.contains("check-app") && tree.contains("check-core"), "failed checks must remain retryable: {tree}");
        }
        assert!(tree.contains(if checking { "Checking" } else if succeeded { "Up to date" } else { "Check your connection" }), "check status missing: {tree}");
    }
    eprintln!("27 settings cases, each twice: {:?}", start.elapsed());
}

#[gpui::test]
fn animated_images_advance_with_explicit_and_implicit_ids(cx: &mut TestAppContext) {
    use gpui::{IntoElement as _, http_client::{AsyncBody, FakeHttpClient, Response}};
    use image::{Delay, Frame, Rgba, RgbaImage, codecs::gif::GifEncoder};
    use std::{sync::Arc, time::Duration};

    cx.update(gpui_shell::init);
    let mut bytes = Vec::new();
    {
        let mut encoder = GifEncoder::new(&mut bytes);
        for color in [[255, 0, 0, 255], [0, 255, 0, 255]] {
            encoder.encode_frame(Frame::from_parts(RgbaImage::from_pixel(2, 2, Rgba(color)), 0, 0, Delay::from_numer_denom_ms(20, 1))).unwrap();
        }
    }
    let client = FakeHttpClient::create(move |_| {
        let bytes = bytes.clone();
        async move { Ok(Response::builder().status(200).body(AsyncBody::from(bytes)).unwrap()) }
    });
    cx.update(|cx| cx.set_http_client(client));
    let staged = Staged::new();
    for explicit in [false, true] {
        let runtime = ShellRuntime::new_isolated().unwrap();
        let url = format!("https://images.example/animation-{explicit}.gif");
        let source = format!("import {{ View, image }} from 'gpui-kit'; export default class Demo extends View {{ render() {{ return image('{url}'){}.size(20); }} }}", if explicit { ".id('clip')" } else { "" });
        std::fs::write(staged.0.join("main.js"), source).unwrap();
        let context = cx.add_empty_window();
        let view = context.update(|window, cx| {
            window.activate_window();
            let root = runtime.try_load(&staged.0, window, cx).unwrap();
            root.read(cx).content().clone()
        });
        let draw = |context: &mut gpui::VisualTestContext| {
            let view = view.clone();
            context.draw(gpui::Point::default(), gpui::size(gpui::px(40.), gpui::px(40.)), move |_, _| view.into_any_element());
        };
        draw(context);
        context.run_until_parked();
        draw(context);
        assert!(context.update(|window, _| window.is_window_active()));
        let resource = gpui::Resource::Uri(url.into());
        let data: Arc<gpui::RenderImage> = context.update(|_, cx| cx.fetch_asset::<gpui::ImgResourceLoader>(&resource).unwrap().unwrap());
        assert_eq!(data.frame_count(), 2);
        assert!(!context.update(|window, _| window.has_image_atlas_entry(&data)));
        // GPUI's image clock is wall time, not the test executor's clock.
        std::thread::sleep(Duration::from_millis(25));
        draw(context);
        assert!(context.update(|window, _| window.has_image_atlas_entry(&data)), "both animation frames must have painted (explicit id: {explicit})");
    }
}

#[gpui::test]
fn settings_keep_notes_in_the_whats_new_window(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let notes = json!([
        {"version":"1.8.0","date":"2026-10-26","items":[{"headline":"Newest headline","detail":"Newest details","fixed":false,"media":[]},{"headline":"","detail":"Newest fix","fixed":true,"media":[]}],"spotlight":true},
        {"version":"1.7.0","date":"2026-10-25","items":[{"headline":"Older headline","detail":"Older details","fixed":false,"media":[]},{"headline":"Another older headline","detail":"Hidden detail","fixed":false,"media":[]}],"spotlight":true}
    ]);
    let mut update = updates("ready", "current");
    update["notes"] = notes.clone();
    let mut p = props();
    p["section"] = json!("updates");
    let tree = describe(cx, &staged, "frame/settings", true, p, update.clone(), None).unwrap();
    assert!(tree.contains("st-whats-new") && tree.contains("st-app-update") && tree.contains("st-check-core"));
    assert!(!tree.contains("Newest details") && !tree.contains("Older headline"));
    let mut p = props();
    p["notes"] = notes;
    let tree = describe(cx, &staged, "frame/whatsnew", true, p, update, None).unwrap();
    assert!(tree.contains("Newest details") && tree.contains("Older details"));
    assert!(tree.contains("Also fixed") && tree.contains("Newest fix"));
    assert!(tree.contains("Oct 26") && !tree.contains("2026-10-26"));
}

#[gpui::test]
fn local_state_and_overlay_initial_descriptions(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let start = Instant::now();
    let staged = Staged::new();
    for page in 1..=4 {
        let tree = describe(
            cx,
            &staged,
            "frame/welcome",
            true,
            props(),
            updates("current", "current"),
            Some(&format!("this.page = {page};")),
        )
        .unwrap();
        let expected = ["Herdr runs your terminals in the background", "Add your other machines", "Choose your panels", "Make it yours"][page - 1];
        assert!(tree.contains(expected), "welcome page {page}: {tree}");
    }
    for setup in ["", "this.sent = true;", "this.error = 'Fixture error';"] {
        let tree = describe(
            cx,
            &staged,
            "frame/feedback",
            true,
            props(),
            updates("current", "current"),
            Some(setup),
        )
        .unwrap();
        assert!(tree.contains("feedback"), "{tree}");
        if setup.is_empty() {
            assert!(
                tree.contains("Input #") && tree.contains("Textarea #") && tree.contains("A prefilled message"),
                "{tree}"
            );
        } else {
            assert!(
                tree.contains(if setup.contains("sent") { "Sent. Thanks" } else { "Fixture error" }),
                "{tree}"
            );
        }
    }
    for query in ["settings", "no-such-command"] {
        let tree = describe(
            cx,
            &staged,
            "frame/palette",
            true,
            props(),
            updates("current", "current"),
            Some(&format!("this.query = {query:?};")),
        )
        .unwrap();
        // The query sits in a native text field; what the list shows tells it apart.
        let expected = if query == "settings" { "Settings" } else { "Nothing matches" };
        assert!(tree.contains(expected), "palette query {query}: {tree}");
    }
    // A mod's palette action lists under the mod's name, found by its keywords too.
    let mut values = fixture(true, props(), updates("current", "current"));
    values.insert("modActions".into(), json!([{"mod":"broadcast","title":"Send to all terminals","command":"mod.open","args":{"name":"broadcast"},"keywords":"smart"}]));
    for query in ["send to all", "smart"] {
        let tree = describe_with_values(cx, &staged, "frame/palette", values.clone(), Some(&format!("this.query = {query:?};"))).unwrap();
        assert!(tree.contains("Send") && tree.contains("broadcast"), "palette query {query}: {tree}");
    }
    // Reviewed overlay contract table, not extracted from arbitrary JS call sites.
    let scenarios = [
        ("mods/menu", json!({"tab":"pane","target":{"pane":"p1","terminal":"t1"}})),
        ("mods/menu", json!({"tab":"tab","target":{"id":"tab1","space":"s1"}})),
        ("mods/menu", json!({"tab":"space","target":{"id":"s1"}})),
        ("mods/prompt", json!({"machines":true})),
        (
            "mods/confirm",
            json!({"title":"Close space?","command":{"name":"space.close","args":{"id":"s1"}}}),
        ),
        (
            "mods/toast",
            json!({"title":"Done","body":"Build complete","level":"success","action":{"name":"app.help"}}),
        ),
        ("mods/peek", json!({"terminal":"t1"})),
        ("mods/peek", json!({"terminal":"missing"})),
        ("mods/worktree", json!({"space":"s1","action":"new"})),
        ("mods/worktree", json!({"space":"s1","action":"remove"})),
    ];
    let entries = staged.entries();
    for (entry, patch) in scenarios {
        assert!(entries.iter().any(|e| e == entry), "orphan scenario: {entry}");
        let mut p = props();
        p.as_object_mut().unwrap().extend(patch.as_object().unwrap().clone());
        describe(cx, &staged, entry, true, p, updates("current", "current"), None).unwrap();
    }
    eprintln!("18 local-state/overlay cases, each twice: {:?}", start.elapsed());
}

#[gpui::test]
fn duplicate_element_negative_control(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let dir = staged.0.join("negative");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("main.js"), "import { View, div } from 'gpui-kit'; export default class Bad extends View { render() { const child = div().child('reused'); return div().child(div().child(child)).child(div().child(child)); } }").unwrap();
    let error = describe(cx, &staged, "negative", false, props(), updates("current", "current"), None).unwrap_err();
    assert!(format!("{error:#}").contains("already added to a parent"), "{error:#}");
}

#[test]
fn shipped_layouts_and_keys() {
    let staged = Staged::new();
    let mods: BTreeSet<_> = staged
        .entries()
        .into_iter()
        .filter_map(|p| p.strip_prefix("mods/").map(str::to_owned))
        .collect();
    let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("defaults/layouts");
    let mut layouts = BTreeSet::new();
    fn references(v: &Value, mods: &BTreeSet<String>) {
        match v {
            Value::Object(o) => {
                if let Some(Value::String(name)) = o.get("mod") {
                    assert!(mods.contains(name), "unknown layout mod {name}");
                }
                for v in o.values() {
                    references(v, mods);
                }
            }
            Value::Array(a) => {
                for v in a {
                    references(v, mods);
                }
            }
            _ => {}
        }
    }
    for entry in std::fs::read_dir(folder).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        serde_json::from_str::<Node>(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        references(&serde_json::from_str::<Value>(&text).unwrap(), &mods);
        layouts.insert(path.file_stem().unwrap().to_str().unwrap().to_owned());
    }
    assert_eq!(layouts, crate::window::shipped_layouts().into_iter().map(str::to_owned).collect());
    let mod_commands = ["ribbon.next"];
    for text in [
        include_str!("../../defaults/keys.json"),
        include_str!("../../defaults/keys.macos.json"),
    ] {
        crate::keys::parse(text).unwrap();
        let sections: BTreeMap<String, BTreeMap<String, String>> = serde_json::from_str(text).unwrap();
        for bindings in sections.values() {
            for action in bindings.values() {
                let name = action.split(':').next().unwrap();
                if let Some(command) = crate::commands::find(name) {
                    let args = match action.split_once(':') {
                        Some((_, value)) => {
                            assert!(!command.args.is_empty(), "unexpected shipped key argument {action}");
                            crate::commands::shorthand(command, value)
                        }
                        None => Value::Null,
                    };
                    crate::commands::validate_args(command, &args).unwrap();
                    assert!(crate::commands::missing(command, &args).is_none(), "missing shipped key argument {action}");
                }
                assert!(
                    action == "terminal"
                        || action.starts_with("send:")
                        || crate::commands::COMMANDS.iter().any(|c| c.name == name)
                        || (mod_commands.contains(&name) && mods.contains(name.split('.').next().unwrap())),
                    "unknown shipped key action {action}"
                );
            }
        }
    }
}

#[gpui::test]
fn picker_scrolls_the_selection_into_view(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let mut values = fixture(true, props(), updates("current", "current"));
    let tabs: Vec<Value> = (1..=12)
        .map(|i| json!({"id":format!("tab{i}"),"index":i,"name":format!("Task {i}"),"active":i==1,"status":"done","panes":1,"zoomed":false,"settled":false,"settled_at":null,"changed_at":null,"working_since":null}))
        .collect();
    values.insert("spaces".into(), json!([
        {"id":"s1","index":1,"name":"Local","folder":"/fixture/project","machine":null,"branch":"main","active":true,"status":"working","tabs":tabs}
    ]));
    // Selecting the last row (with the arrows) must scroll it into the visible 320px.
    let tree = describe_with_values(cx, &staged, "mods/picker", values, Some("this.at = 999; cx.notify();")).unwrap();
    let top = tree.lines()
        .find(|l| l.contains("pk-space-here"))
        .unwrap_or_else(|| panic!("selected row missing: {tree}"));
    let px: f64 = top.split(".top[Number(").nth(1).unwrap_or_else(|| panic!("selected row must be placed inside the scrolled list: {tree}")).split(")").next().unwrap().parse().unwrap();
    assert!(px <= 320.0 - 32.0, "selected row at {px}px is below the visible list: {tree}");
}

#[gpui::test]
fn status_bar_shows_the_real_palette_key(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let mut bound = fixture(true, props(), updates("current", "current"));
    bound.insert("keys".into(), json!([{"section":"global","keys":"ctrl-p","action":"app.palette"}]));
    let tree = describe_with_values(cx, &staged, "mods/workspace-status", bound, None).unwrap();
    assert!(tree.contains("Ctrl+P"), "hint must show the real binding: {tree}");
    assert!(!tree.contains("Shift P"), "stale hardcoded hint: {tree}");

    let mut unbound = fixture(true, props(), updates("current", "current"));
    unbound.insert("keys".into(), json!([{"section":"global","keys":"ctrl-k","action":"app.help"}]));
    let tree = describe_with_values(cx, &staged, "mods/workspace-status", unbound, None).unwrap();
    assert!(tree.contains("commands"), "unbound hint still offers commands: {tree}");
    assert!(!tree.contains("Shift P"), "unbound hint must not invent a key: {tree}");
}

/// The pane headers put the automatic name after the person's and before the
/// program's (which only shows at a bare shell when there's none).
#[gpui::test]
fn pane_headers_show_the_automatic_name_after_the_persons(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let shell = json!({"key":"shell", "name":"zsh", "icon":null, "tint":true, "tile":null, "color":null, "shell":true, "agent":false});
    // What each shows without one: the shell's program name (its label, as its title is empty).
    for (entry, fallback) in [("mods/pane", "zsh"), ("mods/terminals-pane", "zsh")] {
        // A shell's title as Herdr gives it an automatic name: a folder, never empty.
        let header = |name: Value, auto_name: Value, cx: &mut TestAppContext| {
            let title = if auto_name.is_null() { "" } else { "~/src" };
            let mut values = fixture(true, props(), updates("current", "current"));
            values.insert("panes".into(), labelled(json!([{"pane":"p1","terminal":"t1","tab":"tab1","title":title,"name":name,"auto_name":auto_name,"session":null,"cwd":"/fixture/project","branch":null,"status":null,"app":null,"program":shell,"focused":true,"zoomed":false}])));
            describe_with_values(cx, &staged, entry, values, None).unwrap()
        };
        let tree = header(Value::Null, json!("hazy-inlet"), cx);
        assert!(tree.contains("\"hazy-inlet\"") && !tree.contains("\"~/src\""), "{entry}: the automatic name before the folder title: {tree}");
        let tree = header(json!("build"), json!("hazy-inlet"), cx);
        assert!(tree.contains("\"build\"") && !tree.contains("hazy-inlet"), "{entry}: the person's name first: {tree}");
        let tree = header(Value::Null, Value::Null, cx);
        assert!(!tree.contains("hazy-inlet") && tree.contains(&format!("\"{fallback}\"")), "{entry}: without one, as before: {tree}");
    }
}

/// The pane header of a working agent, at the default `working_spinner`, is
/// the header drawn before the setting existed, line for line: the ring.
#[gpui::test]
fn pane_header_draws_the_ring_by_default_as_it_always_has(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let tree = describe(cx, &staged, "mods/pane", true, json!({"pane":"p1","terminal":"t1"}), updates("current", "current"), None).unwrap();
    assert_eq!(tree.trim_end(), r##"h_flex :id[Str("pane-head-p1")] .size_full .px[Number(10.0)] .gap[Number(8.0)] .items_center .bg[Str("#17191d")] .border_b[Number(1.0)] .border_color[Str("#34363a")] .text_size[Number(12.0)] :on_hover(fn) :on_click(fn)
  div .flex_none .truncate .max_w[Number(260.0)] .font_medium .text_color[Str("#eeeeef")]
    text "Pi"
  div .flex_1 .truncate .text_color[Str("#7a7b7d")]
    text "~/project"
  svg "mods/workspace-chrome/spinner.svg" .size[Number(12.0)] .flex_none .text_color[Str("#e5c07b")] :spin[Number(900.0)]"##);
}

/// Every shipped mod that shows an agent at work draws the person's
/// `working_spinner`: the ring unless it is set, else its frames, natively,
/// in the terminal font and the working colour, in a box that keeps its width.
#[gpui::test]
fn agent_spinners_follow_the_working_spinner_setting(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let braille = r#":frames[Number(80.0), Str("⠋"), Str("⠙"), Str("⠹"), Str("⠸"), Str("⠼"), Str("⠴"), Str("⠦"), Str("⠧"), Str("⠇"), Str("⠏")]"#;
    let working = crate::settings::theme()["working"].clone();
    for (entry, p) in [("mods/pane", json!({"pane":"p1","terminal":"t1"})), ("mods/peek", json!({"terminal":"t1","overlay":"o1"})), ("mods/picker", props()), ("mods/tabs", props()), ("mods/agents", props())] {
        let tree = describe(cx, &staged, entry, true, p.clone(), updates("current", "current"), None).unwrap();
        assert!(tree.contains(r#"svg "mods/workspace-chrome/spinner.svg""#) && tree.contains(":spin[Number(900.0)]") && !tree.contains(":frames"), "{entry} draws the ring by default: {tree}");
        let mut values = fixture(true, p, updates("current", "current"));
        let settings = values.get_mut("settings").unwrap();
        *settings = crate::settings::Settings::compose(&serde_json::from_value(json!({"working_spinner": "braille"})).unwrap(), &crate::layout::Look::default(), true).for_mods();
        let font = settings["font_family"].as_str().unwrap().to_owned();
        let tree = describe_with_values(cx, &staged, entry, values, None).unwrap();
        assert!(!tree.contains(":spin["), "{entry}: no ring with braille chosen: {tree}");
        let spinner = tree.lines().skip_while(|l| !l.contains(":frames[")).take(2).collect::<Vec<_>>();
        assert_eq!(spinner.len(), 2, "{entry} draws the frames: {tree}");
        assert!(spinner[0].contains(braille), "{entry}: {}", spinner[0]);
        for style in [".min_w[Number(12.0)]", ".flex_none", ".justify_center", &format!(".font_family[Str({font:?})]"), &format!(".text_color[Str({working:?})]")] {
            assert!(spinner[0].contains(style), "{entry} spinner lacks {style}: {}", spinner[0]);
        }
        assert_eq!(spinner[1].trim(), r#"text "⠋""#, "{entry} starts on the first frame");
    }
}

/// Panes as the app publishes them: `label` and `session` filled in by the
/// app's own resolver (pane_label.rs) from the other fields.
fn labelled(mut panes: Value) -> Value {
    for p in panes.as_array_mut().unwrap() {
        let label = crate::pane_label::label(&crate::pane_label::Parts {
            name: p["name"].as_str(), auto_name: p["auto_name"].as_str(), title: p["title"].as_str().unwrap_or_default(),
            session: p["session"].as_str(), program: &p["program"], address: p["terminal"].as_str().unwrap_or_default(),
        });
        p["label"] = json!(label);
    }
    panes
}

fn texts(tree: &str) -> Vec<&str> {
    tree.lines().filter_map(|l| l.trim().strip_prefix("text ")).collect()
}

/// The mods that name a pane read its `label`: with telling titles they draw
/// what they drew from the title, byte for byte (the whole populated fixture
/// was diffed when they moved to `label`; the names are pinned here), and a
/// title that names nothing gives way to the session's name, else the program's.
#[gpui::test]
fn mods_name_panes_by_their_label(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let p2 = json!({"pane":"p2","terminal":"t2"});
    let w1 = json!({"role":"window","pane":"p1"});
    let w2 = json!({"role":"window","pane":"p2"});
    let shown = |entry: &str, p: &Value, values: Option<BTreeMap<String, Value>>, cx: &mut TestAppContext| {
        let values = values.unwrap_or_else(|| fixture(true, p.clone(), updates("current", "current")));
        let tree = describe_with_values(cx, &staged, entry, values, None).unwrap();
        texts(&tree).into_iter().map(str::to_owned).collect::<Vec<_>>()
    };
    let telling: [(&str, &Value, &[&str]); 6] = [
        ("mods/agents", &props(), &["\"Agents\"", "\"Done\"", "\"1\"", "\"Remote\"", "\"· Remote task\"", "\"<1m\"", "\"Working\"", "\"1\"", "\"Local\"", "\"· Build\"", "\"<1m\""]),
        ("mods/pane", &p2, &["\"Pi\"", "\"/remote/project\""]),
        ("mods/terminals-pane", &props(), &["\"Build\"", "\"~/project\""]),
        ("mods/terminals-pane", &p2, &["\"Remote task\"", "\"/remote/project\""]),
        ("mods/win98", &w1, &["\"Pi - Build\""]),
        ("mods/win98", &w2, &["\"Pi - Remote task\""]),
    ];
    for (entry, p, want) in telling {
        assert_eq!(shown(entry, p, None, cx), want, "{entry} {p}");
    }

    // Claude Code's prompt left in Neovim's title: the session names the agent.
    let nvim = "claude-prompt-9acc2eb1-37f2-4d7e-9c1a-5b6f0e2d8a11.md + (/private/tmp/claude-501) - Nvim";
    let throwaway = |p: &Value, session: Value| {
        let mut values = fixture(true, p.clone(), updates("current", "current"));
        let mut panes = values["panes"].clone();
        panes[1]["title"] = json!(nvim);
        panes[1]["session"] = session;
        let panes = labelled(panes);
        let mut agents = values["agents"].clone();
        agents[1]["title"] = json!(nvim);
        agents[1]["label"] = panes[1]["label"].clone();
        values.insert("panes".into(), panes);
        values.insert("agents".into(), agents);
        values
    };
    let named = |p: &Value| Some(throwaway(p, json!("vendor-insights")));
    assert!(shown("mods/agents", &props(), named(&props()), cx).contains(&"\"· vendor-insights\"".to_owned()));
    assert_eq!(shown("mods/terminals-pane", &p2, named(&p2), cx)[0], "\"vendor-insights\"");
    assert_eq!(shown("mods/win98", &w2, named(&w2), cx), ["\"Pi - vendor-insights\""]);
    assert_eq!(shown("mods/pane", &p2, named(&p2), cx)[0], "\"Pi\"", "an unnamed agent's header shows its program");
    // No session: the program's name, which the agents panel never shows as the tab.
    let unnamed = |p: &Value| Some(throwaway(p, Value::Null));
    assert!(shown("mods/agents", &props(), unnamed(&props()), cx).contains(&"\"· Done\"".to_owned()));
    assert_eq!(shown("mods/terminals-pane", &p2, unnamed(&p2), cx)[0], "\"Pi\"");
    assert_eq!(shown("mods/win98", &w2, unnamed(&w2), cx), [r#""Pi - C:\\remote\\project""#]);

    // A status glyph or a bracket before a telling title is shown as before.
    for title in ["\u{2713} build passed", "[3/5] Building crates"] {
        let titled = |p: &Value| {
            let mut values = fixture(true, p.clone(), updates("current", "current"));
            let mut panes = values["panes"].clone();
            panes[1]["title"] = json!(title);
            values.insert("panes".into(), labelled(panes));
            Some(values)
        };
        assert_eq!(shown("mods/terminals-pane", &p2, titled(&p2), cx)[0], format!("{title:?}"));
        assert_eq!(shown("mods/win98", &w2, titled(&w2), cx), [format!("\"Pi - {title}\"")]);
    }

    // A program that is neither an agent nor a shell: the header shows its label.
    let neovim = json!({"key":"nvim", "name":"Neovim", "icon":null, "tint":true, "tile":null, "color":null, "shell":false, "agent":false});
    let editor = |title: &str, cx: &mut TestAppContext| {
        let mut values = fixture(true, p2.clone(), updates("current", "current"));
        let mut panes = values["panes"].clone();
        panes[1]["title"] = json!(title);
        panes[1]["program"] = neovim.clone();
        values.insert("panes".into(), labelled(panes));
        shown("mods/pane", &p2, Some(values), cx)[0].clone()
    };
    assert_eq!(editor("Fix nvim config", cx), "\"Fix nvim config\"");
    assert_eq!(editor(nvim, cx), "\"Neovim\"");
}

/// The pane menu's copy group: Copy only over a selection, Copy path only
/// with a known folder, Copy all and Paste always, all above Split.
#[gpui::test]
fn pane_menu_copy_items(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let labels = |target: Value, cx: &mut TestAppContext| {
        let mut p = props();
        p["target"] = target;
        let tree = describe(cx, &staged, "mods/menu", true, p, updates("current", "current"), None).unwrap();
        texts(&tree).into_iter().filter(|t| !t.is_empty() && *t != "\"\"").map(str::to_owned).collect::<Vec<_>>()
    };
    let all = labels(json!({"pane":"p1","terminal":"t1","selected":true,"cwd":"/fixture/project"}), cx);
    assert_eq!(all[..5], ["\"Copy\"", "\"Copy all\"", "\"Paste\"", "\"Copy path\"", "\"Split right\""], "{all:?}");
    let bare = labels(json!({"pane":"p1","terminal":"t1","selected":false,"cwd":null}), cx);
    assert_eq!(bare[..3], ["\"Copy all\"", "\"Paste\"", "\"Split right\""], "{bare:?}");
    // No such terminal: nothing to copy from.
    let gone = labels(json!({"pane":"p1","terminal":"t9","selected":true,"cwd":"/x"}), cx);
    assert!(!gone.iter().any(|t| t.contains("Copy") || t.contains("Paste")), "{gone:?}");
}

/// The populated fixture with what the app publishes for icons: `icon` and
/// `own_icon` on every pane and tab, `icon` on every agent.
fn with_icons(values: &mut BTreeMap<String, Value>, pane: (Value, Value), tab: (Value, Value)) {
    for p in values.get_mut("panes").unwrap().as_array_mut().unwrap() {
        p["icon"] = pane.0.clone();
        p["own_icon"] = pane.1.clone();
    }
    for a in values.get_mut("agents").unwrap().as_array_mut().unwrap() {
        a["icon"] = pane.0.clone();
    }
    for s in values.get_mut("spaces").unwrap().as_array_mut().unwrap() {
        for c in s["tabs"].as_array_mut().unwrap() {
            c["icon"] = tab.0.clone();
            c["own_icon"] = tab.1.clone();
        }
    }
}

/// Letter mode with nothing chosen publishes null icons: every shipped mod
/// draws exactly what it drew before the fields existed.
#[gpui::test]
fn null_icons_draw_every_shipped_mod_as_before(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let mut count = 0;
    for entry in staged.entries() {
        let before = describe(cx, &staged, &entry, true, props(), updates("current", "current"), None).unwrap();
        let mut values = fixture(true, props(), updates("current", "current"));
        with_icons(&mut values, (Value::Null, Value::Null), (Value::Null, Value::Null));
        let after = describe_with_values(cx, &staged, &entry, values, None).unwrap();
        // Focus and scroll handles and native elements are numbered per window.
        let handles = regex::Regex::new(r"(:track_(?:focus|scroll)\[Number\(| #)[0-9]+").unwrap();
        let same = |tree: &str| handles.replace_all(tree, "${1}N").into_owned();
        let (before, after) = (same(&before), same(&after));
        let differ = before.lines().zip(after.lines()).find(|(a, b)| a != b);
        assert_eq!(before, after, "{entry}: first difference {differ:?}");
        count += 1;
    }
    assert!(count > 20, "{count} entries");
}

/// A pane's and a tab's own icon: the pane header and the tab bar draw its
/// glyph in the Nerd Font, an emoji or a letter, in place of the program's.
#[gpui::test]
fn pane_header_and_tabs_draw_an_icon_in_place_of_the_programs(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let font = r#".font_family[Str("Symbols Nerd Font")]"#;
    for (entry, p) in [("mods/pane", json!({"pane":"p1","terminal":"t1"})), ("mods/tabs", props())] {
        for (icon, text, glyph) in [(json!({"kind":"glyph","text":"\u{f0073}"}), "\u{f0073}", true), (json!({"kind":"emoji","text":"🌾"}), "🌾", false), (json!({"kind":"letter","text":"B"}), "B", false)] {
            let mut values = fixture(true, p.clone(), updates("current", "current"));
            with_icons(&mut values, (icon.clone(), Value::Null), (icon.clone(), Value::Null));
            values.get_mut("panes").unwrap()[0]["program"]["icon"] = json!("icons/pi.svg");
            let tree = describe_with_values(cx, &staged, entry, values, None).unwrap();
            let line = tree.lines().position(|l| l.trim() == format!("text {text:?}")).unwrap_or_else(|| panic!("{entry} {icon}: {tree}"));
            let parent = tree.lines().nth(line - 1).unwrap();
            assert_eq!(parent.contains(font), glyph, "{entry} {icon}: {parent}");
            assert!(parent.contains(".flex_none") && parent.contains(".text_size[Number(13.0)]"), "{entry}: {parent}");
            assert!(!tree.contains("icons/pi.svg"), "{entry} {icon}: not the program's as well: {tree}");
        }
    }
}

/// Change icon… in the pane and tab menus (the pane's also where the program
/// takes right-clicks: Shift+right-click opens it), and Clear icon only while
/// one is chosen.
#[gpui::test]
fn pane_and_tab_menus_offer_change_and_clear_icon(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let items = |p: Value, own: Value, to_program: bool, cx: &mut TestAppContext| {
        let mut values = fixture(true, p, updates("current", "current"));
        with_icons(&mut values, (Value::Null, own.clone()), (Value::Null, own));
        if to_program {
            values.get_mut("terminals").unwrap()[0]["labels"] = json!({"herdr.right-click": "program"});
        }
        let tree = describe_with_values(cx, &staged, "mods/menu", values, None).unwrap();
        // Items only (each in its `flex_1` label), not the key hints beside them.
        let lines: Vec<&str> = tree.lines().collect();
        lines.windows(2).filter(|w| w[0].trim() == "div .flex_1").filter_map(|w| w[1].trim().strip_prefix("text ")).map(str::to_owned).collect::<Vec<_>>()
    };
    let pane = json!({"tab":"pane","target":{"pane":"p1","terminal":"t1"}});
    let tab = json!({"tab":"tab","target":{"id":"tab1","space":"s1"}});
    let own = json!({"kind":"emoji","value":"🌾"});
    for (p, after) in [(&pane, "\"Zoom\""), (&tab, "\"Rename\"")] {
        let none = items(p.clone(), Value::Null, false, cx);
        let at = none.iter().position(|t| t == after).unwrap();
        assert_eq!(none[at + 1], "\"Change icon…\"", "{none:?}");
        assert!(!none.iter().any(|t| t == "\"Clear icon\""), "{none:?}");
        let chosen = items(p.clone(), own.clone(), false, cx);
        assert_eq!(chosen[at + 1..at + 3], ["\"Change icon…\"", "\"Clear icon\""], "{chosen:?}");
    }
    assert!(items(pane.clone(), Value::Null, true, cx).iter().any(|t| t == "\"Change icon…\""), "the Shift+right-click menu has it too");
    // A terminal block outside a tab has no pane to give an icon.
    assert!(!items(json!({"tab":"pane","target":{"pane":"block","terminal":"t1"}}), Value::Null, false, cx).iter().any(|t| t.contains("icon")));
    let space = items(json!({"tab":"space","target":{"id":"s1"}}), own, false, cx);
    assert!(!space.iter().any(|t| t.contains("icon")), "spaces have their own: {space:?}");
}

/// The picker: Suggested first, its tiles a virtual grid's rows; the chosen
/// one shown and Clear icon only while one is chosen; a pane's or a tab's.
#[gpui::test]
fn icon_picker_shows_the_sets_and_the_choice(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let picker = |p: Value, own: Value, cx: &mut TestAppContext| {
        let mut values = fixture(true, p, updates("current", "current"));
        let icon = if own.is_null() { Value::Null } else { json!({"kind": own["kind"], "text": own["value"]}) };
        with_icons(&mut values, (icon.clone(), own.clone()), (icon, own));
        describe_with_values(cx, &staged, "mods/icon-picker", values, None).unwrap()
    };
    let rocket = json!({"kind":"glyph","value":"\u{f135}"});
    let tree = picker(json!({"pane":"p1","overlay":"o1"}), Value::Null, cx);
    assert!(tree.contains("\"Icon for Build\"") && tree.contains("\"Automatic\"") && !tree.contains("Clear icon"), "{tree}");
    assert!(tree.contains("v_virtual_list \"icon-grid\" ×14"), "Suggested first: 60 glyphs and 60 emoji in rows of ten, two headings: {tree}");
    assert!(!tree.contains("icon-glyph-") && !tree.contains("icon-emoji-"), "the page built a tile: {tree}");
    assert!(tree.contains("\"its program's\""), "{tree}");
    let tree = picker(json!({"pane":"p1","overlay":"o1"}), rocket.clone(), cx);
    assert!(tree.contains("\"Chosen\"") && tree.contains("\"Clear icon\""), "{tree}");
    let emoji = json!({"kind":"emoji","value":"🐙"});
    let tree = picker(json!({"tab_id":"tab1","overlay":"o1"}), emoji, cx);
    assert!(tree.contains("\"Icon for Build\"") && tree.contains("v_virtual_list \"icon-grid\" ×14"), "a tab, on Suggested too: {tree}");
    // A tab's letter is the one Herdr works out (its focused pane's label when unnamed).
    let mut values = fixture(true, json!({"tab_id":"tab1","overlay":"o1"}), updates("current", "current"));
    let letter = json!({"kind":"letter"});
    with_icons(&mut values, (Value::Null, Value::Null), (json!({"kind":"letter","text":"Z"}), letter));
    values.get_mut("spaces").unwrap()[0]["tabs"][0]["letter"] = json!("Z");
    let tree = describe_with_values(cx, &staged, "mods/icon-picker", values, None).unwrap();
    let tile = tree.lines().skip_while(|l| !l.contains("\"icon-letter\"")).nth(2).unwrap_or_default();
    assert_eq!(tile.trim(), "text \"Z\"", "{tree}");
    let tree = picker(json!({"tab_id":"gone","overlay":"o1"}), Value::Null, cx);
    assert!(tree.contains("That pane or tab is gone."), "{tree}");
}

/// Describes a shipped mod, lets its async work (reading files) finish, and
/// describes it again: the first description and its time, the time to settle,
/// and the settled description and its time.
fn describe_settled(cx: &mut TestAppContext, staged: &Staged, entry: &str, values: BTreeMap<String, Value>, setup: Option<&str>) -> (String, Duration, Duration, String, Duration) {
    let calls = Rc::new(RefCell::new(Calls::default()));
    let policy = Policy::new()
        .with_storage_path(staged.0.join("store-settled.json"))
        .with_capabilities(Capabilities::new().storage(true).read_roots([staged.0.clone()]))
        .with_host_module(module(values, calls.clone()))
        .unwrap();
    let runtime = ShellRuntime::new_isolated().unwrap();
    let root = setup.map(|s| staged.wrapper(entry, s)).unwrap_or_else(|| staged.0.join(entry));
    let window = cx.add_window(|_, _| Empty);
    let start = Instant::now();
    let (mut session, first) = window.update(cx, |_, window, cx| {
        let mut session = DescriptionSession::load(runtime, root, policy, window, cx).unwrap();
        let first = session.describe(window, cx).unwrap();
        (session, first)
    }).unwrap();
    let open = start.elapsed();
    let start = Instant::now();
    cx.run_until_parked();
    let settle = start.elapsed();
    let start = Instant::now();
    let settled = window.update(cx, |_, window, cx| session.describe(window, cx).unwrap()).unwrap();
    let redraw = start.elapsed();
    window.update(cx, |_, window, _| window.remove_window()).unwrap();
    cx.run_until_parked();
    assert!(calls.borrow().unexpected.is_empty(), "{entry}: {:?}", calls.borrow().unexpected);
    (first, open, settle, settled, redraw)
}

/// The names are read when the picker opens and every glyph and emoji is a
/// virtual grid's row: the page builds none of them. Opening and a keystroke's
/// search stay inside a frame or so, timed as the Settings themes are.
#[gpui::test]
fn icon_picker_reads_every_name_and_searches_them_quickly(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let values = || fixture(true, json!({"pane":"p1","overlay":"o1"}), updates("current", "current"));
    // Before the names are read: Suggested, with nothing else waiting on them.
    let (first, open, settle, glyphs, _) = describe_settled(cx, &staged, "mods/icon-picker", values(), Some("this.tab = 'glyph';"));
    assert!(first.contains("\"Reading names…\""), "{first}");
    let count = glyphs.lines().find_map(|l| l.trim().strip_prefix("v_virtual_list \"icon-grid\" ×")).and_then(|n| n.split_whitespace().next()?.parse::<usize>().ok()).unwrap_or_else(|| panic!("{glyphs}"));
    assert!(count > 1000, "every glyph, in rows of ten with a heading per set: {count}");
    assert!(!glyphs.contains("icon-glyph-"), "the page built a tile");
    assert!(glyphs.contains("\"Font Awesome\"") && glyphs.contains("\"Powerline\""), "a chip per set: {glyphs}");
    let mut keystrokes = Vec::new();
    for query in ["r", "ro", "roc", "rock", "rocket", "arrow", "partly cloudy", "zzzz"] {
        let (_, _, _, found, redraw) = describe_settled(cx, &staged, "mods/icon-picker", values(), Some(&format!("this.query = {query:?};")));
        keystrokes.push(redraw);
        if query == "rocket" {
            assert!(found.contains("v_virtual_list \"icon-grid\" ×"), "{found}");
            assert!(found.contains("\"fa-rocket\"") || found.contains("\"md-rocket\"") || found.contains("\"rocket\""), "the best match is selected and named: {found}");
        }
        if query == "zzzz" {
            assert!(found.contains("\"Nothing by that name.\""), "{found}");
        }
    }
    let worst = keystrokes.iter().max().unwrap();
    eprintln!("icon picker: open {open:?}, names read and parsed {settle:?}, search + describe per keystroke worst {worst:?} (all {keystrokes:?})");
    assert!(open < Duration::from_millis(500), "open {open:?}");
    assert!(*worst < Duration::from_millis(250), "a keystroke {worst:?}");
}

/// The picker's name files are the bundled font's: every glyph is in its cmap,
/// and every glyph and emoji is a value pane.set_icon and tab.set_icon take.
#[test]
fn icon_picker_names_match_the_font_and_the_commands() {
    let file = |name: &str| -> Value {
        let bytes = DEFAULT_MODS.iter().find(|(p, _)| *p == format!("icon-picker/names/{name}")).unwrap().1;
        serde_json::from_slice(bytes).unwrap()
    };
    let face = ttf_parser::Face::parse(crate::settings::SYMBOLS_FONT_FILE, 0).unwrap();
    let glyphs = file("glyphs.json");
    let rows = glyphs["glyphs"].as_array().unwrap();
    assert!(rows.len() > 10_000, "{}", rows.len());
    let mut seen = BTreeSet::new();
    for row in rows {
        let code = u32::from_str_radix(row[1].as_str().unwrap(), 16).unwrap();
        let ch = char::from_u32(code).unwrap();
        assert!(face.glyph_index(ch).is_some(), "{row}: U+{code:X} isn't in the Symbols Nerd Font");
        assert!(seen.insert(code), "{row}: one row per code point");
        let choice = json!({"kind": "glyph", "value": ch.to_string()});
        assert!(crate::pane_icons::Choice::parse(&choice).is_ok(), "{row}");
    }
    let emoji = file("emoji.json");
    for row in emoji["emoji"].as_array().unwrap() {
        let choice = json!({"kind": "emoji", "value": row[0]});
        assert!(crate::pane_icons::Choice::parse(&choice).is_ok(), "{row}");
    }
}
