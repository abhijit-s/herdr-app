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
        let first = m.screen_rule(&t).and_then(|rule| m.screen.rule.iter().position(|r| std::ptr::eq(r, rule)));
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
    let mut w = Watch::new(0, t0);
    assert_eq!(w.observe(None, at(0)), "idle", "at rest until a rule says otherwise");
    assert_eq!(w.observe(Some("working"), at(0)), "working");
    assert_eq!(w.observe(Some("idle"), at(10)), "working", "one idle frame is not a finished turn");
    assert_eq!(w.observe(Some("working"), at(50)), "working", "work cancels the wait");
    assert_eq!(w.observe(Some("idle"), at(100)), "working");
    assert_eq!(w.observe(Some("idle"), at(200)), "working");
    assert_eq!(w.observe(Some("idle"), at(300)), "working");
    assert_eq!(w.observe(Some("idle"), at(400)), "idle", "three confirmations 100ms apart");

    let mut w = Watch::new(0, t0);
    w.observe(Some("working"), at(0));
    assert_eq!(w.observe(Some("idle"), at(0)), "working");
    assert_eq!(w.observe(Some("idle"), at(500)), "working", "500ms ticks confirm once each");
    assert_eq!(w.observe(Some("idle"), at(1000)), "idle", "the cap publishes it");

    let mut w = Watch::new(0, t0);
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
    let update = |w: &mut Watcher, live: &HashMap<String, Value>| w.update(&r, live, Instant::now(), |_| Some(1), |_, _| (blocked.clone(), String::new()));
    let words = update(&mut w, &live);
    assert_eq!(words.get("t1"), Some(&Unreported { word: "blocked", harness: "claude-code".into() }));
    assert!(!words.contains_key("t2"), "a 7501 report wins");
    assert!(!words.contains_key("t3"), "a shell is not an agent");

    live.insert("t1".into(), record(json!("idle"), "claude", &["claude"]));
    assert!(update(&mut w, &live).is_empty(), "a report that arrives takes over");

    live.insert("t1".into(), record(Value::Null, "claude", &["claude"]));
    assert!(update(&mut w, &live).contains_key("t1"));
    live.insert("t1".into(), record(Value::Null, "zsh", &["-zsh"]));
    assert!(update(&mut w, &live).is_empty(), "the shell is back in front");

    live.insert("t1".into(), record(Value::Null, "claude", &["claude"]));
    live.get_mut("t1").unwrap()["state"] = "finished".into();
    assert!(update(&mut w, &live).is_empty(), "the terminal ended");
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
    let base = "id = \"x\"\n[detect]\ncomm = [\"x-agent\"]\n[screen]\nenabled = true\n";
    for (rule, ok) in [
        ("[[screen.rule]]\nstate = \"idle\"\nall = [\"ready\"]\n", false),
        ("[[screen.rule]]\nstate = \"idle\"\nregion = \"prompt_box\"\nall = [\">\"]\n", true),
        ("[[screen.rule]]\nstate = \"working\"\nregion = \"middle\"\nall = [\"busy\"]\n", false),
        ("[[screen.rule]]\nstate = \"done\"\nall = [\"finished\"]\n", false),
        ("[[screen.rule]]\nstate = \"working\"\nregex = ['(']\n", false),
        ("[[screen.rule]]\nstate = \"working\"\nall = [\"a\"]\nany_of = [ { not = [\"b\"] } ]\n", false),
        ("[[screen.rule]]\nstate = \"working\"\nregion = \"bottom_non_empty_lines(7)\"\nall = [\"busy\"]\n", false),
        ("[[screen.rule]]\nstate = \"working\"\nregion = \"bottom_non_empty_lines(6)\"\nall = [\"busy\"]\n", true),
        ("lines = 201\n", false),
        (&format!("[[screen.rule]]\nstate = \"working\"\nregex = ['{}']\n", "a".repeat(513)), false),
        ("[[title.rule]]\nstate = \"idle\"\nall = [\"ready\"]\n", true),
        ("[[title.rule]]\nstate = \"idle\"\nregion = \"prompt_box\"\nall = [\"ready\"]\n", false),
    ] {
        // `lines` belongs to [screen], which the base opens last.
        let (_, errors) = Registry::parse([("x.toml", format!("{base}{rule}").as_str())]);
        assert_eq!(errors.is_empty(), ok, "{rule}: {errors:?}");
    }
}

#[test]
fn a_later_manifest_replaces_one_with_the_same_id() {
    let a = "id = \"x\"\n[detect]\ncomm = [\"first-agent\"]\n";
    let b = "id = \"x\"\n[detect]\ncomm = [\"second-agent\"]\n";
    let (r, _) = Registry::parse([("a.toml", a), ("b.toml", b)]);
    assert!(r.identify("first-agent", &argv(&["first-agent"])).is_none());
    assert!(r.identify("second-agent", &argv(&["second-agent"])).is_some());
}

#[test]
fn regions_find_the_prompt_box() {
    let t = |s: &[&str]| s.iter().map(|l| l.to_string()).collect::<Vec<_>>();
    let bounds = |r: Region, s: &[&str]| r.bounds(&t(s), prompt_box(&t(s)));
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

#[test]
fn a_short_name_needs_corroboration() {
    let (_, errors) = Registry::parse([("x.toml", "id = \"x\"\n[detect]\ncomm = [\"xy\"]\n")]);
    assert!(errors[0].contains("generic name"), "{errors:?}");
    let (_, errors) = Registry::parse([("x.toml", "id = \"x\"\n[detect]\ncomm = [\"xy\"]\n[detect.require]\nexe_base = [\"node\"]\n")]);
    assert!(errors.is_empty(), "{errors:?}");
    let (_, errors) = Registry::parse([("x.toml", "id = \"x\"\n")]);
    assert!(errors[0].contains("matches nothing"), "{errors:?}");
}

/// tuios's title cases (internal/harness/title_rules_test.go), and every
/// title rule decides one.
#[test]
fn title_rules_classify_tuios_titles() {
    let r = bundled();
    let cases = [
        ("claude-code", "✳ Claude Code", "idle"),
        ("claude-code", "⠂ Fix the flaky test", "working"),
        ("claude-code", "Claude Code", "none"),
        ("codex", "Action Required - codex", "needs_input"),
        ("codex", "⠋ codex", "working"),
        ("codex", "codex", "none"),
        ("codex", "Codex - Action Required", "needs_input"),
        ("codex", "action required: approve the patch?", "needs_input"),
        ("codex", "~/src/api — Action Required", "needs_input"),
        ("codex", "~/src/codex-playground", "none"),
        ("codex", "vim action_required.md", "none"),
        ("gemini-cli", "✋  Action Required (tuios)                                                      ", "needs_input"),
        ("gemini-cli", "⏲  Working… (tuios)", "working"),
        ("gemini-cli", "✦  Refactoring the retry loop (tuios)", "working"),
        ("gemini-cli", "✦  Action required: read the migration notes", "working"),
        ("gemini-cli", "✦  Ready to write the tests (tuios)", "working"),
        ("gemini-cli", "◇  Ready (tuios)", "idle"),
        ("gemini-cli", "Gemini CLI (tuios)", "none"),
        ("gemini-cli", "~/src/ready-player-one", "none"),
        ("amp", "Plugin confirmation needed - amp - ~/src/app", "needs_input"),
        ("amp", "⠹ fix the flaky test", "working"),
        ("amp", "fix the flaky test - amp - ~/src/app", "idle"),
        ("amp", "~/src/amp-notes", "none"),
        ("grok", "⚠ Action Required - review - grok", "needs_input"),
        ("grok", "review ⠧ Running tests", "working"),
        ("grok", "review - grok", "idle"),
        ("grok", "grok", "idle"),
        ("grok", "~/src/grokking", "none"),
        ("hermes", "⚠️ hermes: approve rm -rf build", "needs_input"),
        ("hermes", "⏳ hermes: reading files", "working"),
        ("hermes", "✓ hermes", "idle"),
        ("hermes", "hermes", "none"),
        ("kiro", "◐ kiro: fixing the build", "working"),
        ("kiro", "kiro", "none"),
        ("qwen", "✳ Waiting for confirmation", "needs_input"),
        ("qwen", "◐ Reading files", "working"),
        ("qwen", "Qwen Code", "none"),
    ];
    let mut decided = HashSet::new();
    let mut failures = Vec::new();
    for (harness, title, want) in cases {
        let m = r.get(harness).unwrap();
        if m.classify_title(title) != word_of(want) {
            failures.push(format!("{harness} {title:?}: got {:?}, want {want}", m.classify_title(title)));
        }
        let folded = if m.title.fold_case { title.to_lowercase() } else { title.to_owned() };
        if let Some(i) = m.title.rule.iter().position(|rule| rule.area == Region::Title && rule.gate.matches(title, &folded, contains_token)) {
            decided.insert((harness, i));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    for m in &r.manifests {
        for (i, rule) in m.title.rule.iter().enumerate() {
            assert!(rule.area == Region::Progress || decided.contains(&(m.id.as_str(), i)), "{} title rule {i} ({}) decides no case", m.id, rule.state);
        }
    }
}

#[test]
fn a_working_pane_with_no_sign_of_life_stops() {
    let t0 = Instant::now();
    let mut w = Watch::new(0, t0);
    w.observe(Some("working"), t0);
    assert_eq!(w.observe(None, t0 + STALL - Duration::from_secs(1)), "working");
    assert_eq!(w.observe(None, t0 + STALL), "idle", "silent for the stall interval");

    let mut w = Watch::new(0, t0);
    w.observe(Some("blocked"), t0);
    assert_eq!(w.observe(None, t0 + STALL * 10), "blocked", "a question waits for the person however long it takes");

    let mut w = Watch::new(0, t0);
    w.observe(Some("working"), t0);
    assert_eq!(w.observe(Some("working"), t0 + STALL * 2), "working", "a rule that still sees work is the answer");
}

#[test]
fn the_watcher_reads_a_screen_only_when_it_changed_and_stalls_on_silence() {
    let r = bundled();
    let live = HashMap::from([("t1".to_owned(), record(Value::Null, "node", &["node", "/usr/local/bin/pi"]))]);
    let mut w = Watcher::default();
    let t0 = Instant::now();
    let reads = std::cell::Cell::new(0);
    let working = vec!["  read src/main.ts".to_owned(), "  Working...".to_owned(), ">".to_owned()];
    let mut look = |writes: u64, at: Instant| {
        w.update(&r, &live, at, |_| Some(writes), |_, _| {
            reads.set(reads.get() + 1);
            (working.clone(), String::new())
        })
    };
    let pi = r.get("pi").unwrap();
    assert_eq!(pi.classify(&working), Some("working"), "pi's working line");
    assert_eq!(look(1, t0)["t1"].word, "working");
    assert_eq!(look(1, t0 + Duration::from_secs(10))["t1"].word, "working");
    assert_eq!(reads.get(), 1, "an unchanged screen isn't read again");
    look(2, t0 + Duration::from_secs(20));
    assert_eq!(reads.get(), 2);
    // Pi has no screen idle rule: once its screen stops showing a turn,
    // silence is what ends it.
    let mut w = Watcher::default();
    let quiet = |w: &mut Watcher, writes: u64, lines: Vec<String>, at: Instant| w.update(&r, &live, at, |_| Some(writes), |_, _| (lines.clone(), String::new()));
    assert_eq!(quiet(&mut w, 1, working.clone(), t0)["t1"].word, "working");
    assert_eq!(quiet(&mut w, 2, vec!["all done".into()], t0 + Duration::from_secs(1))["t1"].word, "working", "no opinion keeps the word for now");
    assert_eq!(quiet(&mut w, 2, vec![], t0 + Duration::from_secs(1) + STALL)["t1"].word, "idle", "then silence ends the turn");
}

/// What `tend_terminals` does with one terminal on one tick: the word from
/// the watcher when it owns the terminal, else core's, and core's turn with
/// the shared seen turn only for a terminal that reports.
fn tick(model: &mut crate::model::Model, watcher: &Watcher, addr: &str, word: &str, core_turn: u64, label: Option<u64>, looking: bool) {
    model.set_word(addr, Some(word.into()), None, looking);
    model.set_turn(addr, core_turn, if watcher.owns(addr) { None } else { label });
}

#[test]
fn an_unreported_agent_is_seen_locally_and_a_report_takes_over_cleanly() {
    let r = bundled();
    let box_ = "────────────────────────────────────────\n❯\n────────────────────────────────────────";
    let working = format!("✻ Thinking… (12s · ↑ 1.2k tokens)\n{box_}\n  ⏵⏵ auto mode on (shift+tab to cycle) · esc to interrupt");
    let mut rec = record(Value::Null, "claude", &["claude"]);
    rec["turn"] = 4.into();
    rec["labels"] = json!({ "herdr.seen": "4" });
    let mut live = HashMap::from([("t1".to_owned(), rec)]);
    let mut w = Watcher::default();
    let mut model = crate::model::Model::default();
    let t0 = Instant::now();
    let mut words = Vec::new();
    for (i, screen) in [working.as_str(), box_, box_, box_].iter().enumerate() {
        let lines: Vec<String> = screen.lines().map(str::to_owned).collect();
        let u = w.update(&r, &live, t0 + Duration::from_millis(500 * i as u64), |_| Some(i as u64), |_, _| (lines.clone(), String::new()));
        assert!(w.owns("t1"));
        tick(&mut model, &w, "t1", u["t1"].word, 4, Some(4), false);
        words.push((u["t1"].word, model.status("t1")));
    }
    assert_eq!(words, [("working", Some("working")), ("working", Some("working")), ("working", Some("working")), ("idle", Some("done"))], "a turn finished unwatched is done, though core's turn never moved");
    model.mark_seen(&["t1".to_owned()]);
    assert_eq!(model.status("t1"), Some("idle"), "seen here, with nothing written to the shared label");

    // Its hooks start reporting: core's turns and the label decide again.
    live.get_mut("t1").unwrap()["status"] = "working".into();
    assert!(w.update(&r, &live, t0 + Duration::from_secs(3), |_| Some(9), |_, _| (vec![], String::new())).is_empty());
    assert!(!w.owns("t1"));
    tick(&mut model, &w, "t1", "working", 4, Some(4), false);
    tick(&mut model, &w, "t1", "idle", 5, Some(4), false);
    assert_eq!(model.status("t1"), Some("done"), "the first reported turn is unseen: the label never ran ahead");
    tick(&mut model, &w, "t1", "idle", 5, Some(5), false);
    assert_eq!(model.status("t1"), Some("idle"));
}

#[test]
fn rules_read_digits_spaces_and_words_as_ascii_like_re2() {
    assert_eq!(ascii_classes(r"^\s*\d+\w\b"), r"^[\t\n\f\r ]*[0-9]+[0-9A-Za-z_](?-u:\b)");
    assert_eq!(ascii_classes(r"[^\s─-]\S"), r"[^\t\n\f\r ─-][^\t\n\f\r ]");
    assert_eq!(ascii_classes(r"[]a]\x{276F}\\d"), r"[]a]\x{276F}\\d", "a leading ] and escaped backslashes stay");
    assert_eq!(ascii_classes(r"[[:alpha:]\d]"), r"[[:alpha:]0-9]");
    assert_eq!(ascii_classes(r"[^\S─]"), r"[^[^\t\n\f\r ]─]", "a negated class nests inside a bracket");
    assert_eq!(ascii_classes(r"[a--b&&c~~]"), r"[a-\-b\&\&c\~\~]", "Rust's set operators stay characters");
    let re = Regex::new(&ascii_classes(r"^[a&&b]+$")).unwrap();
    assert!(re.is_match("a&b") && !re.is_match("c"));
    let re = Regex::new(&ascii_classes(r"^[\W]$")).unwrap();
    assert!(re.is_match("é") && !re.is_match("_"), "RE2's \\W is non-ASCII-word, so é is in it");
    let re = Regex::new(&ascii_classes(r"^\d+$")).unwrap();
    assert!(re.is_match("42"));
    assert!(!re.is_match("٤٢"), "Arabic-Indic digits are not \\d in RE2");
}

#[test]
fn globs_match_by_component_without_blowing_up() {
    assert!(glob_path("**/share/claude/versions/*", "/Users/me/.local/share/claude/versions/2.1.280"));
    assert!(glob_path("**/claude", "/opt/homebrew/bin/claude"));
    assert!(glob_path("bin/claude", "/opt/homebrew/bin/claude"), "a relative pattern matches a suffix");
    assert!(!glob_path("/bin/claude", "/opt/homebrew/bin/claude"), "an absolute one the whole path");
    assert!(!glob_path("**/claude", "/opt/claude/bin"));
    assert!(glob_path("**/python*", "/usr/bin/python3.12"));
    assert!(glob_path("**/.cursor/**/agent", "/home/u/.cursor/a/b/agent"));
    assert!(glob_one(b"a*b?c", b"axxbyc") && !glob_one(b"a*b?c", b"axxbc"));
    let started = Instant::now();
    let pattern = "**/".repeat(40) + &"*a".repeat(40);
    let path = "/x".repeat(60) + "/" + &"a".repeat(200) + "b";
    assert!(!glob_path(&pattern, &path));
    assert!(started.elapsed() < Duration::from_millis(200), "took {:?}", started.elapsed());
}

/// tuios reads the token an interpreter runs as the program's name, so a
/// script named after an agent counts as it: deliberate there, kept here.
#[test]
fn a_script_named_after_an_agent_is_that_agent() {
    let r = bundled();
    assert_eq!(id(&r, "python3", &["python3", "claude.py"]).as_deref(), Some("claude-code"));
    assert_eq!(id(&r, "bash", &["bash", "./claude"]).as_deref(), Some("claude-code"));
}
