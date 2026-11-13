//! Shipped JS descriptions and native rendering regressions.
use super::*;
use gpui::TestAppContext;
use gpui_shell::{ShellRuntime, plugin::DescriptionSession, policy::Policy};
use std::{
    cell::Cell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};


#[gpui::test]
fn mod_text_selection_stays_in_its_shell_root(cx: &mut TestAppContext) {
    use gpui::*;
    use gpui_base::{SelectableText, TextSelection, TextView, TextViewState};
    use gpui_shell::ShellRoot;

    struct Column {
        texts: Vec<Entity<TextViewState>>,
        label: &'static str,
    }
    impl Render for Column {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().flex().flex_col().size_full().gap(px(20.))
                .children(self.texts.iter().map(|text| TextView::new(text).scrollable(false).selectable(true)))
                .child(SelectableText::new("plain", format!("{} plain text", self.label)).document_order(u64::MAX))
        }
    }
    struct Columns {
        roots: Vec<Entity<ShellRoot>>,
    }
    impl Render for Columns {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().flex().size_full().children(self.roots.iter().map(|root| {
                div().w(px(240.)).h_full().child(root.clone())
            }))
        }
    }
    cx.update(gpui_shell::init);
    let mut texts = Vec::new();
    let (_, cx) = cx.add_window_view(|window, cx| {
        let roots = ["LEFT", "CENTER", "RIGHT"].into_iter().map(|label| {
            let blocks = ["first", "middle", "second"].into_iter().map(|block| {
                cx.new(|cx| TextViewState::markdown(&format!("{label} {block} block"), cx))
            }).collect::<Vec<_>>();
            texts.push(blocks.clone());
            let content = cx.new(|_| Column { texts: blocks, label }).into();
            cx.new(|cx| ShellRoot::new(content, window, cx))
        }).collect();
        Columns { roots }
    });
    cx.run_until_parked();
    let bounds = |column: usize, block: usize, cx: &mut VisualTestContext| {
        texts[column][block].read_with(cx, |text, _| text.bounds())
    };
    let start = bounds(1, 0, cx).origin + point(px(1.), px(10.));
    let inside = bounds(1, 2, cx);
    let neighbor = bounds(2, 2, cx);
    for end in [
        point(inside.right() - px(1.), inside.top() + px(10.)),
        point(neighbor.right() - px(1.), neighbor.top() + px(10.)),
        point(px(900.), px(400.)),
        point(px(-20.), px(400.)),
    ] {
        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        let copied = cx.update(TextSelection::selected_text);
        assert!(copied.contains("CENTER first block"), "{copied:?}");
        assert!(copied.contains("CENTER middle block"), "end {end:?}: {copied:?}");
        assert!(copied.contains("CENTER second block"), "end {end:?}: {copied:?}");
        assert!(!copied.contains("LEFT") && !copied.contains("RIGHT"), "{copied:?}");
        if end.y == px(400.) {
            assert!(copied.contains("CENTER plain text"), "{copied:?}");
        }
        cx.simulate_mouse_move(neighbor.center(), None, Modifiers::default());
        assert_eq!(cx.update(TextSelection::selected_text), copied);
    }
}

struct Staged(PathBuf);
impl Staged {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = crate::test_tmp().join(format!(
            "herdr-render-{}-{}",
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
        ("menuItems".into(), json!([])),
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
    for name in ["run", "playSound", "setAgentOrder", "setWindowTitle", "openOverlay", "closeOverlay", "notify", "presentUpdateNotes"] {
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
    module = module.async_function("terminalLines", |_| Ok(async { Ok(HostValue::Null) }));
    module = module.function("gitStatus", |_| Ok(HostValue::Null));
    module = module.function("toastLooks", |_| Ok(HostValue::Json(std::sync::Arc::new(crate::settings::Settings::default().toast_looks()))));
    module = module.function("fitHeaderInfo", |a| {
        crate::header_info::fit_for_mods(from_host(a.value(0)?), a.number(1)? as f32, a.number(2)? as f32).map(|v| to_host(&v)).map_err(HostError::new)
    });
    module = module.function("headerInfoPreview", |a| Ok(to_host(&crate::header_info::preview_for_mods(a.string(0)?))));
    module = module.function("cardInfoPreview", |a| Ok(to_host(&crate::header_info::card_preview_for_mods(a.get(0).and_then(HostValue::as_str)))));
    module = module.function("headerTextWidth", |a| Ok(HostValue::Number(crate::header_info::width(a.string(0)?, a.number(1)? as f32) as f64)));
    module = module.function("refreshGitStatus", |_| Ok(HostValue::Null));
    for name in ["nextCommand", "nextGitStatus"] {
        module = module.async_function(name, |_| Ok(std::future::pending::<gpui_shell::HostResult>()));
    }
    module = module.cancellable_async_function("watchFolder", HostValue::Array(vec![]), |_| Ok(std::future::pending::<gpui_shell::HostResult>()));
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
                "hc" if first == json!(["machine", "add", "host", "--json"]) => {
                    Some(json!({"code":1,"stdout":"{\"error\":{\"kind\":\"host_key\",\"message\":\"unknown host key\"}}","stderr":""}))
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

#[gpui::test]
fn mod_copies_share_code_without_sharing_props_state_or_lifetime(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let root = staged.0.join("cache");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("main.js"), r#"
import { View, div } from 'gpui-kit';
import { props, mark, next } from 'copy';
const evaluation = mark();
const row = globalThis.__template(value => div().child(value));
export default class Panel extends View {
  init(_, cx) {
    if (props() === 'bad') throw Error('bad props');
    this.count = 0;
    this.count++;
    this.async = 'waiting';
    cx.spawn(async cx => { await next(); this.async = props(); cx.notify(); });
  }
  render() { return row(props() + ':' + this.count + ':' + evaluation + ':' + this.async); }
}
"#).unwrap();
    let marks = Rc::new(Cell::new(0));
    let policy = |name: &'static str| {
        let marks = marks.clone();
        Policy::new().with_host_module(HostModule::new("copy")
            .function("props", move |_| Ok(name.into()))
            .function("mark", move |_| { marks.set(marks.get() + 1); Ok((marks.get() as f64).into()) })
            .async_function("next", |_| Ok(std::future::ready(Ok(HostValue::Null))))
        ).unwrap()
    };
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|_, _| Empty);
    let (first, mut second) = window.update(cx, |_, window, cx| {
        let mut first = DescriptionSession::load(runtime.clone(), &root, policy("A"), window, cx).unwrap();
        let mut second = DescriptionSession::load(runtime.clone(), &root, policy("B"), window, cx).unwrap();
        assert!(first.describe(window, cx).unwrap().contains("A:1:1"));
        assert!(second.describe(window, cx).unwrap().contains("B:1:1"));
        assert!(DescriptionSession::load(runtime.clone(), &root, policy("bad"), window, cx).is_err());
        (first, second)
    }).unwrap();
    cx.run_until_parked();
    drop(first);
    cx.run_until_parked();
    window.update(cx, |_, window, cx| {
        let tree = second.describe(window, cx).unwrap();
        assert!(tree.contains("B:1:1:B"), "{tree}");
    }).unwrap();
    drop(second);
    cx.run_until_parked();
    window.update(cx, |_, window, cx| {
        let mut reopened = DescriptionSession::load(runtime.clone(), &root, policy("C"), window, cx).unwrap();
        assert!(reopened.describe(window, cx).unwrap().contains("C:1:1"));
    }).unwrap();
    assert_eq!(marks.get(), 1, "reopening and failed init must reuse evaluated code");
    window.update(cx, |_, window, _| window.remove_window()).unwrap();
    cx.run_until_parked();
}

#[gpui::test]
fn mod_code_revisions_share_successes_and_failures_without_poisoning_old_copies(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let root = staged.0.join("cache");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("main.js"), r#"
import { View, div } from 'gpui-kit';
import { version } from './part.js';
export default class Panel extends View { render() { return div().child('version:' + version); } }
"#).unwrap();
    std::fs::write(root.join("part.js"), "import { mark } from 'copy'; mark(); export const version = 1;").unwrap();
    let marks = Rc::new(Cell::new(0));
    let policy = || {
        let marks = marks.clone();
        Policy::new().with_host_module(HostModule::new("copy").function("mark", move |_| {
            marks.set(marks.get() + 1); Ok(HostValue::Null)
        })).unwrap()
    };
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|_, _| Empty);
    window.update(cx, |_, window, cx| {
        let mut old = DescriptionSession::load(runtime.clone(), &root, policy(), window, cx).unwrap();
        for version in [2, 3] {
            std::fs::write(root.join("part.js"), format!("import {{ mark }} from 'copy'; mark(); export const version = {version};{}", " ".repeat(version))).unwrap();
            for _ in 0..20 {
                let mut new = DescriptionSession::load(runtime.clone(), &root, policy(), window, cx).unwrap();
                assert!(new.describe(window, cx).unwrap().contains(&format!("version:{version}")));
            }
        }
        assert_eq!(marks.get(), 3);
        std::fs::write(root.join("part.js"), "import { mark } from 'copy'; mark(); throw Error('broken save'); export const version = 4;").unwrap();
        for _ in 0..20 {
            assert!(DescriptionSession::load(runtime.clone(), &root, policy(), window, cx).is_err());
        }
        assert_eq!(marks.get(), 4, "a failed revision evaluates once, not on every open");
        assert!(old.describe(window, cx).unwrap().contains("version:1"));
        std::fs::write(root.join("part.js"), "import { mark } from 'copy'; mark(); export const version = 5;").unwrap();
        let mut fixed = DescriptionSession::load(runtime.clone(), &root, policy(), window, cx).unwrap();
        assert!(fixed.describe(window, cx).unwrap().contains("version:5"));
        assert_eq!(marks.get(), 5);
        assert!(DescriptionSession::load(runtime.clone(), &root, Policy::new(), window, cx).is_err(),
            "an ungranted host module must not reuse another policy's linked class");
    }).unwrap();
    window.update(cx, |_, window, _| window.remove_window()).unwrap();
    cx.run_until_parked();
}

#[gpui::test]
fn mod_host_imports_keep_sync_async_and_missing_export_surfaces_distinct(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let root = staged.0.join("cache");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("main.js"), r#"
import { View, div } from 'gpui-kit';
import { value } from 'copy';
export default class Panel extends View {
  init() { this.kind = typeof value(); }
  render() { return div().child('kind:' + this.kind); }
}
"#).unwrap();
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|_, _| Empty);
    window.update(cx, |_, window, cx| {
        let sync = HostModule::new("copy").function("value", |_| Ok(HostValue::Number(1.0)));
        let mut a = DescriptionSession::load(runtime.clone(), &root, Policy::new().with_host_module(sync).unwrap(), window, cx).unwrap();
        assert!(a.describe(window, cx).unwrap().contains("kind:number"));
        let asynchronous = HostModule::new("copy").async_function("value", |_| Ok(std::future::ready(Ok(HostValue::Null))));
        let mut b = DescriptionSession::load(runtime.clone(), &root, Policy::new().with_host_module(asynchronous).unwrap(), window, cx).unwrap();
        assert!(b.describe(window, cx).unwrap().contains("kind:object"));
        let missing = HostModule::new("copy").function("other", |_| Ok(HostValue::Null));
        assert!(DescriptionSession::load(runtime.clone(), &root, Policy::new().with_host_module(missing).unwrap(), window, cx).is_err());
        assert!(a.describe(window, cx).unwrap().contains("kind:number"));
    }).unwrap();
    window.update(cx, |_, window, _| window.remove_window()).unwrap();
    cx.run_until_parked();
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

/// A mod's localStorage write lands on the executor; one that lands after the
/// staged folder's drop recreates it, and the folder outlives the test.
#[gpui::test]
fn a_described_mod_leaves_no_folder_behind(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let path = staged.0.clone();
    describe(cx, &staged, "mods/agents", true, props(), updates("current", "current"), None).unwrap();
    drop(staged);
    cx.run_until_parked();
    assert!(!path.exists(), "left behind: {:?}", std::fs::read_dir(&path).map(|d| d.flatten().map(|e| e.file_name()).collect::<Vec<_>>()));
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

struct RootedView(gpui::Entity<gpui_shell::ShellRoot>);
impl gpui::Render for RootedView {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        self.0.clone()
    }
}

#[gpui::test]
fn a_mod_hears_a_pinch_only_over_its_element(cx: &mut TestAppContext) {
    use gpui::{Modifiers, PinchEvent, TouchPhase, VisualTestContext, point, px};

    cx.update(gpui_shell::init);
    let staged = Staged::new();
    std::fs::write(staged.0.join("main.js"), r#"
import { View, div } from "gpui-kit";
import { v_flex } from "gpui-base";
export default class Demo extends View {
  init() { this.log = []; }
  render() {
    return v_flex().w(300).h(200)
      .child(div().w(100).h(50).on_pinch((event, cx) => {
        this.log.push(`${event.delta}:${event.touch_phase}:${event.local_position.x}:${event.modifiers.platform}`);
        cx.notify();
      }))
      .child(div().child(`log=${this.log.join(" ")}`));
  }
}
"#).unwrap();
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|window, cx| {
        RootedView(runtime.try_load(&staged.0, window, cx).unwrap())
    });
    let mut context = VisualTestContext::from_window(window.into(), cx);
    context.update(|window, cx| window.draw(cx).clear(cx));
    let view: gpui::Entity<gpui_shell::ScriptView> = window
        .root(&mut context)
        .unwrap()
        .read_with(&context, |root, cx| root.0.read(cx).content().clone().downcast().unwrap());

    let pinch = |x: f32, delta: f32, phase: TouchPhase, platform: bool| PinchEvent {
        position: point(px(x), px(20.)),
        delta,
        modifiers: Modifiers { platform, ..Modifiers::default() },
        phase,
    };
    // A hitbox counts as hovered only once the last input is the pointer.
    context.simulate_mouse_move(point(px(20.), px(20.)), None, Modifiers::default());
    context.simulate_event(pinch(20., 0.5, TouchPhase::Moved, true));
    // Outside the element: GPUI only calls the listener on its hitbox.
    context.simulate_event(pinch(250., 0.25, TouchPhase::Moved, false));
    context.simulate_event(pinch(30., -0.25, TouchPhase::Ended, false));
    context.run_until_parked();
    context.update(|window, cx| window.draw(cx).clear(cx));

    let tree = context.update(|_, cx| {
        view.read(cx).snapshot().map(gpui_shell::RenderSnapshot::debug_tree).unwrap_or_default()
    });
    assert!(tree.contains("log=0.5:moved:20:true -0.25:ended:30:false"), "{tree}");
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
fn pane_header_divider_faces_the_terminal(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    for (position, reveal) in [("bottom", "always"), ("top", "always"), ("", "always"), ("bottom", "hover"), ("top", "hover")] {
        let mut values = fixture(true, props(), updates("current", "current"));
        values.insert("layout".into(), json!({
            "id": "root", "type": "column", "children": [
                {"id": "hidden", "type": "block", "hidden": true,
                 "content": {"tab": "active", "pane": {"header": {"position": "bottom"}}}},
                {"id": "stack", "type": "stack", "active": "shown", "children": [
                    {"id": "inactive", "type": "block",
                     "content": {"tab": "active", "pane": {"header": {"position": "bottom"}}}},
                    {"id": "shown", "type": "block",
                     "content": {"tab": "active", "pane": {"header": {"position": position, "reveal": reveal}}}}
                ]}
            ]
        }));
        if position.is_empty() {
            values.get_mut("layout").unwrap()["children"][1]["children"][1]["content"]["pane"]["header"].as_object_mut().unwrap().remove("position");
        }
        let tree = describe_with_values(cx, &staged, "mods/pane", values, None).unwrap();
        let head = tree.lines().find(|l| l.contains("pane-head-")).unwrap();
        let (edge, outside) = if position == "bottom" { ("t", "b") } else { ("b", "t") };
        assert!(head.contains(&format!(".border_{edge}[Number(1.0)]")), "{position}: {head}");
        assert!(!head.contains(&format!(".border_{outside}[")), "{position}: {head}");
    }
}

#[gpui::test]
fn layout_previews_stay_inside_their_cards(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let mut p = props();
    p["section"] = json!("layout");
    let mut values = fixture(true, p, updates("current", "current"));
    values.insert("layouts".into(), json!(["main"]));
    let tree = describe_with_values(
        cx, &staged, "frame/settings", values,
        Some("this.previews = { main: 'layouts/portrait.png' }; this.previewsOf = 'main';"),
    ).unwrap();
    let lines: Vec<&str> = tree.lines().collect();
    let image = lines.iter().position(|l| l.contains("layouts/portrait.png")).unwrap();
    assert!(lines[image].contains(".h[Number(62.0)]"), "a preview keeps the card's height: {}", lines[image]);
    assert!(lines[image - 1].contains(".overflow_hidden"), "a preview cannot spill out of its card: {}", lines[image - 1]);
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
fn long_space_names_use_the_sidebar_width(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let name = "quarterly infrastructure cost review";
    let mut values = fixture(true, props(), updates("current", "current"));
    let spaces = values.get_mut("spaces").unwrap().as_array_mut().unwrap();
    spaces[0]["name"] = json!(name);
    // A worktree of the first space's repo, drawn nested under it.
    spaces.insert(1, json!({"id":"s3","index":2,"name":"portal rewrite","folder":"/fixture/portal","machine":null,"branch":null,"active":false,"status":"idle","tabs":[]}));
    // The remote space is on main in a repo named after it: its tail has nothing to show.
    let git = "this.git[JSON.stringify([null, '/fixture/project'])] = { name: 'herdr', branch: 'feature/rate-limiting', ahead: 0, behind: 0, repo: JSON.stringify([null, '/fixture/project/.git']), linked: false };
        this.git[JSON.stringify([null, '/fixture/portal'])] = { name: 'herdr', branch: 'portal-rewrite', ahead: 0, behind: 0, repo: JSON.stringify([null, '/fixture/project/.git']), linked: true };
        this.git[JSON.stringify(['remote', '/remote/project'])] = { name: 'remote', branch: 'main', ahead: 0, behind: 0, repo: JSON.stringify(['remote', '/remote/project/.git']), linked: false };";
    let tree = describe_with_values(cx, &staged, "mods/spaces", values, Some(git)).unwrap();
    let lines: Vec<&str> = tree.lines().collect();
    let text = lines.iter().position(|l| l.contains(&format!("text {name:?}"))).unwrap();
    let label = lines[text - 1];
    assert!(!label.contains(".max_w"), "a wider sidebar must show more of the name: {label}");
    assert!(!label.contains(".flex_shrink[Number(0.0)]"), "the name must give way when the row is full: {label}");
    assert!(label.contains(".min_w[Number(48.0)]"), "the name shrinks, but a short one stays readable: {label}");
    let tail = lines[text + 1..].iter().find(|l| l.contains(".justify_end")).unwrap();
    assert!(tail.contains(".flex_auto") && !tail.contains(".flex_1"), "the git tail must start from its text's width, so both sides give way in proportion: {tail}");
    assert!(tail.contains(".min_w[Number(48.0)]"), "the git tail shrinks, but a short one stays readable: {tail}");
    let remote = lines.iter().position(|l| l.contains("text \"Remote\"")).unwrap();
    let empty = lines[remote + 1..].iter().find(|l| l.contains(".justify_end")).unwrap();
    assert!(empty.contains(".min_w[Number(0.0)]"), "an empty git tail must not take room from the name: {empty}");
    let icon = lines.iter().find(|l| l.contains("branch.svg")).expect("the worktree space is nested");
    assert!(icon.contains(".flex_none"), "a worktree's icon keeps its size while the name gives way: {icon}");
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
fn machine_logout_warning_tracks_current_remote_state(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    for state in ["false", "true", "null", "undefined"] {
        let setup = format!("this.hosts = []; this.added = [{{name:'cloud', target:'host', keeps_running_after_logout:{state}, logout_linger_user:'-alice'}}];");
        let tree = describe(cx, &staged, "mods/machine-add", false, props(), updates("current", "current"), Some(&setup)).unwrap();
        assert_eq!(tree.contains("Agents on cloud stop when you disconnect"), state == "false", "{tree}");
        assert_eq!(tree.contains("ma-logout-copy-cloud"), state == "false", "{tree}");
        if state == "false" { assert!(tree.contains("sudo loginctl enable-linger -- -alice"), "{tree}"); }
        let setup = format!("this.machines = [{{name:'cloud', target:'host', state:'connected', keeps_running_after_logout:{state}}}];");
        let tree = describe(cx, &staged, "mods/spaces", false, props(), updates("current", "current"), Some(&setup)).unwrap();
        assert_eq!(tree.contains("machine-logout-cloud"), state == "false", "{tree}");
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
fn machine_add_needing_input_keeps_the_picker_and_shows_instructions(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let calls = Rc::new(RefCell::new(Calls::default()));
    let policy = Policy::new().with_host_module(module(fixture(false, props(), updates("current", "current")), calls.clone())).unwrap();
    let root = staged.wrapper("mods/machine-add", "this.typed = 'host'; this.add('host', cx);");
    let window = cx.add_window(|_, _| Empty);
    let mut session = window.update(cx, |_, window, cx| {
        DescriptionSession::load(ShellRuntime::new_isolated().unwrap(), root, policy, window, cx).unwrap()
    }).unwrap();
    cx.run_until_parked();
    window.update(cx, |_, window, cx| {
        let tree = session.describe(window, cx).unwrap();
        assert!(tree.contains("machine-add"), "{tree}");
        assert!(tree.contains("ma-message-host"), "{tree}");
        assert!(tree.contains("hc machine add host"), "{tree}");
        window.remove_window();
    }).unwrap();
    let calls = calls.borrow();
    assert!(calls.unexpected.is_empty(), "{:?}", calls.unexpected);
    assert!(!calls.effects.iter().any(|(name, _)| name == "openOverlay" || name == "closeOverlay"), "the picker stays open without a setup dialog");
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
    use gpui::{Styled as _, http_client::{AsyncBody, FakeHttpClient, Response}};
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
    let (ready, waiting) = async_channel::unbounded::<()>();
    let client = FakeHttpClient::create(move |_| {
        let bytes = bytes.clone();
        let waiting = waiting.clone();
        async move {
            waiting.recv().await.unwrap();
            Ok(Response::builder().status(200).body(AsyncBody::from(bytes)).unwrap())
        }
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
            let root = runtime.try_load(&staged.0, window, cx).unwrap();
            root.read(cx).content().clone()
        });
        struct AnimationRoot(gpui::AnyView);
        impl gpui::Render for AnimationRoot {
            fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
                self.0.clone().cached(gpui::StyleRefinement::default().size_full())
            }
        }
        // Load while inactive, then tick frames explicitly so the executor can park.
        context.deactivate_window();
        let root = context.update(|window, cx| window.replace_root(cx, |_, _| AnimationRoot(view.clone())));
        context.run_until_parked();
        // A cached subtree must keep its pending load across an unrelated parent draw.
        context.update(|_, cx| root.update(cx, |_, cx| cx.notify()));
        context.run_until_parked();
        ready.try_send(()).unwrap();
        context.run_until_parked();
        let resource = gpui::Resource::Uri(url.into());
        let still: Arc<gpui::RenderImage> = context.update(|_, cx| cx.fetch_asset::<gpui::ImgResourceLoader>(&resource).unwrap().unwrap());
        assert_eq!(still.frame_count(), 1);
        let data = context.update(|window, _| window.displayed_images().into_iter().find(|image| image.frame_count() == 2)).unwrap();
        assert_eq!(data.as_bytes(0).unwrap().as_ptr(), still.as_bytes(0).unwrap().as_ptr());
        context.update(|window, cx| {
            window.replace_root(cx, |_, _| Empty);
            window.activate_window();
        });
        context.run_until_parked();
        context.update(|window, cx| { window.replace_root(cx, |_, _| AnimationRoot(view.clone())); });
        let draw = |context: &mut gpui::VisualTestContext| {
            context.update(|window, cx| {
                window.simulate_next_frame(cx);
                window.draw(cx).clear(cx);
            });
        };
        draw(context);
        assert!(context.update(|window, _| window.is_window_active()));
        assert_eq!(data.frame_count(), 2);
        assert!(context.update(|window, _| window.has_image_frame_atlas_entry(&data, 0)));
        assert!(!context.update(|window, _| window.has_image_frame_atlas_entry(&data, 1)));
        // GPUI's image clock is wall time, not the test executor's clock.
        std::thread::sleep(Duration::from_millis(25));
        draw(context);
        assert!(context.update(|window, _| window.has_image_frame_atlas_entry(&data, 1)), "the next animation frame must paint (explicit id: {explicit})");
        assert!(context.update(|window, _| window.has_image_frame_atlas_entry(&data, 0)));
        std::thread::sleep(Duration::from_millis(25));
        draw(context);
        assert!(context.update(|window, _| window.has_image_frame_atlas_entry(&data, 0)));
        assert!(context.update(|window, _| window.has_image_frame_atlas_entry(&data, 1)));
        context.update(|window, cx| { window.replace_root(cx, |_, _| Empty); });
        context.deactivate_window();
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

/// The pane header of a working agent, at the default `working_spinner` and
/// without `pane_header_info`, is the header drawn before those settings
/// existed, line for line (the ring), plus its right-click menu.
#[gpui::test]
fn pane_header_draws_the_ring_by_default_as_it_always_has(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let tree = describe(cx, &staged, "mods/pane", true, json!({"pane":"p1","terminal":"t1"}), updates("current", "current"), None).unwrap();
    assert_eq!(tree.trim_end(), r##"h_flex :id[Str("pane-head-p1")] .size_full .px[Number(10.0)] .gap[Number(8.0)] .items_center .bg[Str("#17191d")] .border_b[Number(1.0)] .border_color[Str("#34363a")] .text_size[Number(12.0)] :on_hover(fn) :on_click(fn) :on_mouse_down_right(fn)
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

/// The Changes panel draws its diff as terminal text (`terminal_text()`), at
/// the size that lands on the terminals' own under the panels' text scale
/// (`settings().terminal_font`), in rows as tall as the terminals' line
/// height makes theirs.
#[gpui::test]
fn changes_diff_follows_the_terminal_font(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let mut values = fixture(false, props(), updates("current", "current"));
    let person = json!({"font": "Fixture Mono", "font_size": 15, "font_weight": 400, "line_height": 1.2, "ui_text_scale": 1.25, "ui_font_weight": 500});
    values.insert(
        "settings".into(),
        crate::settings::Settings::compose(
            &serde_json::from_value(person).unwrap(),
            &crate::layout::Look::default(),
            true,
        )
        .for_mods(),
    );
    let tree = describe_with_values(
        cx,
        &staged,
        "mods/changes",
        values,
        Some(r#"this.setLines(["@@ -1 +1 @@", "+let x = 1;"]);"#),
    )
    .unwrap();
    let diff = tree
        .lines()
        .find(|l| l.contains(".terminal_text"))
        .unwrap_or_else(|| panic!("no terminal text: {tree}"));
    // The virtual list's rows aren't described; the line box is the row's
    // height over the text's, 24 px over 15 (12 at text scale 1.25).
    for style in [".text_size[Number(12.0)]", ".line_height[Number(1.6)]"] {
        assert!(diff.contains(style), "diff lacks {style}: {diff}");
    }
    assert!(
        !diff.contains(".font_family[") && !diff.contains(".font_weight["),
        "terminal_text() brings the family and weight: {diff}"
    );
    assert!(
        tree.contains(r#"v_virtual_list "changes-diff" ×2"#),
        "both diff lines are rows: {tree}"
    );

    // Builds without `terminal_font`: terminal text at the old 11 px, no line box.
    let mut values = fixture(false, props(), updates("current", "current"));
    let mut settings = crate::settings::Settings::default().for_mods();
    settings.as_object_mut().unwrap().remove("terminal_font");
    values.insert("settings".into(), settings);
    let tree = describe_with_values(
        cx,
        &staged,
        "mods/changes",
        values,
        Some(r#"this.setLines(["+x"]);"#),
    )
    .unwrap();
    let diff = tree
        .lines()
        .find(|l| l.contains(".terminal_text"))
        .unwrap_or_else(|| panic!("no terminal text: {tree}"));
    assert!(
        diff.contains(".text_size[Number(11.0)]") && !diff.contains(".line_height["),
        "{diff}"
    );
}

/// `terminal_text()` text takes the window's terminal look: its font under
/// the element's own font styles, shaped with no weight shift (text measured
/// when the root lays out too), and its glyph rendering, for the whole
/// subtree; text beside it keeps the window's.
#[test]
fn terminal_text_takes_the_windows_terminal_look() {
    use gpui::{ParentElement as _, Styled as _};
    type Seen = Rc<RefCell<Vec<(gpui::Font, f32, (gpui::GlyphThicken, f32, bool))>>>;
    struct Root(Seen);
    impl gpui::Render for Root {
        fn render(
            &mut self,
            window: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            window.set_font_weight_shift(100.);
            window.set_glyph_thicken(gpui::GlyphThicken::Strength(192));
            window.set_terminal_text(Some(gpui::TerminalText {
                font: gpui::Font {
                    family: "Fixture Mono".into(),
                    features: crate::terminal::ligature_features(true),
                    fallbacks: Some(gpui::FontFallbacks::from_fonts(vec!["Menlo".into()])),
                    weight: gpui::FontWeight(350.),
                    style: Default::default(),
                },
                thicken: gpui::GlyphThicken::Strength(30),
                contrast: 0.3,
                subpixel: true,
            }));
            let probe = |seen: Seen| {
                gpui::canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        seen.borrow_mut().push((
                            window.text_style().font(),
                            window.text_system().weight_shift(),
                            window.glyph_look(),
                        ))
                    },
                )
                .size_full()
            };
            gpui::div()
                .size_full()
                .font_family("Panel Sans")
                .child(
                    gpui::div()
                        .h(gpui::px(10.))
                        .terminal_text()
                        .child(gpui::div().size_full().child(probe(self.0.clone()))),
                )
                .child(
                    gpui::div()
                        .h(gpui::px(10.))
                        .terminal_text()
                        .font_weight(gpui::FontWeight(700.))
                        .child(probe(self.0.clone())),
                )
                .child(gpui::div().h(gpui::px(10.)).child(probe(self.0.clone())))
                .child(gpui::div().terminal_text().child("terminal"))
                .child(gpui::div().child("panel"))
        }
    }
    let text = std::sync::Arc::new(RecordingText::default());
    let cx = &mut TestAppContext::build_with_text_system(
        gpui::TestDispatcher::new(0),
        None,
        text.clone(),
    );
    let seen: Seen = Default::default();
    let window = cx.add_window(|_, _| Root(seen.clone()));
    cx.run_until_parked();
    let seen = seen.borrow().clone();
    assert_eq!(seen.len(), 3, "{seen:?}");
    let terminal_look = (gpui::GlyphThicken::Strength(30), 0.3, true);
    let (
        (nested, nested_shift, nested_look),
        (bold, bold_shift, bold_look),
        (panel, panel_shift, panel_look),
    ) = (&seen[0], &seen[1], &seen[2]);
    assert_eq!(
        (
            nested.family.as_ref(),
            nested.weight,
            *nested_shift,
            *nested_look
        ),
        ("Fixture Mono", gpui::FontWeight(350.), 0., terminal_look),
        "a child of terminal text is terminal text"
    );
    assert_eq!(
        nested
            .fallbacks
            .as_ref()
            .map(|f| f.fallback_list().to_vec()),
        Some(vec!["Menlo".to_string()])
    );
    assert_eq!(nested.features, crate::terminal::ligature_features(true));
    assert_eq!(
        (bold.family.as_ref(), bold.weight, *bold_shift, *bold_look),
        ("Fixture Mono", gpui::FontWeight(700.), 0., terminal_look),
        "the element's own weight wins"
    );
    assert_eq!(
        (
            panel.family.as_ref(),
            panel.fallbacks.clone(),
            *panel_shift,
            panel_look.0
        ),
        ("Panel Sans", None, 100., gpui::GlyphThicken::Strength(192))
    );
    // The strings are measured when the root lays out, outside the scopes.
    // The line wrapper resolves the unshifted font either way: the check that
    // no terminal text shaped at 450 (350 + the shift) is the one that counts.
    let shaped = text.fonts.lock().unwrap().clone();
    assert!(
        shaped
            .iter()
            .any(|f| f.family.as_ref() == "Fixture Mono" && f.weight == gpui::FontWeight(350.)),
        "terminal text shapes unshifted: {shaped:?}"
    );
    assert!(
        !shaped
            .iter()
            .any(|f| f.family.as_ref() == "Fixture Mono" && f.weight == gpui::FontWeight(450.)),
        "{shaped:?}"
    );
    assert!(
        shaped
            .iter()
            .any(|f| f.family.as_ref() == "Panel Sans" && f.weight == gpui::FontWeight(500.)),
        "panel text keeps the shift: {shaped:?}"
    );
    window
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
}

/// A text system that gives each font it resolves its own id and records,
/// for every glyph it is asked to raster, the font (as shaped, its weight
/// shifted) and the thickening. Lines shape in their first run's font.
#[derive(Default)]
struct RecordingText {
    fonts: std::sync::Mutex<Vec<gpui::Font>>,
    glyphs: std::sync::Mutex<Vec<(gpui::FontId, gpui::GlyphThicken)>>,
}

impl gpui::PlatformTextSystem for RecordingText {
    fn add_fonts(&self, _: Vec<std::borrow::Cow<'static, [u8]>>) -> anyhow::Result<()> {
        Ok(())
    }
    fn all_font_names(&self) -> Vec<String> {
        Vec::new()
    }
    fn font_id(&self, font: &gpui::Font) -> anyhow::Result<gpui::FontId> {
        let mut fonts = self.fonts.lock().unwrap();
        let index = fonts.iter().position(|f| f == font).unwrap_or_else(|| {
            fonts.push(font.clone());
            fonts.len() - 1
        });
        Ok(gpui::FontId(index))
    }
    fn font_metrics(&self, id: gpui::FontId) -> gpui::FontMetrics {
        gpui::NoopTextSystem.font_metrics(id)
    }
    fn typographic_bounds(
        &self,
        id: gpui::FontId,
        glyph: gpui::GlyphId,
    ) -> anyhow::Result<gpui::Bounds<f32>> {
        gpui::NoopTextSystem.typographic_bounds(id, glyph)
    }
    fn advance(&self, id: gpui::FontId, glyph: gpui::GlyphId) -> anyhow::Result<gpui::Size<f32>> {
        gpui::NoopTextSystem.advance(id, glyph)
    }
    fn glyph_for_char(&self, id: gpui::FontId, ch: char) -> Option<gpui::GlyphId> {
        gpui::NoopTextSystem.glyph_for_char(id, ch)
    }
    fn glyph_raster_bounds(
        &self,
        params: &gpui::RenderGlyphParams,
    ) -> anyhow::Result<gpui::Bounds<gpui::DevicePixels>> {
        self.glyphs
            .lock()
            .unwrap()
            .push((params.font_id, params.thicken));
        Ok(Default::default())
    }
    fn rasterize_glyph(
        &self,
        params: &gpui::RenderGlyphParams,
        bounds: gpui::Bounds<gpui::DevicePixels>,
    ) -> anyhow::Result<(gpui::Size<gpui::DevicePixels>, Vec<u8>)> {
        gpui::NoopTextSystem.rasterize_glyph(params, bounds)
    }
    fn layout_line(
        &self,
        text: &str,
        font_size: gpui::Pixels,
        runs: &[gpui::FontRun],
    ) -> gpui::LineLayout {
        let mut layout = gpui::NoopTextSystem.layout_line(text, font_size, runs);
        for run in &mut layout.runs {
            run.font_id = runs.first().map_or(run.font_id, |r| r.font_id);
        }
        layout
    }
    fn recommended_rendering_mode(
        &self,
        id: gpui::FontId,
        size: gpui::Pixels,
    ) -> gpui::TextRenderingMode {
        gpui::NoopTextSystem.recommended_rendering_mode(id, size)
    }
}

/// The diff's glyphs draw as a terminal's: the terminals' family, unshifted
/// weight, fallbacks, ligature features and `font_thicken` strength. The
/// panel's own text beside it keeps the panels' look.
#[test]
fn changes_diff_draws_as_terminal_text() {
    let text = std::sync::Arc::new(RecordingText::default());
    let cx = &mut TestAppContext::build_with_text_system(
        gpui::TestDispatcher::new(0),
        None,
        text.clone(),
    );
    cx.update(gpui_shell::init);
    let original = crate::settings::get();
    let person = json!({
        "font": "Fixture Mono", "font_weight": 400, "font_fallbacks": ["ZedMono Nerd Font Mono", "AdwaitaMono Nerd Font Mono"], "font_ligatures": true,
        "font_thicken": true, "font_thicken_strength": 30, "ui_font_thicken": true, "ui_font_thicken_strength": 192, "ui_font_weight": 500,
    });
    let settings = crate::settings::Settings::compose(
        &serde_json::from_value(person).unwrap(),
        &crate::layout::Look::default(),
        true,
    );
    crate::settings::set(settings.clone());
    let staged = Staged::new();
    let mut values = fixture(false, props(), updates("current", "current"));
    values.insert("settings".into(), settings.for_mods());
    let calls = Rc::new(RefCell::new(Calls::default()));
    let policy = Policy::new()
        .with_storage_path(staged.0.join("store.json"))
        .with_capabilities(
            Capabilities::new()
                .storage(true)
                .read_roots([staged.0.clone()]),
        )
        .with_host_module(module(values, calls.clone()))
        .unwrap();
    let root = staged.wrapper(
        "mods/changes",
        r#"this.setLines(["@@ -1 +1 @@", "+let x = 1;"]);"#,
    );
    struct Panel(DescriptionSession);
    impl gpui::Render for Panel {
        fn render(
            &mut self,
            window: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            crate::window::set_text_look(window);
            self.0.view()
        }
    }
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|window, cx| {
        Panel(DescriptionSession::load(runtime, root, policy, window, cx).unwrap())
    });
    let handle: gpui::AnyWindowHandle = window.into();
    cx.simulate_window_frame_request(handle);

    let fonts = text.fonts.lock().unwrap().clone();
    let mut drawn: Vec<_> = text
        .glyphs
        .lock()
        .unwrap()
        .iter()
        .map(|(id, thicken)| (id.0, *thicken))
        .collect();
    drawn.dedup();
    let font = |id: usize| &fonts[id];
    let (terminal, panel): (Vec<_>, Vec<_>) = drawn
        .iter()
        .partition(|(id, _)| font(*id).family.as_ref() == "Fixture Mono");
    assert!(
        !terminal.is_empty() && !panel.is_empty(),
        "both the diff and the panel draw text: {fonts:?}"
    );
    for (id, thicken) in &terminal {
        let f = font(*id);
        assert_eq!(*thicken, gpui::GlyphThicken::Strength(30), "{f:?}");
        assert_eq!(
            f.fallbacks.as_ref().map(|f| f.fallback_list().to_vec()),
            Some(vec![
                "ZedMono Nerd Font Mono".to_string(),
                "AdwaitaMono Nerd Font Mono".to_string()
            ])
        );
        assert_eq!(f.features, crate::terminal::ligature_features(true));
        assert_eq!(f.weight, gpui::FontWeight(400.), "no ui_font_weight shift");
    }
    for (id, thicken) in &panel {
        let f = font(*id);
        assert_eq!(
            (*thicken, &f.fallbacks),
            (gpui::GlyphThicken::Strength(192), &None),
            "{f:?}"
        );
        assert_ne!(f.features, crate::terminal::ligature_features(true));
        assert!(f.weight.0 > 400., "the panels' +100: {f:?}");
    }
    let _ = handle.update(cx, |_, window, _| window.remove_window());
    cx.run_until_parked();
    crate::settings::set((*original).clone());
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

/// The pane menu's copy group: Copy only over a selection, Copy last output
/// only with a finished command's marked output, Copy path only with a known
/// folder, Copy all and Paste always, all above Split.
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
    assert_eq!(all[..6], ["\"Copy\"", "\"Copy all\"", "\"Paste\"", "\"Copy path\"", "\"Rename\"", "\"Split right\""], "{all:?}");
    let bare = labels(json!({"pane":"p1","terminal":"t1","selected":false,"cwd":null,"last_output":false}), cx);
    assert_eq!(bare[..4], ["\"Copy all\"", "\"Paste\"", "\"Rename\"", "\"Split right\""], "{bare:?}");
    // A finished command's output the shell marked (OSC 133): next to Copy.
    let output = labels(json!({"pane":"p1","terminal":"t1","selected":true,"cwd":"/fixture/project","last_output":true}), cx);
    assert_eq!(output[..6], ["\"Copy\"", "\"Copy last output\"", "\"Copy all\"", "\"Paste\"", "\"Copy path\"", "\"Rename\""], "{output:?}");
    let alone = labels(json!({"pane":"p1","terminal":"t1","selected":false,"cwd":null,"last_output":true}), cx);
    assert_eq!(alone[..3], ["\"Copy last output\"", "\"Copy all\"", "\"Paste\""], "{alone:?}");
    // No such terminal: nothing to copy from.
    let gone = labels(json!({"pane":"p1","terminal":"t9","selected":true,"cwd":"/x","last_output":true}), cx);
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
    let tiled = items(pane.clone(), Value::Null, false, cx);
    assert_eq!(tiled[tiled.iter().position(|t| t == "\"Change icon…\"").unwrap() + 1], "\"Float pane\"", "a tiled pane can float: {tiled:?}");
    // A terminal block outside a tab has no pane to give an icon.
    assert!(!items(json!({"tab":"pane","target":{"pane":"block","terminal":"t1"}}), Value::Null, false, cx).iter().any(|t| t.contains("icon")));
    let space = items(json!({"tab":"space","target":{"id":"s1"}}), own, false, cx);
    assert!(!space.iter().any(|t| t.contains("icon")), "spaces have their own: {space:?}");
}

/// Spaces on this machine besides the fixture's: Docs (two tabs) and Ops.
fn with_more_local_spaces(values: &mut BTreeMap<String, Value>) {
    let tab = |id: &str, name: &str, index: u64| json!({"id":id,"index":index,"name":name,"active":index == 1,"status":null,"panes":1,"zoomed":false,"settled":false,"settled_at":null,"changed_at":null,"working_since":null});
    let spaces = values.get_mut("spaces").unwrap().as_array_mut().unwrap();
    spaces.push(json!({"id":"s3","index":3,"name":"Docs","folder":"/fixture/docs","machine":null,"branch":null,"active":false,"status":null,"tabs":[tab("tab3", "Write", 1), tab("tab4", "", 2)]}));
    spaces.push(json!({"id":"s4","index":4,"name":"Ops","folder":"/fixture/ops","machine":null,"branch":null,"active":false,"status":null,"tabs":[tab("tab5", "Logs", 1)]}));
}

/// Move to space… in a tab's menu, right after the moves within its space,
/// only when the tab's machine has another space: the fixture's other space is
/// on another machine, and a tab's terminals stay on their own.
#[gpui::test]
fn tab_menu_offers_move_to_space_only_with_another_space_on_its_machine(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let items = |id: &str, more: bool, cx: &mut TestAppContext| {
        let mut values = fixture(true, json!({"tab":"tab","target":{"id":id,"space":"s1"}}), updates("current", "current"));
        if more {
            with_more_local_spaces(&mut values);
        }
        let tree = describe_with_values(cx, &staged, "mods/menu", values, None).unwrap();
        let lines: Vec<&str> = tree.lines().collect();
        let labels = lines.windows(2).filter(|w| w[0].trim() == "div .flex_1").filter_map(|w| w[1].trim().strip_prefix("text ")).map(|t| t.trim_matches('"').to_owned()).collect::<Vec<_>>();
        (labels, tree)
    };
    let (alone, _) = items("tab1", false, cx);
    assert!(!alone.iter().any(|t| t == "Move to space…"), "{alone:?}");
    let (local, tree) = items("tab1", true, cx);
    assert_eq!(local[..3], ["Rename", "Move to space…", "Change icon…"], "{local:?}");
    assert!(tree.contains("mods/workspace-chrome/layers.svg"), "{tree}");
    let (second, _) = items("tab4", true, cx);
    assert_eq!(second[..4], ["Rename", "Move left", "Move to space…", "Change icon…"], "{second:?}");
    let (remote, _) = items("tab2", true, cx);
    assert!(!remote.iter().any(|t| t == "Move to space…"), "the remote space is alone on its machine: {remote:?}");
}

/// The tab-move picker: the other spaces of the tab's machine, numbered, with
/// their tab counts; a search keeps the matching ones; none to go to says so.
#[gpui::test]
fn tab_move_picker_lists_the_other_spaces_of_the_tabs_machine(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let tree = |more: bool, setup: Option<&str>, cx: &mut TestAppContext| {
        let mut values = fixture(true, json!({"id":"tab1","overlay":"o1"}), updates("current", "current"));
        if more {
            with_more_local_spaces(&mut values);
        }
        describe_with_values(cx, &staged, "mods/tab-move", values, setup).unwrap()
    };
    let all = tree(true, None, cx);
    for text in ["Move Build to a space", "Docs", "2 tabs", "Ops", "1 tab", "Type to search spaces", "1-9 or Enter moves it there · / or type to search · Esc closes"] {
        assert!(all.contains(&format!("text {text:?}")), "{text}: {all}");
    }
    assert!(!all.contains("text \"Local\"") && !all.contains("text \"Remote\""), "not its own space, nor another machine's: {all}");
    let found = tree(true, Some("this.search(\"op\");"), cx);
    assert!(found.contains("text \"Ops\"") && !found.contains("text \"Docs\""), "{found}");
    assert!(found.contains("text \"op▏\""), "the query shows: {found}");
    let none = tree(false, None, cx);
    assert!(none.contains("text \"No other space on this machine.\""), "{none}");
}

/// Mods' terminal menu items: after Herdr's own, before Close pane, one group
/// per mod under a divider, each with its `when` and its target.
#[gpui::test]
fn pane_menu_lists_mods_items_in_groups(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let tree = |target: Value, cx: &mut TestAppContext| {
        let mut values = fixture(true, json!({"tab":"pane","target":target}), updates("current", "current"));
        values.insert("menuItems".into(), json!([
            {"mod":"mover","title":"Move to…","command":"mod.open","args":{"name":"mover"},"when":"pane","icon":"mods/mover/move.svg","fill":["props"]},
            {"mod":"mover","title":"Split under","command":"pane.split","args":{"direction":"down"},"when":null,"icon":null,"fill":["pane"]},
            {"mod":"grab","title":"Grab selection","command":"terminal.copy","args":{},"when":"selected","icon":null,"fill":["terminal"]},
            {"mod":"grab","title":"Open folder","command":"terminal.copy_path","args":{},"when":"cwd","icon":null,"fill":["terminal"]},
            {"mod":"grab","title":"Zoom it","command":"pane.zoom","args":{},"when":"cwd","icon":null,"fill":["pane"]},
        ]));
        describe_with_values(cx, &staged, "mods/menu", values, None).unwrap()
    };
    let labels = |tree: &str| {
        let lines: Vec<&str> = tree.lines().collect();
        lines.windows(2).filter(|w| w[0].trim() == "div .flex_1").filter_map(|w| w[1].trim().strip_prefix("text ")).map(|t| t.trim_matches('"').to_owned()).collect::<Vec<_>>()
    };
    let all = tree(json!({"pane":"p1","terminal":"t1","selected":true,"cwd":"/fixture/project"}), cx);
    let items = labels(&all);
    let at = items.iter().position(|t| t == "Move to…").unwrap_or_else(|| panic!("{items:?}"));
    assert_eq!(items[at..], ["Move to…", "Split under", "Grab selection", "Open folder", "Zoom it", "Close pane"], "{items:?}");
    assert!(all.contains("mods/mover/move.svg"), "a mod's own icon: {all}");
    // A divider (a 1 px rule) before each mod's group and before Close pane.
    let rules = all.matches(".h[Number(1.0)]").count();
    let bare = tree(json!({"pane":"p1","terminal":"t1"}), cx);
    assert_eq!(labels(&bare)[labels(&bare).len() - 3..], ["Move to…", "Split under", "Close pane"], "no selection, no folder: {bare}");
    assert_eq!(rules, bare.matches(".h[Number(1.0)]").count() + 1, "one group fewer, one divider fewer");
    let block = labels(&tree(json!({"pane":"block","terminal":"t1","selected":true}), cx));
    assert!(block.contains(&"Grab selection".into()) && !block.iter().any(|t| t == "Move to…" || t == "Split under"), "a terminal outside a tab has no pane: {block:?}");
}

/// A pane menu taller than its popup: the items scroll within `props.height`,
/// Close pane stays pinned below them, and the keyboard's selection scrolls
/// into view.
#[gpui::test]
fn a_long_pane_menu_scrolls_and_keeps_close_pane_in_reach(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let tree = |setup: Option<&str>, cx: &mut TestAppContext| {
        let target = json!({"pane":"p1","terminal":"t1","selected":true,"cwd":"/fixture/project","last_output":true});
        let mut values = fixture(true, json!({"tab":"pane","target":target,"height":420.0,"overlay":"o1"}), updates("current", "current"));
        values.insert("menuItems".into(), json!([
            {"mod":"mover","title":"Move to…","command":"mod.open","args":{"name":"mover"},"when":"pane","icon":null,"fill":["props"]},
            {"mod":"grab","title":"Zoom it","command":"pane.zoom","args":{},"when":null,"icon":null,"fill":["pane"]},
        ]));
        describe_with_values(cx, &staged, "mods/menu", values, setup).unwrap()
    };
    // The scrolled part: the lines under `menu-body`, indented deeper than it.
    let body = |tree: &str| -> (Vec<String>, Vec<String>) {
        let lines: Vec<&str> = tree.lines().collect();
        let at = lines.iter().position(|l| l.contains("menu-body")).unwrap_or_else(|| panic!("14 rows overflow 420 px: {tree}"));
        let indent = |l: &str| l.len() - l.trim_start().len();
        let end = lines[at + 1..].iter().position(|l| indent(l) <= indent(lines[at])).map_or(lines.len(), |n| at + 1 + n);
        (lines[at..end].iter().map(|l| l.to_string()).collect(), lines[end..].iter().map(|l| l.to_string()).collect())
    };
    let offset = |inside: &[String]| -> f64 {
        let l = inside.iter().find(|l| l.contains(".top[Number(")).expect("the list is placed by its scroll");
        let n = &l[l.find(".top[Number(").unwrap() + 12..];
        n[..n.find(')').unwrap()].parse().unwrap()
    };
    let first = tree(None, cx);
    let (inside, after) = body(&first);
    assert!(inside.iter().any(|l| l.contains("\"Zoom it\"")) && !inside.iter().any(|l| l.contains("\"Close pane\"")), "{first}");
    assert!(after.iter().any(|l| l.contains("\"Close pane\"")), "Close pane pinned below the scrolled items: {first}");
    assert!(inside[0].contains(".h[Number(370.0)]"), "420 less padding and the pinned row with its rule: {}", inside[0]);
    assert_eq!(offset(&inside), 0.0);
    // Selected by the keyboard, the last scrolled item comes into view.
    let moved = tree(Some("this.at = 12; this.follow = true;"), cx);
    let (inside, _) = body(&moved);
    assert!(offset(&inside) < -50.0, "{moved}");
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

/// The agent pane's info as `publish` gives it, on the populated fixture's working pane.
fn with_header_info(values: &mut BTreeMap<String, Value>, info: Value) {
    for p in values.get_mut("panes").unwrap().as_array_mut().unwrap() {
        p["header_info"] = info.clone();
    }
}

/// `pane_header_info` unset: panes carry `header_info: null`, and every
/// shipped mod draws what it drew without the field, byte for byte.
#[gpui::test]
fn null_header_info_draws_every_shipped_mod_as_before(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let handles = regex::Regex::new(r"(:track_(?:focus|scroll)\[Number\(| #)[0-9]+").unwrap();
    let same = |tree: &str| handles.replace_all(tree, "${1}N").into_owned();
    for entry in staged.entries() {
        let before = describe(cx, &staged, &entry, true, props(), updates("current", "current"), None).unwrap();
        let mut values = fixture(true, props(), updates("current", "current"));
        with_header_info(&mut values, Value::Null);
        let after = describe_with_values(cx, &staged, &entry, values, None).unwrap();
        assert_eq!(same(&before), same(&after), "{entry}");
    }
}

/// The shipped pane header with info, at a width (`this.width`, from
/// `on_size_change`) and hovered or not: the name keeps its width, segments go
/// from the right with a waiting ask last, the last one left truncates, the
/// whole line is the tooltip, and the hover buttons keep their room.
#[gpui::test]
fn pane_header_fits_its_info_beside_the_name_and_buttons(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let info = crate::header_info::Template::parse("{ask} · {branch}{ahead}{behind}{dirty} · {state} {elapsed}").resolve(&crate::header_info::Values::sample()).unwrap();
    let header = |width: f64, hovered: bool, cx: &mut TestAppContext| {
        let mut values = fixture(true, json!({"pane":"p1","terminal":"t1"}), updates("current", "current"));
        with_header_info(&mut values, json!(info));
        describe_with_values(cx, &staged, "mods/pane", values, Some(&format!("this.width = {width}; this.hovered = {hovered};"))).unwrap()
    };
    let shown = |tree: &str| -> Vec<String> {
        let mut lines = tree.lines().skip_while(|l| !l.contains("pane-info-p1"));
        let Some(head) = lines.next() else { return vec![] };
        let depth = head.len() - head.trim_start().len();
        lines.take_while(|l| l.len() - l.trim_start().len() > depth).filter_map(|l| l.trim().strip_prefix("text ")).map(|t| t.trim_matches('"').to_owned()).collect()
    };
    let title = r#"div .flex_none .truncate .max_w[Number(260.0)] .font_medium"#;
    // The name ("Pi", 14 px), the ring (12), padding and gaps take 66 px; the
    // segments 130, 210 or 297 (a separator 15: its mark and 4 px either side); the five buttons 140.
    let all = header(400.0, false, cx);
    assert_eq!(shown(&all), ["approve Bash: make", "·", "main ↑2 *", "·", "blocked 3m"], "room for all: {all}");
    assert!(all.contains(":tooltip[Str(\"approve Bash: make · main ↑2 * · blocked 3m\")]"), "the whole line in the tooltip: {all}");
    // Even gaps: each separator is its mark alone, padded the same either side,
    // in the folder's colour and size (it sets neither size nor font).
    let dim = crate::settings::theme()["dim"].clone();
    // The padding fit measures (Rust's SEP_PAD) is the padding the mod draws.
    let sep = format!("div .flex_none .px[Number({:?})] .text_color[Str({dim:?})]", crate::header_info::SEP_PAD as f64);
    let seps: Vec<&str> = all.lines().filter(|l| l.trim() == sep).collect();
    assert_eq!(seps.len(), 2, "both separators padded alike: {all}");
    let folder = format!("div .flex_1 .truncate .text_color[Str({dim:?})]");
    assert!(all.contains(&folder), "the folder in the same colour: {all}");
    let mut from_info = all.lines().skip_while(|l| !l.contains("pane-info-p1"));
    let head = from_info.next().unwrap();
    let depth = head.len() - head.trim_start().len();
    let info_lines: Vec<&str> = std::iter::once(head).chain(from_info.take_while(|l| l.len() - l.trim_start().len() > depth)).collect();
    assert!(!info_lines.iter().any(|l| l.contains(".text_size[") || l.contains(".font_family[")), "the info takes the header's size and font, as the folder does: {all}");
    assert!(all.contains(&format!(".text_color[Str({:?})]", crate::settings::theme()["blocked"].clone())), "the ask in the blocked colour: {all}");
    assert!(all.contains(title) && all.contains("text \"Pi\""), "the name as without info: {all}");
    assert!(all.contains(":on_size_change(fn)"), "it watches its width only with info: {all}");

    let hovered = header(428.0, true, cx);
    assert_eq!(hovered.matches("svg \"mods/pane/").count(), 5, "the buttons: {hovered}");
    assert_eq!(shown(&header(410.0, true, cx)), ["approve Bash: make"], "the buttons' room is all five's (four would leave two segments room)");
    assert_eq!(shown(&hovered), ["approve Bash: make", "·", "main ↑2 *"], "hovered: a segment goes, from the right: {hovered}");
    assert!(hovered.contains(title) && hovered.contains("text \"Pi\""), "the name stays: {hovered}");

    let lone = header(248.0, true, cx);
    assert_eq!(shown(&lone), ["approve Bash: make"], "the ask goes last: {lone}");
    let last = lone.lines().skip_while(|l| !l.contains("pane-info-p1")).nth(1).unwrap();
    assert!(last.contains(".min_w[Number(0.0)] .truncate"), "the lone segment ends in an ellipsis: {last}");

    let gone = header(150.0, true, cx);
    assert!(!gone.contains("pane-info-p1") && gone.contains("text \"Pi\"") && gone.matches("svg \"mods/pane/").count() == 5, "too narrow: no info, name and buttons stay: {gone}");
}

/// The pane header's float button: Float on a tiled pane, Dock on a floating
/// one, each its own icon and tooltip, beside split and before zoom.
#[gpui::test]
fn pane_header_floats_and_docks_its_pane(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let header = |floating: Value, cx: &mut TestAppContext| {
        let mut values = fixture(true, json!({"pane":"p1","terminal":"t1"}), updates("current", "current"));
        for p in values.get_mut("panes").unwrap().as_array_mut().unwrap() {
            p["floating"] = floating.clone();
        }
        describe_with_values(cx, &staged, "mods/pane", values, Some("this.hovered = true;")).unwrap()
    };
    let button = |tree: &str| -> Vec<String> {
        let mut lines = tree.lines().skip_while(|l| !l.contains("ph-f-p1"));
        let head = lines.next().unwrap_or_else(|| panic!("no float button: {tree}")).trim().to_owned();
        vec![head, lines.next().unwrap().trim().to_owned()]
    };
    let tiled = header(Value::Null, cx);
    let [head, icon] = button(&tiled).try_into().unwrap();
    assert!(head.contains(":tooltip[Str(\"Float\")]") && icon.contains("svg \"mods/pane/float.svg\""), "{head} {icon}");
    let floating = header(json!({"rect":[0.2,0.2,0.6,0.6],"z":0,"hidden":false}), cx);
    let [head, icon] = button(&floating).try_into().unwrap();
    assert!(head.contains(":tooltip[Str(\"Dock\")]") && icon.contains("svg \"mods/pane/dock.svg\""), "{head} {icon}");
    let order: Vec<&str> = tiled.lines().filter_map(|l| l.trim().strip_prefix("svg \"mods/pane/")?.split('"').next()).collect();
    assert_eq!(order, ["split-right.svg", "split-down.svg", "float.svg", "zoom.svg", "close.svg"], "{tiled}");
}

/// The agents mod's sub-agent count: its number, and its element's line.
fn sub_count(tree: &str) -> Option<(String, String)> {
    let mut lines = tree.lines().skip_while(|l| !l.contains("\"sub-count\""));
    let head = lines.next()?;
    let depth = head.len() - head.trim_start().len();
    let inner: Vec<&str> = lines.take_while(|l| l.len() - l.trim_start().len() > depth).collect();
    assert!(inner.iter().any(|l| l.contains("svg \"mods/workspace-chrome/copy.svg\"")), "{head}");
    let n = inner.iter().find_map(|l| l.trim().strip_prefix("text ")).map(|t| t.trim_matches('"').to_owned())?;
    Some((n, head.to_owned()))
}

fn sub_count_line(tree: &str) -> String {
    tree.lines().find(|l| l.contains("\"sub-count\"")).unwrap_or_else(|| panic!("no count: {tree}")).to_owned()
}

/// A parent agent's row counts its live sub-agents (a copy icon and N, the rows its tree
/// shows, nested ones too), unless the sidebar is too narrow; folded, then the
/// chevron counts instead.
#[gpui::test]
fn agents_rows_count_their_live_sub_agents(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let program = json!({"key":"pi", "name":"Pi", "icon":null, "tint":true, "tile":null, "color":null, "shell":false, "agent":true});
    let row = |sidebar: Option<f64>, cx: &mut TestAppContext| {
        let mut values = fixture(true, props(), updates("current", "current"));
        let terminals = values.get_mut("terminals").unwrap().as_array_mut().unwrap();
        for (address, parent, state) in [("k1", "t1", "live"), ("k2", "t1", "live"), ("k3", "k1", "live"), ("k4", "t1", "exited"), ("k5", "t9", "live")] {
            terminals.push(json!({"address":address,"name":address,"title":"","argv":["pi"],"parent":parent,"labels":{},"state":state,"status":"working","report":{"app":"pi"},"program":program}));
        }
        if let Some(px) = sidebar {
            values.insert("layout".into(), json!({"id":"root","type":"row","children":[{"id":"sidebar","type":"block","size":{"px":px},"content":{"mod":"agents"}}]}));
        }
        describe_with_values(cx, &staged, "mods/agents", values, None).unwrap()
    };
    let wide = row(None, cx);
    assert_eq!(sub_count(&wide), Some(("3".into(), sub_count_line(&wide))), "three live under t1, one nested: {wide}");
    assert_eq!(wide.matches("\"sub-count\"").count(), 1, "only the parent counts: {wide}");
    assert!(!wide.contains('\u{29c9}'), "a drawn icon, not a mark the UI font lacks: {wide}");
    let narrow = row(Some(200.0), cx);
    assert!(!narrow.contains("\"sub-count\""), "a narrow sidebar drops it: {narrow}");
}

/// The count takes its most urgent live sub-agent's colour (blocked or error:
/// blocked; working; else dim), and a program's OSC 7501 child records (Claude
/// Code's own sub-agents, reported by a hook) are sub-agents too: counted, a
/// row each named by its title, coloured with the rest.
#[gpui::test]
fn agents_sub_agent_count_takes_the_most_urgent_colour_and_counts_child_records(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let program = json!({"key":"pi", "name":"Pi", "icon":null, "tint":true, "tile":null, "color":null, "shell":false, "agent":true});
    let theme = crate::settings::theme();
    let colour = |name: &str| format!(".text_color[Str({:?})]", theme[name].clone());
    let draw = |kids: &[&str], records: Value, cx: &mut TestAppContext| {
        let mut values = fixture(true, props(), updates("current", "current"));
        let terminals = values.get_mut("terminals").unwrap().as_array_mut().unwrap();
        terminals.iter_mut().filter(|t| t["address"] == "t1").for_each(|t| t["records"] = records.clone());
        for (i, status) in kids.iter().enumerate() {
            terminals.push(json!({"address":format!("k{i}"),"name":format!("k{i}"),"title":"","argv":["pi"],"parent":"t1","labels":{},"state":"live","status":status,"report":{"app":"pi"},"program":program}));
        }
        describe_with_values(cx, &staged, "mods/agents", values, None).unwrap()
    };
    let count = |tree: &str| sub_count(tree).map(|(n, _)| n).unwrap_or_else(|| panic!("no count: {tree}"));
    let count_el = |tree: &str| sub_count_line(tree);
    for (kids, expect) in [(&["idle", "done"][..], "dim"), (&["idle", "working"][..], "working"), (&["working", "blocked"][..], "blocked"), (&["idle", "error"][..], "blocked")] {
        let tree = draw(kids, Value::Null, cx);
        assert!(count_el(&tree).contains(&colour(expect)), "{kids:?}: {expect}: {}", count_el(&tree));
        assert_eq!(count(&tree), "2", "{tree}");
    }
    let records = json!({"agent/a1":{"state":"working","title":"Explore"},"agent/b2":{"state":"blocked","title":"Plan"}});
    let tree = draw(&["idle"], records, cx);
    assert_eq!(count(&tree), "3", "two records and a terminal: {tree}");
    assert!(count_el(&tree).contains(&colour("blocked")), "a blocked record: {tree}");
    assert!(tree.contains("\"sub-t1#agent/a1\"") && tree.contains("text \"Explore\"") && tree.contains("text \"Plan\""), "a row each, by title: {tree}");
    let idle = draw(&[], json!({"agent/a1":{"state":"idle"}}), cx);
    assert!(count(&idle) == "1" && count_el(&idle).contains(&colour("dim")) && idle.contains("text \"agent/a1\""), "no title: its id: {idle}");
}

/// `agents_subagents`: tree (and unset) draws t1's sub-agents under it with
/// their count and chevron; folded only the count and chevron until opened;
/// off none of them. A fold the person made is read back from localStorage
/// at start, over the setting, and fold all / unfold all reach every agent.
#[gpui::test]
fn agents_sub_agents_follow_the_setting_remembered_folds_and_fold_all(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let program = json!({"key":"pi", "name":"Pi", "icon":null, "tint":true, "tile":null, "color":null, "shell":false, "agent":true});
    let draw = |mode: Option<&str>, setup: Option<&str>, cx: &mut TestAppContext| {
        let mut values = fixture(true, props(), updates("current", "current"));
        if let Some(mode) = mode {
            values.get_mut("settings").unwrap()["agents_subagents"] = json!(mode);
        }
        let terminals = values.get_mut("terminals").unwrap().as_array_mut().unwrap();
        terminals.iter_mut().filter(|t| t["address"] == "t1").for_each(|t| t["records"] = json!({"agent/a1":{"state":"working","title":"Explore"}}));
        terminals.push(json!({"address":"k1","name":"k1","title":"","argv":["pi"],"parent":"t1","labels":{},"state":"live","status":"working","report":{"app":"pi"},"program":program}));
        describe_with_values(cx, &staged, "mods/agents", values, setup).unwrap()
    };
    let shows = |tree: &str| (tree.contains("\"sub-k1\"") && tree.contains("\"sub-t1#agent/a1\""), tree.contains("\"sub-count\""), tree.contains("\"fold-t1\""));
    for mode in [None, Some("tree")] {
        let tree = draw(mode, None, cx);
        assert_eq!(shows(&tree), (true, true, true), "{mode:?}: {tree}");
        assert_eq!(sub_count(&tree).map(|(n, _)| n).as_deref(), Some("2"), "{tree}");
    }
    let folded = draw(Some("folded"), None, cx);
    assert_eq!(shows(&folded), (false, true, true), "folded: {folded}");
    assert!(!folded.contains("text \"Explore\""), "{folded}");
    let off = draw(Some("off"), None, cx);
    assert_eq!(shows(&off), (false, false, false), "off: {off}");
    // A restart: the fold written, forgotten in memory, read back at init.
    let restart = |folded: bool| format!("this.setFolds([\"t1\"], {folded}); this.folds = {{}}; super.init(props, cx);");
    assert_eq!(shows(&draw(None, Some(&restart(true)), cx)), (false, true, true), "kept folded");
    assert_eq!(shows(&draw(Some("folded"), Some(&restart(false)), cx)), (true, true, true), "kept open over folded");
    assert_eq!(shows(&draw(Some("off"), Some(&restart(false)), cx)), (false, false, false), "off stays off");
    assert_eq!(shows(&draw(None, Some("this.command(\"fold_all\");"), cx)), (false, true, true), "fold all");
    assert_eq!(shows(&draw(Some("folded"), Some("this.command(\"unfold_all\");"), cx)), (true, true, true), "unfold all");
}

/// `agents_card_info`: unset, the shipped agents rows draw as before whatever
/// card info the agents carry; set, a row draws its info without the space
/// (named by its header or its name), and none when there is nothing.
#[gpui::test]
fn agents_rows_draw_card_info_once_set(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let info = json!({"text":"main · Local","segments":[{"sep":"","text":"main","ask":false},{"sep":" · ","text":"Local","ask":false}],"no_space":{"text":"main","segments":[{"sep":"","text":"main","ask":false}]}});
    let draw = |set: Option<&str>, card: Value, cx: &mut TestAppContext| {
        let mut values = fixture(true, props(), updates("current", "current"));
        if let Some(set) = set {
            values.get_mut("settings").unwrap()["agents_card_info"] = json!(set);
        }
        values.get_mut("agents").unwrap().as_array_mut().unwrap().iter_mut().for_each(|a| a["card_info"] = card.clone());
        describe_with_values(cx, &staged, "mods/agents", values, None).unwrap()
    };
    let unset = draw(None, info.clone(), cx);
    let before = describe(cx, &staged, "mods/agents", true, props(), updates("current", "current"), None).unwrap();
    assert_eq!(unset, before, "unset: drawn as before");
    // Another machine's agent, unset: its machine's bare name, as the shipped rows always drew it.
    let mut values = fixture(true, props(), updates("current", "current"));
    values.get_mut("agents").unwrap().as_array_mut().unwrap().iter_mut().for_each(|a| {
        a["address"] = json!(format!("vps/{}", a["address"].as_str().unwrap()));
        a["card_info"] = json!({"text":"@vps · Local","segments":[{"sep":"","text":"@vps","ask":false},{"sep":" · ","text":"Local","ask":false}],"no_space":{"text":"@vps","segments":[]}});
    });
    let remote = describe_with_values(cx, &staged, "mods/agents", values, None).unwrap();
    assert!(remote.contains("text \"vps\"") && !remote.contains("@vps"), "{remote}");
    let set = draw(Some("{branch} · {space}"), info, cx);
    assert!(set.contains("text \"main\"") && !set.contains("main · Local"), "the info without its space: {set}");
    let none = draw(Some(""), Value::Null, cx);
    assert!(!none.contains("text \"main\""), "{none}");
}

/// The pane menu names the pane first among the pane's own items, as the tab
/// menu does its tab; a pane header's right-click opens that menu for its pane
/// and terminal, with no selection to copy.
#[gpui::test]
fn pane_header_right_click_opens_the_pane_menu_with_rename(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let mut p = props();
    p["target"] = json!({"pane":"p1","terminal":"t1"});
    let menu = describe(cx, &staged, "mods/menu", true, p, updates("current", "current"), None).unwrap();
    let items: Vec<&str> = texts(&menu).into_iter().filter(|t| !t.is_empty() && *t != "\"\"").collect();
    let at = items.iter().position(|t| *t == "\"Rename\"").unwrap_or_else(|| panic!("{items:?}"));
    assert_eq!(items[at + 1], "\"Split right\"", "{items:?}");
    let header = describe(cx, &staged, "mods/pane", true, json!({"pane":"p1","terminal":"t1"}), updates("current", "current"), None).unwrap();
    let head = header.lines().next().unwrap();
    assert!(head.contains("pane-head-p1") && head.contains(":on_mouse_down_right(fn)"), "the header takes right-clicks: {head}");
}

/// This thread's CPU time. A test thread on a loaded machine waits for a core
/// for as long as the load lasts; this counts only the work it did.
fn thread_cpu() -> Duration {
    #[cfg(unix)]
    {
        let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: clock_gettime only writes the timespec it is given.
        assert_eq!(unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) }, 0);
        Duration::new(t.tv_sec as u64, t.tv_nsec as u32)
    }
    #[cfg(not(unix))]
    {
        static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        START.get_or_init(Instant::now).elapsed()
    }
}

/// What the picker's limits stand on: thread_cpu counts work and not waiting.
#[cfg(unix)]
#[test]
fn thread_cpu_counts_work_and_not_waiting() {
    let start = thread_cpu();
    std::thread::sleep(Duration::from_millis(100));
    let asleep = thread_cpu() - start;
    assert!(asleep < Duration::from_millis(20), "asleep {asleep:?}");
    let (start, wall) = (thread_cpu(), Instant::now());
    while thread_cpu() - start < Duration::from_millis(30) {
        assert!(wall.elapsed() < Duration::from_secs(10), "30 ms of work never counted");
        std::hint::black_box(0u64);
    }
}

/// Describes a shipped mod, lets its async work (reading files) finish, and
/// describes it again: the first description and the CPU time it took, the
/// time to settle, and the settled description and the CPU time it took.
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
    let start = thread_cpu();
    let (mut session, first) = window.update(cx, |_, window, cx| {
        let mut session = DescriptionSession::load(runtime, root, policy, window, cx).unwrap();
        let first = session.describe(window, cx).unwrap();
        (session, first)
    }).unwrap();
    let open = thread_cpu() - start;
    let start = Instant::now();
    cx.run_until_parked();
    let settle = start.elapsed();
    let start = thread_cpu();
    let settled = window.update(cx, |_, window, cx| session.describe(window, cx).unwrap()).unwrap();
    let redraw = thread_cpu() - start;
    window.update(cx, |_, window, _| window.remove_window()).unwrap();
    cx.run_until_parked();
    assert!(calls.borrow().unexpected.is_empty(), "{entry}: {:?}", calls.borrow().unexpected);
    (first, open, settle, settled, redraw)
}

/// The names are read when the picker opens and every glyph and emoji is a
/// virtual grid's row: the page builds none of them. Opening and a keystroke's
/// search stay inside a frame or so, counted in CPU time so a loaded machine's
/// wait for a core doesn't count as the picker's own.
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
    eprintln!("icon picker CPU time: open {open:?}, search + describe per keystroke worst {worst:?} (all {keystrokes:?}); names read and parsed in {settle:?}");
    assert!(open < Duration::from_millis(500), "open took {open:?} of CPU");
    assert!(*worst < Duration::from_millis(250), "a keystroke took {worst:?} of CPU");
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
