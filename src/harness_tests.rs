use super::*;
use serde_json::json;
use std::collections::HashSet;

fn bundled() -> Registry {
    let (registry, errors) = Registry::parse(HARNESSES.iter().filter(|(n, _)| n.ends_with(".toml")).map(|(n, b)| (*n, std::str::from_utf8(b).unwrap())));
    assert!(errors.is_empty(), "every shipped manifest loads: {errors:?}");
    registry
}

fn argv(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

fn id(r: &Registry, name: &str, a: &[&str]) -> Option<String> {
    r.identify(name, &argv(a)).map(|m| m.id.clone())
}

/// tuios's screen fixtures (tests/harness-screens, copied from
/// internal/harness/testdata/screens): a header naming the harness and the
/// state its rules must find, `---`, then the screen. Several to a file are
/// separated by a line of five equals signs.
struct Fixture {
    name: String,
    harness: String,
    want: String,
    screen: Vec<String>,
}

fn fixtures() -> Vec<Fixture> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/harness-screens");
    let mut paths: Vec<_> = [root.clone(), root.join("rules")]
        .iter()
        .flat_map(|d| std::fs::read_dir(d).unwrap().flatten().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "txt"))
        .collect();
    paths.sort();
    let mut out = Vec::new();
    for p in paths {
        let text = std::fs::read_to_string(&p).unwrap();
        for (n, chunk) in text.split("\n=====\n").enumerate() {
            let (head, body) = chunk.split_once("\n---\n").unwrap_or_else(|| panic!("{}: no --- line", p.display()));
            let field = |k: &str| head.lines().find_map(|l| l.strip_prefix("# ")?.strip_prefix(k)?.strip_prefix(':').map(|v| v.trim().to_owned())).unwrap_or_default();
            out.push(Fixture { name: format!("{}#{}", p.file_name().unwrap().to_string_lossy(), n + 1), harness: field("harness"), want: field("want"), screen: body.lines().map(str::to_owned).collect() });
        }
    }
    out
}

/// What the terminal's tail reads: the last `n` non-empty lines, right-trimmed.
fn tail(screen: &[String], n: usize) -> Vec<String> {
    let mut out: Vec<String> = screen.iter().rev().map(|l| l.trim_end_matches([' ', '\t']).to_owned()).filter(|l| !l.is_empty()).take(n).collect();
    out.reverse();
    out
}

fn word_of(state: &str) -> Option<&'static str> {
    match state {
        "needs_input" => Some("blocked"),
        "working" => Some("working"),
        "idle" => Some("idle"),
        _ => None,
    }
}

#[test]
fn every_shipped_manifest_classifies_its_tuios_screens() {
    let r = bundled();
    let mut covered = HashSet::new();
    let mut failures = Vec::new();
    for fx in fixtures() {
        let m = r.get(&fx.harness).unwrap_or_else(|| panic!("{}: no manifest {:?}", fx.name, fx.harness));
        let got = m.classify(&tail(&fx.screen, m.lines()));
        if got != word_of(&fx.want) {
            failures.push(format!("{}: got {got:?}, want {}", fx.name, fx.want));
        }
        covered.insert(fx.harness);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    let shipped: HashSet<String> = r.manifests.iter().map(|m| m.id.clone()).collect();
    assert_eq!(covered, shipped, "every shipped manifest has screens to classify");
}

#[test]
fn every_screen_rule_decides_a_fixture() {
    let r = bundled();
    let mut decided: HashSet<(String, usize)> = HashSet::new();
    for fx in fixtures() {
        let m = r.get(&fx.harness).unwrap();
        let t = tail(&fx.screen, m.lines());
        let first = m.screen.rule.iter().position(|rule| {
            let Some((lo, hi)) = rule.area.bounds(&t) else { return false };
            let hay = t[lo..hi].join("\n");
            let folded = if m.screen.fold_case { hay.to_lowercase() } else { hay.clone() };
            !hay.is_empty() && rule.gate.positive() && rule.gate.matches(&hay, &folded)
        });
        if let Some(i) = first.filter(|_| m.classify(&t) == word_of(&fx.want)) {
            decided.insert((fx.harness.clone(), i));
        }
    }
    let missing: Vec<String> = r
        .manifests
        .iter()
        .flat_map(|m| (0..m.screen.rule.len()).filter(|i| !decided.contains(&(m.id.clone(), *i))).map(|i| format!("{} rule {i} ({})", m.id, m.screen.rule[i].state)).collect::<Vec<_>>())
        .collect();
    assert!(missing.is_empty(), "rules no fixture decides:\n{}", missing.join("\n"));
}

#[test]
fn claude_code_is_recognised_by_name_argv_package_or_install_path() {
    let r = bundled();
    let claude = Some("claude-code".to_owned());
    assert_eq!(id(&r, "claude", &["claude"]), claude, "the backgrounded session's relay client");
    assert_eq!(id(&r, "claude", &["claude", "--resume", "abc"]), claude);
    assert_eq!(id(&r, "2.1.280", &["/Users/me/.local/share/claude/versions/2.1.280"]), claude, "the native build, named after its version");
    assert_eq!(id(&r, "node", &["node", "/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/cli.js"]), claude);
    assert_eq!(id(&r, "node", &["node", "/usr/local/bin/claude-code"]), claude, "argv0 names the script an interpreter runs");
    assert_eq!(id(&r, "zsh", &["-zsh"]), None);
    assert_eq!(id(&r, "vim", &["vim", "claude.md"]), None);
    assert_eq!(id(&r, "tail", &["tail", "-f", "/src/node_modules/@anthropic-ai/claude-code/cli.js"]), None, "only an interpreter's run token is read");
    assert_eq!(id(&r, "node", &["node", "/Users/me/dev/claude/scripts/build.js"]), None, "a checkout named after an agent is not the agent");
}

#[test]
fn other_harnesses_are_recognised_and_require_their_corroboration() {
    let r = bundled();
    assert_eq!(id(&r, "codex", &["codex"]).as_deref(), Some("codex"));
    assert_eq!(id(&r, "node", &["node", "/usr/lib/node_modules/@openai/codex/bin/codex.js"]).as_deref(), Some("codex"));
    assert_eq!(id(&r, "opencode", &["opencode"]).as_deref(), Some("opencode"));
    assert_eq!(id(&r, "hermes", &["hermes"]).as_deref(), Some("hermes"));
    assert_eq!(id(&r, "node", &["node", "/Users/me/.npm-global/bin/pi"]).as_deref(), Some("pi"), "pi is a node program");
    assert_eq!(id(&r, "pi", &["pi"]), None, "a bare pi without node behind it is some other pi");
}

#[test]
fn needs_input_outranks_working_which_outranks_idle() {
    let r = bundled();
    let m = r.get("claude-code").unwrap();
    let lines = |s: &str| s.lines().map(str::to_owned).collect::<Vec<_>>();
    let box_ = "────────────────────────────────────────\n❯\n────────────────────────────────────────";
    assert_eq!(m.classify(&lines(box_)), Some("idle"));
    let working = format!("✻ Thinking… (12s · ↑ 1.2k tokens)\n{box_}\n  ⏵⏵ auto mode on (shift+tab to cycle) · esc to interrupt");
    assert_eq!(m.classify(&lines(&working)), Some("working"), "the box stays on screen during a turn");
    let asking = format!("✻ Thinking… (12s)\n Do you want to proceed?\n ❯ 1. Yes\n   2. No\n{box_}");
    assert_eq!(m.classify(&lines(&asking)), Some("blocked"));
    assert_eq!(m.classify(&lines("plain output\nno box here")), None, "no rule matching is no opinion");
}

#[test]
fn idle_is_published_on_a_working_pane_only_once_it_holds() {
    let t0 = Instant::now();
    let at = |ms: u64| t0 + Duration::from_millis(ms);
    let mut w = Watch::new("claude-code");
    assert_eq!(w.observe(None, at(0)), "idle", "at rest until a rule says otherwise");
    assert_eq!(w.observe(Some("working"), at(0)), "working");
    assert_eq!(w.observe(Some("idle"), at(10)), "working", "one idle frame is not a finished turn");
    assert_eq!(w.observe(Some("working"), at(50)), "working", "work cancels the wait");
    assert_eq!(w.observe(Some("idle"), at(100)), "working");
    assert_eq!(w.observe(Some("idle"), at(200)), "working");
    assert_eq!(w.observe(Some("idle"), at(300)), "working");
    assert_eq!(w.observe(Some("idle"), at(400)), "idle", "three confirmations 100ms apart");

    let mut w = Watch::new("claude-code");
    w.observe(Some("working"), at(0));
    assert_eq!(w.observe(Some("idle"), at(0)), "working");
    assert_eq!(w.observe(Some("idle"), at(500)), "working", "500ms ticks confirm once each");
    assert_eq!(w.observe(Some("idle"), at(1000)), "idle", "the cap publishes it");

    let mut w = Watch::new("claude-code");
    w.observe(Some("blocked"), at(0));
    assert_eq!(w.observe(Some("idle"), at(1)), "idle", "only a working pane can flap");
    assert_eq!(w.observe(None, at(2)), "idle", "no opinion keeps the last word");
}

fn record(status: Value, name: &str, a: &[&str]) -> Value {
    json!({ "state": "live", "status": status, "foreground": { "pid": 1, "name": name, "argv": a, "cwd": "/" } })
}

#[test]
fn the_watcher_lists_unreported_agents_and_drops_them_when_they_go() {
    let r = bundled();
    let blocked = vec!["Do you want to proceed?".to_owned(), "❯ 1. Yes".to_owned(), "  2. No".to_owned()];
    let mut w = Watcher::default();
    let mut live = HashMap::from([("t1".to_owned(), record(Value::Null, "claude", &["claude"])), ("t2".to_owned(), record(json!("working"), "claude", &["claude"])), ("t3".to_owned(), record(Value::Null, "zsh", &["-zsh"]))]);
    let words = w.update(&r, &live, Instant::now(), |_, _| blocked.clone());
    assert_eq!(words.get("t1"), Some(&("blocked", "claude-code".to_owned())));
    assert!(!words.contains_key("t2"), "a 7501 report wins");
    assert!(!words.contains_key("t3"), "a shell is not an agent");

    live.insert("t1".into(), record(json!("idle"), "claude", &["claude"]));
    assert!(w.update(&r, &live, Instant::now(), |_, _| blocked.clone()).is_empty(), "a report that arrives takes over");

    live.insert("t1".into(), record(Value::Null, "claude", &["claude"]));
    assert!(w.update(&r, &live, Instant::now(), |_, _| blocked.clone()).contains_key("t1"));
    live.insert("t1".into(), record(Value::Null, "zsh", &["-zsh"]));
    assert!(w.update(&r, &live, Instant::now(), |_, _| blocked.clone()).is_empty(), "the shell is back in front");

    live.insert("t1".into(), record(Value::Null, "claude", &["claude"]));
    live.get_mut("t1").unwrap()["state"] = "finished".into();
    assert!(w.update(&r, &live, Instant::now(), |_, _| blocked.clone()).is_empty(), "the terminal ended");
    assert!(w.watching.is_empty());
}

#[test]
fn a_new_manifest_is_data_only() {
    let fake = r#"
schema_version = 1
id = "fake-agent"
priority = 10

[detect]
comm = ["fake-agent"]
argv_path = ["@example/fake-agent"]

[screen]
enabled = true
fold_case = true
lines = 4

[[screen.rule]]
state = "needs_input"
priority = 20
all = ["Allow this?"]
any = ["[y/n]"]

[[screen.rule]]
state = "working"
priority = 10
regex = ['^\s*Working… \(\d+s\)']

[[screen.rule]]
state = "idle"
priority = 5
region = "prompt_box"
regex = ['^\s*> ?$']

[title]
enabled = true
"#;
    let (r, errors) = Registry::parse([("fake-agent.toml", fake)]);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(id(&r, "node", &["node", "/x/node_modules/@example/fake-agent/cli.js"]).as_deref(), Some("fake-agent"));
    let m = r.identify("fake-agent", &argv(&["fake-agent"])).unwrap();
    let lines = |s: &[&str]| s.iter().map(|l| l.to_string()).collect::<Vec<_>>();
    assert_eq!(m.classify(&lines(&["ALLOW THIS? [Y/n]"])), Some("blocked"), "folded case");
    assert_eq!(m.classify(&lines(&["  Working… (3s)", "─────", ">", "─────"])), Some("working"));
    assert_eq!(m.classify(&lines(&["done.", "─────", "> ", "─────"])), Some("idle"));
    assert_eq!(m.classify(&lines(&["done."])), None);
}

#[test]
fn a_manifest_that_could_mislead_is_refused() {
    let base = "id = \"x\"\n[detect]\ncomm = [\"x\"]\n[screen]\nenabled = true\n";
    for (rule, ok) in [
        ("[[screen.rule]]\nstate = \"idle\"\nall = [\"ready\"]\n", false),
        ("[[screen.rule]]\nstate = \"idle\"\nregion = \"prompt_box\"\nall = [\">\"]\n", true),
        ("[[screen.rule]]\nstate = \"working\"\nregion = \"middle\"\nall = [\"busy\"]\n", false),
        ("[[screen.rule]]\nstate = \"done\"\nall = [\"finished\"]\n", false),
        ("[[screen.rule]]\nstate = \"working\"\nregex = ['(']\n", false),
        ("[[screen.rule]]\nstate = \"working\"\nall = [\"a\"]\nany_of = [ { not = [\"b\"] } ]\n", false),
    ] {
        let (_, errors) = Registry::parse([("x.toml", format!("{base}{rule}").as_str())]);
        assert_eq!(errors.is_empty(), ok, "{rule}: {errors:?}");
    }
}

#[test]
fn a_later_manifest_replaces_one_with_the_same_id() {
    let a = "id = \"x\"\n[detect]\ncomm = [\"one\"]\n";
    let b = "id = \"x\"\n[detect]\ncomm = [\"two\"]\n";
    let (r, _) = Registry::parse([("a.toml", a), ("b.toml", b)]);
    assert!(r.identify("one", &argv(&["one"])).is_none());
    assert!(r.identify("two", &argv(&["two"])).is_some());
}

#[test]
fn regions_find_the_prompt_box() {
    let t = |s: &[&str]| s.iter().map(|l| l.to_string()).collect::<Vec<_>>();
    let bounds = |r: Region, s: &[&str]| r.bounds(&t(s));
    assert_eq!(bounds(Region::PromptBox, &["x", "─────", "❯ hi", "─────", "footer"]), Some((2, 3)));
    assert_eq!(bounds(Region::PromptBox, &["╭───╮", "│ > │", "╰───╯"]), Some((1, 2)));
    assert_eq!(bounds(Region::PromptBox, &["❯ hi", "─────"]), None);
    assert_eq!(bounds(Region::PromptBox, &["──", "❯", "──"]), None, "two dashes are not a border");
    assert_eq!(bounds(Region::LastAbovePromptBox, &["a", "b", "", "───", "❯", "───"]), Some((1, 2)));
    assert_eq!(bounds(Region::AfterLastRule, &["a", "───", "b", "c"]), Some((2, 4)));
    assert_eq!(bounds(Region::AfterLastRule, &["a", "b"]), Some((0, 2)));
    assert_eq!(Region::parse("bottom_non_empty_lines(3)"), Ok(Region::Bottom(3)));
    assert!(Region::parse("bottom_non_empty_lines(0)").is_err());
}
