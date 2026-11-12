//! The window. The chrome (`layouts/<name>.json`) is a tree of the locked
//! primitives; a `{ "tab": "active" }` block shows the selected
//! tab's terminals as it arranges them. Every visible part that isn't a
//! terminal is a mod. Commands (commands.rs) are the one way to change
//! anything: keys, mods, menus and agents all run them.

use crate::git_status;
use crate::hc::Core;
mod cursor_shapes;
mod cursor_trail;
mod floats;
mod follow_mouse;
mod pane_floats;
mod movable;
mod pane_drag;
mod pane_size;
mod titlebar;

use crate::layout::{Axis, Background as Paint, Content, Fit, HeaderContent, Kind, Line, Node, PaneFrame, Size as Span};
use crate::model::{self, Tab, Model};
use crate::mods::{self, Found, Instance, Request};
use crate::settings;
use crate::layout_sync;
use crate::terminal::{TerminalEvent, TerminalView, scroll_target};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_shell::action::ShellAction;
use gpui_shell::{ShellRoot, ShellRuntime, Watcher};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

const HEADER_HEIGHT: f32 = 30.0;
const HANDLE: f32 = 6.0;

// The three native buttons plus their trailing breathing room (also used by the tabs mod).
const TRAFFIC_LIGHT_WIDTH: f32 = 64.0;

// A toast is as wide as what it says, within these; its height is fixed, so toasts stack.
const TOAST_MIN_WIDTH: f32 = 200.0;
const TOAST_MAX_WIDTH: f32 = 340.0;
const TOAST_HEIGHT: f32 = 40.0;
const TOAST_HEIGHT_WITH_BODY: f32 = 72.0;
/// The other designs' boxes (`toast_box`), px at zoom 1.
const PILL_MIN_WIDTH: f32 = 96.0;
const PILL_HEIGHT: f32 = 32.0;
const ICON_CARD_MIN_WIDTH: f32 = 300.0;
const ICON_CARD_MAX_WIDTH: f32 = 400.0;
const ICON_CARD_HEIGHT: f32 = 76.0;
/// The icon card's Show and Dismiss buttons, for a toast with an action.
const ICON_CARD_ACTIONS: f32 = 36.0;
const BANNER_HEIGHT: f32 = 40.0;
/// The time-left bar along a toast's bottom (`toast_progress`), px at zoom 1.
const PROGRESS_HEIGHT: f32 = 2.0;
/// How often the time-left bar steps: about this many steps over the
/// shortest toast counting down, within these bounds.
const PROGRESS_STEPS: u32 = 48;
const PROGRESS_STEP_MIN: Duration = Duration::from_millis(100);
const PROGRESS_STEP_MAX: Duration = Duration::from_millis(500);
/// The most toasts shown at once: one more closes the oldest.
const TOAST_LIMIT: usize = 5;
/// Agent alerts of one kind this close together share a toast.
const ALERT_JOIN: Duration = Duration::from_secs(3);
/// The most agents a shared alert toast names; the rest are counted.
const ALERT_NAMES: usize = 4;
/// How long after a start or reconnect terminals may still come back as bare
/// shells and be offered for resuming.
const RESUME_OFFER_WINDOW: Duration = Duration::from_secs(60);

/// How long a toast stays: its `timeout`, else longer for errors and warnings,
/// scaled by the person's `toast_duration`; none: until clicked.
fn toast_timeout(notification: &Value, duration: settings::ToastDuration) -> Option<Duration> {
    if let Some(ms) = notification.get("timeout").and_then(Value::as_f64) {
        return Some(Duration::from_millis(ms as u64));
    }
    let ms = match notification.get("level").and_then(Value::as_str) {
        Some("error" | "warning") => 8000.0,
        _ => 5000.0,
    };
    duration.scale().map(|k| Duration::from_millis((ms * k) as u64))
}

/// An agent alert's toast, which alerts of the same kind arriving soon after
/// join instead of stacking a toast each ("3 agents finished").
struct AlertToast {
    alert: model::Alert,
    /// Newest first, each agent once.
    agents: Vec<AlertedAgent>,
    /// When the last alert joined.
    joined: Instant,
}

#[derive(Clone)]
struct AlertedAgent {
    address: String,
    name: String,
    /// "space / tab".
    place: Option<String>,
}

impl AlertToast {
    /// What the toast mod draws. One agent: its name and place, as ever. More:
    /// how many and who, newest first. A click goes to the newest.
    fn notification(&self) -> Value {
        let newest = &self.agents[0];
        let (title, body) = match self.agents.len() {
            1 => (crate::alert::title(self.alert, &newest.name), json!(newest.place)),
            n => {
                let names: Vec<&str> = self.agents.iter().take(ALERT_NAMES).map(|a| a.name.as_str()).collect();
                let more = n.saturating_sub(ALERT_NAMES);
                let body = if more > 0 { format!("{} and {more} more", names.join(", ")) } else { names.join(", ") };
                (crate::alert::merged_title(self.alert, n), json!(body))
            }
        };
        let level = if self.alert.needs_you() { "error" } else { "info" };
        json!({
            "title": title, "body": body, "level": level,
            "action": { "name": "agent.focus", "args": { "terminal": newest.address } },
        })
    }
}

/// A toast's props with the level mark it draws (the person's `toast_mark`
/// as its design takes it, `ToastDesign::mark`) and `toast_design` (and the
/// terminal design's font), which the toast mod draws: carried in the props
/// so a toast's render reads no settings.
fn with_toast_look(props: &mut Value) {
    let s = settings::get();
    props["toast_mark"] = serde_json::to_value(s.toast_design.mark(s.toast_mark)).unwrap_or(Value::Null);
    props["toast_design"] = json!(s.toast_design.name());
    match props.as_object_mut() {
        Some(p) if s.toast_design == settings::ToastDesign::Terminal => p.insert("toast_font".into(), json!(s.font_family)),
        Some(p) => p.remove("toast_font"),
        None => None,
    };
}

/// The theme colour of a toast's level: its mark, the outline's border and the time-left bar.
fn toast_level(notification: &Value) -> &'static str {
    match notification.get("level").and_then(Value::as_str) {
        Some("error") => "toast_error",
        Some("warning") => "toast_warning",
        Some("success") => "toast_success",
        _ => "toast_info",
    }
}

/// A toast's box in `design`, px at zoom 1: its narrowest, its widest (as
/// wide as what it says between them) and its height. A banner is as wide
/// as the window.
fn toast_box(design: settings::ToastDesign, notification: &Value) -> (f32, f32, f32) {
    use settings::ToastDesign;
    match design {
        ToastDesign::Card | ToastDesign::Terminal | ToastDesign::Outline => (TOAST_MIN_WIDTH, TOAST_MAX_WIDTH, toast_height(notification)),
        ToastDesign::Pill => (PILL_MIN_WIDTH, TOAST_MAX_WIDTH, PILL_HEIGHT),
        ToastDesign::IconCard => {
            let action = notification.pointer("/action/name").and_then(Value::as_str).is_some_and(|n| !n.is_empty());
            (ICON_CARD_MIN_WIDTH, ICON_CARD_MAX_WIDTH, ICON_CARD_HEIGHT + if action { ICON_CARD_ACTIONS } else { 0.0 })
        }
        ToastDesign::Banner => (0.0, 0.0, BANNER_HEIGHT),
    }
}

/// One line without a body, two with one: a body as the toast mod draws one
/// (text that isn't blank, or anything else JavaScript calls true).
fn toast_height(notification: &Value) -> f32 {
    let body = match notification.get("body") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|n| n != 0.0),
        // JavaScript's trim() also takes the byte-order mark; Rust's doesn't.
        Some(Value::String(s)) => !s.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}').is_empty(),
        Some(Value::Array(_) | Value::Object(_)) => true,
    };
    if body { TOAST_HEIGHT_WITH_BODY } else { TOAST_HEIGHT }
}

fn traffic_light_divider_inset(n: &Node, axis: Axis, index: usize, macos: bool, zoom: f32) -> f32 {
    if !macos || axis != Axis::Row || n.titlebar != Some(crate::layout::Titlebar::Layout) {
        return 0.0;
    }
    let Some([x, _]) = n.traffic_lights else { return 0.0 };
    let shown: Vec<_> = n.children().iter().filter(|child| !child.hidden).collect();
    let mut edge = n.padding.map_or(0.0, |padding| padding.axes().1)
        + index as f32 * (n.gap.unwrap_or(0.0) + n.divider.as_ref().map_or(0.0, |line| line.width));
    for child in shown.iter().take(index + 1) {
        let Some(Span::Px { px }) = child.size else { return 0.0 };
        edge += px;
    }
    if edge * zoom < x || edge * zoom > x + TRAFFIC_LIGHT_WIDTH {
        return 0.0;
    }
    // Keep the toolbar continuous above the rail; the divider starts with the content.
    shown.iter().skip(index).take(2).filter_map(|column| {
        let header = column.children().iter().find(|child| !child.hidden)?;
        match header.size {
            Some(Span::Px { px }) if header.window_drag => Some(px),
            _ => None,
        }
    }).reduce(f32::min).unwrap_or(0.0)
}

fn separator_line(row: bool, width: f32, color: Hsla, top: f32) -> Div {
    div().bg(color)
        .when(row, |d| d.w(px(width)))
        .when(row && top == 0.0, |d| d.h_full())
        .when(row && top > 0.0, |d| d.absolute().top(px(top)).bottom_0())
        .when(!row, |d| d.h(px(width)).w_full())
}

fn c(name: &str) -> Hsla {
    settings::color_or_literal(name)
}

fn file(name: &str) -> PathBuf {
    mods::app_dir().join(name)
}

fn is_dark(a: gpui::WindowAppearance) -> bool {
    matches!(a, gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark)
}

/// A layout's theme after picking `name`: it takes the half of its own
/// lightness; the other half stays (a single theme of the other lightness
/// becomes that half).
fn with_theme(was: Option<crate::layout::ThemeChoice>, name: String) -> crate::layout::ThemeChoice {
    use crate::layout::ThemeChoice::{One, Pair};
    let light = settings::theme_is_light(&name);
    let (l, d) = match was {
        Some(Pair { light, dark }) => (light, dark),
        Some(One(x)) if settings::theme_is_light(&x) == light => return One(name),
        Some(One(x)) => (x.clone(), x),
        None => (settings::DEFAULT_LIGHT.into(), settings::DEFAULT_DARK.into()),
    };
    if light { Pair { light: name, dark: d } } else { Pair { light: l, dark: name } }
}

fn mtime(p: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(p).ok()?.modified().ok()
}

pub fn shell() -> String {
    if cfg!(windows) {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let pwsh = std::env::split_paths(&path).any(|d| d.join("pwsh.exe").is_file());
        return if pwsh { "pwsh.exe" } else { "powershell.exe" }.into();
    }
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
}

/// The label on every `terminal.popup` terminal: the app's process id. A
/// popup's terminal ends with the popup, so one listed after the app that
/// started it is gone (a crash, a forced quit) is an orphan nobody can see.
const POPUP_LABEL: &str = "herdr.popup";

/// Popup terminals another app process started and left behind: local ones
/// whose `POPUP_LABEL` names a process that is no longer running, and that
/// nobody kept (a popup kept as a tab is the person's).
fn orphan_popups(
    live: &HashMap<String, Value>,
    own_pid: u32,
    running: impl Fn(u32) -> bool,
) -> impl Iterator<Item = &String> {
    live.iter()
        .filter(|(address, record)| !address.contains('/') && record["keep"] != Value::Bool(true))
        .filter(move |(_, record)| {
            let owner = record
                .pointer(&format!("/labels/{POPUP_LABEL}"))
                .and_then(Value::as_str)
                .and_then(|p| p.parse::<u32>().ok());
            owner.is_some_and(|pid| pid != own_pid && !running(pid))
        })
        .map(|(address, _)| address)
}

/// Whether a process is running. Windows can't tell here, so nothing counts as gone.
fn process_running(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return true;
        };
        // EPERM: running, as another user.
        let signalled = unsafe { libc::kill(pid, 0) } == 0;
        signalled || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

/// A `terminal.popup {wait}` terminal: who waits on it, and how it ended.
#[derive(Default)]
struct Waiting {
    /// Listed live at least once: missing from a later listing means it's gone.
    seen: bool,
    /// The answer, and when it was ready: kept for a waiter on its way, for `WAIT_KEPT`.
    ended: Option<(Value, Instant)>,
    replies: Vec<std::sync::mpsc::Sender<Value>>,
}

/// The last terminal `terminal.popup` started: the next popup ends its
/// program. Addresses come back (a core that restarts counts from the start
/// again), so `created` (the core's `created_ms`) tells it from a later
/// terminal at the same address.
struct Popped {
    address: String,
    created: Option<u64>,
    /// Listed live at least once: missing from a later listing means it's gone.
    seen: bool,
}

impl Popped {
    /// Whether a listed terminal record is this one (an unknown time can't tell).
    fn is(&self, record: &Value) -> bool {
        match (self.created, record["created_ms"].as_u64()) {
            (Some(mine), Some(theirs)) => mine == theirs,
            _ => true,
        }
    }
}

/// How long a finished `terminal.popup {wait}` keeps its exit code (and its
/// terminal) for a waiter that hasn't asked yet.
const WAIT_KEPT: Duration = Duration::from_secs(60);

/// `terminal.popup {hold}`: the program, then its exit status until a key.
/// Ctrl-C reaches the program but not the wrapper, so the status still shows.
fn popup_hold_argv(argv: Vec<String>) -> Result<Vec<String>, String> {
    if cfg!(windows) {
        return Err("terminal.popup hold needs sh, which Windows lacks".into());
    }
    // One key without Enter, in any sh (dash has no `read -n`).
    let script = r#"trap : INT; "$@"; code=$?; printf "\n[exit %s] press a key to close" "$code"; old=$(stty -g 2>/dev/null); stty -icanon -echo 2>/dev/null; dd bs=1 count=1 >/dev/null 2>&1; stty "$old" 2>/dev/null; exit "$code""#;
    Ok(["sh", "-c", script, "popup"].into_iter().map(str::to_owned).chain(argv).collect())
}

fn unix_agent_argv(sh: String, cmd: Vec<String>, login: bool) -> Vec<String> {
    let fish = std::path::Path::new(&sh).file_name().and_then(|s| s.to_str()) == Some("fish");
    let script = match (fish, login) {
        (true, true) => "command $argv[2..]; exec $argv[1] -li",
        (true, false) => "command $argv[2..]; exec $argv[1] -i",
        (false, true) => "\"$@\"; exec \"$0\" -li",
        (false, false) => "\"$@\"; exec \"$0\" -i",
    };
    let mut run = vec![sh.clone(), if login { "-lic" } else { "-ic" }.into(), script.into(), sh];
    run.extend(cmd);
    run
}

fn launch_failure(record: &Value, output: &str) -> Option<String> {
    let code = record["exit_code"].as_i64().filter(|c| *c != 0)?;
    let elapsed = record["finished_ms"].as_u64()?.checked_sub(record["created_ms"].as_u64()?)?;
    if record["reason"].as_str() != Some("exited") || elapsed > 2000 {
        return None;
    }
    let lines = output.lines().filter(|l| !l.trim().is_empty()).collect::<Vec<_>>();
    let tail = lines[lines.len().saturating_sub(5)..].join("\n");
    Some(if tail.is_empty() { format!("Exit code {code}") } else { format!("Exit code {code}\n{tail}") })
}

/// The shipped layouts (defaults/layouts): `main` is Herdr's window (spaces
/// sidebar, tab bar).
const LAYOUTS: &[(&str, &str)] = &[
    ("main", include_str!("../defaults/layouts/main.json")),
    ("terminals", include_str!("../defaults/layouts/terminals.json")),
    ("win98", include_str!("../defaults/layouts/win98.json")),
];

/// Pictures shipped layouts name as their `preview`, written beside them.
const LAYOUT_PREVIEWS: &[(&str, &str)] = &[("win98.svg", include_str!("../defaults/layouts/win98.svg"))];

pub fn shipped_layouts() -> Vec<&'static str> {
    LAYOUTS.iter().map(|(n, _)| *n).collect()
}

fn shipped_layout(name: &str) -> &'static str {
    LAYOUTS.iter().find(|(n, _)| *n == name).unwrap_or(&LAYOUTS[0]).1
}

/// A layout as shipped (main for names Herdr doesn't ship).
fn default_layout(name: &str) -> Node {
    serde_json::from_str(shipped_layout(name)).expect("defaults/layouts are valid")
}

fn layout_path(name: &str) -> PathBuf {
    file(&format!("layouts/{name}.json"))
}

/// Layout names on disk, in order: main first, then the rest alphabetically.
/// A layout whose mods are gone (Zen after the update) isn't offered;
/// `layout.switch {name}` still reaches it.
pub(crate) fn layout_names() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(file("layouts"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.strip_suffix(".json").map(str::to_owned))
        .filter(|n| missing_mods(n).is_empty())
        .collect();
    names.sort_by_key(|n| (n != "main", n.clone()));
    names
}

fn state_path(name: &str) -> PathBuf {
    file(&format!("layouts/.state/{name}.json"))
}

/// The clicks on a layout (layout_sync.rs), none if unreadable.
fn read_state(name: &str) -> layout_sync::State {
    std::fs::read_to_string(state_path(name)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn state_text(s: &layout_sync::State) -> String {
    serde_json::to_string_pretty(s).unwrap_or_default() + "\n"
}

/// Where to set `path` aside: `<path>.<tag>-<time>`, with `-2`, `-3` ... when
/// that's taken, so an older one is never replaced.
pub(crate) fn aside(path: &std::path::Path, tag: &str) -> PathBuf {
    let stamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    (1..).map(|i| path.with_file_name(if i == 1 { format!("{name}.{tag}-{stamp}") } else { format!("{name}.{tag}-{stamp}-{i}") })).find(|p| !p.exists()).unwrap_or_else(|| path.to_path_buf())
}

/// Writes through a temporary file and a rename, so a reader never sees half
/// a file. Says whether it worked.
fn write_atomic(path: &std::path::Path, text: &str) -> bool {
    match crate::persist::write_atomic(path, text) {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!("cannot write {}: {error}", path.display());
            false
        }
    }
}

/// The mods a layout file names that aren't in mods/ (frame mods are always there).
fn missing_mods(name: &str) -> Vec<String> {
    fn walk(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(o) => o.iter().for_each(|(k, v)| match v {
                Value::String(s) if k == "mod" => out.push(s.clone()),
                _ => walk(v, out),
            }),
            Value::Array(a) => a.iter().for_each(|v| walk(v, out)),
            _ => {}
        }
    }
    let Some(layout) = std::fs::read_to_string(layout_path(name)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { return vec![] };
    let mut used = Vec::new();
    walk(&layout, &mut used);
    used.sort();
    used.dedup();
    used.retain(|m| !m.starts_with("frame-") && !mods::mods_dir().join(m).is_dir());
    used
}

fn install_layouts() {
    if let Err(e) = mods::sync_layouts(&mods::app_dir(), LAYOUTS) {
        tracing::warn!("cannot install layouts: {e}");
    }
    for (name, text) in LAYOUT_PREVIEWS {
        let _ = std::fs::write(file(&format!("layouts/{name}")), text);
    }
}

fn clone_layout(name: &str, to: &str) -> std::io::Result<()> {
    if !mods::valid_name(name) || !mods::valid_name(to) {
        return Err(std::io::Error::other("names use letters, digits, - or _"));
    }
    let mut node: Node = serde_json::from_str(&std::fs::read_to_string(layout_path(name))?)?;
    node.cloned_from = Some(crate::layout::ClonedFrom { name: name.into(), version: env!("CARGO_PKG_VERSION").into() });
    let text = layout_sync::text_of(&node);
    use std::io::Write;
    let mut dest = std::fs::OpenOptions::new().write(true).create_new(true).open(layout_path(to))?;
    let result = (|| {
        dest.write_all(text.as_bytes())?;
        mods::copy_layout_state(&mods::app_dir(), name, to)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(layout_path(to));
        let _ = std::fs::remove_file(state_path(to));
    }
    result
}

struct Term {
    view: Entity<TerminalView>,
    address: String,
    focus: FocusHandle,
    _sub: Subscription,
}

enum TerminalChange {
    Keep { space: String, tab: String, pane: String, name: String, target: Option<String>, axis: Axis },
    Park,
}

struct PendingTerminalChange {
    change: TerminalChange,
    reply: std::sync::mpsc::Receiver<Result<(), String>>,
}

/// The slot of a terminal shown in a pane in place of its own (`<pane>#swap`).
const SWAP: &str = "#swap";

struct Copy {
    inst: Instance,
    /// Its number for commands (`mods::prepare_load`); 0 if it never loaded.
    id: u64,
    root: Option<Entity<ShellRoot>>,
    _watch: Option<Watcher>,
    error: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum OverlayKind {
    Dialog,
    Popup,
    Toast,
    /// Stays until closed, moves where it's dropped, comes to the front when
    /// pressed (`movable.rs`). Never holds the keyboard on its own.
    Window,
}

impl OverlayKind {
    /// Dialogs and popups hold the keyboard while open (Esc closes them).
    fn modal_keys(self) -> bool {
        matches!(self, OverlayKind::Dialog | OverlayKind::Popup)
    }
}

struct Overlay {
    id: String,
    kind: OverlayKind,
    slot: String,
    at: Point<Pixels>,
    /// As designed; drawn at the current zoom. Up to 1: a share of the window.
    size: Size<f32>,
    expires: Option<Instant>,
    focus: FocusHandle,
    /// Blocks instead of one mod (terminals too), ids prefixed with the overlay's.
    layout: Option<Node>,
    /// `at: "top"`: centered, this far below the window's top.
    top: Option<f32>,
    /// As tall as its content (size.height is the most).
    fit: bool,
    /// A click beside it doesn't close it (the first-run welcome).
    modal: bool,
    /// An error or warning toast: the toast cap closes these last.
    urgent: bool,
    /// A toast's level colour (`toast_level`).
    level: &'static str,
    /// A toast's box in its design (`toast_box`): its narrowest and widest,
    /// and the height its poster set, which wins.
    widths: (f32, f32),
    asked_height: Option<f32>,
    /// When a toast's time began (it restarts when an alert joins), for its time-left bar.
    opened: Instant,
    /// `frame: "none"`: no border, corners, shadow or background; the mod draws its own.
    bare: bool,
    /// A window: where its top left is (None: centered) and its name (`key`).
    place: Option<Point<Pixels>>,
    key: Option<String>,
    /// The app's own screen (a `frame-` mod: What's new, the welcome): only
    /// its own close ends it, never a layout switch.
    frame: bool,
    /// An agent alert's toast, which alerts of the same kind can join.
    alert: Option<AlertToast>,
}

struct Drag {
    container: String,
    index: usize,
    axis: Axis,
}

#[derive(Default)]
struct Watched {
    settings: Option<SystemTime>,
    keys: Option<SystemTime>,
    programs: Option<SystemTime>,
}

pub struct AppWindow {
    core: Arc<Core>,
    runtime: Option<Rc<ShellRuntime>>,
    chrome: Node,
    /// Which layout `chrome` is (`layouts/<name>.json`).
    layout_name: String,
    /// The agent an `{ "agent": "focused" }` block shows (by address); any core
    /// terminal, so sub-agents outside workspaces work too.
    agent: Option<String>,
    /// The agent view that has the keyboard, so it's only taken on change.
    agent_focused: Option<String>,
    /// Panes showing another terminal for now (pane → address), e.g. a sub-agent
    /// looked at in place of its parent. Not saved; the pane keeps its own.
    swapped: HashMap<String, String>,
    float_drag: Option<floats::FloatDrag>,
    pane_drag: Option<pane_drag::PaneDrag>,
    /// Focus follows mouse: the wait for the pane under the pointer, and its timer.
    follow: follow_mouse::Follow,
    follow_timer: Option<Task<()>>,
    pane_grip_hover: Option<String>,
    /// Each pane's top row the grip sits centered in: its top header, else a strip.
    pane_grip_rows: RefCell<HashMap<String, f32>>,
    /// A window (`kind: "window"` overlay) being pressed or dragged.
    win_drag: Option<movable::WinDrag>,
    float_anims: HashMap<String, floats::FloatAnim>,
    float_reveal: HashMap<String, floats::Reveal>,
    float_hover: HashSet<String>,
    /// Bumped on every hover change, so a delayed leave knows it's still current.
    float_leave: HashMap<String, u64>,
    model: Model,
    /// Terminal views by block id (panes and chrome terminal blocks).
    terms: HashMap<String, Term>,
    /// Mod copies by slot (block id, `<stack>#header`, or `overlay:<id>`).
    copies: HashMap<String, Copy>,
    overlays: Vec<Overlay>,
    folders: BTreeMap<String, Found>,
    /// Each look at the mod folders counts here, so a background look that
    /// started before a newer one is dropped (`sync_folders_later`).
    folder_scans: u64,
    folder_scan_running: bool,
    /// Mods placed once; later the layout decides where (or whether) they show.
    seen: BTreeSet<String>,
    bounds: Rc<RefCell<HashMap<String, Bounds<Pixels>>>>,
    /// Where the window's top chrome starts and ends, as last painted
    /// (`top_chrome`): a top toast stack starts below it.
    top_chrome: Rc<std::cell::Cell<(f32, f32)>>,
    /// Whether `render_node` is drawing the main layout (`chrome`), whose bars
    /// `top_chrome` measures, rather than an overlay's.
    painting_main: std::cell::Cell<bool>,
    /// The terminal cell size, measured each frame the shown tab has floating panes.
    float_cell: std::cell::Cell<Size<Pixels>>,
    /// While focus follows the pointer: `pane.focus` leaves a floating pane where it is in z-order.
    hover_focus: bool,
    hidden_fit: pane_size::HiddenFit,
    drag: Option<Drag>,
    written_layout: String,
    /// The shown layout as its file has it; `chrome` is this plus the clicks
    /// (layout_sync.rs).
    file_chrome: Node,
    written_state: String,
    picked_theme: Option<crate::layout::ThemeChoice>,
    /// Whether the shown layout's file read and parsed (or is simply
    /// missing): a broken one is never written over.
    layout_file_ok: bool,
    written_view: String,
    /// settings.json: zoom and sounds.
    person: settings::Person,
    /// What the last command made or why it didn't (for callers that wait for it).
    reply: Option<Value>,
    /// The `where` label last written per terminal address.
    written_where: HashMap<String, String>,
    /// `herdr.seen` turns written but not yet back from the core.
    written_seen: HashMap<String, u64>,
    /// Agents the person is looking at, and the turn they were shared as seen at.
    looked: HashMap<String, u64>,
    watched: Watched,
    /// Core's terminal records by address.
    live: HashMap<String, Value>,
    listed_once: bool,
    /// Resumable programs on record (resume.rs).
    resume: crate::resume::Tracker,
    /// Since when an offer to resume them is due (after a start or reconnect).
    resume_offer: Option<Instant>,
    /// Live folders by address (from the shell process).
    cwds: HashMap<String, String>,
    /// The automatic names panes show (`pane_auto_names`), by pane, as last published.
    auto_names: HashMap<String, String>,
    /// What each pane is called (pane_label.rs), by pane, as last published.
    labels: HashMap<String, String>,
    /// Agent sessions' own names, by terminal (pane_label.rs).
    session_names: crate::pane_label::Sessions,
    /// When each terminal's repo and branch were last read, and what it was doing.
    /// Whether each terminal was busy at the last look at its folder.
    folders_busy: HashMap<String, bool>,
    /// When Herdr started each terminal: a listing already in flight can't know them yet.
    born: RefCell<HashMap<String, Instant>>,
    /// `terminal.popup {wait}` terminals, until a waiter has their exit code.
    waits: HashMap<String, Waiting>,
    /// The terminal the peek overlay shows (or last showed).
    peeked: Option<String>,
    /// The last terminal `terminal.popup` started, shown or hidden, until it is
    /// ended or gone: closing the peek overlay, or the next popup, ends its program.
    popped: Option<Popped>,
    /// Popups on their way into a tab (`terminal.keep`), until the keep lands.
    keeping_popups: HashMap<String, Popped>,
    /// Whether the first listing's orphaned popups (`POPUP_LABEL`) were ended.
    reaped_orphans: bool,
    /// Connection epoch: ignore replies from an old connection.
    core_generation: u64,
    checking_closed: bool,
    documents_watching: bool,
    draining: bool,
    changing_terminals: HashSet<String>,
    /// Closed terminals stay excluded until kill completes and a listing drops them.
    closing_terminals: HashMap<String, bool>,
    pending_terminals: std::collections::BTreeMap<String, PendingTerminalChange>,
    /// Each machine owns a document; the empty key is this core.
    documents: std::collections::BTreeMap<String, crate::mirror_sync::Slot>,
    initial_view: Option<crate::mirror::View>,
    /// Places the person settled on, shared as the core document `herdr.focus`.
    focus_history: crate::focus_history::History,
    pointer: Point<Pixels>,
    /// Where a press on a `window_drag` node started, until it moves the window.
    drag_from: Option<Point<Pixels>>,
    titlebar_applied: Option<(crate::layout::Titlebar, Option<[f32; 2]>)>,
    focused_pane: Option<String>,
    /// The cursor trail being drawn, and the (tab, pane) focused at the last draw (window/cursor_trail.rs).
    trail: Option<cursor_trail::Trail>,
    trail_seen: Option<cursor_trail::Seen>,
    /// The window's drawing below the trail and above it, each drawn again
    /// only when the window is (a frame the trail alone asked for reuses
    /// them), and the trail's own layer between them.
    lower: Entity<Body>,
    upper: Entity<Body>,
    trail_layer: Entity<cursor_trail::TrailLayer>,
    /// The mod block holding the keyboard for directional navigation.
    focused_block: Option<String>,
    synced_model_pane: Option<String>,
    /// A terminal block in a window (an overlay's layout) holding the keyboard.
    /// `focused()` reports it like a layout terminal block; the focused pane stays.
    window_terminal: Option<String>,
    /// Whether the last sync showed the tab, to hand it the keyboard back when a stack shows it again.
    tab_shown: bool,
    pane_history: Vec<String>,
    window_active: bool,
    /// The Dock's count of agents waiting on the person (dock_badge.rs).
    dock_badge: crate::dock_badge::Badge,
    /// The next time a pane's `{elapsed}` reads differently, and the timer
    /// that publishes then (header_info.rs).
    header_tick: Option<(u64, Task<()>)>,
    /// The time-left bars' step timer, while a toast counting down shows one (`tick_progress`).
    progress_tick: Option<Task<()>>,
    /// The banner shown when `count_banners` last looked, to restart one coming forward.
    banner_shown: Option<String>,
    /// Agents with a system notification up (alert.rs), taken down once old
    /// news (`Notified::withdraw`).
    notified: crate::alert::Notified,
    /// Which agent each `system.notify` notification focuses when clicked.
    notify_clicks: crate::alert::NotifyClicks,
    /// Agents that don't report (OSC 7501), read off their screens (harness.rs).
    unreported: crate::harness::Watcher,
    /// Why `alert_command` didn't start, each said once in a toast.
    alert_command_errors: HashSet<String>,
    /// The system's appearance; a layout with a light and a dark theme follows it.
    system_dark: bool,
    /// The OS tells us when files change (`files.rs`); else the disk is polled.
    file_events: Option<crate::files::FileEvents>,
    polls: u64,
    /// The downloaded update the frame's pill offers, and the one closed for now.
    /// What's waiting, in words (`update_label`).
    update_ready: Option<String>,
    update_dismissed: Option<String>,
    update_success: bool,
    update_status: String,
    /// The "Copied" toast on screen: the next copy replaces it.
    copied_toast: Option<String>,
    focus: FocusHandle,
    _tasks: Vec<Task<()>>,
    /// Gives the keyboard back when what had it leaves the screen (`restore_focus`).
    _focus_lost: Option<Subscription>,
    /// Shades the sidebars as the window gains or loses focus (`sidebar_shade`).
    _activation: Option<Subscription>,
    /// Remembers where the window is (`place.rs`); the write waiting to settle.
    _place: Option<Subscription>,
    place_write: Option<Task<()>>,
}

impl AppWindow {
    pub fn new(core: Arc<Core>, requests: async_channel::Receiver<Request>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let runtime = ShellRuntime::new(cx).map_err(|e| tracing::error!("script runtime failed, mods are off: {e:#}")).ok();
        let file_events = crate::files::watch(&mods::app_dir(), #[cfg(test)] cx.background_executor().clone());
        if let (Some(runtime), Some(events)) = (&runtime, &file_events) {
            runtime.set_source_revision_provider(events.revision_provider());
        }
        mods::note_fonts(cx);
        let seen = std::fs::read_to_string(file("seen.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let me = cx.weak_entity();
        let mut w = AppWindow {
            core,
            runtime,
            chrome: default_layout("main"),
            layout_name: "main".into(),
            agent: None,
            agent_focused: None,
            swapped: HashMap::new(),
            float_drag: None,
            pane_drag: None,
            follow: Default::default(),
            follow_timer: None,
            pane_grip_hover: None,
            pane_grip_rows: RefCell::default(),
            win_drag: None,
            float_anims: HashMap::new(),
            float_reveal: HashMap::new(),
            float_hover: HashSet::new(),
            float_leave: HashMap::new(),
            model: Model::default(),
            terms: HashMap::new(),
            copies: HashMap::new(),
            overlays: Vec::new(),
            folders: scan_folders(mods::scan()).0,
            folder_scans: 0,
            folder_scan_running: false,
            seen,
            bounds: Default::default(),
            top_chrome: Default::default(),
            painting_main: Default::default(),
            float_cell: Default::default(),
            hover_focus: false,
            hidden_fit: Default::default(),
            drag: None,
            written_layout: String::new(),
            file_chrome: default_layout("main"),
            written_state: String::new(),
            picked_theme: None,
            layout_file_ok: true,
            written_view: String::new(),
            person: settings::Person::default(),
            reply: None,
            written_where: HashMap::new(),
            written_seen: HashMap::new(),
            looked: HashMap::new(),
            watched: Watched::default(),
            live: HashMap::new(),
            listed_once: false,
            dock_badge: Default::default(),
            header_tick: None,
            progress_tick: None,
            banner_shown: None,
            resume: crate::resume::Tracker::load(&resume_path()),
            resume_offer: Some(Instant::now()),
            cwds: HashMap::new(),
            auto_names: HashMap::new(),
            labels: HashMap::new(),
            session_names: Default::default(),
            folders_busy: HashMap::new(),
            born: RefCell::new(HashMap::new()),
            waits: HashMap::new(),
            peeked: None,
            popped: None,
            keeping_popups: HashMap::new(),
            reaped_orphans: false,
            core_generation: 0,
            checking_closed: false,
            documents_watching: false,
            draining: false,
            changing_terminals: HashSet::new(),
            closing_terminals: HashMap::new(),
            pending_terminals: Default::default(),
            documents: {
                let mut local = crate::mirror_sync::Slot::default();
                local.connection(true, None, None);
                [(String::new(), local)].into()
            },
            initial_view: std::fs::read_to_string(file("view.json")).ok().and_then(|s| serde_json::from_str(&s).ok()),
            focus_history: crate::focus_history::History::new(),
            pointer: Point::default(),
            drag_from: None,
            titlebar_applied: None,
            focused_pane: None,
            trail: None,
            trail_seen: None,
            lower: cx.new(|_| Body {
                app: me.clone(),
                upper: false,
            }),
            upper: cx.new(|_| Body {
                app: me.clone(),
                upper: true,
            }),
            trail_layer: cx.new(|_| cursor_trail::TrailLayer { app: me.clone() }),
            focused_block: None,
            synced_model_pane: None,
            window_terminal: None,
            tab_shown: true,
            pane_history: Vec::new(),
            window_active: true,
            notified: Default::default(),
            notify_clicks: crate::alert::NotifyClicks::default(),
            unreported: Default::default(),
            alert_command_errors: HashSet::new(),
            system_dark: is_dark(window.appearance()),
            file_events,
            polls: 0,
            update_ready: None,
            update_dismissed: None,
            update_success: false,
            update_status: String::new(),
            copied_toast: None,
            focus: cx.focus_handle(),
            _tasks: Vec::new(),
            _focus_lost: None,
            _activation: None,
            _place: None,
            place_write: None,
        };
        w._focus_lost = Some(cx.on_focus_lost(window, |this, window, cx| this.restore_focus(window, cx)));
        settings::set_window_active(window.is_window_active());
        w._activation = Some(cx.observe_window_activation(window, |this, window, cx| this.shade_sidebar(window, cx)));
        w._place = Some(cx.observe_window_bounds(window, |this, window, cx| {
            if let Some(t) = crate::place::changed(window, cx) {
                this.place_write = Some(t);
            }
        }));
        let closing = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            // macOS: the red button hides the app instead, so alerts keep ringing;
            // the Dock icon or a new launch brings it back, Cmd+Q quits.
            if cfg!(target_os = "macos") {
                cx.hide();
                return false;
            }
            let _ = closing.update(cx, |this, cx| {
                let deadline = std::time::Instant::now() + Duration::from_millis(250);
                this.drain_documents(window, cx);
                this.close_mods_now(deadline, window, cx);
                this.end_popups_now(deadline);
            });
            crate::place::save(window, cx);
            true
        });
        settings::install_themes();
        crate::programs::install_icons();
        w.reload_config(window, cx);
        install_layouts();
        let name = mods::shared("layout").and_then(|v| v.as_str().map(str::to_owned)).filter(|n| layout_path(n).exists());
        // A layout whose mods are gone (Zen, T4, ADE after the update took
        // theirs) starts on Herdr's instead of a wall of "did not load".
        let gone = name.as_deref().map(missing_mods).unwrap_or_default();
        w.load_layout(if gone.is_empty() { name.as_deref().unwrap_or("main") } else { "main" });
        w.apply_look(window, cx);
        w.show_layout_notes(window, cx);
        if let (Some(n), false) = (&name, gone.is_empty()) {
            let note = json!({ "title": format!("{n} needs mods that aren't here"), "body": format!("Showing Herdr instead. Missing: {}. Your {n} layout file is still there.", gone.join(", ")), "level": "info", "timeout": 12000 });
            w.open_overlay(mods::new_overlay_id(), json!({ "kind": "toast", "notification": note }), window, cx);
        }

        // Clicking a system notification goes there, as clicking the toast does.
        let (handle, this) = (window.window_handle(), cx.entity().downgrade());
        // A `system.notify` one goes to its agent, when it named one.
        cx.on_system_notification_response(move |response, cx| {
            let _ = handle.update(cx, |_, window, cx| {
                let _ = this.update(cx, |this, cx| {
                    let addr = match crate::alert::address_of(&response.tag) {
                        Some(addr) => {
                            this.notified.forget(addr);
                            addr.to_owned()
                        }
                        None => match this.notify_clicks.clicked(&response.tag) {
                            Some(addr) => addr,
                            None => return,
                        },
                    };
                    cx.activate(true);
                    window.activate_window();
                    this.run("agent.focus", json!({ "terminal": addr }), window, cx);
                });
            });
        });
        w._tasks.push(cx.spawn_in(window, async move |this, cx| {
            loop {
                if this.update_in(cx, |this, window, cx| this.poll(window, cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(Duration::from_millis(400)).await;
            }
        }));
        if let Some(events) = w.file_events.clone() {
            w._tasks.push(cx.spawn_in(window, async move |this, cx| {
                while let Ok(mut changed) = events.changes.recv().await {
                    // An editor's save is several events: take them together.
                    cx.background_executor().timer(Duration::from_millis(80)).await;
                    while let Ok(more) = events.changes.try_recv() {
                        changed.extend(more);
                    }
                    if this.update_in(cx, |this, window, cx| this.files_changed(changed, window, cx)).is_err() {
                        break;
                    }
                }
            }));
        }
        w._tasks.push(cx.spawn_in(window, async move |this, cx| {
            while let Ok(req) = requests.recv().await {
                if this.update_in(cx, |this, window, cx| this.handle(req, window, cx)).is_err() {
                    break;
                }
            }
        }));
        w._tasks.push(cx.spawn_in(window, async move |this, cx| {
            // The terminals come from the core's events (a snapshot, then each
            // change), not from listing them all every tick; machines, which
            // send no events, are asked now and then.
            let mut subscription = None;
            let mut subscribed_generation = u64::MAX;
            let mut event_lines = crate::hc::EventLines::default();
            let mut known_machines = Value::Null;
            let (mut tick, mut relist) = (0u64, false);
            loop {
                let Ok((core, generation)) = this.update(cx, |this, _| {
                    (this.core.clone(), this.core_generation)
                }) else { break };
                if generation != subscribed_generation {
                    drop(subscription.take());
                    event_lines = crate::hc::EventLines::default();
                    subscribed_generation = generation;
                    let local_core = core.clone();
                    let (sub, result) = cx.background_executor().spawn(async move {
                        let sub = local_core.subscribe().ok();
                        let result = crate::mirror_sync::round(&local_core, crate::mirror_sync::Work {
                            machine: String::new(), version: 0, write: None,
                        });
                        (sub, result)
                    }).await;
                    subscription = sub.map(|(channel, events, snapshot)| {
                        event_lines = crate::hc::EventLines::new(&snapshot);
                        (channel, events)
                    });
                    (tick, relist) = (0, true);
                    if this.update_in(cx, |this, window, cx| {
                        if generation != this.core_generation { return; }
                        this.documents.entry(String::new()).or_default().connection(true, None, None);
                        this.documents_updated(None, vec![result], window, cx);
                        this.sync(window, cx);
                    }).is_err() { break; }
                }
                let ask_machines = tick.is_multiple_of(5);
                tick += 1;
                let relisting = std::mem::take(&mut relist);
                let (sub, machines, (refresh, focus), listed, lines, machine_names) = cx.background_executor().spawn(async move {
                    // The local stream forwards remote doc events too.
                    let mut sub = subscription;
                    // The focus history's change: None, else the version it's at (None: unknown).
                    let (mut refresh, mut focus, mut terminals) = (false, None, relisting);
                    if sub.is_none() && let Ok((channel, events, snapshot)) = core.subscribe() {
                        event_lines = crate::hc::EventLines::new(&snapshot);
                        sub = Some((channel, events));
                        (refresh, focus, terminals) = (true, Some(None), true);
                    }
                    let mut closed = false;
                    if let Some((channel, events)) = &sub {
                        while let Ok(event) = events.try_recv() {
                            if let crate::hc::Stream::Data(bytes) = event {
                                let pushed = event_lines.push(&bytes);
                                refresh |= pushed.docs;
                                if pushed.focus { focus = Some(pushed.focus_version); }
                                terminals |= pushed.terminals;
                                core.credit(*channel, bytes.len());
                            } else { (refresh, focus, closed) = (true, Some(None), true); }
                        }
                    }
                    // A stream that ended is subscribed to again, with a fresh snapshot.
                    if closed { sub = None; }
                    let machines = if ask_machines || terminals { core.call("machine.list", json!({})).ok() } else { None };
                    let machines_changed = machines.as_ref().is_some_and(|m| m != &known_machines);
                    if let Some(m) = &machines { known_machines = m.clone(); }
                    let listed = (terminals || machines_changed).then(|| (event_lines.live_for(&core.hello, &known_machines), event_lines.ended()));
                    (sub, machines, (refresh, focus), listed, event_lines, known_machines)
                }).await;
                let watching = sub.is_some();
                subscription = sub;
                event_lines = lines;
                known_machines = machine_names;
                let (seen, resynced) = event_lines.take_seen();
                if this.update_in(cx, |this, window, cx| {
                    if generation != this.core_generation { return; }
                    // Our seen writes the core has applied (or, after a fresh
                    // snapshot, can't tell): its labels decide again.
                    if resynced {
                        this.written_seen.clear();
                    } else {
                        this.written_seen.retain(|a, w| seen.get(a).is_none_or(|s| s < w));
                    }
                    this.documents_watching = watching;
                    if watching { this.documents.entry(String::new()).or_default().connection(true, None, None); }
                    let changed = this.documents_updated(machines, vec![], window, cx);
                    match listed {
                        Some((live, ended)) => {
                            this.terminals_listed(live, window, cx);
                            this.settle_waits(&ended, Instant::now(), cx);
                        }
                        None => {
                            this.tend_terminals(false, window, cx);
                            this.expire_waits(Instant::now(), cx);
                        }
                    }
                    if changed { this.sync(window, cx); }
                    this.fit_hidden_tabs(window, cx);
                    this.flush_documents(refresh, window, cx);
                    this.flush_focus(focus, cx);
                }).is_err() { break; }
                cx.background_executor().timer(Duration::from_millis(500)).await;
            }
        }));
        w._tasks.push(cx.spawn_in(window, async move |this, cx| {
            // Cores restart (upgrades, crashes) and their programs survive: keep
            // the panes, reconnect, reattach the same terminals.
            loop {
                let Ok(core) = this.update(cx, |this, _| this.core.clone()) else { break };
                core.closed().await;
                tracing::warn!("lost the core; reconnecting");
                crate::telemetry::count("core_lost");
                let fresh = loop {
                    cx.background_executor().timer(Duration::from_millis(1000)).await;
                    if let Ok(core) = cx.background_executor().spawn(async { Core::connect() }).await {
                        break core;
                    }
                };
                if this.update(cx, |this, cx| this.reconnected(fresh, cx)).is_err() {
                    break;
                }
            }
        }));
        Self::note_core(&w.core);
        w.sync(window, cx);
        w.restore_focus(window, cx);
        // The first time the app opens: the frame's welcome (it sets
        // "frame.welcomed" when you start or skip).
        cx.defer_in(window, |this, window, cx| this.open_kept_windows(window, cx));
        // 0.1.0 kept space colors under its own word.
        if mods::shared("space.colors").is_none()
            && let Some(c) = mods::shared("project.colors")
        {
            mods::set_shared("space.colors".into(), c);
        }
        let first_install = mods::shared("frame.welcomed").is_none();
        crate::update_notes::started(first_install);
        if first_install {
            cx.defer_in(window, |this, window, cx| this.open_welcome(window, cx));
        } else if !crate::update_notes::spotlights().is_empty() {
            cx.defer_in(window, |this, window, cx| this.open_whats_new(false, window, cx));
        } else {
            w.update_success = crate::update_notes::changed();
        }
        crate::telemetry::window_open();
        w
    }

    /// What telemetry.rs counts: how much is open, never what's in it.
    fn observe(&self) {
        if !crate::telemetry::collecting() {
            return;
        }
        let m = &self.model;
        let agents: Vec<&model::Agent> = m.agents.iter().filter(|(a, _)| self.live.contains_key(*a) && m.status(a).is_some()).map(|(_, a)| a).collect();
        crate::telemetry::observe(crate::telemetry::Observation {
            foreground: self.window_active,
            agents_working: agents.iter().filter(|a| a.word.as_deref() == Some("working")).count(),
            agents: agents.iter().map(|a| a.app.clone()).collect(),
            spaces: m.spaces.len(),
            tabs: m.spaces.iter().map(|p| p.tabs.len()).sum(),
            terminals: m.spaces.iter().flat_map(|p| &p.tabs).map(|c| c.addresses().len()).sum(),
            layout: self.layout_name.clone(),
            mods: self.visible().1.into_iter().map(|(_, i)| i.name).collect(),
            theme: settings::get().theme.clone(),
            welcome: mods::shared("frame.welcomed").and_then(|v| v.as_str().map(str::to_owned)),
        });
    }

    /// After an update: unseen spotlights. Asked for: the pending range, or this version's notes.
    fn open_whats_new(&mut self, asked: bool, window: &mut Window, cx: &mut Context<Self>) {
        let notes = if asked { crate::update_notes::notes(crate::update::ready().as_deref()) } else { crate::update_notes::spotlights() };
        if notes.is_empty() {
            self.run("app.settings", json!({ "section": "updates" }), window, cx);
            return;
        }
        let viewport = window.viewport_size();
        let width = 880.0_f32.min((f32::from(viewport.width) / settings::ui_scale() - 24.0).max(1.0));
        let height = 760.0_f32.min((f32::from(viewport.height) / settings::ui_scale() - 24.0).max(1.0));
        let spec = json!({ "mod": "frame-whatsnew", "kind": "window", "width": width, "height": height, "props": { "notes": notes } });
        self.open_overlay(mods::new_overlay_id(), spec, window, cx);
    }

    fn open_welcome(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A window over the live app (nothing dims), centered and movable:
        // each step changes what's behind it, and a click beside it doesn't end it.
        let spec = json!({ "mod": "frame-welcome", "kind": "window", "width": 660, "height": 600 });
        self.open_overlay(mods::new_overlay_id(), spec, window, cx);
    }

    /// Tells the updater which core this is and who owns it (off the UI thread).
    fn note_core(core: &Arc<Core>) {
        let core = core.clone();
        std::thread::spawn(move || {
            if let Ok(s) = core.call("server.status", json!({})) {
                let text = |k: &str| s.get(k).and_then(Value::as_str).map(str::to_owned);
                crate::update::set_core(text("version").map(|v| v.trim_start_matches("hc ").to_owned()), text("build"), text("installed_by"));
            }
        });
    }

    fn reconnected(&mut self, core: Arc<Core>, cx: &mut Context<Self>) {
        tracing::info!("reconnected to the core");
        self.core_generation += 1;
        self.documents_watching = false;
        self.focus_history.reconnected();
        for (machine, slot) in &mut self.documents {
            slot.available = false;
            slot.connected = machine.is_empty();
            slot.read_attempted = false;
            slot.pending = None;
            slot.blocked = false;
        }
        Self::note_core(&core);
        self.core = core.clone();
        // Programs the old core lost may be shells now: offer them again.
        self.resume.forget_seen();
        self.resume_offer = Some(Instant::now());
        self.publish(cx);
        for t in self.terms.values() {
            t.view.update(cx, |v, cx| v.reattach(core.clone(), cx));
        }
        cx.notify();
    }

    fn report(&self, what: &str, message: &str) {
        tracing::warn!("{what}.json not applied: {message}");
        let _ = std::fs::write(file(&format!("{what}.error")), format!("{what}.json was not applied: {message}\n"));
    }

    // ---- config files ------------------------------------------------------

    /// Changes settings.json from inside the app (the zoom keys, the focus
    /// toggles). If the file is broken mid-edit, the change applies for now
    /// without touching it. Whether it was saved, and if not why.
    fn write_person(&mut self, change: impl FnOnce(&mut settings::Person), window: &mut Window, cx: &mut Context<Self>) -> PersonSaved {
        let broken = settings::load_person().is_err();
        let saved = change_person(&mut self.person, change, broken, settings::write_person);
        if !broken {
            self.watched.settings = mtime(&settings::path());
        }
        self.apply_look(window, cx);
        saved
    }

    /// Runs with the shown layout's look (its root node) at the person's zoom.
    fn apply_look(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A layout's fonts arrive with its mod: added since the start, they load now.
        if settings::load_home_fonts(cx) {
            mods::note_fonts(cx);
        }
        let s = settings::Settings::compose(&self.person, &self.chrome.look, self.system_dark);
        // A new command gets its own say about why it fails.
        if s.alert_command != settings::get().alert_command {
            self.alert_command_errors.clear();
        }
        let problems = s.problems();
        if !problems.is_empty() {
            self.report_layout(&problems.join("; "));
        }
        let republish = s.pane_auto_names != settings::get().pane_auto_names || s.space_avatar != settings::get().space_avatar || s.pane_header_info != settings::get().pane_header_info
            || s.agents_card_info != settings::get().agents_card_info;
        let git_counts = |s: &settings::Settings| crate::header_info::counts_git(s.pane_header_info.as_deref(), s.agents_card_info.as_deref());
        let recount = git_counts(&s) != git_counts(&settings::get());
        let redesign = s.toast_design != settings::get().toast_design || (s.toast_design == settings::ToastDesign::Terminal && s.font_family != settings::get().font_family);
        if settings::set(s) {
            self.theme_changed(window, cx);
        }
        if redesign {
            self.redesign_toasts(cx);
        }
        self.tick_progress(cx);
        // Folder info counts ahead, behind and dirty only while a template shows them.
        if recount {
            self.track_folders(cx);
        }
        if republish {
            self.publish(cx);
        }
        self.badge_dock(self.dock_badge.waiting());
    }

    fn theme_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for term in self.terms.values() {
            term.view.read(cx).refresh_theme();
        }
        mods::apply_theme(cx);
        self.refresh_mods(cx);
        window.refresh();
    }

    /// `sidebar_shade` follows the window's focus; when nothing can change
    /// (`settings::follows_focus`), it does nothing more. Only `chrome` can change,
    /// which neither terminals nor the kit's tokens use: the mods that read
    /// `theme()` redraw, and the window.
    fn shade_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        settings::set_window_active(window.is_window_active());
        if settings::follows_focus() && settings::reshade() {
            mods::bump_revision("theme");
            self.refresh_snapshot_mods(cx);
            window.refresh();
        }
    }

    fn reload_config(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let s_time = mtime(&settings::path());
        if s_time.is_none() || s_time != self.watched.settings {
            let loaded = settings::load_person();
            crate::telemetry::set_setting(settings::telemetry_setting(&loaded));
            match loaded {
                Ok(p) => {
                    self.person = p;
                    self.model.default_arrange = self.person.default_arrange();
                    let _ = std::fs::remove_file(file("settings.error"));
                }
                Err(e) => self.report("settings", &e),
            }
            self.watched.settings = mtime(&settings::path());
            self.apply_look(window, cx);
        }
        let k_time = mtime(&crate::keys::path());
        if k_time.is_none() || k_time != self.watched.keys {
            match crate::keys::load() {
                Ok(bindings) => {
                    crate::keys::install(bindings, cx);
                    let _ = std::fs::remove_file(file("keys.error"));
                }
                Err(e) => {
                    self.report("keys", &e);
                    if self.watched.keys.is_none() {
                        crate::keys::install(vec![], cx);
                    }
                }
            }
            self.watched.keys = mtime(&crate::keys::path());
        }
        let p_time = mtime(&crate::programs::path());
        if p_time != self.watched.programs {
            match crate::programs::load() {
                Ok(()) => {
                    let _ = std::fs::remove_file(file("programs.error"));
                }
                Err(e) => self.report("programs", &e),
            }
            self.watched.programs = p_time;
            self.publish(cx);
        }
    }

    /// Makes `name` the shown layout (its file, or its default if missing or
    /// broken), with the person's clicks on it.
    fn load_layout(&mut self, name: &str) {
        self.layout_name = name.to_owned();
        self.written_layout.clear();
        let mut from_file = false;
        // A file that's there but broken or unreadable is shown as shipped for
        // now and never written to (nor its clicks) until it's fixed.
        self.layout_file_ok = true;
        let mut chrome = match std::fs::read_to_string(layout_path(name)) {
            Ok(text) => match serde_json::from_str::<Node>(&text) {
                Ok(n) => {
                    self.written_layout = text;
                    from_file = true;
                    let _ = std::fs::remove_file(file("layout.error"));
                    n
                }
                Err(e) => {
                    self.report_layout(&e.to_string());
                    self.layout_file_ok = false;
                    default_layout(name)
                }
            },
            Err(e) => {
                self.layout_file_ok = e.kind() == std::io::ErrorKind::NotFound;
                default_layout(name)
            }
        };
        if from_file {
            self.keep_ids(&mut chrome);
        } else {
            fill_ids(&mut chrome, &mut self.model);
        }
        self.file_chrome = chrome.clone();
        let mut state = read_state(name);
        if self.layout_file_ok {
            // Clicks on fields the file changed since (an edit while the app
            // was closed or another layout showed, an update) give way.
            state.forget_since_saved(&chrome);
        }
        self.picked_theme = state.theme.clone();
        state.apply(&mut chrome);
        self.chrome = chrome;
        self.written_state.clear();
        if self.layout_file_ok {
            self.write_state(&state);
        }
        mods::set_shared("layout".into(), json!(name));
    }

    fn report_layout(&self, message: &str) {
        let name = &self.layout_name;
        tracing::warn!("layouts/{name}.json not applied: {message}");
        let _ = std::fs::write(file("layout.error"), format!("layouts/{name}.json was not applied: {message}\n"));
    }

    /// Shows the layout `name`, or the next one after the current.
    fn switch_layout(&mut self, name: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        let names = layout_names();
        let next = match name {
            Some(n) if mods::valid_name(n) && layout_path(n).exists() => n.to_owned(),
            Some(n) => {
                tracing::warn!("no layout {n:?} in layouts/");
                return;
            }
            None => {
                let i = names.iter().position(|n| *n == self.layout_name).map_or(0, |i| i + 1);
                match names.get(i % names.len().max(1)) {
                    Some(n) => n.clone(),
                    None => return,
                }
            }
        };
        self.save();
        self.float_drag = None;
        self.pane_drag = None;
        self.float_anims.clear();
        self.float_reveal.clear();
        self.float_hover.clear();
        let other = next != self.layout_name;
        if other {
            self.close_windows(window, cx);
            self.drop_layout_copies(window, cx);
        }
        self.load_layout(&next);
        self.apply_look(window, cx);
        // Whatever the new layout shows takes the keyboard.
        self.focused_pane = None;
        self.agent_focused = None;
        self.sync(window, cx);
        if other {
            self.open_kept_windows(window, cx);
        }
    }

    fn check_layout_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(text) = std::fs::read_to_string(layout_path(&self.layout_name)) else { return };
        if text == self.written_layout {
            return;
        }
        match serde_json::from_str::<Node>(&text) {
            Ok(mut n) => {
                self.written_layout = text;
                let was_broken = !self.layout_file_ok;
                self.layout_file_ok = true;
                self.keep_ids(&mut n);
                // What the file now sets on purpose wins over a click on it.
                // Coming back from a broken file, the clicks are the saved ones
                // (what showed meanwhile was only the shipped stand-in).
                let state = if was_broken {
                    let mut s = read_state(&self.layout_name);
                    s.forget_since_saved(&n);
                    s
                } else {
                    let mut s = layout_sync::State::between(&self.file_chrome, &self.chrome);
                    s.theme = self.picked_theme.clone();
                    s.forget_edited(&self.file_chrome, &n);
                    s
                };
                self.file_chrome = n.clone();
                self.chrome = n;
                self.picked_theme = state.theme.clone();
                state.apply(&mut self.chrome);
                self.write_state(&state);
                let _ = std::fs::remove_file(file("layout.error"));
                self.apply_look(window, cx);
                self.sync(window, cx);
            }
            Err(e) => {
                self.written_layout = text;
                self.layout_file_ok = false;
                self.report_layout(&e.to_string());
            }
        }
    }

    /// Live folders used for unnamed spaces; not restart state.
    fn note_pane_folders(&mut self) {
        let panes: Vec<(String, String)> =
            self.model.tabs().flat_map(Tab::terminals).filter_map(|n| Some((n.id.clone(), n.terminal()?.to_owned()))).collect();
        self.model.pane_folders.retain(|p, _| panes.iter().any(|(id, _)| id == p));
        for (pane, addr) in panes {
            if let Some(cwd) = self.cwds.get(&addr) {
                self.model.pane_folders.insert(pane, cwd.clone());
            }
        }
    }

    /// Writes the clicks to the layout's state file and, only when something
    /// else changed (a mod docked, `layout.set`), the layout file.
    fn save(&mut self) {
        // Quitting (also the OS ending the session): programs may end in any
        // order now. Their panes aren't closing; the next start brings them back.
        if QUITTING.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        self.note_pane_folders();
        let mut state = layout_sync::State::between(&self.file_chrome, &self.chrome);
        state.theme = self.picked_theme.clone();
        if self.layout_file_ok {
            self.write_state(&state);
        }
        let edited = layout_sync::without_clicks(&self.file_chrome, &self.chrome);
        if edited != self.file_chrome && self.layout_file_ok && !shipped_layouts().contains(&self.layout_name.as_str()) {
            let path = layout_path(&self.layout_name);
            // Someone (an agent) changed the file since we read it: theirs
            // wins; the watcher loads it next.
            let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
            if !self.written_layout.is_empty() && on_disk != self.written_layout {
                tracing::info!("layouts/{}.json changed meanwhile: not overwritten", self.layout_name);
            } else {
                let layout = layout_sync::text_of(&edited);
                if write_atomic(&path, &layout) {
                    self.written_layout = layout;
                    self.file_chrome = edited;
                }
            }
        }
        self.save_view();
    }

    fn save_view(&mut self) {
        let mut view = crate::mirror::View::capture(&self.model);
        if let Some(saved) = &self.initial_view {
            view.preserve_unloaded(saved, |machine| self.documents.get(machine.unwrap_or("")).is_some_and(|s| s.document.is_some()));
        }
        let view = serde_json::to_string_pretty(&view).unwrap_or_default() + "\n";
        if self.documents.values().any(|s| s.document.is_some()) && view != self.written_view && write_atomic(&file("view.json"), &view) {
            self.written_view = view;
        }
    }

    /// What installing layouts kept of the person's own, as toasts.
    fn show_layout_notes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for clone in mods::take_sync_notes() {
            let n = json!({ "title": format!("Your edits are in {clone}"), "body": "Herdr's own layouts and mods can't be changed: clone one to make it yours.", "level": "info", "timeout": 12000 });
            self.open_overlay(mods::new_overlay_id(), json!({ "kind": "toast", "notification": n }), window, cx);
        }
    }

    /// Writes the clicks, with the file they were made on (so a later change
    /// to the file can win over them).
    fn write_state(&mut self, state: &layout_sync::State) {
        let mut state = state.clone();
        if !state.is_empty() {
            state.file = serde_json::to_value(&self.file_chrome).ok();
        }
        let text = state_text(&state);
        if text == self.written_state {
            return;
        }
        let path = state_path(&self.layout_name);
        let ok = if state.is_empty() {
            std::fs::remove_file(&path).is_ok() || !path.exists()
        } else {
            let _ = std::fs::create_dir_all(file("layouts/.state"));
            write_atomic(&path, &text)
        };
        if ok {
            self.written_state = text;
        }
    }

    /// Gives nodes without an id one and writes it into the file, so clicks
    /// on them (keyed by id) still find them next time.
    fn keep_ids(&mut self, n: &mut Node) {
        let before = n.clone();
        // Never one the tree (or yesterday's generated ids in it) already has.
        let taken: std::collections::HashSet<String> = n.walk().iter().map(|x| x.id.clone()).collect();
        let mut i = 0;
        n.fill_ids(&mut || loop {
            i += 1;
            let id = format!("node-{i}");
            if !taken.contains(&id) {
                break id;
            }
        });
        if *n != before {
            let text = layout_sync::text_of(n);
            if write_atomic(&layout_path(&self.layout_name), &text) {
                self.written_layout = text;
            }
        }
    }

    // ---- making the screen match the model ---------------------------------

    /// Visible slots: terminal blocks (id → address) and mod slots (key → instance).
    fn visible(&self) -> (Vec<(String, String)>, Vec<(String, Instance)>) {
        let mut terms = Vec::new();
        let mut mods = Vec::new();
        let visit = |n: &Node, terms: &mut Vec<(String, String)>, mods: &mut Vec<(String, Instance)>| match n.content() {
            Some(Content::Terminal { terminal }) => terms.push((n.id.clone(), terminal.clone())),
            Some(Content::Mod { name, props }) => mods.push((n.id.clone(), Instance { name: name.clone(), props: props.clone() })),
            _ => {}
        };
        let stacks = |root: &Node, mods: &mut Vec<(String, Instance)>| {
            for n in visible_nodes(root) {
                if let Kind::Stack { header: Some(h), .. } = &n.kind
                    && let HeaderContent::Mod { name, props } = &h.content
                {
                    mods.push((format!("{}#header", n.id), Instance { name: name.clone(), props: props.clone() }));
                }
            }
        };
        for n in self.chrome.visible_blocks() {
            visit(n, &mut terms, &mut mods);
            // An agent block with an agent outside every tab gets its own view.
            if matches!(n.content(), Some(Content::Agent { .. }))
                && let Some(addr) = self.shown_agent()
                && self.model.pane_of(&addr).is_none()
            {
                terms.push((n.id.clone(), addr));
            }
        }
        stacks(&self.chrome, &mut mods);
        for n in visible_nodes(&self.chrome) {
            for f in n.floating.iter().filter(|f| !f.hidden) {
                match &f.content {
                    Content::Mod { name, props } => mods.push(float_mod(&f.id, name, props)),
                    Content::Terminal { terminal } => terms.push((floats::slot(&f.id), terminal.clone())),
                    _ => {}
                }
            }
        }
        if self.shows_tab()
            && let Some(c) = self.model.active_tab()
        {
            let header = self.pane_frame().and_then(|f| f.header.as_ref()).and_then(|h| match &h.content {
                HeaderContent::Mod { name, props } => Some((name.clone(), props.clone())),
                HeaderContent::Builtin(_) => None,
            });
            let float_header = self.float_frame().and_then(|f| f.header.as_ref()).and_then(|h| match &h.content {
                HeaderContent::Mod { name, props } => Some((name.clone(), props.clone())),
                HeaderContent::Builtin(_) => None,
            });
            let tiled = c.shown().visible_blocks().into_iter().map(|n| (n, &header));
            for (n, header) in tiled.chain(c.shown_floats().map(|f| (&f.pane, &float_header))) {
                if let Some((name, props)) = header
                    && n.is_pane()
                {
                    mods.push(pane_header_mod(n, name, props));
                }
                visit(n, &mut terms, &mut mods);
                if let Some(a) = self.swapped.get(&n.id) {
                    terms.push((format!("{}{SWAP}", n.id), a.clone()));
                }
            }
        }
        for o in &self.overlays {
            if let Some(c) = self.copies.get(&o.slot) {
                mods.push((o.slot.clone(), c.inst.clone()));
            }
            if let Some(l) = &o.layout {
                for n in l.visible_blocks() {
                    visit(n, &mut terms, &mut mods);
                }
            }
        }
        (terms, mods)
    }

    /// Mod slots whose copy may stay alive: every one in the layout (hidden
    /// nodes, inactive stack children, hidden floats), every pane mod of every
    /// tab (zoomed or not shown), and the overlays.
    fn owned_mods(&self) -> HashMap<String, Instance> {
        let mut out = HashMap::new();
        let inst = |name: &String, props: &Value| Instance { name: name.clone(), props: props.clone() };
        for n in self.chrome.walk() {
            if let Some(Content::Mod { name, props }) = n.content() {
                out.insert(n.id.clone(), inst(name, props));
            }
            if let Kind::Stack { header: Some(h), .. } = &n.kind
                && let HeaderContent::Mod { name, props } = &h.content
            {
                out.insert(format!("{}#header", n.id), inst(name, props));
            }
            for f in &n.floating {
                if let Content::Mod { name, props } = &f.content {
                    let (slot, i) = float_mod(&f.id, name, props);
                    out.insert(slot, i);
                }
            }
        }
        // The layout's pane header, shown or not (`pane_frame` says what's drawn now).
        let header = self
            .chrome
            .walk()
            .into_iter()
            .find_map(|n| match n.content() {
                Some(Content::Context { pane: Some(f), .. }) => f.header.as_ref(),
                _ => None,
            })
            .and_then(|h| match &h.content {
                HeaderContent::Mod { name, props } => Some((name.clone(), props.clone())),
                HeaderContent::Builtin(_) => None,
            });
        for c in self.model.tabs() {
            for n in c.panes() {
                if let Some(Content::Mod { name, props }) = n.content() {
                    out.insert(n.id.clone(), inst(name, props));
                }
                if let Some((name, props)) = &header
                    && n.is_pane()
                {
                    let (slot, i) = pane_header_mod(n, name, props);
                    out.insert(slot, i);
                }
            }
        }
        for o in &self.overlays {
            if let Some(c) = self.copies.get(&o.slot) {
                out.insert(o.slot.clone(), c.inst.clone());
            }
            if let Some(l) = &o.layout {
                for n in l.visible_blocks() {
                    if let Some(Content::Mod { name, props }) = n.content() {
                        out.insert(n.id.clone(), inst(name, props));
                    }
                }
            }
        }
        out
    }

    /// Another layout: its mods start fresh. Pane mods (they belong to their
    /// tab) and overlays stay.
    fn drop_layout_copies(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let gone: Vec<_> = self.copies.keys().filter(|s| {
            self.model.tab_of(s).is_none() && !self.overlays.iter().any(|o| o.slot == **s || o.layout.as_ref().is_some_and(|l| l.find(s).is_some()))
        }).cloned().collect();
        for slot in gone {
            self.remove_copy(&slot, window, cx);
        }
    }

    fn remove_copy(&mut self, slot: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(copy) = self.copies.remove(slot)
            && let (Some(runtime), Some(root)) = (&self.runtime, copy.root)
        {
            runtime.close(&root, std::time::Instant::now() + Duration::from_millis(250), window, cx).detach();
        }
    }

    pub(crate) fn close_mods_now(&self, deadline: std::time::Instant, window: &mut Window, cx: &mut Context<Self>) {
        let Some(runtime) = &self.runtime else { return };
        mods::final_cleanup(self.core.clone(), deadline, || {
            for root in self.copies.values().filter_map(|copy| copy.root.as_ref()) {
                runtime.close_now(root, deadline, window, cx);
            }
        });
    }

    fn shows_tab(&self) -> bool {
        self.chrome.visible_blocks().iter().any(|n| matches!(n.content(), Some(Content::Context { .. })))
    }

    fn agent_block(&self) -> Option<String> {
        self.chrome.visible_blocks().iter().find(|n| matches!(n.content(), Some(Content::Agent { .. }))).map(|n| n.id.clone())
    }

    fn shows_agent(&self) -> bool {
        self.agent_block().is_some()
    }

    /// The mod listing agents chooses navigation order; model order is the fallback.
    fn agent_order(&self) -> Vec<String> {
        let m = &self.model;
        mods::agent_order(m.all_addresses().into_iter().filter(|a| m.status(a).is_some()).collect())
    }

    /// The agent an agent block shows: the chosen one while it lives, else the
    /// focused pane if it's an agent, else the first agent.
    fn shown_agent(&self) -> Option<String> {
        let focused = self.model.focused_pane().and_then(|p| self.model.address_of(&p));
        self.agent
            .clone()
            .filter(|a| self.live.contains_key(a))
            .or_else(|| focused.filter(|a| self.model.status(a).is_some()))
            .or_else(|| self.agent_order().into_iter().next())
    }

    /// The terminal view showing `addr` in an agent block: its pane's, or the block's own.
    fn agent_view(&self, block: &str, addr: &str) -> Option<&Term> {
        self.model.pane_of(addr).and_then(|p| self.terms.get(&p)).or_else(|| self.terms.get(block).filter(|t| t.address == addr))
    }

    /// What's on screen now, for done-vs-seen.
    fn visible_addresses(&self) -> Vec<String> {
        let (terms, _) = self.visible();
        // A swapped-in terminal is drawn instead of its pane's own.
        let covered: HashSet<&str> = terms.iter().filter_map(|(slot, _)| slot.strip_suffix(SWAP)).collect();
        let mut out: Vec<String> = terms.iter().filter(|(slot, _)| !covered.contains(slot.as_str())).map(|(_, a)| a.clone()).collect();
        if self.shows_agent() {
            out.extend(self.shown_agent());
        }
        out
    }

    /// Makes `addr` the shown agent; a pane showing it also becomes the focused pane.
    fn focus_agent(&mut self, addr: &str) {
        if let Some(p) = self.model.pane_of(addr) {
            self.model.focus_pane(&p);
        }
        self.focused_pane = None;
        self.agent = Some(addr.to_owned());
    }

    /// The parent chain's top: the agent a sub-agent belongs to in the agent order.
    fn root_agent(&self, addr: &str) -> String {
        let mut cur = addr.to_owned();
        for _ in 0..32 {
            let parent = self.live.get(&cur).and_then(|r| r.get("parent")).and_then(Value::as_str);
            match parent {
                Some(p) if self.live.contains_key(p) => cur = p.to_owned(),
                _ => break,
            }
        }
        cur
    }

    /// Creates what's visible, drops what isn't, publishes state, saves, focuses.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.model.order_spaces();
        let (model, live) = (&self.model, &self.live);
        self.swapped.retain(|p, a| model.address_of(p).is_some() && live.contains_key(a));
        let (mut terms, mods_visible) = self.visible();
        let covered: HashSet<&str> = terms.iter().filter_map(|(slot, _)| slot.strip_suffix(SWAP)).collect();
        let mut displayed: HashSet<String> = terms.iter().filter(|(slot, _)| !covered.contains(slot.as_str())).map(|(slot, _)| slot.clone()).collect();
        // Agent blocks reuse their agent's pane view even when its tab isn't shown.
        if self.shows_agent()
            && let Some(addr) = self.shown_agent()
            && let Some(pane) = self.model.pane_of(&addr)
        {
            displayed.insert(pane);
        }
        for (id, t) in &self.terms {
            if !displayed.contains(id) {
                t.view.update(cx, |v, _| v.blur());
            }
        }
        // Every tab's terminals stay attached (only visible ones are drawn): tab
        // switches are instant and kitty images survive, which a repaint can't bring back.
        for c in self.model.tabs() {
            for n in c.terminals() {
                if let Some(a) = n.terminal()
                    && !terms.iter().any(|(id, _)| *id == n.id)
                {
                    terms.push((n.id.clone(), a.to_owned()));
                }
            }
        }
        let want_terms: HashSet<String> = terms.iter().map(|(id, _)| id.clone()).collect();
        for (id, address) in terms {
            if self.terms.get(&id).is_some_and(|t| t.address == address) {
                continue;
            }
            let rec = self.live.get(&address);
            let dim = |k: &str, d: u64| rec.and_then(|r| r.get(k)).and_then(Value::as_u64).unwrap_or(d) as u16;
            let size_hint = (dim("cols", 120), dim("rows", 40));
            let view = cx.new(|cx| TerminalView::attach(self.core.clone(), address.clone(), size_hint, cx));
            self.tell_term(&view, &address, cx);
            let (block, addr) = (id.clone(), address.clone());
            let sub = cx.subscribe_in(&view, window, move |this, view, ev: &TerminalEvent, window, cx| match ev {
                TerminalEvent::Peek(address) => this.peek_terminal(address, window, cx),
                TerminalEvent::TitleChanged => this.publish(cx),
                TerminalEvent::Ended => this.terminal_ended(&block, window, cx),
                TerminalEvent::ContextMenu(at) => {
                    // What the copy items need, read before the menu takes the keyboard.
                    let target = json!({
                        "pane": block.trim_end_matches(SWAP), "terminal": addr,
                        "selected": view.read(cx).has_selection(), "cwd": this.terminal_cwd(&addr),
                        "last_output": view.read(cx).has_last_output(),
                    });
                    this.open_menu("pane", target, Some(*at), window, cx)
                }
                TerminalEvent::Copied => this.show_copied(window, cx),
                TerminalEvent::CursorJump(from) => this.cursor_jump(block.trim_end_matches(SWAP), *from, cx),
            });
            let focus = view.read(cx).focus_handle().clone();
            self.terms.insert(id, Term { view, address, focus, _sub: sub });
        }
        let gone: Vec<String> = self.terms.keys().filter(|id| !want_terms.contains(*id)).cloned().collect();
        for id in gone {
            if let Some(t) = self.terms.remove(&id) {
                t.view.read(cx).detach();
            }
        }

        // A mod copy lives while its slot is in the layout, hidden or not (an
        // inactive stack child, a hidden column, a pane in another tab), so it
        // keeps its place; only shown slots start a copy. Gone or changed: dropped.
        let owned = self.owned_mods();
        let gone: Vec<_> = self.copies.iter().filter(|(slot, copy)| owned.get(*slot).is_none_or(|inst| *inst != copy.inst)).map(|(slot, _)| slot.clone()).collect();
        for slot in gone {
            self.remove_copy(&slot, window, cx);
        }
        let want: HashMap<String, Instance> = mods_visible.into_iter().collect();
        for (slot, inst) in want {
            if !self.copies.contains_key(&slot) {
                let copy = self.load(inst, window, cx);
                if slot.starts_with("float:")
                    && let Some(root) = &copy.root
                {
                    root.update(cx, |r, cx| r.set_fit_content(true, cx));
                }
                self.copies.insert(slot, copy);
            }
        }
        self.publish_shown_copies(cx);

        // Keyboard to the focused pane when it changes (never steal it from an open dialog).
        let model_pane = self.model.focused_pane();
        let block = terminal_block(&self.chrome, self.focused_pane.as_deref());
        let focused = if block.is_some() && model_pane == self.synced_model_pane {
            self.focused_pane.clone()
        } else {
            model_pane.clone()
        };
        let pane_changed = focused != self.focused_pane;
        // A pane focused some other way, or the block leaving, ends it.
        if (pane_changed && self.focused_block != focused) || self.focused_block.as_ref().is_some_and(|b| !self.mod_block_shown(b)) {
            self.focused_block = None;
        }
        // Mod blocks too, so `pane.go` goes back to a mod it came from.
        self.pane_history.retain(|id| {
            self.copies.contains_key(id) || self.model.tabs().any(|t| t.find(id).is_some())
        });
        if let Some(id) = self.focused_block.as_ref().or(model_pane.as_ref()) {
            remember_pane(&mut self.pane_history, id);
        }
        self.synced_model_pane = model_pane;
        self.focused_pane = focused.clone();
        let no_dialog = !self.overlays.iter().any(|o| o.kind.modal_keys());
        // The tab shown again (`layout.select` back from another stack child): its
        // focused pane gets the keyboard, which went to the window while it was hidden.
        let tab_shown = self.shows_tab();
        let revealed = tab_shown && !self.tab_shown;
        self.tab_shown = tab_shown;
        if terminal_block(&self.chrome, focused.as_deref()).is_some() {
            if pane_changed && no_dialog && let Some(f) = focused.as_deref().and_then(|p| self.pane_focus(p, cx)) {
                f.focus(window, cx);
            }
        } else if let Some(block) = self.agent_block() {
            let shown = self.shown_agent();
            if shown != self.agent_focused || pane_changed {
                self.agent_focused = shown.clone();
                if no_dialog && let Some(t) = shown.and_then(|a| self.agent_view(&block, &a)) {
                    t.focus.focus(window, cx);
                }
            }
        } else if pane_changed {
            // Outside an agent view, the agent shown later follows the focused pane.
            self.agent = None;
            // A mod pane takes the keyboard on a click; until then the window holds
            // it, so another tab's terminal can't keep it (and pull its tab back).
            if no_dialog {
                focused.and_then(|p| self.pane_focus(&p, cx)).unwrap_or_else(|| self.focus.clone()).focus(window, cx);
            }
        } else if revealed {
            self.refocus_pane(window, cx);
        }
        if self.window_active {
            let visible = self.visible_addresses();
            self.model.mark_seen(&visible);
        }
        self.share_seen(cx);
        self.publish(cx);
        self.save();
        cx.notify();
    }

    /// Shares what the person looks at as `herdr.seen` (docs/labels.md): an
    /// agent on screen in the active window, once per turn while it stays
    /// there, so a phone marking it unread isn't undone by a screen nobody
    /// watches. Only ever raises it. True when a status changed.
    fn share_seen(&mut self, cx: &mut Context<Self>) -> bool {
        let looking: HashSet<String> = if self.window_active { self.visible_addresses().into_iter().collect() } else { HashSet::new() };
        self.looked.retain(|a, _| looking.contains(a));
        let mut changed = false;
        for addr in looking {
            let Some(a) = self.model.agents.get(&addr) else { continue };
            let turn = a.turn;
            if a.word.is_none() || !self.live.contains_key(&addr) || self.unreported.owns(&addr) || self.looked.get(&addr) == Some(&turn) {
                continue;
            }
            self.looked.insert(addr.clone(), turn);
            if a.seen_turn.is_some_and(|s| s >= turn) {
                continue;
            }
            // Shown as seen now, not after the core's round trip.
            changed |= self.model.set_turn(&addr, turn, Some(turn));
            self.written_seen.insert(addr.clone(), turn);
            let core = self.core.clone();
            cx.spawn(async move |this, cx| {
                let params = json!({ "terminal": addr, "labels": { "herdr.seen": turn.to_string() } });
                let Err(e) = cx.background_executor().spawn(async move { core.call("terminal.label", params) }).await else { return };
                tracing::warn!("seen label for {addr}: {e:#}");
                let _ = this.update(cx, |this, _| {
                    if this.written_seen.get(&addr) == Some(&turn) {
                        this.written_seen.remove(&addr);
                        this.looked.remove(&addr);
                    }
                });
            })
            .detach();
        }
        changed
    }

    /// The keyboard never ends up nowhere. When what had it leaves the screen (a
    /// mod hidden but kept alive, a closed pane, a hidden mod focusing itself on a
    /// reload), it goes to the top dialog, else the shown agent or focused pane,
    /// else the window, where the app's keys work.
    fn restore_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = self.overlays.iter().rev().find(|o| o.kind.modal_keys()).map(|o| o.focus.clone());
        // A mod block holding the keyboard gets it back (a popup over it closed).
        if dialog.is_none() && self.focused_block.as_ref().is_some_and(|b| self.mod_block_shown(b)) {
            self.focus.focus(window, cx);
            return self.hand_keyboard();
        }
        let shown = if terminal_block(&self.chrome, self.focused_pane.as_deref()).is_some() {
            self.focused_pane.as_deref().and_then(|p| self.pane_focus(p, cx))
        } else if let Some(block) = self.agent_block() {
            self.shown_agent().and_then(|a| self.agent_view(&block, &a)).map(|t| t.focus.clone())
        } else if self.shows_tab() {
            self.model.focused_pane().and_then(|p| self.pane_focus(&p, cx))
        } else {
            None
        };
        dialog.or(shown).unwrap_or_else(|| self.focus.clone()).focus(window, cx);
    }

    /// The terminal view a pane shows: the swapped-in one, else its own.
    fn pane_term(&self, pane: &str) -> Option<&Term> {
        self.swapped.get(pane).and_then(|_| self.terms.get(&format!("{pane}{SWAP}"))).or_else(|| self.terms.get(pane))
    }

    /// Key path for `terminal.copy`. Copies and clears when there is a selection.
    /// Returns whether it copied. False means the key should reach the program.
    fn copy_key(&mut self, window: &Window, cx: &mut Context<Self>) -> bool {
        let Some(t) = self.focused_terminal(window, cx) else { return true };
        t.view.update(cx, |v, cx| {
            if !v.copy(cx) {
                return false;
            }
            v.clear_selection(cx);
            true
        })
    }

    /// Whether a key bound to `pane.zoom_text_*` zooms: `pane_zoom_keys` is on
    /// and a terminal has the keyboard.
    fn pane_zoom_keys_apply(&self, window: &Window, cx: &App) -> bool {
        settings::get().pane_zoom_keys && self.terms.values().any(|t| t.view.read(cx).owns_keyboard_focus(window, cx))
    }

    /// Peeks, floats and swapped-in views own keyboard focus without changing the model pane.
    fn focused_terminal(&self, window: &Window, cx: &App) -> Option<&Term> {
        self.terms.values().find(|t| t.view.read(cx).owns_keyboard_focus(window, cx))
            .or_else(|| self.focused_pane.as_deref().and_then(|p| self.pane_term(p)))
            .or_else(|| self.model.focused_pane().and_then(|p| self.pane_term(&p)))
    }

    /// The folder a terminal is in: its live folder, else the core's record of it.
    fn terminal_cwd(&self, address: &str) -> Option<String> {
        let rec = self.live.get(address);
        self.cwds
            .get(address)
            .cloned()
            .or_else(|| rec.and_then(|r| r.pointer("/foreground/cwd").or_else(|| r.get("cwd"))).and_then(Value::as_str).map(str::to_owned))
            .filter(|cwd| !cwd.is_empty())
    }

    /// The view a command with a `terminal` argument acts on. A copy takes
    /// the view holding a selection: the same terminal can show in a pane
    /// and a peek, each with a selection of its own.
    fn command_view(&self, address: &str, copying: bool, window: &Window, cx: &App) -> Result<Entity<TerminalView>, String> {
        let selected = copying.then(|| self.terms.values().find(|t| t.address == address && t.view.read(cx).has_selection())).flatten();
        match selected {
            Some(t) if self.live.contains_key(address) => Ok(t.view.clone()),
            _ => self.shown_terminal_view(address, window, cx),
        }
    }

    /// `terminal.copy_all`: the screen and all the history the core keeps, as
    /// `hc read` prints it, without the blank lines below the last text.
    fn copy_all(&mut self, address: String, window: &mut Window, cx: &mut Context<Self>) {
        // The core reads a full-screen program's history by scrolling it, holding
        // typed keys, for up to seconds: say so, or the pane seems to move by itself.
        let copying = self.terms.values().any(|t| t.address == address && t.view.read(cx).full_screen()).then(|| self.show_copying(window, cx));
        let core = self.core.clone();
        let read = cx.background_executor().spawn(async move { core.call_timeout("terminal.read", copy_all_params(&address), COPY_ALL_READ_LIMIT) });
        cx.spawn_in(window, async move |this, cx| {
            let read = read.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let note = match read {
                    Ok(r) if !r["text"].is_string() => Err(json!({ "title": "Couldn't copy the terminal", "body": format!("unexpected reply: {r}"), "level": "error" })),
                    Ok(r) => match copied_text(r["text"].as_str().unwrap_or_default()) {
                        Some((text, lines)) => {
                            cx.write_to_clipboard(ClipboardItem::new_string(text));
                            let mut note = json!({ "title": format!("Copied {} {}", thousands(lines), if lines == 1 { "line" } else { "lines" }) });
                            if r["complete"] == json!(false) {
                                note["body"] = json!("Some of its history wasn't available");
                            }
                            Ok(note)
                        }
                        None => Err(json!({ "title": "Nothing to copy", "level": "info" })),
                    },
                    Err(e) => Err(json!({ "title": "Couldn't copy the terminal", "body": format!("{e:#}"), "level": "error" })),
                };
                this.copied_all(copying, note, window, cx);
            });
        })
        .detach();
    }

    /// The "Copying…" toast of one `copy_all`, its own: a selection copied
    /// meanwhile says "Copied" beside it, and it stays up as long as the read may take.
    fn show_copying(&mut self, window: &mut Window, cx: &mut Context<Self>) -> String {
        let id = mods::new_overlay_id();
        let timeout = (COPY_ALL_READ_LIMIT + Duration::from_secs(5)).as_millis() as u64;
        let note = json!({ "title": "Copying…", "body": "Scrolling the program to read its history", "level": "info", "timeout": timeout });
        self.open_overlay(id.clone(), json!({ "kind": "toast", "notification": note }), window, cx);
        id
    }

    /// A `copy_all` finished: its "Copying…" toast goes, and what came of it
    /// shows: copied, as a copy does, or why not.
    fn copied_all(&mut self, copying: Option<String>, note: Result<Value, Value>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = copying {
            self.remove_overlay(&id, window, cx);
        }
        match note {
            Ok(note) => self.show_copied_as(note, window, cx),
            Err(note) => self.open_overlay(mods::new_overlay_id(), json!({ "kind": "toast", "notification": note }), window, cx),
        }
    }

    fn shown_terminal_view(&self, address: &str, window: &Window, cx: &App) -> Result<Entity<TerminalView>, String> {
        if !self.live.contains_key(address) {
            return Err(format!("no running terminal {address:?}"));
        }

        let own_pane = self.model.pane_of(address);
        // The view the person sees wins over a hidden one, such as a peek over its own tab's pane.
        let best = self.terms.iter().filter(|(_, t)| t.address == address).max_by_key(|(pane, t)| {
            let view = t.view.read(cx);
            (view.owns_keyboard_focus(window, cx), view.on_screen_this_frame(), own_pane.as_deref() == Some(pane.as_str()))
        });
        best.map(|(_, t)| t.view.clone()).ok_or_else(|| format!("terminal {address:?} is running but not shown in this window"))
    }

    fn scroll_terminal(&self, args: &Value, window: &Window, cx: &mut Context<Self>) -> Result<(), String> {
        let target = scroll_target(args)?;

        let view = match Self::owned(args, "terminal") {
            Some(address) => self.shown_terminal_view(&address, window, cx)?,
            None => self.focused_terminal(window, cx).map(|t| t.view.clone()).ok_or("no focused terminal to scroll")?,
        };

        view.update(cx, |view, cx| view.scroll_history(target, cx));
        Ok(())
    }

    /// Gives the focused pane the keyboard, unless something other than the
    /// window holds it: a dialog, a mod block, an input.
    fn refocus_pane(&self, window: &mut Window, cx: &mut App) {
        let dialog = self.overlays.iter().any(|o| o.kind.modal_keys());
        let window_holds_keys = window.focused(cx).is_none() || self.focus.is_focused(window);
        if dialog || self.focused_block.is_some() || !window_holds_keys {
            return;
        }
        if let Some(f) = self.focused_pane.as_ref().and_then(|p| self.pane_focus(p, cx)) {
            f.focus(window, cx);
        }
    }

    fn pane_focus(&self, pane: &str, cx: &App) -> Option<FocusHandle> {
        self.pane_term(pane).map(|t| t.view.read(cx).focus_target(cx))
    }

    fn load(&self, inst: Instance, window: &mut Window, cx: &mut Context<Self>) -> Copy {
        let mut copy = Copy { inst, id: 0, root: None, _watch: None, error: None };
        let (Some(runtime), Some(folder)) = (&self.runtime, self.folders.get(&copy.inst.name)) else {
            copy.error = Some(format!("no mod folder ~/.config/herdr-app/mods/{}", copy.inst.name));
            return copy;
        };
        copy.id = mods::prepare_load(&copy.inst);
        match runtime.try_load(&folder.dir, window, cx) {
            Ok(root) => {
                if self.file_events.is_none() {
                    copy._watch = runtime.watch(&root, window, cx).map_err(|e| tracing::warn!("{}: no hot reload: {e:#}", copy.inst.name)).ok();
                }
                copy.root = Some(root);
            }
            Err(e) => {
                tracing::warn!("mod {} failed to load: {e:#}", copy.inst.name);
                copy.error = Some(format!("{e:#}"));
            }
        }
        copy
    }

    fn refresh_mods(&self, cx: &mut Context<Self>) {
        self.refresh_mods_matching(true, cx);
    }

    /// Only the copies whose reads changed (`mods::snapshot_changed`).
    fn refresh_snapshot_mods(&self, cx: &mut Context<Self>) {
        self.refresh_mods_matching(false, cx);
    }

    /// A hidden copy draws nothing: it catches up when shown (`publish_shown_copies`).
    fn refresh_mods_matching(&self, all: bool, cx: &mut Context<Self>) {
        if let Some(runtime) = &self.runtime {
            for c in self.copies.values() {
                let Some(root) = &c.root else { continue };
                if !mods::is_shown(c.id) {
                    if all {
                        mods::missed(c.id);
                    }
                } else if all || mods::snapshot_changed(c.id) {
                    let _ = runtime.refresh(root, cx);
                }
            }
        }
    }

    /// How a terminal shows (`program` for mods): its agent, else what it runs.
    fn program(&self, addr: &str) -> Value {
        let rec = self.live.get(addr).and_then(Value::as_object);
        program_of(rec.and_then(|r| r.get("report")).unwrap_or(&Value::Null), rec.and_then(running_argv))
    }

    /// Tells mods what changed (they re-render only if something did).
    fn publish(&mut self, cx: &mut Context<Self>) {
        let m = &self.model;
        let home = home();
        // A window's terminal with the keyboard first, then a layout terminal block.
        let block = overlay_terminal_block(&self.overlays, self.window_terminal.as_deref())
            .or_else(|| terminal_block(&self.chrome, self.focused_pane.as_deref()));
        let focused_pane = if block.is_some() { None } else { m.focused_pane() };
        let mut agents = Vec::new();
        let mut rows = Vec::new();
        let mut panes = Vec::new();
        let auto_names_on = settings::get().pane_auto_names;
        let icon_style = crate::pane_icons::style(settings::get().space_avatar.as_deref());
        let mut auto_names = HashMap::new();
        let mut labels = HashMap::new();
        let mut icons: HashMap<&str, Value> = HashMap::new();
        // Every pane's label, mod panes too: an unnamed tab's letter is its focused pane's.
        let mut shown_labels: HashMap<&str, String> = HashMap::new();
        let header = settings::get().pane_header_info.as_deref().map(crate::header_info::Template::parse).filter(|t| !t.is_empty());
        let card = crate::header_info::card_template(settings::get().agents_card_info.as_deref());
        let templates: Vec<&crate::header_info::Template> = header.iter().chain(card.iter()).collect();
        let now = model::now_ms();
        let mut next_tick: Option<u64> = None;
        for pr in &m.spaces {
            let space_name = m.space_name(pr, &home);
            for (ci, ctx) in pr.tabs.iter().enumerate() {
                let tab_panes = ctx.panes();
                let names = if auto_names_on { crate::pane_names::for_tab(tab_panes.iter().map(|p| p.id.as_str())) } else { HashMap::new() };
                for p in tab_panes {
                    let focused = m.active.as_deref() == Some(pr.id.as_str()) && focused_pane.as_deref() == Some(p.id.as_str());
                    let zoomed = ctx.zoomed.as_deref() == Some(p.id.as_str());
                    let floating = ctx.floating.iter().position(|f| f.pane.id == p.id).map(|z| json!({ "rect": ctx.floating[z].rect, "z": z, "hidden": false }));
                    // A mod pane: what it shows, no terminal.
                    if let Some(Content::Mod { name, .. }) = p.content() {
                        let label = crate::pane_label::given(p.title.as_deref()).unwrap_or(name);
                        let own = m.icons.get(&p.id);
                        let icon = crate::pane_icons::pane(own, None, label, icon_style);
                        icons.insert(&p.id, icon.clone());
                        shown_labels.insert(&p.id, label.to_owned());
                        panes.push(json!({ "pane": p.id, "terminal": null, "mod": name, "tab": ctx.id, "title": name, "name": p.title, "auto_name": null, "label": label, "session": null, "icon": icon, "own_icon": own, "focused": focused, "zoomed": zoomed, "floating": floating, "movable": self.pane_movable(&p.id) }));
                        continue;
                    }
                    let Some(addr) = p.terminal() else { continue };
                    let a = m.agents.get(addr);
                    let title = self.terms.get(&p.id).map(|t| t.view.read(cx).title.clone()).or_else(|| {
                        self.live.get(addr).and_then(|r| r.get("title")).and_then(Value::as_str).map(str::to_owned)
                    });
                    let program = self.program(addr);
                    let cwd = self.cwds.get(addr);
                    let auto_name = names
                        .get(&p.id)
                        .filter(|_| crate::pane_names::shown(p.title.is_some(), &program, title.as_deref().unwrap_or_default(), addr, cwd.map(String::as_str), &home));
                    if let Some(n) = auto_name {
                        auto_names.insert(p.id.clone(), n.clone());
                    }
                    let session = self.session_names.name(addr, self.live.get(addr).and_then(|r| r.pointer("/foreground/pid")).and_then(Value::as_u64));
                    let label = crate::pane_label::label(&crate::pane_label::Parts {
                        name: p.title.as_deref(), auto_name: auto_name.map(String::as_str), title: title.as_deref().unwrap_or_default(),
                        session, program: &program, address: addr,
                    });
                    let own = m.icons.get(&p.id);
                    let icon = crate::pane_icons::pane(own, auto_name.map(String::as_str), &label, icon_style);
                    icons.insert(&p.id, icon.clone());
                    shown_labels.insert(&p.id, label.clone());
                    labels.insert(p.id.clone(), label.clone());
                    // What the header and card templates show, worked out once for both.
                    let values = m.status(addr).filter(|_| !templates.is_empty()).map(|status| {
                        let since = a.map(|a| a.since).filter(|s| *s > 0);
                        if let Some(since) = since.filter(|_| templates.iter().any(|t| t.uses_elapsed())) {
                            let at = crate::header_info::next_change(since, now);
                            next_tick = Some(next_tick.map_or(at, |t| t.min(at)));
                        }
                        let blocked = status == "blocked";
                        let said = crate::alert::Said::of(self.live.get(addr).and_then(|r| r.get("report")));
                        let folder = m.folder_info.get(addr);
                        let mut v = crate::header_info::Values {
                            ask: crate::header_info::ask(blocked, &said),
                            blocked,
                            state: status.to_owned(),
                            elapsed: since.map(|s| crate::header_info::elapsed(now.saturating_sub(s))).unwrap_or_default(),
                            branch: folder.and_then(|f| f.branch.clone()).unwrap_or_default(),
                            ahead: folder.map_or(0, |f| f.ahead),
                            behind: folder.map_or(0, |f| f.behind),
                            dirty: folder.is_some_and(|f| f.dirty),
                            subagents: if templates.iter().any(|t| t.uses_subagents()) { crate::header_info::count_subagents(&self.live, addr, |k| m.status(k)) } else { Default::default() },
                            space: space_name.clone(),
                            ..Default::default()
                        };
                        v.on_machine(addr);
                        v
                    });
                    let header_info = header.as_ref().zip(values.as_ref()).and_then(|(t, v)| t.resolve(v));
                    let card_info = card.as_ref().zip(values.as_ref()).and_then(|(t, v)| crate::header_info::card(t, v));
                    panes.push(json!({
                        "pane": p.id, "terminal": addr, "tab": ctx.id, "title": title.clone().unwrap_or_default(), "name": p.title, "auto_name": auto_name,
                        "label": label, "session": session, "icon": icon, "own_icon": own,
                        "cwd": cwd, "branch": m.folder_info.get(addr).and_then(|f| f.branch.clone()),
                        "status": m.status(addr), "app": a.and_then(|a| a.app.clone()), "program": program,
                        "focused": focused, "zoomed": zoomed, "floating": floating, "movable": self.pane_movable(&p.id),
                        "text_size": self.pane_term(&p.id).and_then(|t| t.view.read(cx).text_size()),
                        "header_info": header_info,
                    }));
                    let Some(status) = m.status(addr) else { continue };
                    let pinned = self.live.get(addr).and_then(|r| r.pointer("/labels/herdr.pinned")).and_then(Value::as_str).is_some_and(|v| !v.is_empty());
                    rows.push(crate::dock_badge::Listed { address: addr, status, pinned });
                    agents.push(json!({
                        "address": addr, "pane": p.id, "space": pr.id, "space_name": space_name,
                        "tab": ctx.id, "tab_index": ci + 1, "tab_name": Model::tab_name(pr, ctx),
                        "status": status, "app": a.and_then(|a| a.app.clone()), "program": self.program(addr), "title": title.unwrap_or_default(),
                        "label": label, "session": session, "icon": icon,
                        "seq": a.map(|a| a.seq).unwrap_or(0),
                        "pinned": pinned,
                        "focused": m.active.as_deref() == Some(pr.id.as_str()) && focused_pane.as_deref() == Some(p.id.as_str()),
                        "card_info": card_info,
                    }));
                }
            }
        }
        let spaces: Vec<Value> = m
            .spaces
            .iter()
            .enumerate()
            .map(|(i, p)| {
                // The branch of the terminal the space shows first.
                let first = p.active_tab().and_then(|c| c.focused_pane()).and_then(|pane| m.address_of(&pane));
                let branch = first.and_then(|a| m.folder_info.get(&a)).and_then(|f| f.branch.clone());
                let tab_icon = |c: &model::Tab| {
                    let focused = c.focused_pane().and_then(|pane| icons.get(pane.as_str()));
                    let name = if c.name.is_empty() { c.focused_pane().and_then(|pane| shown_labels.get(pane.as_str())).cloned().unwrap_or_else(|| Model::tab_name(p, c)) } else { c.name.clone() };
                    (crate::pane_icons::tab(m.icons.get(&c.id), &name, focused), crate::pane_icons::letter(&name))
                };
                model::space_json(m, p, i, branch.as_deref(), &home, &tab_icon)
            })
            .collect();
        let focused = m.active_space().and_then(|p| {
            let ctx = p.active_tab()?;
            if let Some(block) = block {
                let address = block.terminal()?;
                return Some(json!({ "space": p.id, "tab": ctx.id, "pane": null, "terminal": address,
                    "shown": null, "agent": self.shown_agent(), "cwd": self.cwds.get(address) }));
            }
            let pane = ctx.focused_pane()?;
            let cwd = (if self.shows_agent() { self.shown_agent() } else { m.address_of(&pane) }).and_then(|a| self.cwds.get(&a).cloned());
            Some(json!({ "space": p.id, "tab": ctx.id, "terminal": m.address_of(&pane), "shown": self.swapped.get(&pane),
                "pane": pane, "agent": self.shown_agent(), "cwd": cwd }))
        });
        self.auto_names = auto_names;
        self.labels = labels;
        let machines = self.documents.iter().map(|(name, slot)| json!({
            "name": (!name.is_empty()).then_some(name), "layout": slot.layout(),
        })).collect();
        let live: Vec<_> = self
            .live
            .iter()
            .filter(|(_, r)| r.get("state").and_then(Value::as_str) == Some("live"))
            .map(|(address, r)| crate::dock_badge::Live {
                address,
                parent: r.get("parent").and_then(Value::as_str).filter(|p| !p.is_empty()),
                word: r.get("status").and_then(Value::as_str),
            })
            .collect();
        let waiting = crate::dock_badge::count_rows(&rows, &live);
        let mut changed = mods::publish_machines(Value::Array(machines));
        changed |= mods::publish_spaces(Value::Array(spaces));
        changed |= mods::publish_agents(Value::Array(agents));
        changed |= mods::publish_panes(Value::Array(panes));
        changed |= mods::publish_focused(focused.unwrap_or(Value::Null));
        changed |= mods::publish_floats(self.float_placements());
        changed |= mods::publish_layout(&self.chrome);
        changed |= mods::publish_spacing(layout_sync::spacing_fields(&self.chrome));
        changed |= self.note_focus(Instant::now());
        if changed {
            self.refresh_snapshot_mods(cx);
        }
        self.write_where(cx);
        self.badge_dock(waiting);
        self.tick_header_info(next_tick, cx);
    }

    /// Publishes again when the next `{elapsed}` changes (one timer, on the
    /// earliest), so headers move on by the minute and never by the frame.
    fn tick_header_info(&mut self, at: Option<u64>, cx: &mut Context<Self>) {
        let Some(at) = at else {
            self.header_tick = None;
            return;
        };
        if self.header_tick.as_ref().is_some_and(|(t, _)| *t == at) {
            return;
        }
        let wait = Duration::from_millis(at.saturating_sub(model::now_ms()) + 20);
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |this, cx| {
                this.header_tick = None;
                this.publish(cx);
            });
        });
        self.header_tick = Some((at, task));
    }

    /// The Dock badge counts the agents list's waiting rows; the Dock
    /// hears only of changes. `dock_bounce_on_wait` bounces it once when
    /// something starts waiting and nothing did.
    fn badge_dock(&mut self, waiting: usize) {
        let s = settings::get();
        let change = self.dock_badge.update(waiting, s.dock_badge);
        if let Some(label) = change.label {
            crate::dock_badge::show(&label);
        }
        if change.rose && s.dock_bounce_on_wait && !self.window_active {
            crate::dock_badge::bounce();
        }
    }

    /// Where the person is: the active tab's focused pane. Overlays, popups and
    /// layout blocks holding the keyboard (an agent sidebar's terminal) leave it.
    fn here(&self) -> Option<crate::focus_history::Place> {
        let m = &self.model;
        let space = m.active_space()?;
        let tab = space.active_tab()?;
        let pane = tab.focused_pane()?;
        Some(crate::focus_history::Place {
            terminal: m.address_of(&pane), pane, tab: Some(tab.id.clone()), space: Some(space.id.clone()),
            machine: space.machine.clone(), at: model::now_ms(), ..Default::default()
        })
    }

    /// Notes where the person is and whatever place they settled on; true
    /// when `recentPlaces()` changed. Runs on every publish (a title change
    /// too), so nothing settling costs no list.
    fn note_focus(&mut self, now: Instant) -> bool {
        let here = self.here();
        let history = &mut self.focus_history;
        let left = history.settle.observe(here, now);
        let mut noted = false;
        for place in left.into_iter().chain(history.settle.settled(now)) {
            history.note(place);
            noted = true;
        }
        noted && self.publish_places()
    }

    /// `recentPlaces()` for mods; true when it changed.
    fn publish_places(&self) -> bool {
        mods::publish_places(serde_json::to_value(self.focus_history.places()).unwrap_or_default())
    }

    /// Writes the places noted since the last round to `herdr.focus` (each
    /// tick; only when they add a visit), and reads it again after a change to
    /// it (`changed`: the version it's at, None when unknown).
    fn flush_focus(&mut self, changed: Option<Option<u64>>, cx: &mut Context<Self>) {
        if self.draining { return; }
        if let Some(version) = changed {
            self.focus_history.changed(version);
        }
        if !self.documents_watching { return; }
        if self.note_focus(Instant::now()) {
            self.refresh_snapshot_mods(cx);
        }
        let Some((known, unsent)) = self.focus_history.due(Instant::now()) else { return };
        let (core, generation) = (self.core.clone(), self.core_generation);
        cx.spawn(async move |this, cx| {
            let sent = unsent.clone();
            let result = cx.background_executor().spawn(async move {
                let deadline = Instant::now() + Duration::from_secs(5);
                crate::focus_history::sync(&crate::focus_history::OnCore(&core, deadline), known, &unsent)
            }).await;
            let _ = this.update(cx, |this, cx| {
                if generation != this.core_generation { return; }
                this.focus_history.finished(&sent, result, Instant::now());
                if this.publish_places() {
                    this.refresh_snapshot_mods(cx);
                }
            });
        }).detach();
    }

    /// Tells the core where each terminal lives, in words: the `where` label,
    /// `<space> / <tab>`, which hc shows in its picker and `ls`.
    /// Sub-agents get their parent's. The ids (`herdr.space`,
    /// `herdr.tab`) are what the app groups by; only this follows renames
    /// and moves. Written only when it changes.
    fn write_where(&mut self, cx: &mut Context<Self>) {
        let m = &self.model;
        let home = home();
        let mut want: HashMap<String, String> = HashMap::new();
        for p in &m.spaces {
            let name = m.space_name(p, &home);
            for c in &p.tabs {
                for a in c.addresses() {
                    want.insert(a, format!("{name} / {}", Model::tab_name(p, c)));
                }
            }
        }
        for addr in self.live.keys() {
            if want.contains_key(addr) {
                continue;
            }
            let mut cur = addr.clone();
            for _ in 0..32 {
                let Some(p) = self.live.get(&cur).and_then(|r| r.get("parent")).and_then(Value::as_str) else { break };
                if let Some(place) = want.get(p).cloned() {
                    want.insert(addr.clone(), place);
                    break;
                }
                cur = p.to_owned();
            }
        }
        self.written_where.retain(|a, _| self.live.contains_key(a));
        for (addr, place) in want {
            if self.written_where.get(&addr) == Some(&place) || !self.live.contains_key(&addr) {
                continue;
            }
            // Off the UI thread (a terminal on a slow machine answers late); a
            // failed write is forgotten so the next publish tries again.
            self.written_where.insert(addr.clone(), place.clone());
            let core = self.core.clone();
            cx.spawn(async move |this, cx| {
                let params = json!({ "terminal": addr, "labels": { "where": place } });
                let Err(e) = cx.background_executor().spawn(async move { core.call("terminal.label", params) }).await else { return };
                tracing::warn!("where label for {addr}: {e:#}");
                let _ = this.update(cx, |this, _| {
                    if this.written_where.get(&addr) == Some(&place) {
                        this.written_where.remove(&addr);
                    }
                });
            })
            .detach();
        }
    }

    fn flush_documents(&mut self, refresh: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.draining || !self.documents_watching { return; }
        let generation = self.core_generation;
        for (machine, slot) in &mut self.documents {
            slot.dirty |= refresh;
            if !slot.ready() { continue; }
            let doc = crate::mirror::Document::capture(&self.model, (!machine.is_empty()).then_some(machine.as_str()));
            let write = (self.drag.is_none() && slot.available && slot.document.as_ref() != Some(&doc)).then_some(doc);
            if write.is_none() && slot.available && !slot.dirty { continue; }
            let (tx, rx) = std::sync::mpsc::channel();
            slot.pending = Some(rx);
            slot.dirty = false;
            let work = crate::mirror_sync::Work { machine: machine.clone(), version: slot.version, write };
            let core = self.core.clone();
            // Quit can block the foreground before the continuation gets a turn.
            let job = cx.background_executor().spawn(async move {
                let (machine, outcome) = crate::mirror_sync::round(&core, work);
                let _ = tx.send(outcome);
                machine
            });
            cx.spawn_in(window, async move |this, cx| {
                let machine = job.await;
                let _ = this.update_in(cx, |this, window, cx| {
                    if generation != this.core_generation || this.draining { return; }
                    let result = this.documents.get_mut(&machine).and_then(|s| s.pending.take()).and_then(|rx| rx.try_recv().ok());
                    if let Some(result) = result
                        && this.documents_updated(None, vec![(machine, result)], window, cx) { this.sync(window, cx); }
                });
            }).detach();
        }
    }

    fn show_copied(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_copied_as(json!({ "title": "Copied" }), window, cx);
    }

    /// One "Copied" toast at a time: quick selections replace it instead of stacking.
    fn show_copied_as(&mut self, mut note: Value, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(old) = self.copied_toast.take() {
            self.remove_overlay(&old, window, cx);
        }
        let id = mods::new_overlay_id();
        for (k, v) in [("level", json!("success")), ("timeout", json!(1500))] {
            if note.get(k).is_none() {
                note[k] = v;
            }
        }
        self.open_overlay(id.clone(), json!({ "kind": "toast", "notification": note }), window, cx);
        self.copied_toast = Some(id);
    }

    fn layout_notice(&mut self, title: &str, body: &str, window: &mut Window, cx: &mut Context<Self>) {
        let spec = json!({"kind":"toast", "notification":{"title":title,"body":body,"level":"info"}});
        self.open_overlay(mods::new_overlay_id(), spec, window, cx);
    }

    /// Finish the accepted edit and any newer projection before the window goes.
    pub fn drain_documents(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.draining { return; }
        self.save();
        self.draining = true;
        QUITTING.store(true, std::sync::atomic::Ordering::SeqCst);
        while let Some(address) = self.pending_terminals.keys().next().cloned() {
            let pending = self.pending_terminals.remove(&address).unwrap();
            let timeout = Duration::from_secs(if address.contains('/') { 2 } else { 10 });
            let result = pending.reply.recv_timeout(timeout).unwrap_or_else(|e| Err(format!("terminal change did not finish before quit: {e}")));
            self.apply_terminal_change(address, pending.change, result, window, cx);
        }
        let machines = self.documents.keys().cloned().collect::<Vec<_>>();
        for machine in machines {
            let timeout = Duration::from_secs(if machine.is_empty() { 10 } else { 2 });
            let deadline = Instant::now() + timeout;
            let pending = self.documents.get_mut(&machine).and_then(|s| s.pending.take());
            if let Some(pending) = pending {
                match pending.recv_timeout(timeout) {
                    Ok(result) => { self.documents_updated(None, vec![(machine.clone(), result)], window, cx); }
                    Err(e) => {
                        tracing::error!("layout on {machine:?} could not finish saving before quit: {e}");
                        continue;
                    }
                }
            }
            let slot = &self.documents[&machine];
            let doc = crate::mirror::Document::capture(&self.model, (!machine.is_empty()).then_some(machine.as_str()));
            if slot.document.as_ref() == Some(&doc) { continue; }
            if !slot.available {
                tracing::error!("layout on {machine:?} could not save before quit: {}", slot.error.as_deref().unwrap_or("machine unavailable"));
                continue;
            }
            let work = crate::mirror_sync::Work { machine: machine.clone(), version: slot.version, write: Some(doc) };
            let result = crate::mirror_sync::round_timeout(&self.core, work, deadline.saturating_duration_since(Instant::now()));
            self.documents_updated(None, vec![result], window, cx);
        }
        // The place the person was on when they quit; a round still running merges with it.
        self.note_focus(Instant::now());
        // Nothing unsent: no read just to quit. One try, whatever the backoff.
        self.focus_history.pending = false;
        self.focus_history.retry_now();
        if self.focus_history.has_unsent()
            && let Some((known, unsent)) = self.focus_history.due(Instant::now())
        {
            let deadline = Instant::now() + Duration::from_secs(1);
            let result = crate::focus_history::sync(&crate::focus_history::OnCore(&self.core, deadline), known, &unsent);
            self.focus_history.finished(&unsent, result, Instant::now());
        }
        self.save_view();
    }

    fn documents_updated(&mut self, machines: Option<Value>, results: Vec<(String, crate::mirror_sync::Outcome)>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        use crate::mirror_sync::Outcome;
        let mut changed = false;
        // The listing every 500 ms brings the machines alone: redraw only when
        // one's state moved, or the whole window draws twice a second for nothing.
        let shown = |d: &std::collections::BTreeMap<String, crate::mirror_sync::Slot>| {
            d.iter().map(|(m, s)| (m.clone(), s.connected, s.available, s.blocked, s.error.clone(), s.layout())).collect::<Vec<_>>()
        };
        let (listing, was) = (results.is_empty(), shown(&self.documents));
        for (machine, result) in results {
            let Some(slot) = self.documents.get_mut(&machine) else { continue };
            let first = slot.document.is_none();
            slot.read_attempted = true;
            let (document, version, replace, conflict, written) = match result {
                Outcome::Read(d, v) => {
                    let replace = !slot.available || slot.version != v || slot.document.is_none();
                    let projection = crate::mirror::Document::capture(&self.model, (!machine.is_empty()).then_some(machine.as_str()));
                    let unsent = slot.document.as_ref().is_some_and(|old| old != &projection);
                    let conflict = replace && unsent && projection != d;
                    (d, v, replace, conflict, false)
                }
                Outcome::Written(d, v) => (d, v, false, false, true),
                Outcome::Conflict(d, v) => (d, v, true, true, false),
                error @ (Outcome::Unavailable(_) | Outcome::Undecodable(_) | Outcome::Blocked(_)) => {
                    let blocked = !matches!(&error, Outcome::Unavailable(_));
                    let error = match error {
                        Outcome::Unavailable(e) | Outcome::Undecodable(e) | Outcome::Blocked(e) => e,
                        _ => unreachable!(),
                    };
                    let notify = slot.connected && blocked && slot.notified_error.as_ref() != Some(&error);
                    slot.available = false;
                    slot.blocked = blocked;
                    if slot.connected || slot.error.is_none() { slot.error = Some(error.clone()); }
                    if notify {
                        tracing::warn!("layout on {machine:?} unavailable: {error}");
                        slot.notified_error = Some(error.clone());
                        self.layout_notice("Layout unavailable", &format!("{}: {error}", if machine.is_empty() { "This machine" } else { &machine }), window, cx);
                    }
                    continue;
                }
            };
            slot.version = version;
            slot.available = slot.connected;
            slot.blocked = false;
            if slot.connected { slot.error = None; slot.notified_error = None; }
            // A read at the version already held is the document already held
            // (writes are compare-and-swap): keep this build's capture of it, or
            // the raw read would bring back what reading tidied and write it again.
            if written {
                slot.document = Some(document.clone());
            }
            if replace {
                let on = (!machine.is_empty()).then_some(machine.as_str());
                document.replace(&mut self.model, on);
                // What this build makes of it is what's there: a read alone writes
                // nothing back, even when reading tidied it (a float's tiled copy
                // an older build adopted), so mixed builds never rewrite each other.
                slot.document = Some(crate::mirror::Document::capture(&self.model, on));
                if first && let Some(view) = &self.initial_view { view.apply_machine(&mut self.model, on); }
                changed = true;
            }
            if conflict {
                self.layout_notice("Layout changed elsewhere", "The other screen's layout is shown.", window, cx);
            }
        }
        if let Some(machines) = machines {
            // Temporary machines are agents' short-lived sandboxes: the app leaves them out.
            let kept = || machines["machines"].as_array().into_iter().flatten().filter(|m| m["temporary"] != true);
            let states = kept().filter_map(|m| Some((m["name"].as_str()?, m["state"].as_str()?))).collect::<HashMap<_, _>>();
            for machine in kept() {
                let Some(name) = machine["name"].as_str() else { continue };
                let state = machine["state"].as_str().unwrap_or("unavailable");
                let reason = (state != "connected").then(|| machine["why"].as_str().map(str::to_owned).unwrap_or_else(|| match state {
                    "needs_update" => format!("{name} needs a core update; run hc machine update {name}"),
                    "needs_login" => format!("{name} needs login; run hc machine login {name}"),
                    _ => format!("{name} is {state}"),
                }));
                self.documents.entry(name.to_owned()).or_default().connection(state == "connected", machine["since_ms"].as_u64(), reason);
            }
            if self.model.spaces.iter().any(|s| s.machine.as_deref().is_some_and(|m| !states.contains_key(m))) {
                let mut view = crate::mirror::View::capture(&self.model);
                if let Some(saved) = &self.initial_view {
                    view.preserve_unloaded(saved, |m| self.documents.get(m.unwrap_or("")).is_some_and(|s| s.document.is_some()));
                }
                self.initial_view = Some(view);
            }
            self.documents.retain(|m, _| m.is_empty() || states.contains_key(m.as_str()));
            let before = self.model.spaces.len();
            self.model.retain_machines(|m| states.contains_key(m));
            changed |= before != self.model.spaces.len();
        }
        if !listing || changed || shown(&self.documents) != was {
            self.publish(cx);
            cx.notify();
        }
        changed
    }

    // ---- core facts --------------------------------------------------------

    /// What a terminal view takes from the core's facts: its folder, where right-clicks go.
    fn tell_term(&self, view: &Entity<TerminalView>, addr: &str, cx: &mut Context<Self>) {
        let cwd = self.cwds.get(addr).map(std::path::PathBuf::from);
        let to_program = self.live.get(addr).and_then(|r| r.pointer("/labels/herdr.right-click")).and_then(Value::as_str) == Some("program");
        view.update(cx, |v, _| {
            v.cwd = cwd;
            v.right_click_to_program = to_program;
        });
    }

    fn terminals_listed(&mut self, v: Value, window: &mut Window, cx: &mut Context<Self>) {
        let mut list = v.get("terminals").cloned().unwrap_or(Value::Array(vec![]));
        let mut live = HashMap::new();
        for t in list.as_array_mut().into_iter().flatten() {
            let Some(obj) = t.as_object_mut() else { continue };
            let report = obj.remove("status").unwrap_or(Value::Null);
            let word = report.get("state").cloned().unwrap_or_else(|| if report.is_string() { report.clone() } else { Value::Null });
            obj.insert("status".into(), word);
            obj.insert("program".into(), program_of(&report, running_argv(obj)));
            obj.insert("report".into(), report);
            // As the panes show it: a spinning title would change every listing.
            if let Some(title) = obj.get("title").and_then(Value::as_str).map(|t| crate::terminal::shown_title(t).into_owned()) {
                obj.insert("title".into(), title.into());
            }
            if let Some(a) = obj.get("address").and_then(Value::as_str) {
                live.insert(a.to_owned(), Value::Object(obj.clone()));
            }
        }
        self.live = live;
        self.forget_gone_popped();
        if !std::mem::replace(&mut self.reaped_orphans, true) {
            let orphans: Vec<String> =
                orphan_popups(&self.live, std::process::id(), process_running)
                    .cloned()
                    .collect();
            for address in orphans {
                self.end_terminal(address, true, cx);
            }
        }
        self.closing_terminals.retain(|addr, completed| !*completed || self.live.contains_key(addr));
        // A new sub-agent changes only this list; mods that show teams need to redraw.
        // A listing that changed nothing as the app shows it (an agent's spinning
        // title, stripped above) is an ordinary tick, not news for the mods.
        let terminals_changed = mods::publish_terminals(list);
        self.tend_terminals(terminals_changed, window, cx);
        // Only a listing from the core counts: a tick before it has no terminals.
        self.listed_once = true;
        if terminals_changed {
            self.refresh_snapshot_mods(cx);
        }
    }

    /// Every tick, with or without news from the core: where the terminals go
    /// (a layout that arrived late can place one now), what they mean now (a
    /// finished agent you're looking at is no longer news), their folders, their
    /// streams. `listed`: the list itself changed.
    fn tend_terminals(&mut self, listed: bool, window: &mut Window, cx: &mut Context<Self>) {
        // Place newly labeled terminals first: one that is blocked on its first
        // listing is ours, and must ring.
        let mut changed = self.reconcile(window, cx);
        let visible: HashSet<String> = self.visible_addresses().into_iter().collect();
        let mut alerts = Vec::new();
        let sticky = settings::get().notification_sticky;
        let terms = &self.terms;
        let view = |addr: &str| terms.values().find(|t| t.address == addr).map(|t| t.view.read(cx));
        let unreported = self.unreported.update(crate::harness::registry(), &self.live, Instant::now(), |addr| view(addr).map(|v| v.writes()), |addr, lines| {
            view(addr).map(|v| (v.tail_text(lines), v.raw_title().to_owned())).unwrap_or_default()
        });
        for (addr, rec) in &self.live {
            let (word, app) = match unreported.get(addr) {
                Some(u) => (Some(u.word.to_owned()), Some(u.harness.clone())),
                None => (
                    rec.get("status").and_then(Value::as_str).map(str::to_owned),
                    rec.get("report").and_then(|r| r.get("app")).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned),
                ),
            };
            let looking = visible.contains(addr) && self.window_active;
            let prev = self.model.agents.get(addr).and_then(|a| a.word.clone());
            let child = rec.get("parent").and_then(Value::as_str).is_some_and(|p| !p.is_empty());
            if let Some(alert) = self.model.alert_for(addr, child, prev.as_deref(), word.as_deref(), looking) {
                alerts.push((addr.clone(), alert, prev.clone(), looking));
            }
            let said = word.clone();
            changed |= self.model.set_word(addr, word, app, looking);
            let turn = rec.get("turn").and_then(Value::as_u64).unwrap_or(0);
            let label = rec.pointer("/labels/herdr.seen").and_then(Value::as_str).and_then(|s| s.parse::<u64>().ok());
            // Core counts no turns for an agent read off its screen, so the
            // shared seen turn can't say whether one was seen: this app's own
            // seen flag does (`Model::status`).
            let seen_turn = if unreported.contains_key(addr) { None } else { label.max(self.written_seen.get(addr).copied()) };
            changed |= self.model.set_turn(addr, turn, seen_turn);
            // A notification from before Herdr started (a quit, a crash, an
            // update) can still be up: the core outlived it. Adopt the one the
            // agent's state would have posted, and take any other down.
            if !self.listed_once && !child {
                match (said.as_deref(), self.model.status(addr)) {
                    (Some("blocked"), _) => self.notified.posted(addr, model::Alert::Blocked),
                    (Some("error"), _) => self.notified.posted(addr, model::Alert::Error),
                    (_, Some("done")) => self.notified.posted(addr, model::Alert::Done),
                    _ => crate::alert::dismiss(&crate::alert::tag(addr), cx),
                }
            }
            if self.notified.withdraw(addr, said.as_deref(), looking, sticky) {
                crate::alert::dismiss(&crate::alert::tag(addr), cx);
            }
        }
        self.written_seen.retain(|a, _| self.live.contains_key(a));
        for gone in self.notified.gone(|a| self.live.contains_key(a)) {
            crate::alert::dismiss(&crate::alert::tag(&gone), cx);
        }
        changed |= self.share_seen(cx);
        // Changes already made before Herdr started aren't news.
        if self.listed_once {
            for (addr, alert, prev, looking) in alerts {
                self.alert(&addr, alert, prev, looking, window, cx);
            }
        }
        self.track_folders(cx);
        self.track_resume(window, cx);
        self.track_session_names(listed, cx);
        for t in self.terms.values() {
            if self.live.get(&t.address).is_some_and(|r| r["pid"].as_u64().is_some()) {
                t.view.update(cx, |v, cx| v.reattach_if_needed(self.core.clone(), cx));
            }
            self.tell_term(&t.view, &t.address, cx);
        }
        if changed {
            self.sync(window, cx);
        } else if listed {
            self.publish(cx);
        }
    }

    /// Records the resumable programs running now (resume.rs), and once after
    /// a start or reconnect offers to resume those whose terminals are bare
    /// shells again, as `resume_agents` says.
    fn track_resume(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (lookups, changed) = self.resume.observe(&self.live, crate::programs::resume_rule);
        if changed {
            self.save_resume();
        }
        for lookup in lookups {
            let core = self.core.clone();
            let job = cx.background_executor().spawn(async move {
                let session = crate::resume::find_session(&lookup, &home(), |path, machine| match machine {
                    None => std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}")),
                    Some(m) => core
                        .call("file.read", json!({ "path": path, "on": m }))
                        .map(|r| r["text"].as_str().unwrap_or_default().to_owned())
                        .map_err(|e| format!("{m}:{path}: {e:#}")),
                });
                (lookup, session)
            });
            cx.spawn_in(window, async move |this, cx| {
                let (lookup, session) = job.await;
                let _ = this.update(cx, |this, _| {
                    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
                    if this.resume.found(&lookup, session, now) {
                        this.save_resume();
                    }
                });
            })
            .detach();
        }
        self.offer_resume(window, cx);
    }

    /// Agent sessions' own names (`session` on panes, pane_label.rs), read off
    /// the main thread; a name that changed is published.
    fn track_session_names(&mut self, listed: bool, cx: &mut Context<Self>) {
        let checks = self.session_names.checks(&self.live, listed, Instant::now(), &home(), crate::programs::session_name);
        if checks.is_empty() {
            return;
        }
        let job = cx.background_executor().spawn(async move { checks.into_iter().map(crate::pane_label::look).collect::<Vec<_>>() });
        cx.spawn(async move |this, cx| {
            let found = job.await;
            let _ = this.update(cx, |this, cx| {
                if this.session_names.found(found) {
                    this.publish(cx);
                }
            });
        })
        .detach();
    }

    fn save_resume(&self) {
        if let Err(e) = self.resume.save(&resume_path()) {
            tracing::warn!("resume records not saved: {e}");
        }
    }

    /// The records that can be resumed now (of one terminal, when given), and
    /// the others with why not.
    fn resume_plan(&self, terminal: Option<&str>) -> (Vec<crate::resume::Record>, Vec<(crate::resume::Record, String)>) {
        let is_shell = |argv: &[String]| crate::programs::of(None, argv)["shell"] == true;
        let (mut ready, mut skipped) = (Vec::new(), Vec::new());
        for (r, why) in self.resume.readiness(&self.live, is_shell) {
            if terminal.is_some_and(|t| t != r.terminal) {
                continue;
            }
            match why {
                None => ready.push(r.clone()),
                Some(why) => skipped.push((r.clone(), why)),
            }
        }
        (ready, skipped)
    }

    fn offer_resume(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(since) = self.resume_offer else { return };
        let (ready, _) = self.resume_plan(None);
        if ready.is_empty() {
            // Terminals of other machines come in late; after a while, nothing's due.
            if since.elapsed() > RESUME_OFFER_WINDOW {
                self.resume_offer = None;
            }
            return;
        }
        self.resume_offer = None;
        match self.person.resume_agents {
            settings::ResumeAgents::Off => {}
            settings::ResumeAgents::Auto => {
                let (_, skipped) = self.resume_plan(None);
                self.resume_sessions(ready, skipped, true, window, cx);
            }
            settings::ResumeAgents::Ask => {
                let n = ready.len();
                let spec = json!({ "kind": "toast", "notification": {
                    "title": format!("Resume {n} session{}", if n == 1 { "" } else { "s" }),
                    "body": resumed_names(&ready),
                    "level": "info",
                    "timeout": 30000,
                    "action": { "name": "session.resume", "args": { "confirmed": true } },
                } });
                self.open_overlay(mods::new_overlay_id(), spec, window, cx);
            }
        }
    }

    /// Types each record's command line into its terminal, if that is still a
    /// bare shell when it gets there. What it did, as `session.resume` answers.
    fn resume_sessions(&mut self, ready: Vec<crate::resume::Record>, skipped: Vec<(crate::resume::Record, String)>, tell: bool, window: &mut Window, cx: &mut Context<Self>) -> Value {
        let mut resumed = Vec::new();
        for r in &ready {
            let powershell = self.live.get(&r.terminal).is_some_and(|rec| {
                let shell = crate::resume::foreground_argv(rec).into_iter().next()
                    .or_else(|| rec.pointer("/argv/0").and_then(Value::as_str).map(str::to_owned))
                    .unwrap_or_default();
                let name = shell.rsplit(['/', '\\']).next().unwrap_or(&shell).to_ascii_lowercase();
                name.starts_with("pwsh") || name.starts_with("powershell")
            });
            let text = crate::resume::command_line(&r.cwd, &r.argv, powershell);
            let (core, terminal, name) = (self.core.clone(), r.terminal.clone(), r.name.clone());
            let job = cx.background_executor().spawn(async move { core.call("terminal.send", json!({ "terminal": terminal, "text": text })).map_err(|e| format!("{e:#}")) });
            let (terminal, place) = (r.terminal.clone(), r.place.clone().unwrap_or_else(|| r.terminal.clone()));
            cx.spawn_in(window, async move |this, cx| {
                if let Err(why) = job.await {
                    tracing::warn!("resuming {name} in {terminal}: {why}");
                    let _ = this.update_in(cx, |this, window, cx| {
                        let spec = json!({"kind":"toast", "notification":{"title":format!("Could not resume {name}"),"body":format!("{place}: {why}"),"level":"error"}});
                        this.open_overlay(mods::new_overlay_id(), spec, window, cx);
                    });
                }
            })
            .detach();
            resumed.push(json!({ "terminal": r.terminal, "name": r.name, "where": r.place }));
        }
        if tell && !skipped.is_empty() {
            let body = skipped.iter().take(3).map(|(r, why)| format!("{} in {}: {why}", r.name, r.place.as_deref().unwrap_or(&r.terminal))).collect::<Vec<_>>().join("; ");
            let spec = json!({"kind":"toast", "notification":{"title":format!("Resumed {}, left {} as a shell", ready.len(), skipped.len()),"body":body,"level":"warning"}});
            self.open_overlay(mods::new_overlay_id(), spec, window, cx);
        }
        json!({
            "resumed": resumed,
            "skipped": skipped.iter().map(|(r, why)| json!({ "terminal": r.terminal, "name": r.name, "where": r.place, "why": why })).collect::<Vec<_>>(),
        })
    }

    /// `system.notify`: a notification any program posts as Herdr (alert.rs).
    fn system_notify(&mut self, args: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let notify = match crate::alert::parse_notify(args) {
            Ok(notify) => notify,
            Err(error) => return self.fail(error),
        };
        let agent = match notify.agent.as_deref().map(|who| self.agent_called(who)).transpose() {
            Ok(agent) => agent,
            Err(error) => return self.fail(error),
        };
        let settings = settings::get();
        match crate::alert::notify_to(settings.system_notifications, self.window_active, notify.urgent) {
            crate::alert::NotifyTo::Nowhere => self.fail("system.notify: system_notifications is off"),
            crate::alert::NotifyTo::Toast => {
                let body = [notify.subtitle.as_str(), notify.body.as_str()].into_iter().filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n");
                let mut note = json!({ "title": notify.title, "body": body, "level": "info" });
                if let Some(addr) = &agent {
                    note["action"] = json!({ "name": "agent.focus", "args": { "terminal": addr } });
                }
                self.open_overlay(mods::new_overlay_id(), json!({ "kind": "toast", "notification": note }), window, cx);
                self.answer(Ok(json!({ "ok": true, "shown": "toast" })));
            }
            crate::alert::NotifyTo::System if !crate::alert::can_post(cx) => {
                self.fail("system.notify: Herdr can't show system notifications here: allow them in System Settings > Notifications, and run Herdr from its app")
            }
            crate::alert::NotifyTo::System => {
                use crate::notification_icon::Source;
                let icon = match &notify.icon {
                    Some(icon) => Source::file(icon),
                    None if settings.notification_icons => agent.as_ref().and_then(|addr| self.model.agents.get(addr)?.app.clone()).and_then(|app| Source::program(&app)),
                    None => None,
                };
                let tag = crate::alert::notify_tag(notify.id.as_deref());
                crate::alert::show(crate::alert::notify_notification(&notify, &tag), icon, cx);
                self.notify_clicks.posted(&tag, agent.as_deref());
                self.answer(Ok(json!({ "ok": true, "shown": "notification", "tag": tag })));
            }
        }
    }

    /// A running terminal by its address, else the one agent with that name.
    fn agent_called(&self, who: &str) -> Result<String, String> {
        if self.live.contains_key(who) {
            return Ok(who.to_owned());
        }
        let named: Vec<&String> = self.live.iter().filter(|(_, rec)| rec.get("name").and_then(Value::as_str) == Some(who)).map(|(addr, _)| addr).collect();
        match named.as_slice() {
            [addr] => Ok((*addr).clone()),
            [] => Err(format!("system.notify: no running terminal or agent called {who:?}")),
            _ => Err(format!("system.notify: {} agents are called {who:?}: give its terminal address", named.len())),
        }
    }

    /// Tells the person an agent needs them or finished: Herdr's sounds, and a
    /// toast (drawn by the `toast` mod; click it to go there) unless they're
    /// looking at that agent. Outside the window too, as `system_notifications`
    /// says: a system notification, or the person's `alert_command` instead.
    fn alert(&mut self, addr: &str, alert: model::Alert, prev: Option<String>, looking: bool, window: &mut Window, cx: &mut Context<Self>) {
        let settings = settings::get();
        if settings.sounds.enabled {
            crate::sound::play(if alert.needs_you() { "blocked" } else { "done" });
        }
        // Needing the person brings a settled tab back.
        if let Some(ctx) = self.model.pane_of(&self.root_agent(addr)).and_then(|p| self.model.tab_of(&p)).map(|c| c.id.clone())
            && self.model.unsettle(&ctx)
        {
            self.save();
            self.publish(cx);
        }
        if looking {
            return;
        }
        let rec = self.live.get(addr);
        let app = self.model.agents.get(addr).and_then(|a| a.app.clone());
        let name = rec
            .and_then(|r| r.get("name").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned))
            .or_else(|| app.as_deref().map(crate::programs::name))
            .unwrap_or_else(|| "An agent".into());
        // A sub-agent lives where its top agent does.
        let home = self.root_agent(addr);
        let place = self.model.pane_of(&home).and_then(|pane| {
            let (p, c) = (self.model.space_of(&pane)?, self.model.tab_of(&pane)?);
            Some(format!("{} / {}", self.model.space_name(p, &self::home()), Model::tab_name(p, c)))
        });
        let said = crate::alert::Said::of(rec.and_then(|r| r.get("report")));
        let note = crate::alert::Note { address: addr.to_owned(), alert, prev, app, name, place, said };
        if crate::alert::should_notify(settings.system_notifications, self.window_active, looking) {
            match crate::alert::send(&note, &settings, cx) {
                crate::alert::Sent::Posted => {
                    self.notified.posted(addr, alert);
                }
                crate::alert::Sent::Fallback(why) => {
                    self.notified.posted(addr, alert);
                    self.alert_command_failed(why, window, cx);
                }
                // It may have shown something before failing: the system
                // notification goes too, late, but no second toast for the alert.
                crate::alert::Sent::Ran(failed) => {
                    let note = note.clone();
                    cx.spawn_in(window, async move |this, cx| {
                        let Ok(why) = failed.recv().await else { return };
                        let _ = this.update_in(cx, |this, window, cx| {
                            // Up to 30 s on: only if it is still news and the
                            // person isn't looking at the agent now.
                            let settings = settings::get();
                            let looking = this.window_active && this.visible_addresses().contains(&note.address);
                            let word = this.model.agents.get(&note.address).and_then(|a| a.word.clone());
                            if this.live.contains_key(&note.address) && crate::alert::still_says(note.alert, word.as_deref()) && crate::alert::should_notify(settings.system_notifications, this.window_active, looking) {
                                crate::alert::post(&note, &settings, cx);
                                this.notified.posted(&note.address, note.alert);
                            }
                            this.alert_command_failed(why, window, cx);
                        });
                    })
                    .detach();
                }
            }
        }
        if alert.needs_you() && settings.dock_bounce && !self.window_active {
            window.request_attention();
        }
        let agent = AlertedAgent { address: addr.to_owned(), name: note.name, place: note.place };
        if self.join_alert_toast(alert, agent.clone(), cx) {
            return;
        }
        let toast = AlertToast { alert, agents: vec![agent], joined: Instant::now() };
        let id = mods::new_overlay_id();
        self.open_overlay(id.clone(), json!({ "kind": "toast", "notification": toast.notification() }), window, cx);
        if let Some(o) = self.overlays.iter_mut().find(|o| o.id == id) {
            o.alert = Some(toast);
        }
    }

    /// Puts an alert in the toast of an alert of the same kind that came
    /// moments ago, so a wave of agents finishing is one toast, redrawn, not one
    /// mod loaded per agent. Returns whether such a toast took it.
    fn join_alert_toast(&mut self, alert: model::Alert, agent: AlertedAgent, cx: &mut Context<Self>) -> bool {
        let now = Instant::now();
        let Some(o) = self.overlays.iter_mut().rev().find(|o| o.alert.as_ref().is_some_and(|t| t.alert == alert && now.duration_since(t.joined) < ALERT_JOIN)) else {
            return false;
        };
        let Some(toast) = o.alert.as_mut() else { return false };
        toast.agents.retain(|a| a.address != agent.address);
        toast.agents.insert(0, agent);
        toast.joined = now;
        let mut props = toast.notification();
        props["overlay"] = json!(o.id);
        with_toast_look(&mut props);
        o.expires = toast_timeout(&props, settings::get().toast_duration).map(|t| now + t);
        o.opened = now;
        let id = o.id.clone();
        self.set_toast_props(&id, props, cx);
        self.count_banners(cx);
        self.tick_progress(cx);
        true
    }

    /// Redraws an open toast with `props`, in its box for the design now.
    fn set_toast_props(&mut self, id: &str, props: Value, cx: &mut Context<Self>) {
        let Some(o) = self.overlays.iter_mut().find(|o| o.id == id) else { return };
        let (min, max, height) = toast_box(settings::get().toast_design, &props);
        o.widths = (min, max);
        o.size.height = o.asked_height.unwrap_or(height);
        let slot = o.slot.clone();
        if let Some(copy) = self.copies.get_mut(&slot) {
            copy.inst.props = props.clone();
            mods::set_props(copy.id, props);
            if let (Some(runtime), Some(root)) = (&self.runtime, &copy.root) {
                let _ = runtime.refresh(root, cx);
            }
        }
        cx.notify();
    }

    /// The props of every open toast, in the order opened.
    fn toast_props(&self) -> Vec<(String, Value)> {
        self.overlays.iter().filter(|o| o.kind == OverlayKind::Toast).filter_map(|o| Some((o.id.clone(), self.copies.get(&o.slot)?.inst.props.clone()))).collect()
    }

    /// A new `toast_design` (or the terminal design's font) redraws the toasts already open in it.
    fn redesign_toasts(&mut self, cx: &mut Context<Self>) {
        for (id, mut props) in self.toast_props() {
            with_toast_look(&mut props);
            self.set_toast_props(&id, props, cx);
        }
        self.count_banners(cx);
    }

    /// The banner that shows, when toasts are banners: the newest error or
    /// warning, else the newest. An error waits behind no routine banner, as
    /// the toast cap closes routine toasts first.
    fn shown_banner(&self) -> Option<&Overlay> {
        if settings::get().toast_design != settings::ToastDesign::Banner {
            return None;
        }
        let toasts = || self.overlays.iter().rev().filter(|o| o.kind == OverlayKind::Toast);
        toasts().find(|o| o.urgent).or_else(|| toasts().next())
    }

    /// Whether a toast is a banner waiting behind the one shown: it's drawn
    /// nowhere, and its time doesn't run.
    fn waiting_banner(&self, o: &Overlay) -> bool {
        o.kind == OverlayKind::Toast && self.shown_banner().is_some_and(|shown| shown.id != o.id)
    }

    /// Banners show one at a time (`shown_banner`), which says how many more
    /// are open (`toast_more`); other designs show them all and say nothing.
    /// A toast that stops waiting (it comes forward, or toasts stop being
    /// banners) gets its whole time from then: its clock didn't run while it waited.
    fn count_banners(&mut self, cx: &mut Context<Self>) {
        let shown = self.shown_banner().map(|o| o.id.clone());
        if shown != self.banner_shown {
            let now = Instant::now();
            let (was, is) = (self.banner_shown.take(), shown.clone());
            let waiting = |front: &Option<String>, id: &str| front.as_deref().is_some_and(|f| f != id);
            for o in self.overlays.iter_mut().filter(|o| o.kind == OverlayKind::Toast && waiting(&was, &o.id) && !waiting(&is, &o.id)) {
                if let Some(expires) = o.expires {
                    o.expires = Some(now + expires.saturating_duration_since(o.opened));
                    o.opened = now;
                }
            }
            self.banner_shown = shown.clone();
        }
        let toasts = self.toast_props();
        let more = toasts.len().saturating_sub(1);
        for (id, mut props) in toasts {
            let want = (more > 0 && shown.as_ref() == Some(&id)).then(|| json!(more));
            if props.get("toast_more") == want.as_ref() {
                continue;
            }
            match (want, props.as_object_mut()) {
                (Some(n), Some(p)) => p.insert("toast_more".into(), n),
                (None, Some(p)) => p.remove("toast_more"),
                _ => continue,
            };
            self.set_toast_props(&id, props, cx);
        }
    }

    /// While a toast with a time-left bar counts down, redraws the window a
    /// step at a time (`PROGRESS_STEPS` over the shortest such toast, 2 to
    /// 10 a second); it stops at the first step with none left.
    fn tick_progress(&mut self, cx: &mut Context<Self>) {
        if self.progress_tick.is_some() || self.progress_step().is_none() {
            return;
        }
        self.progress_tick = Some(cx.spawn(async move |this, cx| {
            while let Ok(Some(step)) = this.update(cx, |this, _| this.progress_step()) {
                cx.background_executor().timer(step).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    return;
                }
            }
            let _ = this.update(cx, |this, _| this.progress_tick = None);
        }));
    }

    /// The share of a toast's time still left, or none: it stays until clicked.
    fn time_left(&self, o: &Overlay) -> Option<f32> {
        let total = o.expires?.saturating_duration_since(o.opened).as_secs_f32();
        let left = o.expires?.saturating_duration_since(Instant::now()).as_secs_f32();
        Some(if total > 0.0 { (left / total).clamp(0.0, 1.0) } else { 0.0 })
    }

    /// How long until the time-left bars step, or none: no toast counting down shows one.
    fn progress_step(&self) -> Option<Duration> {
        if !settings::get().toast_progress {
            return None;
        }
        // Banners waiting behind the shown one draw no bar, and their clocks don't run.
        let shortest = self.overlays.iter().filter(|o| o.kind == OverlayKind::Toast && !self.waiting_banner(o)).filter_map(|o| Some(o.expires?.saturating_duration_since(o.opened))).min()?;
        Some((shortest / PROGRESS_STEPS).clamp(PROGRESS_STEP_MIN, PROGRESS_STEP_MAX))
    }

    /// The error toast for an `alert_command` that didn't start or failed:
    /// said once per reason, not on every alert.
    fn alert_command_failed(&mut self, why: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.alert_command_errors.insert(why.clone()) {
            let failed = json!({ "title": "alert_command failed", "body": why, "level": "error" });
            self.open_overlay(mods::new_overlay_id(), json!({ "kind": "toast", "notification": failed }), window, cx);
        }
    }

    /// Keeps the model true to core, on every listing: drops panes whose terminal
    /// is gone (ended while hidden, or while the app was closed) and places
    /// terminals labeled `herdr.space` that aren't shown (started by an
    /// agent, a script, or before a restart). Returns whether anything changed.
    fn reconcile(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        // No fresh shells while quitting: the next start brings them back.
        if QUITTING.load(std::sync::atomic::Ordering::SeqCst) {
            return false;
        }
        let missing = self.model.tabs().flat_map(Tab::terminals).chain(self.chrome.terminals())
            .filter_map(|n| n.terminal().filter(|a| !self.live.contains_key(*a) && !self.just_born(a)).map(|a| (n.id.clone(), a.to_owned())))
            .collect::<Vec<_>>();
        if !missing.is_empty() && !self.checking_closed {
            self.check_closed(missing, window, cx);
        }
        let mut orphans = self.live.iter()
            .filter(|(a, _)| self.model.pane_of(a).is_none() && !self.changing_terminals.contains(*a) && !self.closing_terminals.contains_key(*a))
            .filter_map(|(a, r)| {
                let machine = a.split_once('/').map(|(m, _)| m);
                if !self.documents.get(machine.unwrap_or("")).is_some_and(|s| s.available) { return None; }
                let labels = r.get("labels")?;
                let space = labels.get("herdr.space")?.as_str().filter(|s| !s.is_empty())?;
                let tab = labels.get("herdr.tab").and_then(Value::as_str).filter(|s| !s.is_empty());
                Some((a.clone(), crate::mirror::qualify(space, machine), tab.map(|t| crate::mirror::qualify(t, machine))))
            }).collect::<Vec<_>>();
        orphans.sort();
        for (addr, space, tab) in orphans {
            self.start_adoption(addr, space, tab, window, cx);
        }
        false
    }

    fn start_adoption(&mut self, addr: String, space: String, tab: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.changing_terminals.insert(addr.clone());
        let core = self.core.clone();
        let generation = self.core_generation;
        cx.spawn_in(window, async move |this, cx| {
            let (address, wanted_space, wanted_tab) = (addr.clone(), space.clone(), tab.clone());
            let ready = cx.background_executor().spawn(async move {
                let record = core.call("terminal.get", json!({"terminal":address}))?;
                let machine = address.split_once('/').map(|(m, _)| m);
                if !adoptable(&record, &wanted_space, wanted_tab.as_deref(), machine) { return Ok(false); }
                if record["keep"].as_bool() != Some(true) {
                    core.call("terminal.keep", json!({"terminal":address,"keep":true}))?;
                }
                // A popup whose keep failed halfway, adopted all the same: a tab now, not a popup.
                if record.pointer(&format!("/labels/{POPUP_LABEL}")).is_some()
                    && let Err(e) = core.call("terminal.label", json!({"terminal":address,"labels":{ POPUP_LABEL: "" }}))
                {
                    tracing::warn!("popup label for adopted {address}: {e:#}");
                }
                anyhow::Ok(true)
            }).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.changing_terminals.remove(&addr);
                if generation != this.core_generation || this.draining { return; }
                if matches!(ready, Ok(true)) && this.model.pane_of(&addr).is_none()
                    && this.require_document(addr.split_once('/').map(|(m, _)| m)).is_ok()
                {
                    this.adopt(&addr, &space, tab.as_deref());
                    this.sync(window, cx);
                    this.flush_documents(false, window, cx);
                }
            });
        }).detach();
    }

    /// Puts a live terminal where its labels say, without changing what's
    /// shown. A space or tab the app doesn't know is made under the label's
    /// id, unnamed (called Herdr's way). A labeled tab that moved to another
    /// space (tab.move_to_space) is found there, never made twice.
    fn adopt(&mut self, addr: &str, space: &str, ctx: Option<&str>) {
        let moved = ctx.and_then(|t| self.model.space_of(t)).map(|s| s.id.clone());
        let space = moved.as_deref().unwrap_or(space);
        let machine = addr.split_once('/').map(|(m, _)| m.to_owned());
        let pane = self.model.fresh_id_on("p", machine.as_deref());
        let ctx_id = ctx.map(str::to_owned).unwrap_or_else(|| self.model.fresh_id_on("tb", machine.as_deref()));
        // Found by its labels: no names, so they're called Herdr's way.
        let new_ctx = Tab::new(&ctx_id, "", &pane, addr);
        if self.model.space(space).is_none() {
            // Its live folder may not be read yet: the core's record has it.
            let rec = self.live.get(addr);
            let folder = self
                .cwds
                .get(addr)
                .cloned()
                .or_else(|| rec.and_then(|r| r.get("foreground").and_then(|f| f.get("cwd")).or_else(|| r.get("cwd"))).and_then(Value::as_str).map(str::to_owned))
                .unwrap_or_else(|| mods::home().to_string_lossy().into_owned());
            let active = self.model.active.clone();
            self.model.add_space(space, "", &folder, new_ctx);
            self.model.space_mut(space).unwrap().machine = machine;
            self.model.active = active.or(Some(space.to_owned()));
            return;
        }
        // Its last tiled pane; a tab holding only floating panes splits one of
        // them, which lands in its tiled tree (`Tab::panes` lists the tiled first).
        let target = self.model.tab(&ctx_id).and_then(|c| c.panes().get(c.tiled_count().saturating_sub(1)).map(|p| p.id.clone()));
        if let Some(target) = target {
            let tab = self.model.tab_of(&target).unwrap();
            let focused = tab.focused.clone();
            let before = tab.before.clone();
            let zoomed = tab.zoomed.clone();
            let id = tab.id.clone();
            let axis = self.model.auto_axis(&target);
            self.model.split(&target, axis, &pane, addr);
            let tab = self.model.tab_mut(&id).unwrap();
            tab.focused = focused;
            tab.before = before;
            tab.zoomed = zoomed;
            return;
        }
        let keep = self.model.space(space).and_then(|p| p.active.clone());
        self.model.add_tab(space, new_ctx);
        if let Some(p) = self.model.space_mut(space) {
            p.active = keep;
        }
    }

    /// Each terminal's live folder, and its repo and branch from the shared git
    /// status (`git_status`): asked once per folder, again when its repo changes
    /// on disk. One that isn't watched (another machine's, no repo) is asked again
    /// when a command or an agent's turn ends there (a clone, a checkout).
    fn track_folders(&mut self, cx: &mut Context<Self>) {
        let home = mods::home().to_string_lossy().into_owned();
        let mut moved = false;
        let mut changed = false;
        let counts = crate::header_info::counts_git(settings::get().pane_header_info.as_deref(), settings::get().agents_card_info.as_deref());
        for (addr, rec) in &self.live {
            // Core's live folder of what runs there (any machine), else the shell's (local).
            let cwd = rec
                .get("foreground")
                .and_then(|f| f.get("cwd"))
                .and_then(Value::as_str)
                .map(str::to_owned)
                // (a pid on another machine means nothing here)
                .or_else(|| rec.get("pid").and_then(Value::as_u64).filter(|_| !addr.contains('/')).and_then(crate::process_cwd::read).map(|p| p.to_string_lossy().into_owned()))
                .or_else(|| rec.get("cwd").and_then(Value::as_str).map(str::to_owned));
            let Some(cwd) = cwd else { continue };
            let busy = busy(rec);
            let ended = self.folders_busy.insert(addr.clone(), busy) == Some(true) && !busy;
            moved |= self.cwds.get(addr) != Some(&cwd);
            self.cwds.insert(addr.clone(), cwd.clone());
            // Another machine's terminal (`vps/t7`): git runs there.
            let on = addr.split_once('/').map(|(m, _)| m.to_owned());
            let info = if on.is_none() && cwd == home {
                Some(model::FolderInfo { top: cwd, ..Default::default() })
            } else {
                let place = git_status::Place { on, folder: cwd.clone() };
                let (info, unwatched) = git_status::read(&place, |v, _, lasting| (folder_info(v, &cwd, counts), !lasting || v == &Value::Bool(false)));
                if ended && unwatched {
                    git_status::refresh(&place);
                }
                info
            };
            if let Some(info) = info
                && self.model.folder_info.get(addr) != Some(&info)
            {
                self.model.folder_info.insert(addr.clone(), info);
                changed = true;
            }
        }
        self.folders_busy.retain(|a, _| self.live.contains_key(a));
        // Refresh unnamed spaces when a terminal moves; mods show its folder.
        if moved {
            self.save();
        }
        if changed || moved {
            self.publish(cx);
            cx.notify();
        }
    }

    fn terminal_ended(&mut self, block: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pane) = block.strip_suffix(SWAP) {
            self.swapped.remove(pane);
            self.focused_pane = None;
            return self.sync(window, cx);
        }
        if let Some(address) = self.model.address_of(block) {
            self.notify_launch_failure(&address, window, cx);
        }
        if let Some(o) = self.overlays.iter().find(|o| o.layout.as_ref().is_some_and(|l| l.find(block).is_some())) {
            let id = o.id.clone();
            return self.close_overlay(&id, window, cx);
        }
        // How its stream ended, for `reconcile` (`hc run --rm` leaves no record).
        if let Some(t) = self.terms.get(block)
            && let Some(why) = t.view.read(cx).ended.clone()
        {
            // The program ended on its own (or was killed): its pane goes now. A
            // stream that just broke (the core died) says neither.
            let closed = matches!(why.as_str(), "exited" | "killed" | "finished");
            if closed && self.chrome.find(block).and_then(Node::terminal) == Some(t.address.as_str()) {
                self.chrome.remove_slot(block);
                return self.sync(window, cx);
            }
            if closed && self.model.address_of(block).as_deref() == Some(t.address.as_str()) {
                self.model.remove_pane(block);
                return self.sync(window, cx);
            }
        }
        // Otherwise the next listing checks closure; core owns lost terminals.
    }

    fn notify_launch_failure(&mut self, address: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.born.borrow_mut().remove(address).is_none() {
            return;
        }
        let core = self.core.clone();
        let address = address.to_owned();
        cx.spawn_in(window, async move |this, cx| {
            let failure = cx.background_executor().spawn(async move {
                let record = core.call("terminal.get", json!({"terminal": address})).ok()?;
                launch_failure(&record, "")?;
                let output = core.call("terminal.read", json!({"terminal": address, "lines": 5})).unwrap_or(Value::Null);
                launch_failure(&record, output["text"].as_str().unwrap_or_default())
            }).await;
            if let Some(body) = failure {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.open_overlay(mods::new_overlay_id(), json!({"kind":"toast", "notification":{"title":"Terminal failed to start", "body":body, "level":"error"}}), window, cx);
                });
            }
        }).detach();
    }

    // ---- creating terminals ------------------------------------------------

    /// The folder new terminals in `space` start in: where its shown tab's focused
    /// pane is now (Herdr's "follow"), else the folder the space was made in.
    fn folder_for(&self, space: &str) -> String {
        let p = self.model.space(space);
        let here = p.and_then(|p| p.active_tab()).and_then(|c| c.focused_pane()).and_then(|f| self.model.address_of(&f)).and_then(|a| self.cwds.get(&a).cloned());
        here.or_else(|| p.map(|p| p.folder.clone())).unwrap_or_else(home)
    }

    /// The machine a space's terminals run on, None for this one.
    fn require_document(&self, machine: Option<&str>) -> Result<(), String> {
        match self.documents.get(machine.unwrap_or("")) {
            Some(slot) if slot.available => Ok(()),
            Some(slot) => Err(slot.error.clone().unwrap_or_else(|| "This machine's layout is unavailable. Try again when it reconnects.".into())),
            None => Err(match machine {
                Some(m) => format!("Herdr has no machine called {m}. See hc machine ls; machines added with --temp stay out of Herdr."),
                None => "This machine's layout is unavailable. Try again when it reconnects.".into(),
            }),
        }
    }

    fn machine_of(&self, space: &str) -> Option<String> {
        self.model.space(space).and_then(|p| p.machine.clone())
    }

    /// Starts a terminal for `space`, on `on` (another machine) or here. An
    /// empty `cwd`: the machine's home. With no argv, that machine's core
    /// picks its own shell.
    fn spawn(&mut self, argv: Option<Vec<String>>, cwd: String, follow: bool, space: &str, ctx: &str, on: Option<&str>, size: Option<(u16, u16)>, window: &Window) -> Result<(String, String), String> {
        let argv = argv.unwrap_or_default();
        self.require_document(on)?;
        let labels = json!({ "herdr.space": crate::mirror::local(space, on), "herdr.tab": crate::mirror::local(ctx, on), "herdr.seen": "0" });
        let (addr, cwd) = run_following_folder(cwd, follow, on, |cwd| match self.core.run(argv.clone(), cwd.clone(), labels.clone(), on, size, crate::terminal::initial_display(window)) {
            // An agent on a machine without `sh` (Windows): in its PowerShell instead.
            Err(e) if on.is_some() && argv.first().map(String::as_str) == Some("sh") && format!("{e:#}").contains("not found") => {
                self.core.run(Self::powershell_argv(&argv), cwd, labels.clone(), on, size, crate::terminal::initial_display(window))
            }
            r => r,
        })
        .map_err(|e| format!("cannot start a terminal: {e:#}"))?;
        self.born.borrow_mut().insert(addr.clone(), Instant::now());
        // Follow the launch folder until the first live folder report arrives.
        if !cwd.is_empty() {
            self.cwds.insert(addr.clone(), cwd.clone());
        }
        Ok((addr, cwd))
    }

    /// The PowerShell form of a remote agent start (`agent_argv`): the program,
    /// then a shell that stays.
    fn powershell_argv(sh: &[String]) -> Vec<String> {
        let cmd = sh.get(4..).unwrap_or_default();
        vec!["powershell.exe".into(), "-NoLogo".into(), "-NoExit".into(), "-Command".into(), crate::programs::powershell_line(cmd)]
    }

    /// An agent with its first message, in a shell that stays when the agent exits
    /// (on another machine: that machine's shell).
    fn agent_argv(argv: Option<Vec<String>>, task: Option<&str>, on: Option<&str>) -> Option<Vec<String>> {
        let task = task.map(str::trim).filter(|t| !t.is_empty());
        argv.filter(|a| !a.is_empty()).map(|mut cmd| {
            cmd.extend(task.map(str::to_owned));
            if on.is_some() {
                return crate::remote_shell::launch(cmd);
            }
            let sh = shell();
            if cfg!(windows) {
                // PowerShell has no `-ic`: run it, then stay. `&` finds npm's `.cmd` shims too.
                return vec![sh, "-NoExit".into(), "-Command".into(), crate::programs::powershell_line(&cmd)];
            }
            // macOS terminal sessions load login profiles (including Homebrew's PATH).
            unix_agent_argv(sh, cmd, cfg!(target_os = "macos"))
        })
    }

    /// A space with its first tab and terminal, shown at once.
    /// `machine`: another machine (`hc machine ls`); its space starts in that machine's home.
    fn new_space(&mut self, name: &str, folder: Option<String>, ctx_name: Option<&str>, argv: Option<Vec<String>>, task: Option<&str>, machine: Option<String>, window: &Window) -> Result<Value, String> {
        let follow = folder.is_none();
        let on = machine.as_deref();
        // The focused pane's folder only means something on its own machine.
        let here = || self.model.focused_pane().and_then(|p| self.model.address_of(&p))
            .filter(|a| a.split_once('/').map(|(m, _)| m) == on)
            .and_then(|a| self.cwds.get(&a).cloned());
        let folder = folder.or_else(here).unwrap_or_else(|| if on.is_some() { String::new() } else { home() });
        let (space, ctx, pane) = (self.model.fresh_id_on("sp", on), self.model.fresh_id_on("tb", on), self.model.fresh_id_on("p", on));
        let size = self.initial_size(&Tab::new(&ctx, "", &pane, "").root, &pane, window);
        let (addr, folder) = self.spawn(Self::agent_argv(argv.clone(), task, on).or(argv), folder, follow, &space, &ctx, on, size, window)?;
        let ctx_name = ctx_name.map(str::trim).unwrap_or_default();
        self.model.add_space(&space, name.trim(), &folder, Tab::new(&ctx, ctx_name, &pane, &addr));
        if let Some(p) = self.model.space_mut(&space) {
            p.machine = machine;
        }
        Ok(json!({ "space": space, "tab": ctx, "terminal": addr }))
    }

    /// A tab in a space (default: the shown one), shown at once.
    fn new_tab(&mut self, space: Option<String>, name: &str, folder: Option<String>, argv: Option<Vec<String>>, task: Option<&str>, window: &Window) -> Result<Value, String> {
        let space = space.or_else(|| self.model.active_space().map(|p| p.id.clone())).ok_or("no space yet: make one first (space.new)")?;
        if self.model.space(&space).is_none() {
            return Err(format!("no space {space:?}"));
        }
        let follow = folder.is_none();
        let folder = folder.unwrap_or_else(|| self.folder_for(&space));
        let on = self.machine_of(&space);
        let (ctx, pane) = (self.model.fresh_id_on("tb", on.as_deref()), self.model.fresh_id_on("p", on.as_deref()));
        let size = self.initial_size(&Tab::new(&ctx, "", &pane, "").root, &pane, window);
        let (addr, _) = self.spawn(Self::agent_argv(argv.clone(), task, on.as_deref()).or(argv), folder, follow, &space, &ctx, on.as_deref(), size, window)?;
        self.model.add_tab(&space, Tab::new(&ctx, name.trim(), &pane, &addr));
        self.model.active = Some(space.clone());
        Ok(json!({ "space": space, "tab": ctx, "terminal": addr }))
    }

    /// A tab whose only pane is a mod (a file, a board), shown at once.
    fn new_mod_tab(&mut self, space: Option<String>, name: &str, m: String, props: Value) -> Result<Value, String> {
        let space = space.or_else(|| self.model.active_space().map(|p| p.id.clone())).ok_or("no space yet: make one first (space.new)")?;
        if self.model.space(&space).is_none() {
            return Err(format!("no space {space:?}"));
        }
        if !self.folders.contains_key(&m) {
            return Err(format!("no mod {m:?} in mods/"));
        }
        let machine = self.machine_of(&space);
        self.require_document(machine.as_deref())?;
        let (ctx, pane) = (self.model.fresh_id_on("tb", machine.as_deref()), self.model.fresh_id_on("p", machine.as_deref()));
        self.model.add_tab(&space, Tab::with_pane(&ctx, name.trim(), Node::block(pane.clone(), Content::Mod { name: m, props })));
        self.model.active = Some(space.clone());
        Ok(json!({ "space": space, "tab": ctx, "pane": pane }))
    }

    fn split(&mut self, pane: Option<String>, axis: Axis, argv: Option<Vec<String>>, folder: Option<String>, window: &Window) -> Result<Value, String> {
        let target = pane.or_else(|| self.model.focused_pane()).ok_or("no pane to split")?;
        let (Some(p), Some(c)) = (self.model.space_of(&target), self.model.tab_of(&target)) else { return Err(format!("no pane {target:?}")) };
        let (space, ctx, fallback) = (p.id.clone(), c.id.clone(), p.folder.clone());
        let target_addr = self.model.address_of(&target);
        let follow = folder.is_none();
        let cwd = folder.unwrap_or_else(|| target_addr.as_ref().and_then(|a| self.cwds.get(a).cloned()).unwrap_or(fallback));
        // A split stays on the machine of the pane it splits (else its space's).
        let on = target_addr.and_then(|a| a.split_once('/').map(|(m, _)| m.to_owned())).or_else(|| self.machine_of(&space));
        let new = self.model.fresh_id_on("p", on.as_deref());
        let Some(root) = self.model.split_tree(&target, axis, Node::block(new.clone(), Content::Terminal { terminal: String::new() })) else { return Err(format!("cannot split {target:?}")) };
        let size = self.model.active_tab().filter(|tab| tab.id == ctx)
            .and_then(|_| self.initial_size(&root, &new, window));
        let (addr, _) = self.spawn(argv, cwd, follow, &space, &ctx, on.as_deref(), size, window)?;
        self.model.split(&target, axis, &new, &addr);
        Ok(json!({ "pane": new, "terminal": addr }))
    }

    /// A mod as a pane beside the target (a viewer next to the shell).
    fn split_mod(&mut self, pane: Option<String>, axis: Axis, name: String, props: Value) -> Result<Value, String> {
        let target = pane.or_else(|| self.model.focused_pane()).ok_or("no pane to split")?;
        if self.model.tab_of(&target).is_none() {
            return Err(format!("no pane {target:?}"));
        }
        if !self.folders.contains_key(&name) {
            return Err(format!("no mod {name:?} in mods/"));
        }
        let machine = self.model.space_of(&target).and_then(|s| s.machine.clone());
        self.require_document(machine.as_deref())?;
        let new = self.model.fresh_id_on("p", machine.as_deref());
        self.model.split_with(&target, axis, Node::block(new.clone(), Content::Mod { name, props }));
        Ok(json!({ "pane": new }))
    }

    /// Ask only whether a missing terminal is closed. Core owns restarting it.
    fn check_closed(&mut self, missing: Vec<(String, String)>, window: &mut Window, cx: &mut Context<Self>) {
        self.checking_closed = true;
        let core = self.core.clone();
        let generation = self.core_generation;
        cx.spawn_in(window, async move |this, cx| {
            let checked = cx.background_executor().spawn(async move {
                missing.into_iter().map(|(pane, addr)| {
                    let result = core.call("terminal.get", json!({"terminal":addr}));
                    (pane, addr, result)
                }).collect::<Vec<_>>()
            }).await;
            let _ = this.update_in(cx, |this, window, cx| this.checked_closed(generation, checked, window, cx));
        }).detach();
    }

    fn checked_closed(&mut self, generation: u64, checked: Vec<(String, String, anyhow::Result<Value>)>, window: &mut Window, cx: &mut Context<Self>) {
        self.checking_closed = false;
        if generation != self.core_generation { return; }
        let mut changed = false;
        for (pane, addr, result) in checked {
            if self.chrome.find(&pane).and_then(Node::terminal) == Some(addr.as_str()) {
                // A failed lookup is not evidence the program exited.
                if result.as_ref().is_ok_and(|v| v["state"] == "finished") {
                    self.chrome.remove_slot(&pane);
                    changed = true;
                }
            } else if self.model.address_of(&pane).as_deref() == Some(&addr) {
                let closed = match result {
                    Ok(v) => matches!(v["reason"].as_str(), Some("exited" | "killed")),
                    Err(e) => crate::hc::error_kind(&e) == Some("not_found"),
                };
                if closed {
                    self.notify_launch_failure(&addr, window, cx);
                    self.model.remove_pane(&pane);
                    changed = true;
                }
            }
        }
        if changed { self.sync(window, cx); }
    }

    fn just_born(&self, addr: &str) -> bool {
        let mut born = self.born.borrow_mut();
        // Keep launch facts until hidden panes have passed through `check_closed`.
        born.retain(|_, t| t.elapsed() < Duration::from_secs(30));
        born.get(addr).is_some_and(|t| t.elapsed() < Duration::from_secs(5))
    }

    /// Gives a running terminal without a pane (a sub-agent, say) one in the
    /// shown space and focuses it; already in a pane: focuses that.
    fn keep_terminal(&mut self, addr: &str, place: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<Value, String> {
        if let Some(p) = self.model.pane_of(addr) {
            self.model.focus_pane(&p);
            return Ok(json!({ "pane": p }));
        }
        if !self.live.contains_key(addr) { return Err(format!("no running terminal {addr:?}")); }
        if self.changing_terminals.contains(addr) { return Err("This terminal already has a pane change in progress.".into()); }
        let space = self.model.active_space().map(|p| p.id.clone()).ok_or("no space to keep it in")?;
        let machine = self.machine_of(&space);
        if addr.split_once('/').map(|(m, _)| m) != machine.as_deref() {
            return Err("A terminal's pane must stay on its own machine.".into());
        }
        self.require_document(machine.as_deref())?;
        let target = matches!(place, "right" | "down").then(|| self.model.focused_pane()).flatten();
        let ctx = target.as_deref().and_then(|p| self.model.tab_of(p)).map(|t| t.id.clone())
            .unwrap_or_else(|| self.model.fresh_id_on("tb", machine.as_deref()));
        let pane = self.model.fresh_id_on("p", machine.as_deref());
        let name = self.live.get(addr).and_then(|r| r["name"].as_str()).filter(|s| !s.is_empty()).map(str::to_owned)
            .or_else(|| self.model.agents.get(addr).and_then(|a| a.app.as_deref()).map(crate::programs::name)).unwrap_or_else(|| addr.to_owned());
        let (address, place) = (addr.to_owned(), place.to_owned());
        let reply = json!({"pending":true,"terminal":addr,"pane":pane,"tab":ctx});
        self.start_terminal_change(address, TerminalChange::Keep {
            space, tab: ctx, pane, name, target, axis: if place == "right" { Axis::Row } else { Axis::Column },
        }, window, cx);
        Ok(reply)
    }

    /// Tells the core the space a moved tab's terminals live in now
    /// (`herdr.space`; the tab keeps its id), so one ever found without a
    /// pane is put back there, not in a space that's gone.
    fn label_space(&self, tab: &str, cx: &mut Context<Self>) {
        let Some(space) = self.model.space_of(tab) else { return };
        let label = crate::mirror::local(&space.id, space.machine.as_deref()).to_owned();
        for addr in space.tab(tab).map(Tab::addresses).unwrap_or_default() {
            let core = self.core.clone();
            let params = json!({ "terminal": addr, "labels": { "herdr.space": label } });
            cx.background_executor().spawn(async move {
                if let Err(e) = core.call("terminal.label", params) { tracing::warn!("space label for {addr}: {e:#}"); }
            }).detach();
        }
    }

    /// Parking changes labels/keep in the background; only a successful reply removes the pane.
    fn park_terminal(&mut self, addr: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<Value, String> {
        self.require_document(addr.split_once('/').map(|(m, _)| m))?;
        if self.changing_terminals.contains(addr) { return Err("This terminal already has a pane change in progress.".into()); }
        self.start_terminal_change(addr.to_owned(), TerminalChange::Park, window, cx);
        Ok(json!({"pending":true,"terminal":addr}))
    }

    fn start_terminal_change(&mut self, address: String, change: TerminalChange, window: &mut Window, cx: &mut Context<Self>) {
        let machine = address.split_once('/').map(|(m, _)| m);
        let (keep, labels) = match &change {
            TerminalChange::Keep { space, tab, .. } => (true, json!({
                "herdr.space":crate::mirror::local(space, machine), "herdr.tab":crate::mirror::local(tab, machine),
            })),
            TerminalChange::Park => (false, json!({"herdr.space":"","herdr.tab":""})),
        };
        // Kept, a popup is a popup no more: parked later, it stays (`orphan_popups`).
        // Only once the keep took, so a failed one can still be cleaned up.
        let was_popup = keep && self.live.get(&address).and_then(|r| r.pointer(&format!("/labels/{POPUP_LABEL}"))).is_some();
        let (tx, rx) = std::sync::mpsc::channel();
        self.changing_terminals.insert(address.clone());
        self.pending_terminals.insert(address.clone(), PendingTerminalChange { change, reply: rx });
        let core = self.core.clone();
        let addr = address.clone();
        // The drain must not need the foreground continuation to start this work.
        let job = cx.background_executor().spawn(async move {
            let result = (|| {
                core.call("terminal.label", json!({"terminal":addr,"labels":labels}))?;
                core.call("terminal.keep", json!({"terminal":addr,"keep":keep}))?;
                // Kept either way: a label left behind only matters if it's parked later.
                if was_popup
                    && let Err(e) = core.call(
                        "terminal.label",
                        json!({"terminal":addr,"labels":{ POPUP_LABEL: "" }}),
                    )
                {
                    tracing::warn!("popup label for kept {addr}: {e:#}");
                }
                anyhow::Ok(())
            })().map_err(|e| format!("{e:#}"));
            let _ = tx.send(result);
        });
        cx.spawn_in(window, async move |this, cx| {
            job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.draining { return; }
                let Some(pending) = this.pending_terminals.remove(&address) else { return };
                let result = pending.reply.try_recv().unwrap_or_else(|e| Err(format!("terminal change reply missing: {e}")));
                this.apply_terminal_change(address, pending.change, result, window, cx);
            });
        }).detach();
    }

    fn apply_terminal_change(&mut self, address: String, change: TerminalChange, result: Result<(), String>, window: &mut Window, cx: &mut Context<Self>) {
        self.changing_terminals.remove(&address);
        let kept_popup = self.keeping_popups.remove(&address);
        if let Err(error) = result {
            tracing::error!("terminal change for {address} failed: {error}");
            if !self.draining { self.layout_notice("Could not change terminal pane", &error, window, cx); }
            self.reshow_popup(kept_popup, window, cx);
            return;
        }
        match change {
            TerminalChange::Keep { space, tab, pane, name, target, axis } => {
                if !self.live.contains_key(&address) { return; }
                // Its tab may have moved to another space meanwhile (tab.move_to_space).
                let space = self.model.space_of(&tab).map_or(space, |s| s.id.clone());
                if self.model.space(&space).is_none() {
                    // A popup kept into a space that closed meanwhile goes with
                    // the space: parked, nothing would ever show it again.
                    if kept_popup.is_some() && !self.held_elsewhere(&address) {
                        let rm = !self.waits.contains_key(&address);
                        self.end_terminal(address, rm, cx);
                    } else if let Err(error) = self.park_terminal(&address, window, cx) {
                        tracing::error!("cannot undo pane placement into closed space: {error}");
                    }
                    return;
                }
                if self.model.pane_of(&address).is_none() {
                    let destination = target.filter(|p| self.model.tab_of(p).is_some_and(|t| t.id == tab))
                        .or_else(|| self.model.tab(&tab).and_then(|t| t.panes().first().map(|p| p.id.clone())));
                    if let Some(target) = destination {
                        self.model.split(&target, axis, &pane, &address);
                    } else {
                        self.model.add_tab(&space, Tab::new(&tab, &name, &pane, &address));
                    }
                }
                if let Some(pane) = self.model.pane_of(&address) { self.model.focus_pane(&pane); }
            }
            TerminalChange::Park => {
                if let Some(pane) = self.model.pane_of(&address) { self.model.remove_pane(&pane); }
            }
        }
        if !self.draining {
            self.sync(window, cx);
            self.flush_documents(false, window, cx);
        }
    }

    /// Kills terminals and their sub-agents, all the way down. Core keeps a
    /// sub-agent running when its parent dies; closing one here means done with
    /// its whole tree. Kept sub-agents (given a tab) are the person's: spared.
    fn kill(&mut self, addresses: &[String], cx: &mut Context<Self>) {
        let mut doomed: Vec<String> = addresses.to_vec();
        let mut i = 0;
        while i < doomed.len() {
            let parent = doomed[i].clone();
            for (addr, rec) in &self.live {
                let child = rec.get("parent").and_then(Value::as_str) == Some(parent.as_str());
                let kept = rec.get("labels").and_then(|l| l.get("herdr.space")).and_then(Value::as_str).is_some_and(|w| !w.is_empty());
                if child && !kept && !doomed.contains(addr) {
                    doomed.push(addr.clone());
                }
            }
            i += 1;
        }
        self.closing_terminals.extend(doomed.iter().cloned().map(|a| (a, false)));
        // Off the UI thread: a kill waits for the program to go, and for slow machines.
        let core = self.core.clone();
        cx.spawn(async move |this, cx| {
            for address in doomed {
                let core = core.clone();
                let addr = address.clone();
                let result = cx.background_executor().spawn(async move {
                    core.call("terminal.kill", json!({ "terminal": addr }))
                }).await;
                let _ = this.update(cx, |this, _| {
                    if result.is_ok() && this.live.contains_key(&address) {
                        // The live snapshot may still predate the kill's reply.
                        this.closing_terminals.insert(address, true);
                    } else {
                        this.closing_terminals.remove(&address);
                    }
                });
            }
        }).detach();
    }

    // ---- commands ----------------------------------------------------------

    /// Runs a keys.json action string: `name`, `name:value` (the value fills
    /// the command's first argument), `send:<text>`, or `<mod>.<command>`.
    fn run_action(&mut self, action: &str, window: &mut Window, cx: &mut Context<Self>) {
        // `send:` is only meaningful in a terminal, which handles it before it gets here.
        if !action.starts_with("send:") {
            self.run_key(action, Value::Null, window, cx);
        }
    }

    fn run_key(&mut self, name: &str, args: Value, window: &mut Window, cx: &mut Context<Self>) {
        // Close the dialog holding the keyboard, not the terminal behind it.
        let implicit_close = match crate::keys::current(name).as_str() {
            "tab.close" => Self::arg(&args, "id").is_none(),
            "pane.close" => Self::arg(&args, "pane").is_none(),
            _ => false,
        };
        if implicit_close && self.overlays.iter().any(|o| o.kind.modal_keys())
        {
            self.run("overlay.close", json!({}), window, cx);
        } else {
            self.run(name, args, window, cx);
        }
    }

    fn argv(args: &Value) -> Option<Vec<String>> {
        args.get("argv").and_then(|v| serde_json::from_value(v.clone()).ok())
    }

    fn arg<'a>(args: &'a Value, k: &str) -> Option<&'a str> {
        args.get(k).and_then(Value::as_str)
    }

    fn owned(args: &Value, k: &str) -> Option<String> {
        Self::arg(args, k).map(str::to_owned)
    }

    fn answer(&mut self, r: Result<Value, String>) {
        if let Err(e) = &r {
            tracing::warn!("{e}");
        }
        self.reply = Some(r.unwrap_or_else(|e| json!({ "error": e })));
    }

    fn fail(&mut self, message: impl Into<String>) {
        self.answer(Err(message.into()));
    }

    /// Runs a command for the person (keys, clicks, menus, mods): a missing
    /// name is asked for.
    fn run(&mut self, name: &str, args: Value, window: &mut Window, cx: &mut Context<Self>) {
        // A click or key has nobody to read the reply: show why it failed.
        if let Some(error) = self.run_as(name, args, true, window, cx).get("error").and_then(Value::as_str) {
            self.command_error(name, error, window, cx);
        }
    }

    fn command_error(&mut self, name: &str, error: &str, window: &mut Window, cx: &mut Context<Self>) {
        let spec = json!({"kind":"toast", "notification":{"title":format!("{name} failed"),"body":error,"level":"error"}});
        self.open_overlay(mods::new_overlay_id(), spec, window, cx);
    }

    /// Runs a command and says what it made or why not. Scripts and agents
    /// (`person: false`) never get a dialog: a missing argument is an error.
    fn run_as(&mut self, name: &str, mut args: Value, person: bool, window: &mut Window, cx: &mut Context<Self>) -> Value {
        // A mod someone edited under 0.1.0 still says project.* / context.*.
        let current = crate::keys::current(name);
        let name = current.as_str();
        // `name:value`, as in keys.json.
        if let Some((n, v)) = name.split_once(':')
            && let Some(c) = crate::commands::find(n)
        {
            if let Err(error) = crate::commands::validate_args(c, &args) {
                return json!({ "error": error });
            }
            if c.args.is_empty() {
                return json!({ "error": format!("{n}: unexpected shorthand argument; valid arguments: none") });
            }
            let mut short = crate::commands::shorthand(c, v);
            if let (Some(s), Some(extra)) = (short.as_object_mut(), args.as_object()) {
                s.extend(extra.clone());
            }
            args = short;
            return self.run_as(n, args, person, window, cx);
        }
        let Some(command) = crate::commands::find(name) else {
            // `<mod>.<command>` goes to that mod's nextCommand().
            if let Some((m, c)) = name.split_once('.')
                && mods::send_command(m, c)
            {
                return json!({ "ok": true });
            }
            tracing::warn!("unknown command {name:?}");
            return json!({ "error": format!("unknown command {name:?}: see `herdr-app commands`; a mod's are <mod>.<command> and need the mod loaded") });
        };
        if let Err(error) = crate::commands::validate_args(command, &args) {
            return json!({ "error": error });
        }
        // pane.go from a mod block starts at the block, not at the terminal block focused before it.
        if terminal_block(&self.chrome, self.focused_pane.as_deref()).is_some()
            && needs_model_pane(name, &args)
            && !(name == "pane.go" && self.focused_block.is_some())
        {
            return json!({ "error": "a terminal layout block is focused, not a pane; focus a pane or pass an explicit pane id" });
        }
        if person && name.ends_with(".rename") && args.get("name").is_none_or(Value::is_null) {
            self.ask_name(name, args, window, cx);
            return json!({ "asked": "name" });
        }
        if let Some(arg) = crate::commands::missing(command, &args) {
            return json!({ "error": format!("{name} needs {arg}") });
        }
        if person {
            crate::telemetry::command(command.name);
        }
        if name == "space.new" {
            let machine = new_space_machine(&args, person, &self.model);
            if !args.is_object() { args = json!({}); }
            args["machine"] = json!(machine);
        }
        if matches!(name, "space.new" | "space.close" | "space.rename" | "space.move" | "tab.new" | "tab.close" | "tab.rename" | "tab.move" | "tab.move_to_space" | "tab.move_left" | "tab.move_right" | "tab.set_icon" | "pane.split" | "pane.close" | "pane.rename" | "pane.set_icon" | "pane.swap" | "pane.arrange" | "terminal.keep" | "terminal.park")
            || pane_floats::COMMANDS.contains(&name)
        {
            let node = ["id", "space", "pane"].into_iter().find_map(|k| Self::arg(&args, k))
                .or_else(|| if name == "pane.arrange" { Self::arg(&args, "tab") } else { None });
            let target = node.and_then(|id| self.model.space(id).or_else(|| self.model.space_of(id))).or_else(|| self.model.active_space());
            let machine = if name == "space.new" { Self::arg(&args, "machine") }
                else if name == "terminal.park" { Self::arg(&args, "terminal").and_then(|a| a.split_once('/').map(|(m, _)| m)) }
                else { target.and_then(|s| s.machine.as_deref()) };
            if let Err(error) = self.require_document(machine) { return json!({"error":error}); }
        }
        if name == "layout.resize"
            && let Some(space) = Self::arg(&args, "id").and_then(|id| self.model.space_of(id))
            && let Err(error) = self.require_document(space.machine.as_deref())
        { return json!({"error":error}); }
        self.reply = None;
        self.dispatch(name, &args, person, window, cx);
        self.flush_documents(false, window, cx);
        self.reply.take().unwrap_or_else(|| json!({ "ok": true }))
    }

    /// Asks the person for a new name (space.rename, tab.rename, pane.rename), then runs it with it.
    /// The prompt starts with the name someone gave, empty for an automatic one,
    /// except a pane's `pane_auto_names` name, which Enter keeps as its own.
    fn ask_name(&mut self, name: &str, mut args: Value, window: &mut Window, cx: &mut Context<Self>) {
        if !args.is_object() {
            args = json!({});
        }
        let (title, hint, current) = match name {
            "space.rename" => {
                let id = Self::owned(&args, "id").or_else(|| self.model.active.clone());
                let current = id.as_deref().and_then(|i| self.model.space(i)).map(|p| p.name.clone());
                args["id"] = json!(id);
                ("rename space", "empty: named after its folder", current)
            }
            "pane.rename" => {
                let pane = Self::owned(&args, "pane").or_else(|| self.model.focused_pane());
                let given = pane.as_deref().and_then(|p| self.model.pane_name(p));
                let auto = pane.as_deref().and_then(|p| self.auto_names.get(p));
                let hint = if given.is_none() && auto.is_some() { "empty: its automatic name" } else { "empty: its program's name" };
                let current = given.or(auto.map(String::as_str)).map(str::to_owned);
                args["pane"] = json!(pane);
                ("rename pane", hint, current)
            }
            _ => {
                let id = Self::owned(&args, "id").or_else(|| self.model.active_tab().map(|c| c.id.clone()));
                let current = id.as_deref().and_then(|i| self.model.tab(i)).map(|c| c.name.clone());
                args["id"] = json!(id);
                ("rename tab", "empty: its number", current)
            }
        };
        // Once: a second rename press closes the open prompt instead of stacking another.
        if self.overlays.iter().any(|o| o.id == "rename") {
            self.close_overlay("rename", window, cx);
            return;
        }
        let spec = json!({ "mod": "prompt", "kind": "dialog", "width": 380,
            "props": { "title": title, "value": current.as_deref().unwrap_or(""), "hint": hint, "command": { "name": name, "args": args } } });
        self.open_overlay("rename".into(), spec, window, cx);
    }

    fn dispatch(&mut self, name: &str, a: &Value, person: bool, window: &mut Window, cx: &mut Context<Self>) {
        let active_ctx = self.model.active_tab().map(|c| c.id.clone());
        match name {
            // ---- spaces
            "space.new" => {
                let name = Self::arg(a, "name").unwrap_or_default();
                let r = self.new_space(name, Self::owned(a, "folder"), Self::arg(a, "tab"), Self::argv(a), Self::arg(a, "task"), Self::owned(a, "machine").filter(|m| !m.is_empty()), window);
                self.answer(r);
            }
            "space.close" => {
                let Some(id) = Self::owned(a, "id").or_else(|| self.model.active.clone()) else { return self.fail("no space to close") };
                let Some(p) = self.model.space(&id) else { return self.fail(format!("no space {id:?}")) };
                if a.get("confirmed").and_then(Value::as_bool) != Some(true) {
                    if !person {
                        return self.fail("space.close ends its terminals: pass confirmed: true");
                    }
                    let panes: usize = p.tabs.iter().map(|c| c.panes().len()).sum();
                    let detail = format!("{} \u{2014} {} tabs, {} terminals", self.model.space_name(p, &home()), p.tabs.len(), panes);
                    self.confirm("Close space?", &detail, "space.close", json!({ "id": id, "confirmed": true }), window, cx);
                    return;
                }
                let addrs = self.model.remove_space(&id);
                self.kill(&addrs, cx);
            }
            "space.rename" => {
                let Some(id) = Self::owned(a, "id").or_else(|| self.model.active.clone()) else { return self.fail("no space to rename") };
                let Some(name) = Self::arg(a, "name") else { return self.fail("space.rename needs name (empty: automatic)") };
                let Some(p) = self.model.space_mut(&id) else { return self.fail(format!("no space {id:?}")) };
                p.name = name.trim().to_owned();
            }
            "space.move" => {
                let (Some(id), Some(i)) = (Self::arg(a, "id"), a.get("index").and_then(Value::as_u64)) else { return self.fail("space.move needs id and index") };
                if !self.model.move_space(id, i as usize) { return self.fail(format!("no space {id:?}")); }
            }
            "space.select" => {
                let id = Self::owned(a, "id").or_else(|| {
                    let i = a.get("index").and_then(Value::as_u64)? as usize;
                    self.model.spaces.get(i.checked_sub(1)?).map(|p| p.id.clone())
                });
                match id.filter(|id| self.model.space(id).is_some()) {
                    Some(id) => self.model.active = Some(id),
                    None => return self.fail("no such space"),
                }
            }
            "space.next" | "space.prev" => {
                let n = self.model.spaces.len();
                if let Some(i) = self.model.active_index().filter(|_| n > 0) {
                    let j = if name == "space.next" { (i + 1) % n } else { (i + n - 1) % n };
                    self.model.active = Some(self.model.spaces[j].id.clone());
                }
            }
            // ---- tabs
            "tab.new" => {
                let name = Self::arg(a, "name").unwrap_or_default();
                let r = match Self::owned(a, "mod") {
                    Some(m) => self.new_mod_tab(Self::owned(a, "space"), name, m, a.get("props").cloned().unwrap_or(Value::Null)),
                    // A person's new tab with no space open starts one, as space.new does.
                    None if person && a.get("space").is_none() && self.model.active_space().is_none() => {
                        self.new_space("", Self::owned(a, "folder"), Some(name), Self::argv(a), Self::arg(a, "task"), None, window)
                    }
                    None => self.new_tab(Self::owned(a, "space"), name, Self::owned(a, "folder"), Self::argv(a), Self::arg(a, "task"), window),
                };
                self.answer(r);
            }
            "tab.close" => {
                let Some(id) = Self::owned(a, "id").or(active_ctx) else { return self.fail("no tab to close") };
                let Some(p) = self.model.space_of(&id) else { return self.fail(format!("no tab {id:?}")) };
                // The last tab takes its space with it: ask like closing the space.
                if p.tabs.len() == 1 {
                    let mut args = json!({ "id": p.id });
                    args["confirmed"] = a.get("confirmed").cloned().unwrap_or(Value::Null);
                    return self.dispatch("space.close", &args, person, window, cx);
                }
                let (addrs, _) = self.model.remove_tab(&id);
                self.kill(&addrs, cx);
            }
            "tab.rename" => {
                let Some(id) = Self::owned(a, "id").or(active_ctx) else { return self.fail("no tab to rename") };
                let Some(name) = Self::arg(a, "name") else { return self.fail("tab.rename needs name (empty: its number)") };
                let Some(c) = self.model.tab_mut(&id) else { return self.fail(format!("no tab {id:?}")) };
                c.name = name.trim().to_owned();
            }
            "tab.move" => {
                let (Some(id), Some(i)) = (Self::arg(a, "id"), a.get("index").and_then(Value::as_u64)) else { return self.fail("tab.move needs id and index") };
                if !self.model.move_tab(id, i as usize) { return self.fail(format!("no tab {id:?}")); }
            }
            "tab.move_to_space" => {
                let Some(id) = Self::owned(a, "id").or(active_ctx) else { return self.fail("no tab to move") };
                let Some(space) = Self::arg(a, "space") else { return self.fail("tab.move_to_space needs space") };
                let index = match a.get("index") {
                    None | Some(Value::Null) => None,
                    Some(i) => match i.as_u64() {
                        Some(i) => Some(i as usize),
                        None => return self.fail(format!("tab.move_to_space index is a position (1 = first), not {i}")),
                    },
                };
                if let Err(error) = self.model.move_tab_to_space(&id, space, index) { return self.fail(error); }
                self.label_space(&id, cx);
            }
            "tab.move_left" | "tab.move_right" => {
                let Some(id) = Self::owned(a, "id").or(active_ctx) else { return self.fail("no tab to move") };
                let step = if name == "tab.move_left" { -1 } else { 1 };
                if self.model.step_tab(&id, step).is_none() { return self.fail(format!("no tab {id:?}")); }
            }
            "tab.settle" | "tab.unsettle" => {
                let Some(id) = Self::owned(a, "id").or(active_ctx) else { return self.fail("no tab") };
                let found = if name == "tab.settle" { self.model.settle(&id, model::now_ms()) } else { self.model.tab(&id).is_some() && { self.model.unsettle(&id); true } };
                if !found {
                    return self.fail(format!("no tab {id:?}"));
                }
            }
            "tab.select" => {
                // By position among the tabs the lists show (settled ones are put away).
                let id = Self::owned(a, "id").or_else(|| {
                    let i = a.get("index").and_then(Value::as_u64)? as usize;
                    self.model.active_space()?.tabs.iter().filter(|c| c.settled.is_none()).nth(i.checked_sub(1)?).map(|c| c.id.clone())
                });
                if !id.is_some_and(|id| self.model.select_tab(&id)) {
                    return self.fail("no such tab");
                }
            }
            "tab.next" | "tab.prev" => {
                if let Some(p) = self.model.active_space()
                    && let Some(i) = active_ctx.as_ref().and_then(|c| p.tabs.iter().position(|x| x.id == *c))
                {
                    let n = p.tabs.len();
                    let step = |k: usize| if name == "tab.next" { (i + k) % n } else { (i + n * k - k) % n };
                    if let Some(id) = (1..n).map(|k| &p.tabs[step(k)]).find(|c| c.settled.is_none()).map(|c| c.id.clone()) {
                        self.model.select_tab(&id);
                    }
                }
            }
            // ---- panes
            "pane.split" => {
                let axis = match Self::arg(a, "direction") {
                    Some("right") => Axis::Row,
                    Some("down") => Axis::Column,
                    other => return self.fail(format!("pane.split direction is \"right\" or \"down\", not {other:?}")),
                };
                let r = match Self::owned(a, "mod") {
                    Some(name) => self.split_mod(Self::owned(a, "pane"), axis, name, a.get("props").cloned().unwrap_or(Value::Null)),
                    None => self.split(Self::owned(a, "pane"), axis, Self::argv(a), None, window),
                };
                self.answer(r);
            }
            "pane.close" => {
                let Some(pane) = Self::owned(a, "pane").or_else(|| self.model.focused_pane()) else { return self.fail("no pane to close") };
                let Some(c) = self.model.tab_of(&pane) else { return self.fail(format!("no pane {pane:?}")) };
                if c.find(&pane).is_some_and(|n| !n.closable()) {
                    return;
                }
                if c.panes().len() == 1 {
                    let id = c.id.clone();
                    return self.dispatch("tab.close", &json!({ "id": id }), person, window, cx);
                }
                let addr = self.model.address_of(&pane);
                self.model.remove_pane(&pane);
                self.kill(&addr.into_iter().collect::<Vec<_>>(), cx);
            }
            "pane.rename" => {
                let Some(pane) = Self::owned(a, "pane").or_else(|| self.model.focused_pane()) else { return self.fail("no pane to rename") };
                let Some(name) = Self::arg(a, "name") else { return self.fail("pane.rename needs name (empty: its program's)") };
                if !self.model.rename_pane(&pane, name) { return self.fail(format!("no pane {pane:?}")) }
            }
            "pane.set_icon" | "tab.set_icon" => {
                let target = if name == "pane.set_icon" { Self::owned(a, "pane").or_else(|| self.model.focused_pane()) } else { Self::owned(a, "id").or(active_ctx) };
                let Some(id) = target else { return self.fail(format!("{name}: no pane or tab")) };
                let icon = match crate::pane_icons::Choice::parse(a.get("icon").unwrap_or(&Value::Null)) {
                    Ok(icon) => icon,
                    Err(e) => return self.fail(format!("{name}: {e}")),
                };
                let found = if name == "pane.set_icon" { self.model.tab_of(&id).and_then(|c| c.pane(&id)).is_some() } else { self.model.tab(&id).is_some() };
                if !found || !self.model.set_icon(&id, icon) {
                    return self.fail(format!("no {} {id:?}", if name == "pane.set_icon" { "pane" } else { "tab" }));
                }
            }
            "pane.menu" => {
                let Some(pane) = Self::owned(a, "pane").or_else(|| self.model.focused_pane()) else { return self.fail("no pane") };
                if self.model.tab_of(&pane).is_none() && self.pane_term(&pane).is_none() {
                    return self.fail(format!("no pane {pane:?}"));
                }
                // What the pane shows: a terminal swapped in (`terminal.show`), else its own.
                let terminal = self.pane_term(&pane).map(|t| t.address.clone()).or_else(|| self.model.address_of(&pane));
                let cwd = terminal.as_deref().and_then(|t| self.terminal_cwd(t));
                let last_output = self.pane_term(&pane).is_some_and(|t| t.view.read(cx).has_last_output());
                self.open_menu("pane", json!({ "pane": pane, "terminal": terminal, "cwd": cwd, "last_output": last_output }), None, window, cx);
            }
            "pane.zoom" => {
                let pane = Self::owned(a, "pane").or_else(|| self.model.focused_pane());
                if let Some(pane) = pane
                    && let Some(ctx) = self.model.tab_of(&pane).map(|c| c.id.clone())
                    && let Some(c) = self.model.tab_mut(&ctx)
                    && c.panes().len() > 1
                {
                    if c.zoomed.as_deref() == Some(pane.as_str()) {
                        c.zoomed = None;
                    } else {
                        // The pane shown is the one the keys go to.
                        c.focus(&pane);
                        c.zoomed = Some(pane);
                        self.focused_block = None;
                        self.focused_pane = None;
                    }
                }
            }
            "pane.focus" => {
                if Self::arg(a, "pane").is_none()
                    && let Some(terminal) = Self::arg(a, "terminal")
                    && let Some(block) = self.chrome.visible_blocks().into_iter().find(|n| n.terminal() == Some(terminal))
                {
                    let id = block.id.clone();
                    self.focused_block = None;
                    self.focused_pane = Some(id.clone());
                    self.synced_model_pane = self.model.focused_pane();
                    if !self.overlays.iter().any(|o| o.kind.modal_keys()) && let Some(f) = self.pane_focus(&id, cx) {
                        f.focus(window, cx);
                    }
                    self.publish(cx);
                    cx.notify();
                    return;
                }
                let pane = Self::owned(a, "pane").or_else(|| Self::arg(a, "terminal").and_then(|t| self.model.pane_of(t)));
                let Some(p) = pane.filter(|p| self.model.tab_of(p).is_some()) else { return self.fail("no such pane") };
                self.focused_block = None;
                self.model.focus_pane_raising(&p, !self.hover_focus);
                self.focused_pane = None;
                self.agent = self.model.address_of(&p);
                self.swapped.remove(&p);
            }
            "pane.refocus" => {
                // The keyboard back to what has the focus, without changing it (pane.focus
                // would end a swapped-in sub-agent and reset the agent shown).
                self.restore_focus(window, cx);
                return;
            }
            "pane.go" => {
                let dir = Self::arg(a, "direction").unwrap_or_default();
                if !matches!(dir, "left" | "right" | "up" | "down") {
                    return self.fail(format!("{name} direction is left, right, up or down, not {dir:?}"));
                }
                return self.go(dir, window, cx);
            }
            "pane.swap" => {
                let by = match swap_by(a) {
                    Ok(by) => by,
                    Err(e) => return self.fail(e),
                };
                let explicit = Self::arg(a, "pane").is_some();
                let pane = Self::owned(a, "pane").or_else(|| self.model.focused_pane());
                let r = match (by, pane) {
                    (SwapBy::Direction(dir), Some(pane)) => self.swap_pane(&pane, explicit, dir),
                    (SwapBy::Direction(_), None) => Ok(()),
                    (SwapBy::Target(target), Some(pane)) => self.model.swap_panes(&pane, target),
                    (SwapBy::Target(_), None) => Err("no pane to swap".into()),
                };
                if let Err(e) = r {
                    return self.fail(e);
                }
            }
            "pane.arrange" => {
                use crate::arrange::Mode;
                let tab = Self::owned(a, "tab").or_else(|| self.model.active_tab().map(|t| t.id.clone()));
                let Some(tab) = tab.filter(|t| self.model.tab(t).is_some()) else { return self.fail("pane.arrange: no such tab") };
                let mode = match Self::arg(a, "mode").unwrap_or_default() {
                    "next" => Mode::next(self.model.tab(&tab).and_then(|t| t.arrange)),
                    "off" => {
                        self.model.tab_mut(&tab).unwrap().arrange = None;
                        return self.answer(Ok(json!({ "ok": true, "tab": tab, "mode": "off" })));
                    }
                    other => match Mode::parse(other) {
                        Some(mode) => mode,
                        None => return self.fail(format!("pane.arrange mode is tiled, bsp, master, columns, rows, next or off, not {other:?}")),
                    },
                };
                let r = self.model.arrange_tab(&tab, mode).map(|()| json!({ "ok": true, "tab": tab, "mode": mode.name() }));
                self.answer(r);
            }
            name if pane_floats::COMMANDS.contains(&name) => {
                if let Err(e) = self.float_command(name, a, window) {
                    return self.fail(e);
                }
            }
            // ---- terminals
            "terminal.open" => {
                let argv = Self::argv(a);
                let r = match Self::arg(a, "place").unwrap_or("tab") {
                    "right" => self.split(None, Axis::Row, argv, Self::owned(a, "folder"), window),
                    "down" => self.split(None, Axis::Column, argv, Self::owned(a, "folder"), window),
                    _ => {
                        let name = Self::owned(a, "name")
                            .or_else(|| argv.as_ref().and_then(|v| v.first()).map(|p| p.rsplit('/').next().unwrap_or(p).to_owned()))
                            .unwrap_or_else(|| "shell".into());
                        self.new_tab(None, &name, Self::owned(a, "folder"), argv, None, window)
                    }
                };
                self.answer(r);
            }
            "terminal.keep" => {
                let t = Self::arg(a, "terminal").unwrap_or_default();
                let r = self.keep_terminal(t, Self::arg(a, "place").unwrap_or("tab"), window, cx);
                if let Ok(reply) = &r {
                    // A popup kept as a tab is the person's now; one on its way
                    // comes back as a popup if the keep fails (`apply_terminal_change`).
                    if let Some(p) = self.popped.take_if(|p| p.address == t)
                        && reply["pending"] == Value::Bool(true)
                    {
                        self.keeping_popups.insert(p.address.clone(), p);
                    }
                    // Kept: the peek of it has done its job, closed as any close
                    // does (a popup hidden under it ends). A failed keep leaves it up.
                    if self.peeked.as_deref() == Some(t) {
                        self.close_overlay("terminal-peek", window, cx);
                    }
                }
                self.answer(r);
            }
            "terminal.park" => {
                let r = self.park_terminal(Self::arg(a, "terminal").unwrap_or_default(), window, cx);
                self.answer(r);
            }
            "terminal.right_click" => {
                let to = match Self::arg(a, "to") {
                    Some("program") => "program",
                    Some("menu") => "",
                    other => return self.fail(format!("terminal.right_click to is \"program\" or \"menu\", not {other:?}")),
                };
                let t = Self::owned(a, "terminal").or_else(|| self.focused_terminal(window, cx).map(|t| t.address.clone()));
                let Some(t) = t.filter(|t| self.live.contains_key(t)) else { return self.fail("no such running terminal") };
                // Off the UI thread: a terminal on a slow machine answers late.
                let core = self.core.clone();
                let params = json!({ "terminal": t, "labels": { "herdr.right-click": to } });
                cx.background_executor()
                    .spawn(async move {
                        if let Err(e) = core.call("terminal.label", params) {
                            tracing::warn!("right-click label: {e:#}");
                        }
                    })
                    .detach();
            }
            "terminal.pin" => {
                let Some(pinned) = a.get("pinned").and_then(Value::as_bool) else { return self.fail("terminal.pin pinned is true or false") };
                let t = Self::arg(a, "terminal").unwrap_or_default().to_owned();
                if !self.live.contains_key(&t) {
                    return self.fail(format!("no running terminal {t:?}"));
                }
                let core = self.core.clone();
                let params = json!({ "terminal": t, "labels": { "herdr.pinned": if pinned { "1" } else { "" } } });
                cx.background_executor()
                    .spawn(async move {
                        if let Err(e) = core.call("terminal.label", params) {
                            tracing::warn!("pin label: {e:#}");
                        }
                    })
                    .detach();
            }
            "terminal.kill" => {
                let t = Self::arg(a, "terminal").unwrap_or_default();
                if !self.live.contains_key(t) {
                    return self.fail(format!("no running terminal {t:?}"));
                }
                self.kill(&[t.to_owned()], cx);
            }
            "terminal.popup" => {
                let r = self.popup(a, window, cx);
                self.answer(r);
            }
            "terminal.peek" => {
                let t = Self::arg(a, "terminal").unwrap_or_default();
                if !self.live.contains_key(t) {
                    return self.fail(format!("no running terminal {t:?}"));
                }
                self.peek_terminal(t, window, cx);
            }
            "terminal.show" => {
                let pane = Self::owned(a, "pane").or_else(|| self.model.focused_pane());
                let target = Self::arg(a, "terminal").filter(|t| self.live.contains_key(*t)).map(str::to_owned);
                if let Some(p) = pane {
                    match target {
                        // One with a pane of its own is shown there.
                        Some(t) if let Some(own) = self.model.pane_of(&t) => {
                            self.swapped.remove(&p);
                            self.model.focus_pane(&own);
                        }
                        Some(t) => {
                            self.swapped.insert(p.clone(), t);
                            self.model.focus_pane(&p);
                        }
                        None => {
                            self.swapped.remove(&p);
                        }
                    }
                    self.focused_pane = None;
                }
            }
            "terminal.search" => {
                let Some(t) = self.focused_terminal(window, cx) else { return self.fail("no focused terminal to search") };
                t.view.update(cx, |view, cx| view.open_search(window, cx));
                return;
            }
            "terminal.copy" | "terminal.paste" | "terminal.paste_image" => {
                let view = match Self::arg(a, "terminal") {
                    Some(t) => match self.command_view(t, name == "terminal.copy", window, cx) {
                        Ok(view) => view,
                        Err(error) => return self.fail(error),
                    },
                    None => match self.focused_terminal(window, cx) {
                        Some(t) => t.view.clone(),
                        None => return,
                    },
                };
                // A command, not a key: copy and leave the selection.
                let copied = view.update(cx, |v, cx| match name {
                    "terminal.paste" => { v.paste(cx); false }
                    "terminal.paste_image" => { v.paste_image(cx); false }
                    _ => v.copy(cx),
                });
                if copied {
                    self.show_copied(window, cx);
                }
                return;
            }
            "terminal.copy_all" => {
                let t = Self::owned(a, "terminal").or_else(|| self.focused_terminal(window, cx).map(|t| t.address.clone()));
                let Some(t) = t.filter(|t| self.live.contains_key(t)) else { return self.fail("no such running terminal") };
                self.copy_all(t, window, cx);
            }
            "terminal.copy_path" => {
                let t = Self::owned(a, "terminal").or_else(|| self.focused_terminal(window, cx).map(|t| t.address.clone()));
                let Some(t) = t.filter(|t| self.live.contains_key(t)) else { return self.fail("no such running terminal") };
                let Some(cwd) = self.terminal_cwd(&t) else { return self.fail(format!("terminal {t:?}'s folder isn't known")) };
                cx.write_to_clipboard(ClipboardItem::new_string(cwd));
                self.show_copied_as(json!({ "title": "Copied path" }), window, cx);
            }
            "terminal.copy_last_output" => {
                let t = Self::owned(a, "terminal").or_else(|| self.focused_terminal(window, cx).map(|t| t.address.clone()));
                let Some(t) = t else { return self.fail("no such running terminal") };
                let shown = match self.shown_terminal_view(&t, window, cx) {
                    Ok(view) => view,
                    Err(error) => return self.fail(error),
                };
                // Each view of a terminal (its pane, a peek) has its own copy of the
                // screen, and one attached later lacks the marks before it: the one
                // seen first, else any other, as the menu read the clicked one.
                let others = self.terms.values().filter(|v| v.address == t).map(|v| &v.view);
                let Some(text) = std::iter::once(&shown).chain(others).find_map(|v| v.read(cx).last_output()) else {
                    return self.fail(format!("terminal {t:?} has no finished command's output: its shell marks no prompts (OSC 133), a full-screen program is running, or the command printed nothing"));
                };
                let lines = text.lines().count();
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                self.show_copied_as(json!({ "title": format!("Copied {} {}", thousands(lines), if lines == 1 { "line" } else { "lines" }) }), window, cx);
            }
            "terminal.scroll" => {
                if let Err(error) = self.scroll_terminal(a, window, cx) {
                    self.fail(error);
                }
            }
            "system.notify" => self.system_notify(a, window, cx),
            // ---- agents
            "agent.focus" => {
                let t = Self::arg(a, "terminal").unwrap_or_default().to_owned();
                if !self.live.contains_key(&t) {
                    return self.fail(format!("no running terminal {t:?}"));
                }
                self.focus_agent(&t);
            }
            "agent.next" | "agent.prev" => {
                let order = self.agent_order();
                if order.is_empty() {
                    return;
                }
                let current = self.shown_agent().map(|a| self.root_agent(&a));
                let n = order.len();
                let next = match current.and_then(|c| order.iter().position(|a| *a == c)) {
                    Some(i) if name == "agent.next" => (i + 1) % n,
                    Some(i) => (i + n - 1) % n,
                    None if name == "agent.next" => 0,
                    None => n - 1,
                };
                let addr = order[next].clone();
                self.focus_agent(&addr);
            }
            // ---- layout
            "layout.switch" => {
                self.switch_layout(Self::arg(a, "name"), window, cx);
                return;
            }
            "layout.clone" => {
                self.save();
                let name = Self::owned(a, "name").unwrap_or_else(|| self.layout_name.clone());
                let to = Self::owned(a, "as").unwrap_or_else(|| format!("{name}-copy"));
                if let Err(e) = clone_layout(&name, &to) {
                    return self.fail(format!("cannot clone layout: {e}"));
                }
                self.switch_layout(Some(&to), window, cx);
                return;
            }
            "mod.clone" => {
                let name = Self::arg(a, "name").unwrap_or_default();
                let to = Self::owned(a, "as").unwrap_or_else(|| format!("{name}-copy"));
                if let Err(e) = mods::clone_mod(name, &to) {
                    return self.fail(format!("cannot clone mod: {e}"));
                }
                self.folder_scans += 1;
                let (folders, actions_changed) = scan_folders(mods::scan());
                self.folders = folders;
                if actions_changed {
                    self.refresh_snapshot_mods(cx);
                }
            }
            "layout.set" if shipped_layouts().contains(&self.layout_name.as_str()) => {
                let n = match serde_json::from_value::<Node>(a.get("layout").cloned().unwrap_or_default()) {
                    Ok(n) => n,
                    Err(e) => return self.fail(format!("layout: {e}")),
                };
                if layout_sync::without_clicks(&self.chrome, &n) != self.chrome {
                    return self.fail("Only supported click/look fields can change on Herdr's own layouts: clone one for other changes");
                }
                let mut state = layout_sync::State::between(&self.file_chrome, &n);
                if n.look.theme == self.chrome.look.theme {
                    state.theme = self.picked_theme.clone();
                }
                self.chrome = self.file_chrome.clone();
                state.apply(&mut self.chrome);
                self.picked_theme = state.theme;
                self.apply_look(window, cx);
            }
            // A deliberate edit: the file becomes this tree, clicks start over.
            "layout.set" => match serde_json::from_value::<Node>(a.get("layout").cloned().unwrap_or_default()) {
                Ok(mut n) => {
                    fill_ids(&mut n, &mut self.model);
                    let text = layout_sync::text_of(&n);
                    if !write_atomic(&layout_path(&self.layout_name), &text) {
                        return self.fail(format!("cannot write layouts/{}.json", self.layout_name));
                    }
                    self.written_layout = text;
                    self.layout_file_ok = true;
                    self.file_chrome = n.clone();
                    self.chrome = n;
                    self.picked_theme = None;
                    self.write_state(&layout_sync::State::default());
                    self.apply_look(window, cx);
                }
                Err(e) => return self.fail(format!("layout: {e}")),
            },
            "layout.toggle" | "layout.show" | "layout.hide" => {
                let id = Self::arg(a, "id").unwrap_or_default();
                let Some(n) = self.chrome.find_mut(id) else { return self.fail(format!("no node {id:?} in the shown layout")) };
                n.hidden = match name {
                    "layout.toggle" => !n.hidden,
                    "layout.show" => false,
                    _ => true,
                };
            }
            "layout.resize" => {
                let id = Self::arg(a, "id").unwrap_or_default();
                let Some(n) = self.find_node_mut(id) else { return self.fail(format!("no node {id:?}")) };
                if let Some(px) = a.get("px").and_then(Value::as_f64) {
                    n.size = Some(Span::Px { px: px as f32 });
                } else if let Some(w) = a.get("weight").and_then(Value::as_f64) {
                    n.size = Some(Span::Weight { weight: w as f32 });
                }
            }
            "layout.spacing" => {
                // Clicks made over the shipped stand-in of a broken file are dropped on repair.
                if !self.layout_file_ok {
                    return self.fail(format!("layout.spacing: layouts/{}.json has errors: fix it first", self.layout_name));
                }
                let spacing = match serde_json::from_value::<layout_sync::Spacing>(if a.is_null() { json!({}) } else { a.clone() }) {
                    Ok(s) => s,
                    Err(e) => return self.fail(format!("layout.spacing: {e}")),
                };
                if let Err(e) = spacing.apply(&mut self.chrome, &self.file_chrome) {
                    return self.fail(format!("layout.spacing: {e}"));
                }
                // terminal_padding reaches the terminals through the composed settings;
                // gaps and padding are read from the tree as it draws.
                if spacing.changes_look() {
                    self.apply_look(window, cx);
                }
            }
            "layout.reset" => {
                let name = Self::owned(a, "name").unwrap_or_else(|| self.layout_name.clone());
                let Some((_, text)) = LAYOUTS.iter().find(|(n, _)| *n == name) else {
                    return self.fail(format!("{name:?} isn't a layout Herdr ships"));
                };
                // Theirs is kept under a name nothing else has (never over an older one).
                if layout_path(&name).exists() {
                    if let Err(e) = std::fs::rename(layout_path(&name), aside(&layout_path(&name), "old")) {
                        return self.fail(format!("cannot set layouts/{name}.json aside: {e}"));
                    }
                }
                if !write_atomic(&layout_path(&name), text) {
                    return self.fail(format!("cannot write layouts/{name}.json"));
                }
                let _ = std::fs::remove_file(state_path(&name));
                if name == self.layout_name {
                    self.load_layout(&name);
                    self.apply_look(window, cx);
                }
            }
            "layout.select" => {
                let (s, ch) = (Self::arg(a, "stack").unwrap_or_default(), Self::arg(a, "child").unwrap_or_default());
                if !self.chrome.select(s, ch) {
                    return self.fail(format!("no stack {s:?} with a child {ch:?}"));
                }
            }
            "mod.open" => {
                let name = Self::arg(a, "name").unwrap_or_default();
                let Some(f) = self.folders.get(name) else { return self.fail(format!("no mod folder {name:?}")) };
                let mut spec = mod_overlay(&f.dir);
                spec["mod"] = json!(name);
                let asked = a.get("props").filter(|p| p.is_object());
                if let Some(props) = asked {
                    spec["props"] = props.clone();
                }
                // One popup per mod: running mod.open again toggles it, like app.settings,
                // unless it asks for other props (another of the mod's palette actions). A
                // layout overlay's mods sit in its blocks and take no props: it only toggles.
                let id = format!("mod-{name}");
                if let Some((slot, layout)) = self.overlays.iter().find(|o| o.id == id).map(|o| (o.slot.clone(), o.layout.is_some())) {
                    let mut shown = self.copies.get(&slot).map(|c| c.inst.props.clone()).unwrap_or_default();
                    if let Some(p) = shown.as_object_mut() {
                        p.remove("overlay");
                    }
                    match asked {
                        // Removed, not closed: a keyed window keeps where it was dragged.
                        Some(p) if !layout && *p != shown => {
                            self.remove_overlay(&id, window, cx);
                            self.open_overlay(id, spec, window, cx);
                        }
                        _ => self.close_overlay(&id, window, cx),
                    }
                } else {
                    self.open_overlay(id, spec, window, cx);
                }
            }
            "overlay.close" => {
                let id = Self::owned(a, "id").or_else(|| self.overlays.iter().rev().find(|o| o.kind.modal_keys()).map(|o| o.id.clone()));
                if let Some(id) = id {
                    self.close_overlay(&id, window, cx);
                }
                return;
            }
            // ---- the app
            "app.welcome" => {
                self.open_welcome(window, cx);
                return;
            }
            "app.whats_new" => {
                self.open_whats_new(true, window, cx);
                return;
            }
            "session.resume" => {
                let terminal = Self::arg(a, "terminal");
                let (ready, skipped) = self.resume_plan(terminal);
                if let Some(t) = terminal && ready.is_empty() && skipped.is_empty() {
                    return self.fail(format!("no saved session for {t:?}: see session.saved"));
                }
                if ready.is_empty() {
                    if person {
                        let body = skipped.iter().take(3).map(|(r, why)| format!("{}: {why}", r.name)).collect::<Vec<_>>().join("; ");
                        let spec = json!({"kind":"toast", "notification":{"title":"Nothing to resume","body":body,"level":"info"}});
                        self.open_overlay(mods::new_overlay_id(), spec, window, cx);
                    }
                    let r = self.resume_sessions(ready, skipped, false, window, cx);
                    return self.answer(Ok(r));
                }
                if a.get("confirmed").and_then(Value::as_bool) != Some(true) {
                    if !person {
                        return self.fail("session.resume types into terminals: pass confirmed: true");
                    }
                    let n = ready.len();
                    let title = format!("Resume {n} session{}?", if n == 1 { "" } else { "s" });
                    let args = json!({ "terminal": terminal, "confirmed": true });
                    self.confirm(&title, &resumed_names(&ready), "session.resume", args, window, cx);
                    return;
                }
                let r = self.resume_sessions(ready, skipped, person, window, cx);
                self.answer(Ok(r));
                return;
            }
            "session.saved" => {
                let is_shell = |argv: &[String]| crate::programs::of(None, argv)["shell"] == true;
                let sessions: Vec<Value> = self.resume.readiness(&self.live, is_shell).iter().map(|(r, why)| crate::resume::shown(r, why)).collect();
                self.answer(Ok(json!({ "sessions": sessions })));
                return;
            }
            "app.palette" => {
                // Once: a second press closes the palette instead of stacking another.
                if self.overlays.iter().any(|o| o.id == "palette") {
                    self.close_overlay("palette", window, cx);
                } else {
                    let spec = json!({ "mod": "frame-palette", "kind": "popup", "at": "top", "offset": 60, "width": 620, "height": 0.85 });
                    self.open_overlay("palette".into(), spec, window, cx);
                }
                return;
            }
            "app.feedback" => {
                let from = Self::owned(a, "from").unwrap_or_else(|| "person".into());
                if !matches!(from.as_str(), "person" | "agent") {
                    return self.fail("from must be person or agent");
                }
                // A window, not a dialog: the app stays usable behind it (copy an error, paste it in).
                let spec = json!({ "mod": "frame-feedback", "kind": "window", "key": "feedback", "width": 680,
                    "props": { "message": Self::owned(a, "message").unwrap_or_default(), "from": from, "layout": self.layout_name } });
                self.open_overlay(mods::new_overlay_id(), spec, window, cx);
                return;
            }
            "app.settings" => {
                // Once: a second press doesn't stack another; asking for a section opens it there.
                let section = Self::owned(a, "section");
                let open = self.overlays.iter().any(|o| o.id == "settings");
                if open && section.is_some() {
                    self.close_overlay("settings", window, cx);
                }
                if !open || section.is_some() {
                    let spec = json!({ "mod": "frame-settings", "kind": "dialog", "width": 880, "props": { "section": section } });
                    self.open_overlay("settings".into(), spec, window, cx);
                }
                return;
            }
            "app.check_updates" => crate::update::check_now(),
            "app.update_core" => {
                if crate::update::core().ready.is_none() && crate::update::core().error.is_none() {
                    return self.fail("herdr-core is up to date");
                }
                crate::update::update_core();
            }
            "app.help" => {
                if self.overlays.iter().any(|o| o.id == "help") {
                    self.close_overlay("help", window, cx);
                } else {
                    let spec = json!({ "mod": "help", "kind": "dialog", "width": 640, "height": 520 });
                    self.open_overlay("help".into(), spec, window, cx);
                }
                return;
            }
            "app.reset" => self.reset(window, cx),
            "app.theme" => {
                let name = Self::arg(a, "name").unwrap_or_default().to_owned();
                if !settings::theme_exists(&name) {
                    return self.fail(format!("no theme {name:?}"));
                }
                // A layout is a look: the theme goes on the shown layout's root,
                // as its light or its dark half (the other half stays).
                let current = crate::layout::ThemeChoice::Pair {
                    light: settings::get().theme_light.clone(),
                    dark: settings::get().theme_dark.clone(),
                };
                self.chrome.look.theme = Some(with_theme(Some(current), name));
                self.picked_theme = self.chrome.look.theme.clone();
                self.save();
                self.apply_look(window, cx);
            }
            "focus.toggle_follows_mouse" => return self.toggle_follows_mouse(person, window, cx),
            "pane.zoom_text_in" | "pane.zoom_text_out" | "pane.zoom_text_reset" => {
                let steps = match name {
                    "pane.zoom_text_in" => 1,
                    "pane.zoom_text_out" => -1,
                    _ => 0,
                };
                let term = match Self::arg(a, "pane") {
                    Some(pane) => self.pane_term(pane),
                    None => self.focused_terminal(window, cx),
                };
                let Some(view) = term.map(|t| t.view.clone()) else { return self.fail("no terminal pane to zoom") };
                view.update(cx, |v, cx| v.zoom_text(steps, cx));
                // Off screen it won't draw, so nothing would refit it: its program gets the new grid now.
                if !view.read(cx).on_screen_this_frame()
                    && let Some(pane) = self.terms.iter().find(|(_, t)| t.view == view).map(|(p, _)| p.trim_end_matches(SWAP).to_owned())
                    && let Some(tab) = self.model.tab_of(&pane)
                    && let Some(area) = self.shown_area(tab, &pane)
                {
                    view.read(cx).fit(area, window);
                }
            }
            "app.zoom_in" | "app.zoom_out" | "app.zoom_reset" => {
                let zoom = settings::next_zoom(match name {
                    "app.zoom_in" => 1,
                    "app.zoom_out" => -1,
                    _ => 0,
                });
                self.write_person(|p| p.zoom = zoom, window, cx);
            }
            "app.quit" => cx.quit(),
            "app.update" => {
                if !crate::update::apply() {
                    return self.fail("no update is ready yet");
                }
                self.drain_documents(window, cx);
                crate::update::relaunch();
                cx.quit();
            }
            "window.close" | "window.minimize" | "window.maximize" => return self.window_command(name, window, cx),
            // Hiding apps is macOS's (bound natively there).
            "app.hide" | "app.hide_others" | "app.show_all" => return,
            other => {
                tracing::warn!("command {other:?} is listed but not handled");
                return;
            }
        }
        self.sync(window, cx);
    }

    fn find_node_mut(&mut self, id: &str) -> Option<&mut Node> {
        if self.chrome.find(id).is_some() {
            return self.chrome.find_mut(id);
        }
        self.model.spaces.iter_mut().flat_map(|p| p.tabs.iter_mut()).find_map(|c| c.find_mut(id))
    }

    /// The nearest visible pane, also considering focusable mods for `pane.go`.
    fn neighbor(&self, from: &str, dir: &str, use_history: bool) -> Option<String> {
        let bounds = self.bounds.borrow();
        let cur = *bounds.get(from)?;
        // Floating panes first, front to back: the frontmost wins a tie (`nearest` keeps the first).
        let tab = self.model.active_tab();
        let floats = tab.into_iter().filter(|_| use_history).flat_map(|c| c.shown_floats().rev().map(|f| f.pane.id.clone()));
        let tiled = tab.into_iter().flat_map(|c| c.shown().visible_blocks().into_iter().filter(|n| n.is_pane()).map(|n| n.id.clone()));
        let mut candidates: Vec<String> = floats.chain(tiled).collect();
        if use_history {
            candidates.extend(self.chrome.visible_blocks().into_iter().filter(|n| self.focusable(&n.id)).map(|n| n.id.clone()));
        }
        let candidates = candidates.into_iter().filter(|id| id != from).filter_map(|id| Some((id.clone(), *bounds.get(&id)?)));
        nearest(cur, dir, candidates, if use_history { &self.pane_history } else { &[] })
    }

    /// `pane.go`: the nearest pane or focusable mod.
    fn go(&mut self, dir: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(from) = self.focused_block.clone().or_else(|| self.model.focused_pane()) else { return };
        let Some(to) = self.neighbor(&from, dir, true) else { return };
        if self.focusable(&to) {
            return self.focus_block(&to, window, cx);
        }
        self.focused_block = None;
        self.model.focus_pane(&to);
        self.focused_pane = None;
        self.sync(window, cx);
    }

    /// Until the mod takes the keyboard, the window holds the keys.
    fn focus_block(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.model.tab_of(id).is_some() {
            self.model.focus_pane(id);
            self.focused_pane = None;
        }
        self.focused_block = Some(id.to_owned());
        self.sync(window, cx);
        if !self.overlays.iter().any(|o| o.kind.modal_keys()) {
            self.focus.focus(window, cx);
            self.hand_keyboard();
        }
    }

    /// Its copy gets `focus` from `nextCommand()`.
    fn hand_keyboard(&self) {
        if let Some(copy) = self.focused_block.as_ref().and_then(|b| self.copies.get(b)) {
            mods::send_command_to(copy.id, "focus");
        }
    }

    /// A mod block on screen: in the layout, or a mod pane of the shown tab.
    fn mod_block_shown(&self, id: &str) -> bool {
        self.copies.contains_key(id)
            && (self.chrome.visible_blocks().iter().any(|n| n.id == id && matches!(n.content(), Some(Content::Mod { .. })))
                || self.model.active_tab().is_some_and(|t| t.shown().visible_blocks().iter().any(|n| n.id == id) || t.shown_floats().any(|f| f.pane.id == id)))
    }

    /// Its mod.json says `"focusable": true` (`go` asks only of visible blocks).
    fn focusable(&self, id: &str) -> bool {
        self.copies.get(id).and_then(|c| self.folders.get(&c.inst.name)).is_some_and(|f| f.hint.focusable)
    }

    fn swap_pane(&mut self, from: &str, explicit: bool, dir: &str) -> Result<(), String> {
        let neighbor = self.neighbor(from, dir, false);
        swap_with_neighbor(&mut self.model, from, explicit, neighbor)
    }

    // ---- overlays ----------------------------------------------------------

    fn confirm(&mut self, title: &str, detail: &str, command: &str, args: Value, window: &mut Window, cx: &mut Context<Self>) {
        let spec = json!({ "mod": "confirm", "kind": "dialog", "width": 380,
            "props": { "title": title, "detail": detail, "command": { "name": command, "args": args } } });
        self.open_overlay(mods::new_overlay_id(), spec, window, cx);
    }

    #[allow(clippy::too_many_arguments)]
    fn open_menu(&mut self, tab: &str, target: Value, at: Option<Point<Pixels>>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = at {
            self.pointer = p;
        }
        let requested = self.chrome.terminal_menu.as_deref().unwrap_or("menu");
        let name = if self.folders.contains_key(requested) {
            requested
        } else {
            tracing::warn!(mod_name = requested, "terminal menu mod not found; using menu");
            "menu"
        };
        let defaults = self.folders.get(name).map(|f| mod_overlay(&f.dir)).unwrap_or_else(|| json!({}));
        let spec = terminal_menu_spec(name, &defaults, tab, target, f32::from(window.viewport_size().height), settings::ui_scale());
        self.open_overlay(mods::new_overlay_id(), spec, window, cx);
    }

    /// The layout's notification mod (`"toast"` on the root), else the shipped `toast`.
    fn toast_mod(&self) -> &str {
        let requested = self.chrome.toast.as_deref().unwrap_or("toast");
        if self.folders.contains_key(requested) {
            requested
        } else {
            tracing::warn!(mod_name = requested, "toast mod not found; using toast");
            "toast"
        }
    }

    fn open_overlay(&mut self, id: String, spec: Value, window: &mut Window, cx: &mut Context<Self>) {
        let kind = match spec.get("kind").and_then(Value::as_str) {
            Some("popup") => OverlayKind::Popup,
            Some("toast") => OverlayKind::Toast,
            Some("window") => OverlayKind::Window,
            _ => OverlayKind::Dialog,
        };
        let key = spec.get("key").and_then(Value::as_str).filter(|k| !k.is_empty() && kind == OverlayKind::Window).map(str::to_owned);
        // One window per name: opening it again replaces it, in its place.
        let mut place = key.as_deref().and_then(movable::place_of);
        if let Some(k) = &key
            && let Some(old) = self.overlays.iter().find(|o| o.key.as_ref() == Some(k)).map(|o| (o.id.clone(), o.place))
        {
            place = old.1.or(place);
            self.remove_overlay(&old.0, window, cx);
        }
        let layout = spec.get("layout").filter(|_| kind != OverlayKind::Toast).and_then(|l| match serde_json::from_value::<Node>(l.clone()) {
            Ok(n) => Some(overlay_layout(n, &id)),
            Err(e) => {
                tracing::warn!("overlay layout: {e}");
                None
            }
        });
        let (name, mut props) = if kind == OverlayKind::Toast {
            (self.toast_mod().to_owned(), spec.get("notification").cloned().unwrap_or(Value::Null))
        } else {
            (spec.get("mod").and_then(Value::as_str).unwrap_or_default().to_owned(), spec.get("props").cloned().unwrap_or(json!({})))
        };
        if !props.is_object() {
            props = json!({});
        }
        props["overlay"] = json!(id);
        if kind == OverlayKind::Toast {
            with_toast_look(&mut props);
        }
        let urgent = kind == OverlayKind::Toast && matches!(props.get("level").and_then(Value::as_str), Some("error" | "warning"));
        let num = |k: &str, d: f32| spec.get(k).and_then(Value::as_f64).map_or(d, |v| v as f32);
        let (min_w, max_w, toast_h) = toast_box(settings::get().toast_design, &props);
        let asked_height = spec.get("height").and_then(Value::as_f64).map(|h| h as f32).filter(|_| kind == OverlayKind::Toast);
        let size = match kind {
            // No width: as wide as its content, between the toast bounds.
            OverlayKind::Toast => size(0.0, asked_height.unwrap_or(toast_h)),
            // No size: as big as its content (0 = unset).
            OverlayKind::Window => size(num("width", 0.0), num("height", 0.0)),
            // No height: as tall as the content, up to most of the window.
            _ => size(num("width", 420.0), num("height", 0.85)),
        };
        let at = match spec.get("at") {
            Some(v) if v.get("x").is_some() => point(px(num_in(v, "x")), px(num_in(v, "y"))),
            _ => self.pointer,
        };
        let top = (spec.get("at").and_then(Value::as_str) == Some("top")).then(|| num("offset", 0.0));
        let fit = layout.is_none() && (kind == OverlayKind::Popup || (matches!(kind, OverlayKind::Dialog | OverlayKind::Window) && spec.get("height").is_none()));
        if kind == OverlayKind::Window && place.is_none() && spec.get("at").is_some_and(|v| v.get("x").is_some()) {
            place = Some(at);
        }
        let timeout = toast_timeout(&props, settings::get().toast_duration);
        let level = toast_level(&props);
        let slot = format!("overlay:{id}");
        // The frame takes the keyboard first, so a mod that focuses itself while loading wins.
        let focus = cx.focus_handle();
        if kind.modal_keys() {
            focus.focus(window, cx);
        }
        if layout.is_none() {
            let copy = self.load(Instance { name, props }, window, cx);
            // Popups (menus) are as tall as what's in them; `height` is the most.
            // A toast's height is set, but it's as wide as what's in it.
            if (fit || kind == OverlayKind::Toast)
                && let Some(root) = &copy.root
            {
                root.update(cx, |r, cx| {
                    r.set_fit_content(true, cx);
                    // As wide as its text, but a toast's box can be wider (its minimum) and is as tall as set.
                    r.set_fill_fitted(kind == OverlayKind::Toast, cx);
                });
            }
            self.copies.insert(slot.clone(), copy);
        }
        let first_term = layout.as_ref().and_then(|l| l.visible_blocks().into_iter().find(|n| matches!(n.content(), Some(Content::Terminal { .. }))).map(|n| n.id.clone()));
        let has_layout = layout.is_some();
        self.overlays.push(Overlay {
            id,
            kind,
            slot,
            at,
            size,
            expires: timeout.filter(|_| kind == OverlayKind::Toast).map(|t| Instant::now() + t),
            focus,
            layout,
            top,
            fit,
            modal: spec.get("modal").and_then(Value::as_bool).unwrap_or(false),
            urgent,
            level,
            widths: (min_w, max_w),
            asked_height,
            opened: Instant::now(),
            bare: spec.get("frame").and_then(Value::as_str) == Some("none"),
            place,
            key: key.clone(),
            frame: spec.get("mod").and_then(Value::as_str).is_some_and(|m| m.starts_with("frame-")),
            alert: None,
        });
        if kind == OverlayKind::Toast {
            self.close_toasts_past_limit(window, cx);
            self.count_banners(cx);
            self.tick_progress(cx);
        }
        if let Some(k) = &key {
            if spec.get("keep").and_then(Value::as_bool) == Some(true) {
                movable::keep(&self.layout_name, k, &spec);
            } else {
                movable::forget(&self.layout_name, k);
            }
        }
        if has_layout {
            self.sync(window, cx);
            if kind.modal_keys()
                && let Some(t) = first_term.and_then(|b| self.terms.get(&b))
            {
                t.focus.focus(window, cx);
            }
        } else {
            self.publish_shown_copies(cx);
        }
        cx.notify();
    }

    /// A burst of toasts shows at most `TOAST_LIMIT`: past it, the oldest
    /// close, routine ones before errors and warnings, so a burst of
    /// "finished" toasts can't push out an error nobody has read yet. The
    /// toast just opened always shows: with five errors kept until clicked,
    /// a new "Copied" closes the oldest error rather than itself.
    fn close_toasts_past_limit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let toasts: Vec<(String, bool)> = self.overlays.iter().filter(|o| o.kind == OverlayKind::Toast).map(|o| (o.id.clone(), o.urgent)).collect();
        let excess = toasts.len().saturating_sub(TOAST_LIMIT);
        let Some((_, older)) = toasts.split_last() else { return };
        let routine = older.iter().filter(|(_, urgent)| !urgent);
        let urgent = older.iter().filter(|(_, urgent)| *urgent);
        let closing: Vec<String> = routine.chain(urgent).take(excess).map(|(id, _)| id.clone()).collect();
        for id in closing {
            self.close_overlay(&id, window, cx);
        }
    }

    fn peek_terminal(&mut self, address: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open_peek(address, (0.8, 0.8), window, cx);
    }

    /// A new popup ends the program of the last one `terminal.popup` started,
    /// open or hidden (under a plain peek), unless the core gave the new one its address.
    fn replace_popped(&mut self, new: Popped, cx: &mut Context<Self>) {
        if self
            .popped
            .as_ref()
            .is_some_and(|p| p.address != new.address)
        {
            self.end_popped(cx);
        }
        self.popped = Some(new);
    }

    /// Ends the program of the last popup, if that terminal is still the one
    /// at its address: a popup lives only as long as it can be seen.
    fn end_popped(&mut self, cx: &mut Context<Self>) {
        let Some(last) = self.popped.take() else {
            return;
        };
        if self.live.get(&last.address).is_some_and(|t| !last.is(t))
            || self.held_elsewhere(&last.address)
        {
            return;
        }
        // A waited-on one is removed once its waiter has the exit code.
        let rm = !self.waits.contains_key(&last.address);
        self.end_terminal(last.address, rm, cx);
    }

    /// A keep of a popup that failed: it is a popup again, shown, so closing
    /// it still ends it. With another popup up meanwhile, that one stays and
    /// this one ends: nothing would show it (the "could not change" notice says why).
    fn reshow_popup(&mut self, popup: Option<Popped>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(popup) =
            popup.filter(|p| !self.draining && self.live.get(&p.address).is_some_and(|t| p.is(t)))
        else {
            return;
        };
        let address = popup.address.clone();
        if self.popped.as_ref().is_some_and(|p| p.address != address) {
            if self.held_elsewhere(&address) {
                return;
            }
            let rm = !self.waits.contains_key(&address);
            self.end_terminal(address, rm, cx);
            return;
        }
        self.popped = Some(popup);
        self.open_peek(&address, (0.8, 0.8), window, cx);
    }

    /// Whether a popup's terminal is the person's beyond the popup: in a pane
    /// (kept, swapped in, given one by `hc keep` or another window), core-kept,
    /// or on its way into a pane. Such a terminal is never ended with the popup.
    fn held_elsewhere(&self, address: &str) -> bool {
        self.model.pane_of(address).is_some()
            || self.swapped.values().any(|a| a == address)
            || self.changing_terminals.contains(address)
            || self
                .live
                .get(address)
                .is_some_and(|t| t["keep"] == Value::Bool(true))
    }

    /// After a listing: the last popup's terminal finished, went, or another
    /// took its address, so there is nothing left for the next popup to end.
    fn forget_gone_popped(&mut self) {
        let Some(p) = &mut self.popped else { return };
        match self.live.get(&p.address) {
            Some(t) if p.is(t) => p.seen = true,
            Some(_) => self.popped = None,
            None if p.seen => self.popped = None,
            // Not listed yet: it started after this listing began.
            None => {}
        }
    }

    /// The peek overlay. It replaces the one open: a plain peek over a popup
    /// (a link clicked in it) only hides the popup, whose program ends when
    /// the peek overlay closes or the next popup opens.
    fn open_peek(
        &mut self,
        address: &str,
        (width, height): (f64, f64),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.peeked = Some(address.to_owned());
        self.remove_overlay("terminal-peek", window, cx);
        self.open_overlay("terminal-peek".into(), json!({
            "kind": "dialog",
            "width": width,
            "height": height,
            "layout": {
                "type": "column",
                "children": [
                    { "type": "block", "size": { "px": 34 }, "content": { "mod": "peek", "props": { "terminal": address } } },
                    { "type": "block", "content": { "terminal": address } }
                ]
            }
        }), window, cx);
    }

    /// `terminal.popup`: a program on this computer, as `hc run --bg --no-parent`
    /// starts one (`--rm` unless `wait`), shown in the peek overlay at once.
    fn popup(&mut self, a: &Value, window: &mut Window, cx: &mut Context<Self>) -> Result<Value, String> {
        let argv = Self::argv(a).filter(|v| !v.is_empty()).ok_or("terminal.popup argv is the program and its arguments, as a list")?;
        let flag = |k: &str| a.get(k).and_then(Value::as_bool).unwrap_or(false);
        let (hold, wait) = (flag("hold"), flag("wait"));
        let argv = if hold { popup_hold_argv(argv)? } else { argv };
        let size = |k: &str| match a.get(k) {
            None | Some(Value::Null) => Ok(0.8),
            Some(v) => v.as_f64().filter(|n| *n > 0.0).ok_or(format!("terminal.popup {k} is a positive number")),
        };
        let size = (size("width")?, size("height")?);
        let cwd = Self::owned(a, "cwd").unwrap_or_else(home);
        let labels = json!({ POPUP_LABEL: std::process::id().to_string() });
        let params = json!({ "argv": argv, "cwd": cwd, "no_parent": true, "rm": !wait, "labels": labels, "display": crate::terminal::initial_display(window) });
        let record = self.core.start_record(params).map_err(|e| format!("cannot start a terminal: {e:#}"))?;
        let address = record["address"].as_str().unwrap_or_default().to_owned();
        if wait {
            self.waits.insert(address.clone(), Waiting::default());
        }
        self.replace_popped(Popped { address: address.clone(), created: record["created_ms"].as_u64(), seen: false }, cx);
        self.open_peek(&address, size, window, cx);
        Ok(json!({ "terminal": address }))
    }

    /// The `herdr-app popup` that asked for this one gave up before the answer
    /// (it timed out, or Ctrl-C): nobody will wait on it or remove it.
    fn abandon_popup(&mut self, answer: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let Some(address) = answer["terminal"].as_str() else { return };
        if self.waits.remove(address).is_none() {
            return;
        }
        if self.popped.as_ref().is_some_and(|p| p.address == address) {
            self.popped = None;
        }
        if self.peeked.as_deref() == Some(address) {
            self.peeked = None;
            self.close_overlay("terminal-peek", window, cx);
        }
        self.end_terminal(address.to_owned(), true, cx);
    }

    /// Ends a terminal's program off the UI thread, then removes the terminal if `rm`.
    fn end_terminal(&self, address: String, rm: bool, cx: &mut Context<Self>) {
        let core = self.core.clone();
        cx.background_executor()
            .spawn(async move {
                if let Err(e) = core.call("terminal.kill", json!({ "terminal": address })) {
                    tracing::warn!("popup terminal.kill: {e:#}");
                }
                if rm && let Err(e) = core.call("terminal.rm", json!({ "terminal": address })) {
                    tracing::warn!("popup terminal.rm: {e:#}");
                }
            })
            .detach();
    }

    fn rm_terminal(&self, address: String, cx: &mut Context<Self>) {
        let core = self.core.clone();
        cx.background_executor()
            .spawn(async move {
                if let Err(e) = core.call("terminal.rm", json!({ "terminal": address })) {
                    tracing::warn!("popup terminal.rm: {e:#}");
                }
            })
            .detach();
    }

    /// Closing: no popup outlives the window. The last one's program ends, and
    /// its terminal goes with the waited-on ones (no waiter is answered once
    /// the window has gone).
    pub(crate) fn end_popups_now(&mut self, deadline: Instant) {
        let popped = self
            .popped
            .take()
            .filter(|p| self.live.get(&p.address).is_none_or(|t| p.is(t)))
            .map(|p| p.address);
        let ours = |a: &&String| !self.held_elsewhere(a);
        let running: std::collections::BTreeSet<&String> = popped
            .iter()
            .chain(
                self.waits
                    .iter()
                    .filter(|(_, w)| w.ended.is_none())
                    .map(|(a, _)| a),
            )
            .filter(ours)
            .collect();
        let all: std::collections::BTreeSet<&String> = popped
            .iter()
            .chain(self.waits.keys())
            .filter(ours)
            .collect();
        let calls = running
            .into_iter()
            .map(|a| ("terminal.kill", a))
            .chain(all.into_iter().map(|a| ("terminal.rm", a)));
        for (method, address) in calls {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            if let Err(e) = self
                .core
                .call_timeout(method, json!({ "terminal": address }), left)
            {
                tracing::warn!("popup {method}: {e:#}");
            }
        }
        self.waits.clear();
    }

    /// `{"wait": terminal}` on the control socket: answered with its exit code
    /// once it ends, now if it already has.
    fn wait_for(&mut self, address: &str, reply: std::sync::mpsc::Sender<Value>, cx: &mut Context<Self>) {
        let Some(w) = self.waits.get_mut(address) else {
            let _ = reply.send(json!({ "error": format!("nothing waits on {address:?}: only terminal.popup with wait does") }));
            return;
        };
        match &w.ended {
            Some((ended, _)) => {
                if reply.send(ended.clone()).is_ok() {
                    self.settled(address.to_owned(), cx);
                }
            }
            None => w.replies.push(reply),
        }
    }

    /// After the core's events: waited-on popups that finished (`ended`: how
    /// they ended) or went without an end (removed by someone else) answer
    /// their waiters, and the terminal is removed. One nobody waits on yet
    /// keeps its answer, and its terminal, for `WAIT_KEPT`.
    fn settle_waits(&mut self, ended: &HashMap<String, Value>, now: Instant, cx: &mut Context<Self>) {
        let mut answered = Vec::new();
        for (address, w) in self.waits.iter_mut().filter(|(_, w)| w.ended.is_none()) {
            let end = match ended.get(address) {
                Some(end) => end,
                None if self.live.contains_key(address) => {
                    w.seen = true;
                    continue;
                }
                None if w.seen => &Value::Null,
                // Not listed yet: it started after this listing began.
                None => continue,
            };
            let answer = json!({ "terminal": address, "exit_code": end["exit_code"], "signal": end["signal"] });
            // A waiter that hung up (Ctrl-C) takes nothing.
            let taken = w.replies.drain(..).fold(false, |taken, reply| reply.send(answer.clone()).is_ok() || taken);
            w.ended = Some((answer, now));
            if taken {
                answered.push(address.clone());
            }
        }
        for address in answered {
            self.settled(address, cx);
        }
        self.expire_waits(now, cx);
    }

    /// Finished popups whose waiter never came, after `WAIT_KEPT`.
    fn expire_waits(&mut self, now: Instant, cx: &mut Context<Self>) {
        let expired: Vec<String> = self.waits.iter().filter(|(_, w)| w.ended.as_ref().is_some_and(|(_, at)| now.duration_since(*at) >= WAIT_KEPT)).map(|(a, _)| a.clone()).collect();
        for address in expired {
            self.settled(address, cx);
        }
    }

    /// A finished popup is done with: removed, and nothing left to end.
    fn settled(&mut self, address: String, cx: &mut Context<Self>) {
        self.waits.remove(&address);
        if self.popped.as_ref().is_some_and(|p| p.address == address) {
            self.popped = None;
        }
        // Kept as a tab: its pane shows how it ended, as any kept terminal's does.
        if !self.held_elsewhere(&address) {
            self.rm_terminal(address, cx);
        }
    }

    /// Which copies are on screen, for `<mod>.<command>` (`mods::send_command`)
    /// and refreshes; one coming into view catches up on what changed meanwhile.
    fn publish_shown_copies(&self, cx: &mut Context<Self>) {
        let (_, shown) = self.visible();
        let fresh = mods::set_shown_copies(shown.iter().filter_map(|(s, i)| self.copies.get(s).filter(|c| c.inst == *i)).map(|c| c.id).collect());
        if let Some(runtime) = &self.runtime {
            for c in self.copies.values().filter(|c| fresh.contains(&c.id)) {
                if let Some(root) = &c.root
                    && (mods::take_missed(c.id) | mods::snapshot_changed(c.id))
                {
                    let _ = runtime.refresh(root, cx);
                }
            }
        }
    }

    /// Closes an overlay; a kept window stays closed on the next start too.
    /// The peek overlay closing (✕, Esc, the backdrop) ends the last popup's
    /// program, shown or hidden under it: nothing would show it again.
    fn close_overlay(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(k) = self.overlays.iter().find(|o| o.id == id).and_then(|o| o.key.clone()) {
            movable::forget(&self.layout_name, &k);
        }
        if id == "terminal-peek" {
            self.end_popped(cx);
        }
        self.remove_overlay(id, window, cx);
    }

    fn remove_overlay(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(i) = self.overlays.iter().position(|o| o.id == id) {
            let o = self.overlays.remove(i);
            if self.win_drag.as_ref().is_some_and(|d| d.id == o.id) {
                self.win_drag = None;
            }
            self.bounds.borrow_mut().remove(&o.slot);
            self.remove_copy(&o.slot, window, cx);
            if o.layout.is_some() {
                self.sync(window, cx);
            } else {
                self.publish_shown_copies(cx);
            }
            // The keyboard never ends up nowhere: back to a dialog still open,
            // else the pane, else the window (where the app's keys work). A
            // window in the background had none to give back (one that had it:
            // `restore_focus` runs on its own when its focus goes).
            if o.kind.modal_keys() {
                self.restore_focus(window, cx);
            }
            if o.kind == OverlayKind::Toast {
                self.count_banners(cx);
            }
            cx.notify();
        }
    }

    /// The escape hatch: set the files aside and go back to the default screen.
    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let layout = format!("layouts/{}.json", self.layout_name);
        for name in [layout.as_str(), "keys.json"] {
            if file(name).exists() {
                let _ = std::fs::rename(file(name), aside(&file(name), "reset"));
            }
        }
        let _ = std::fs::remove_file(state_path(&self.layout_name));
        tracing::warn!("escape hatch: {layout} and keys.json set aside as *.reset-<time>");
        self.watched.keys = None;
        install_layouts();
        self.drop_layout_copies(window, cx);
        self.load_layout("main");
        self.apply_look(window, cx);
        self.focused_pane = None;
        self.agent_focused = None;
        let ids: Vec<String> = self.overlays.iter().map(|o| o.id.clone()).collect();
        for id in ids {
            self.close_overlay(&id, window, cx);
        }
        self.show_layout_notes(window, cx);
        self.reload_config(window, cx);
    }

    fn handle(&mut self, req: Request, window: &mut Window, cx: &mut Context<Self>) {
        match req {
            Request::Run { name, args } => self.run(&name, args, window, cx),
            Request::OpenOverlay { id, spec } => self.open_overlay(id, spec, window, cx),
            Request::CloseOverlay { id } => self.close_overlay(&id, window, cx),
            Request::Refresh => self.refresh_mods(cx),
            Request::Changed => self.refresh_snapshot_mods(cx),
            Request::Git(update) => {
                if git_status::apply(update) {
                    self.track_folders(cx);
                    self.refresh_snapshot_mods(cx);
                }
            }
            Request::Lines { terminal, lines, reply } => {
                let read = self.terms.values().find(|t| t.address == terminal).map(|t| t.view.read(cx).tail_lines(lines));
                let answer = read.map_or(Value::Null, |lines| {
                    Value::Array(lines.into_iter().map(|l| json!({ "text": l.text, "faint": l.faint })).collect())
                });
                let _ = reply.try_send(answer);
            }
            Request::Control { cmd, reply } => match cmd.get("wait").and_then(Value::as_str) {
                Some(address) => self.wait_for(address, reply, cx),
                None => {
                    let popup = cmd.get("do").and_then(Value::as_str) == Some("terminal.popup");
                    let answer = self.control(cmd, window, cx);
                    if let Err(unsent) = reply.send(answer)
                        && popup
                    {
                        self.abandon_popup(&unsent.0, window, cx);
                    }
                }
            },
        }
    }

    fn control(&mut self, cmd: Value, window: &mut Window, cx: &mut Context<Self>) -> Value {
        if let Some(name) = cmd.get("do").and_then(Value::as_str) {
            let args = cmd.get("args").cloned().unwrap_or(Value::Null);
            return self.run_as(name, args, false, window, cx);
        }
        if let Some(path) = cmd.get("shot").and_then(Value::as_str) {
            return match crate::control::save_shot(window, std::path::Path::new(path)) {
                Ok(()) => json!({ "path": path }),
                Err(error) => json!({ "error": format!("cannot save screenshot: {error:#}") }),
            };
        }
        if cmd.get("commands").is_some() {
            return crate::commands::listing();
        }
        // Herdr was launched again: this window comes to the front instead.
        if cmd.get("raise").is_some() {
            cx.activate(true);
            window.activate_window();
            return json!({ "ok": true });
        }
        if cmd.get("ls").is_some() {
            return self.world();
        }
        if cmd.get("status").is_some() {
            let round = |v: f32| (v as f64 * 100.0).round() / 100.0;
            let errors: Vec<String> = ["settings", "keys", "layout", "programs"]
                .iter()
                .filter_map(|w| std::fs::read_to_string(file(&format!("{w}.error"))).ok())
                .chain(self.copies.values().filter_map(|c| Some(format!("mod {}: {}", c.inst.name, c.error.as_ref()?))))
                .chain(mods::actions(self.folders.values()).1)
                .chain(mods::menu_items(self.folders.values()).1)
                .chain(self.documents.iter().filter_map(|(m, s)| s.error.as_ref().map(|e| format!("layout {}: {e}", if m.is_empty() { "local" } else { m }))))
                .collect();
            return json!({
                "zoom": round(settings::zoom()),
                "font_size": round(settings::font_size()),
                "ui_font_size": round(settings::ui_font_size()),
                "ui_zoom": round(settings::get().ui_zoom),
                "ui_text_scale": round(settings::get().ui_text_scale),
                "space": self.model.active,
                "tab": self.model.active_tab().map(|c| c.id.clone()),
                "focused_pane": self.focused_pane,
                "layout": self.layout_name,
                "agent": self.shown_agent(),
                "floats": self.float_status(),
                "overlays": self.overlays.iter().map(|o| match &o.layout {
                    Some(_) => Some("layout".to_owned()),
                    None => self.copies.get(&o.slot).map(|c| c.inst.name.clone()),
                }).collect::<Vec<_>>(),
                "errors": errors,
            });
        }
        json!({ "error": "expected { do }, { ls }, { commands } or { status }" })
    }

    /// Spaces, their tabs and terminals, for `herdr-app ls`. Sub-agents are listed
    /// under the terminal that started them.
    fn world(&self) -> Value {
        let m = &self.model;
        let children = |addr: &str| -> Vec<String> {
            let mut out: Vec<String> = self
                .live
                .iter()
                .filter(|(_, r)| r.get("parent").and_then(Value::as_str) == Some(addr))
                .map(|(a, _)| a.clone())
                .collect();
            out.sort();
            out
        };
        let terminal = |pane: &str, addr: &str| {
            let rec = self.live.get(addr);
            json!({
                "terminal": addr, "pane": pane, "status": m.status(addr),
                "app": m.agents.get(addr).and_then(|a| a.app.clone()),
                "name": rec.and_then(|r| r.get("name")).cloned(),
                "label": self.labels.get(pane), "session": self.session_names.name(addr, rec.and_then(|r| r.pointer("/foreground/pid")).and_then(Value::as_u64)),
                "sub_agents": children(addr),
            })
        };
        let spaces: Vec<Value> = m
            .spaces
            .iter()
            .map(|p| {
                let active = p.active_tab().map(|c| c.id.clone());
                let tabs: Vec<Value> = p
                    .tabs
                    .iter()
                    .map(|c| {
                        let terms: Vec<Value> = c
                            .terminals()
                            .iter()
                            .filter_map(|n| {
                                let mut t = terminal(&n.id, n.terminal()?);
                                if let Some(f) = c.float(&n.id) {
                                    t["floating"] = json!(f.rect);
                                }
                                Some(t)
                            })
                            .collect();
                        json!({ "id": c.id, "name": Model::tab_name(p, c), "active": Some(&c.id) == active.as_ref(), "settled": c.settled.is_some(), "arrange": c.arrange.map(crate::arrange::Mode::name), "terminals": terms })
                    })
                    .collect();
                json!({ "id": p.id, "name": m.space_name(p, &home()), "folder": p.folder, "machine": p.machine, "active": m.active.as_deref() == Some(&p.id), "tabs": tabs })
            })
            .collect();
        json!({ "spaces": spaces, "recent": m.recent })
    }

    // ---- mods on disk ----------------------------------------------------

    fn poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.flush_documents(false, window, cx);
        let ready = Self::update_label();
        if ready != self.update_ready {
            self.update_ready = ready;
            cx.notify();
        }
        // The settings screen shows versions and progress: redraw mods when they move.
        let status = crate::update::status().to_string();
        if status != self.update_status {
            self.update_status = status;
            self.refresh_mods(cx);
        }
        if let Some(r) = &self.runtime {
            crate::bench::note_mods(&r.read_metrics());
        }
        // With file events the disk is only checked now and then, in case one
        // was missed.
        self.polls += 1;
        if self.polls.is_multiple_of(25) {
            self.observe();
        }
        if self.file_events.is_none() || self.polls.is_multiple_of(25) {
            self.reload_config(window, cx);
            self.check_layout_file(window, cx);
            self.sync_folders_later(window, cx);
        }
        let now = Instant::now();
        let expired: Vec<String> = self.overlays.iter().filter(|o| o.expires.is_some_and(|e| e <= now) && !self.waiting_banner(o)).map(|o| o.id.clone()).collect();
        for id in expired {
            self.close_overlay(&id, window, cx);
        }
        let active = window.is_window_active();
        if active != self.window_active {
            self.window_active = active;
            if active {
                self.sync(window, cx);
            } else {
                self.looked.clear();
            }
            // Tell mods now: render publishes window() too, but a mod it refreshes
            // waits for another frame, which an idle window that just lost focus
            // may not draw for a long time.
            if mods::publish_window(self.window_json(window)) {
                self.refresh_snapshot_mods(cx);
            }
        }
        let dark = is_dark(window.appearance());
        if dark != self.system_dark {
            self.system_dark = dark;
            self.apply_look(window, cx);
        }
    }

    /// Files people or agents edited: apply what they change.
    fn files_changed(&mut self, changed: Vec<std::path::PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        for path in changed_asset_paths(&mods::app_dir(), &changed) {
            invalidate_asset(&path, window, cx);
        }
        let top = |p: &std::path::PathBuf| p.components().next().and_then(|c| c.as_os_str().to_str().map(str::to_owned)).unwrap_or_default();
        if changed.iter().any(|p| matches!(top(p).as_str(), "settings.json" | "keys.json" | "programs.json" | "themes")) {
            self.reload_config(window, cx);
        }
        // The theme showing may be the file that changed.
        if changed.iter().any(|p| top(p) == "themes") {
            self.apply_look(window, cx);
        }
        if changed.iter().any(|p| top(p) == "layouts") {
            self.check_layout_file(window, cx);
        }
        let edited: BTreeSet<String> = changed.iter().filter_map(|p| crate::files::mod_of(p)).collect();
        if edited.is_empty() {
            return;
        }
        // Copies that load now (a new folder, a fixed failed load) already run the
        // saved code: reloading them too would start them twice.
        let loaded: BTreeSet<u64> = self.copies.values().filter(|c| c.root.is_some()).map(|c| c.id).collect();
        self.sync_folders(window, cx);
        let Some(runtime) = self.runtime.clone() else { return };
        for copy in self.copies.values_mut().filter(|c| edited.contains(&c.inst.name) && loaded.contains(&c.id)) {
            let Some(root) = &copy.root else { continue };
            let error = runtime.reload_now(root, window, cx).err().map(|e| format!("{e:#}"));
            if let Some(e) = &error {
                tracing::warn!("mod reload failed: {e}");
            }
            copy.error = error;
        }
    }

    fn sync_folders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.runtime.is_none() {
            return;
        }
        self.folder_scans += 1;
        self.apply_folders(mods::scan(), window, cx);
    }

    /// `sync_folders` for the poll, which looks in case a file event was missed
    /// (or with no file events at all): reading ~60 folders takes 15–25 ms, so
    /// it reads them off the main thread. A look begun since (an edit's
    /// `sync_folders`) wins over this one's older answer.
    fn sync_folders_later(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.runtime.is_none() || self.folder_scan_running {
            return;
        }
        self.folder_scan_running = true;
        self.folder_scans += 1;
        let scan = self.folder_scans;
        cx.spawn_in(window, async move |this, cx| {
            let found = cx.background_executor().spawn(async { mods::scan() }).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.folder_scan_running = false;
                if this.folder_scans == scan {
                    this.apply_folders(found, window, cx);
                }
            });
        })
        .detach();
    }

    fn apply_folders(&mut self, scanned: Vec<Found>, window: &mut Window, cx: &mut Context<Self>) {
        let (found, actions_changed) = scan_folders(scanned);
        let edited: BTreeSet<String> = found.values().filter(|f| self.folders.get(&f.id).is_some_and(|o| o.stamp != f.stamp)).map(|f| f.id.clone()).collect();
        let added = found.keys().any(|k| !self.folders.contains_key(k));
        let removed = self.folders.keys().any(|k| !found.contains_key(k));
        let fresh: Vec<Found> = found.values().filter(|f| !self.seen.contains(&f.id)).cloned().collect();
        self.folders = found;

        // Copies that failed get another try when their folder changes; loaded ones hot-reload.
        let retry: Vec<String> = self.copies.iter().filter(|(_, c)| c.root.is_none() && (edited.contains(&c.inst.name) || added)).map(|(s, _)| s.clone()).collect();
        for slot in &retry {
            self.copies.remove(slot);
        }
        // A mod seen for the first time goes where it asked (unless the layout already shows it).
        for f in fresh {
            if shipped_layouts().contains(&self.layout_name.as_str()) && f.hint.place.as_deref().unwrap_or("right") != "none" {
                continue;
            }
            self.seen.insert(f.id.clone());
            write_atomic(&file("seen.json"), &serde_json::to_string(&self.seen).unwrap_or_default());
            let shown = self.chrome.walk().iter().any(|n| matches!(n.content(), Some(Content::Mod { name, .. }) if *name == f.id));
            let place = f.hint.place.as_deref().unwrap_or("right");
            if !shown && place != "none" && !shipped_layouts().contains(&self.layout_name.as_str()) {
                let mut block = Node::block(self.model.fresh_id("b"), Content::Mod { name: f.id.clone(), props: Value::Null });
                block.size = Some(Span::Px { px: f.hint.size.unwrap_or(320.0) });
                dock(&mut self.chrome, block, place, self.model.fresh_id("r"));
            }
        }
        if !retry.is_empty() || added || removed {
            self.sync(window, cx);
        }
        if actions_changed {
            self.refresh_snapshot_mods(cx);
        }
    }

    // ---- rendering ---------------------------------------------------------

    fn focused_path(&self, window: &Window, cx: &App) -> HashSet<String> {
        let mut out = HashSet::new();
        // A mod block in the layout holding the keyboard: its `focus` looks, not the pane's.
        if let Some(path) = self.focused_block.as_ref().and_then(|b| self.chrome.path_to(b)) {
            out.extend(path);
            return out;
        }
        if let Some(p) = &self.focused_pane {
            // Pane selection survives an input, float or dialog taking the keyboard.
            if self.pane_term(p).is_some_and(|t| !t.view.read(cx).owns_keyboard_focus(window, cx)) {
                return out;
            }
            if let Some(c) = self.model.active_tab()
                && let Some(path) = c.path_to(p)
            {
                out.extend(path);
            }
            out.insert(p.clone());
        }
        out
    }

    /// The shown tab block's pane frame, if its panes are framed now: when
    /// the tab is split, or always with `lone`.
    fn pane_frame(&self) -> Option<&PaneFrame> {
        let frame = self.chrome.visible_blocks().into_iter().find_map(|n| match n.content() {
            Some(Content::Context { pane, .. }) => pane.as_ref(),
            _ => None,
        })?;
        let ctx = self.model.active_tab()?;
        (frame.lone || ctx.panes().len() > 1).then_some(frame)
    }

    /// A pane in its frame: the look, and the header mod above or below it
    /// (on hover: drawn over the terminal, so it never resizes).
    fn frame_pane(&self, n: &Node, f: &PaneFrame, on: bool, inner: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        let line = pane_size::frame_border(f, on);
        let group = SharedString::from(format!("pane-{}", n.id));
        let mut el = div().size_full().relative().flex().flex_col().overflow_hidden().group(group.clone());
        if let Some(r) = f.radius {
            el = el.rounded(settings::z(r));
        }
        if let Some(bg) = &f.background {
            el = paint(el, bg);
        }
        let width = line.map_or(0.0, |(width, _)| width);
        if let Some((_, color)) = line {
            el = el.border(settings::z(width)).border_color(c(color));
        }
        let pad = pane_size::frame_inset(f) - width;
        if pad > 0.0 {
            el = el.p(settings::z(pad));
        }
        el = shadow(el, f.shadow.as_deref());
        let body = div().flex_1().min_h_0().flex().child(inner);
        let Some(h) = &f.header else { return el.child(body).into_any_element() };
        let head = div().flex_none().h(settings::z(pane_size::header_height(h))).w_full().flex()
            .child(self.render_slot(&format!("{}#frame", n.id), cx));
        if h.hover() {
            // Absolute children sit against the padding box: inset by the pad so the header stays put as the focus moves.
            let at = settings::z(pad.max(0.0));
            let head = head.absolute().left(at).right(at).opacity(0.).group_hover(group, |s| s.opacity(1.));
            let head = if h.bottom() { head.bottom(at) } else { head.top(at) };
            return el.child(body).child(head).into_any_element();
        }
        if h.bottom() { el.child(body).child(head) } else { el.child(head).child(body) }.into_any_element()
    }

    /// `frame`: how panes are wrapped here (inside a framed tab), else None.
    fn render_node(&self, n: &Node, focused: &HashSet<String>, frame: Option<&PaneFrame>, cx: &mut Context<Self>) -> AnyElement {
        let mut el = div().relative().overflow_hidden().size_full().flex();
        let bounds_map = self.bounds.clone();
        let id = n.id.clone();
        // Only the main layout's bars are the window's chrome, not an overlay's.
        let main = self.painting_main.get();
        let (chrome, root, drag) = (self.top_chrome.clone(), main && n.id == self.chrome.id, main && n.window_drag);
        el = el.child(canvas(move |b, window, _| {
            bounds_map.borrow_mut().insert(id, b);
            if root || drag {
                chrome.set(top_chrome(chrome.get(), b, root, drag, window.viewport_size()));
            }
        }, |_, _, _, _| {}).absolute().size_full());
        el = apply_look(el, n, focused.contains(&n.id));
        if n.window_drag {
            el = self.window_drag(el, cx);
        }
        let el = match &n.kind {
            Kind::Block { content } => {
                let inner = self.render_block(n, content, focused, cx);
                match frame.filter(|_| n.is_pane()) {
                    Some(f) => el.child(self.frame_pane(n, f, focused.contains(&n.id), inner, cx)),
                    None => el.child(inner),
                }
            }
            Kind::Row { children } | Kind::Column { children } => {
                let axis = n.axis().unwrap();
                el = if axis == Axis::Row { el.flex_row() } else { el.flex_col() };
                let shown: Vec<&Node> = children.iter().filter(|c| !c.hidden).collect();
                let gap = pane_size::gap(n, frame);
                // Weights are shares of what the fixed-size siblings leave, so they always fill it.
                let total: f32 = shown
                    .iter()
                    .map(|c| pane_size::weight(c))
                    .sum::<f32>()
                    .max(f32::EPSILON);
                for (i, child) in shown.iter().enumerate() {
                    if i > 0 {
                        el = el.child(self.separator(n, axis, gap, i - 1, cx));
                    }
                    // No clipping here: a child's shadow may fall outside it (the child clips itself).
                    let mut slot = div().relative().flex().min_w_0().min_h_0();
                    slot = sized(slot, child, axis, total);
                    el = el.child(slot.child(self.render_node(child, focused, frame, cx)));
                }
                el
            }
            Kind::Stack { header, .. } => {
                el = el.flex_col();
                let head = header.as_ref().map(|h| {
                    let height = pane_size::header_height(h);
                    let head: AnyElement = match &h.content {
                        HeaderContent::Mod { .. } => self.render_slot(&format!("{}#header", n.id), cx),
                        HeaderContent::Builtin(_) => self.builtin_tabs(n, cx),
                    };
                    (h.bottom(), div().flex_none().h(settings::z(height)).w_full().flex().child(head))
                });
                let body = match n.active_child() {
                    Some(child) => self.render_node(child, focused, frame, cx),
                    None => div().into_any_element(),
                };
                let body = div().flex_1().min_h_0().flex().child(body);
                match head {
                    Some((true, head)) => el.child(body).child(head),
                    Some((false, head)) => el.child(head).child(body),
                    None => el.child(body),
                }
            }
        };
        let row = frame.and_then(|f| f.header.as_ref()).filter(|h| !h.bottom() && !h.hover()).map(pane_size::header_height);
        el.children(self.render_floats(n, cx)).children(self.pane_grip(&n.id, row, cx)).into_any_element()
    }

    fn render_block(&self, n: &Node, content: &Content, focused: &HashSet<String>, cx: &mut Context<Self>) -> AnyElement {
        match content {
            Content::Terminal { .. } => match self.pane_term(&n.id) {
                Some(t) => {
                    let id = n.id.clone();
                    div()
                        .size_full()
                        .on_any_mouse_down(cx.listener(move |this, _, _, cx| {
                            // Overlay terminals keep their own focus; chrome blocks share the pane focus API.
                            if this.focused_pane.as_deref() != Some(id.as_str())
                                && (this.model.tab_of(&id).is_some() || terminal_block(&this.chrome, Some(&id)).is_some())
                            {
                                this.model.focus_pane(&id);
                                this.focused_pane = Some(id.clone());
                                this.synced_model_pane = this.model.focused_pane();
                                this.publish(cx);
                                this.save();
                                cx.notify();
                            }
                        }))
                        .child(shown_terminal(&t.view, cx))
                        .into_any_element()
                }
                None => div().into_any_element(),
            },
            Content::Mod { .. } => self.render_slot(&n.id, cx),
            Content::Agent { .. } => match self.shown_agent() {
                Some(addr) => match self.agent_view(&n.id, &addr) {
                    Some(t) => div().size_full().child(shown_terminal(&t.view, cx)).into_any_element(),
                    None => div().into_any_element(),
                },
                None => div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(c("dim"))
                    .child("no agents yet. start claude, codex or pi in a terminal")
                    .into_any_element(),
            },
            Content::Context { .. } => match self.model.active_tab() {
                Some(c) => self.render_tab(c, focused, cx),
                None if self.documents.get("").is_some_and(|s| s.layout() == "loading") => div().size_full().into_any_element(),
                None => {
                    let key = crate::keys::listing().into_iter().find(|(_, _, a)| a == "space.new").map(|(_, k, _)| format!(" ({k})"));
                    div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(c("dim"))
                        .child(format!("No space open. Make one{}.", key.unwrap_or_default()))
                        .into_any_element()
                }
            },
        }
    }

    fn render_slot(&self, slot: &str, cx: &mut Context<Self>) -> AnyElement {
        let in_overlay = slot.starts_with("overlay:") || self.overlays.iter().any(|o| o.layout.as_ref().is_some_and(|l| l.find(slot).is_some()));
        // A mod that is a pane in a tab: a click makes it the focused pane, and the
        // keyboard is its own (an editor, a board), like a terminal's.
        let in_pane = self.model.tab_of(slot).is_some();
        match self.copies.get(slot) {
            Some(Copy { root: Some(root), inst, .. }) => div()
                .size_full()
                .key_context(format!("Mod mod = {}", inst.name).as_str())
                .when(in_pane, |d| {
                    let id = slot.to_owned();
                    d.on_any_mouse_down(cx.listener(move |this, _, _, cx| {
                        if this.focused_pane.as_deref() != Some(id.as_str()) {
                            this.model.focus_pane(&id);
                            this.focused_pane = Some(id.clone());
                            this.publish(cx);
                            this.save();
                            cx.notify();
                        }
                    }))
                })
                // Bare layout clicks leave the keyboard with the pane, but an input
                // or other focusable control owns the keyboard once it takes focus.
                .when(!in_overlay && !in_pane, |d| {
                    d.capture_any_mouse_down(cx.listener(|_, _, window, cx| {
                        cx.defer_in(window, |this, window, cx| this.refocus_pane(window, cx));
                    }))
                })
                // Popups size to their content, up to the window: a column, so a mod
                // root with `min_h(0)` can shrink and scroll. Layout mods fill their block.
                // Not cached here: ShellRoot caches the mod's view itself.
                .map(|d| if in_overlay { d.h_auto().min_h_0().flex().flex_col().child(root.clone()) } else { d.child(root.clone()) })
                .into_any_element(),
            Some(Copy { error, inst, .. }) => div()
                .id(SharedString::from(format!("err-{slot}")))
                .size_full()
                .p(settings::z(6.5))
                .text_size(settings::text_rems(0.75))
                .text_color(c("blocked"))
                .overflow_y_scroll()
                .child(format!("{} did not load:\n\n{}\n\nFix the files; Herdr retries on save.", inst.name, error.as_deref().unwrap_or("?")))
                .into_any_element(),
            None => div().into_any_element(),
        }
    }

    /// Plain fallback for `"header": { "content": "tabs" }`.
    fn builtin_tabs(&self, n: &Node, cx: &mut Context<Self>) -> AnyElement {
        let active = n.active_child().map(|c| c.id.clone());
        let stack = n.id.clone();
        let mut row = div().size_full().flex().flex_row().bg(c("surface")).text_size(settings::text_rems(0.75));
        for (i, ch) in n.children().iter().enumerate() {
            let (s, id) = (stack.clone(), ch.id.clone());
            let on = active.as_deref() == Some(ch.id.as_str());
            row = row.child(
                div()
                    .id(SharedString::from(format!("tab-{}", ch.id)))
                    .px(settings::z(9.75))
                    .flex()
                    .items_center()
                    .text_color(if on { c("foreground") } else { c("muted") })
                    .when(on, |d| d.bg(c("raised")))
                    .on_click(cx.listener(move |this, _, w, cx| this.run("layout.select", json!({ "stack": s, "child": id }), w, cx)))
                    .child(ch.title.clone().unwrap_or_else(|| (i + 1).to_string())),
            );
        }
        row.into_any_element()
    }

    fn separator(&self, n: &Node, axis: Axis, gap: f32, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let row = axis == Axis::Row;
        let zoom = settings::ui_scale();
        let line = n.divider.as_ref().map(|d| (d.width * zoom, c(&d.color)));
        let width = pane_size::separator_width(n, gap) * zoom;
        let mut sep = div().flex_none().relative().flex().items_center().justify_center();
        sep = if row { sep.w(px(width)).h_full() } else { sep.h(px(width)).w_full() };
        if let Some((w, color)) = line {
            let top = traffic_light_divider_inset(n, axis, index, cfg!(target_os = "macos"), zoom) * zoom;
            if top > 0.0
                && let Some(Paint::Color(background)) = n.children().iter().filter(|child| !child.hidden).nth(index).and_then(|child| child.background.as_ref())
            {
                sep = sep.bg(c(background));
            }
            sep = sep.child(separator_line(row, w, color, top));
        }
        if n.resizable() {
            let container = n.id.clone();
            let hit = HANDLE.max(width);
            sep = sep.child(
                div()
                    .absolute()
                    .when(row, |d| d.top_0().bottom_0().left(px((width - hit) / 2.0)).w(px(hit)).cursor_col_resize())
                    .when(!row, |d| d.left_0().right_0().top(px((width - hit) / 2.0)).h(px(hit)).cursor_row_resize())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            if let Some(space) = this.model.space_of(&container)
                                && this.require_document(space.machine.as_deref()).is_err() { return; }
                            this.drag = Some(Drag { container: container.clone(), index, axis });
                            cx.stop_propagation();
                        }),
                    ),
            );
        }
        sep.into_any_element()
    }

    fn drag_to(&mut self, pos: Point<Pixels>) {
        let Some(d) = &self.drag else { return };
        let (container, index, axis) = (d.container.clone(), d.index, d.axis);
        let Some(node) = self.find_node_mut(&container).map(|n| n.clone()) else { return };
        if !node.resizable() { return; }
        let shown: Vec<&Node> = node.children().iter().filter(|c| !c.hidden).collect();
        let (Some(a), Some(b)) = (shown.get(index), shown.get(index + 1)) else { return };
        let bounds = self.bounds.borrow();
        let (Some(ba), Some(bb)) = (bounds.get(&a.id).copied(), bounds.get(&b.id).copied()) else { return };
        drop(bounds);
        let (start, p) = match axis {
            Axis::Row => (f32::from(ba.left()), f32::from(pos.x)),
            Axis::Column => (f32::from(ba.top()), f32::from(pos.y)),
        };
        let (la, lb) = match axis {
            Axis::Row => (f32::from(ba.size.width), f32::from(bb.size.width)),
            Axis::Column => (f32::from(ba.size.height), f32::from(bb.size.height)),
        };
        // Measured on screen; sizes are stored as designed (unzoomed).
        let zoom = settings::ui_scale();
        let (la, lb, start, p) = (la / zoom, lb / zoom, start / zoom, p / zoom);
        let total = la + lb;
        // Squeezed and zero-size siblings cannot both keep the drag minimum.
        if total <= 48.0 { return; }
        let mut new_a = (p - start).clamp(24.0, total - 24.0);
        if let Some(min) = a.min {
            new_a = new_a.max(min);
        }
        if let Some(max) = a.max {
            new_a = new_a.min(max);
        }
        let new_a = new_a.min(total - 24.0);
        let (a_id, b_id) = (a.id.clone(), b.id.clone());
        let (sa, sb) = (a.size, b.size);
        let Some(parent) = self.find_node_mut(&container) else { return };
        let Some(children) = parent.children_mut() else { return };
        let set = |children: &mut Vec<Node>, id: &str, s: Span| {
            if let Some(n) = children.iter_mut().find(|n| n.id == id) {
                n.size = Some(s);
            }
        };
        match (sa, sb) {
            (Some(Span::Px { .. }), _) => set(children, &a_id, Span::Px { px: new_a.round() }),
            (_, Some(Span::Px { .. })) => set(children, &b_id, Span::Px { px: (total - new_a).round() }),
            _ => {
                let wa = match sa { Some(Span::Weight { weight }) => weight, _ => 1.0 };
                let wb = match sb { Some(Span::Weight { weight }) => weight, _ => 1.0 };
                let sum = wa + wb;
                set(children, &a_id, Span::Weight { weight: sum * new_a / total });
                set(children, &b_id, Span::Weight { weight: sum * (total - new_a) / total });
            }
        }
    }

    fn render_overlays(&self, viewport: Size<Pixels>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        let s = settings::get();
        let top = s.toast_position.top();
        // Drawn last, so it stays clickable over an open dialog.
        let pill = self.render_update_pill(cx);
        let chrome_px = self.top_chrome.get().1;
        let chrome = if top { chrome_px / settings::ui_scale() } else { 0.0 };
        // `top_chrome` is as the layout under the overlays placed its bars
        // this frame (it prepaints first); should a bar move after that, this
        // checks once every bar has painted and draws once more. Last of all;
        // `refresh` is ignored while painting.
        let probe = (top && (self.overlays.iter().any(|o| o.kind == OverlayKind::Toast) || crate::update::core().error.is_some())).then(|| {
            let painted = self.top_chrome.clone();
            canvas(move |_, window, _| {
                if painted.get().1 != chrome_px {
                    window.on_next_frame(|window, _| window.refresh());
                }
            }, |_, _, _, _| {}).absolute().size_0().into_any_element()
        });
        let design = s.toast_design;
        let banner = design == settings::ToastDesign::Banner;
        let start = if banner { banner_start(chrome, s.toast_offset.y, pill.is_some() && !top) } else { toast_start(chrome, s.toast_offset.y, pill.is_some() && !top && s.toast_position.side() != settings::ToastSide::Left) };
        let toasts: Vec<&Overlay> = self.overlays.iter().filter(|o| o.kind == OverlayKind::Toast).collect();
        // Banners show one at a time: the newest, which counts the rest.
        let shown_banner = self.shown_banner().map(|o| o.id.clone());
        let heights: Vec<f32> = toasts.iter().filter(|o| shown_banner.as_ref().is_none_or(|id| *id == o.id)).map(|o| o.size.height).collect();
        let (spots, past) = if banner { (vec![start; heights.len()], start + heights.first().map_or(0.0, |h| h + TOAST_GAP)) } else { toast_stack(&heights, start, top) };
        let mut spots = spots.into_iter();
        let place = |el: AnyElement, along: f32| if banner { at_banner_spot(el, top, along) } else { at_toast_spot(el, &s, along) };
        // Windows under dialogs, popups and toasts; each kind in the order opened
        // (a window pressed last is last: on top).
        let order = self.overlays.iter().filter(|o| o.kind == OverlayKind::Window).chain(self.overlays.iter().filter(|o| o.kind != OverlayKind::Window));
        for o in order {
            if o.kind == OverlayKind::Toast && shown_banner.as_ref().is_some_and(|id| *id != o.id) {
                continue;
            }
            let id = o.id.clone();
            // Up to 1: a share of the window (terminals in overlays want room, not px).
            let span = |v: f32, full: Pixels| if v <= 1.0 { full * v } else { settings::z(v) };
            let (w, h) = (span(o.size.width, viewport.width), span(o.size.height, viewport.height));
            let sized = |v: f32| v > 0.0 || !matches!(o.kind, OverlayKind::Window | OverlayKind::Toast);
            let content = match &o.layout {
                Some(l) => self.render_node(l, &HashSet::new(), None, cx),
                None => self.render_slot(&o.slot, cx),
            };
            let body = div()
                .id(SharedString::from(format!("ov-{}", o.id)))
                .track_focus(&o.focus)
                // Esc stays with terminals (agents use it to interrupt) and windows.
                .when(o.layout.is_none() && o.kind.modal_keys(), |d| d.key_context("Overlay"))
                .when(sized(o.size.width), |d| d.w(w))
                .when(o.kind == OverlayKind::Toast && !banner, |d| d.min_w(settings::z(o.widths.0)).max_w(settings::z(o.widths.1)))
                .when(o.kind == OverlayKind::Toast && banner, |d| d.w_full())
                .debug_selector(|| format!("overlay-{id}"))
                .when(sized(o.size.height), |d| if o.fit { d.max_h(h) } else { d.h(h) })
                .when(!o.bare && o.kind != OverlayKind::Toast, |d| d.bg(c("popover")).border_1().border_color(c("border")).rounded(settings::z(8.0)).shadow_lg())
                .when(!o.bare && o.kind == OverlayKind::Toast, |d| toast_frame(d, design, s.toast_corners, o.level, top))
                .overflow_hidden()
                .flex()
                // Scroll, hover and cursor stop here, not on the pane behind.
                .occlude()
                .when(o.kind != OverlayKind::Window, |d| d.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()))
                .child(content)
                .when_some((s.toast_progress && o.kind == OverlayKind::Toast).then(|| self.time_left(o)).flatten(), |d, left| {
                    // Inset from the sides by the corner radius, so the frame's rounded clip never cuts it.
                    let inset = settings::z(design.radius(s.toast_corners).min(o.size.height / 2.0));
                    d.child(
                        div().absolute().left(inset).right(inset).bottom_0().h(settings::z(PROGRESS_HEIGHT))
                            .child(div().h_full().w(relative(left)).bg(c(o.level)).debug_selector(|| format!("progress-{id}"))),
                    )
                });
            match o.kind {
                OverlayKind::Window => out.push(self.render_window(o, body, viewport, cx)),
                OverlayKind::Dialog => out.push(
                    div()
                        .absolute()
                        .inset_0()
                        .bg(hsla(0.0, 0.0, 0.0, 0.35))
                        .occlude()
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(!o.modal, |d| d.on_mouse_down(MouseButton::Left, cx.listener(move |this, _, w, cx| this.close_overlay(&id, w, cx))))
                        .child(body)
                        .into_any_element(),
                ),
                OverlayKind::Popup => {
                    let at = match o.top {
                        // Centered, hanging from `offset` px below the top (a drop-down from a bar).
                        Some(off) => point((viewport.width - w) / 2.0, settings::z(off)),
                        None => o.at,
                    };
                    out.push(
                        div()
                            .absolute()
                            .inset_0()
                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, w, cx| this.close_overlay(&id, w, cx)))
                            .on_mouse_down(MouseButton::Right, cx.listener({
                                let id = o.id.clone();
                                move |this, _, w, cx| this.close_overlay(&id, w, cx)
                            }))
                            .child(position_popup(at, body))
                            .into_any_element(),
                    );
                }
                OverlayKind::Toast => out.push(place(body.into_any_element(), spots.next().unwrap_or(past))),
            }
        }
        // Past the toasts, on their edge: it stays until an update goes through.
        if let Some(error) = crate::update::core().error {
            let terminal = design == settings::ToastDesign::Terminal;
            let mark = design.mark(s.toast_mark);
            let notice = toast_frame(div().when(banner, |d| d.w_full()).when(!banner, |d| d.max_w(settings::z(540.0))).flex().overflow_hidden(), design, s.toast_corners, "toast_error", top)
                .text_size(settings::zt(13.0)).text_color(c("toast_foreground"))
                .when(terminal, |d| d.font_family(s.font_family.clone()))
                .id("core-update-error").cursor_pointer()
                .on_click(cx.listener(|this, _, w, cx| this.run("app.settings", json!({ "section": "updates" }), w, cx)))
                .when(mark == settings::ToastMark::Stripe, |d| d.child(div().w(settings::z(TOAST_STRIPE)).flex_none().bg(c("toast_error"))))
                .when(mark == settings::ToastMark::Dot, |d| {
                    d.child(div().pl(settings::z(12.0)).flex().items_center().child(div().size(settings::z(8.0)).rounded(settings::z(4.0)).bg(c("toast_error"))))
                })
                .when(terminal, |d| d.child(div().pl(settings::z(12.0)).flex().items_center().text_color(c("toast_error")).child("[ERR]")))
                .child(div().p(settings::z(12.0)).child(format!("herdr-core update failed: {error} · Open Updates")));
            out.push(place(notice.into_any_element(), past));
        }
        out.extend(pill);
        out.extend(probe);
        out
    }
}

const UPDATE_PILL_HEIGHT: f32 = 34.0;

/// A toast stack's distance from the window's edges, and between its toasts (px at zoom 1).
const TOAST_MARGIN: f32 = 16.0;
const TOAST_GAP: f32 = 8.0;
/// The level stripe down a toast's left side, as the toast mod draws it (px at zoom 1).
const TOAST_STRIPE: f32 = 3.0;

/// The theme colours a toast's box draws in `design`, for a toast of
/// `level` (`toast_level`): its fill, its border and its shadow, if any.
/// All are `toast_colors`' (settings.rs), so they follow the theme.
fn toast_frame_colors(design: settings::ToastDesign, level: &'static str) -> (&'static str, &'static str, Option<&'static str>) {
    match design {
        // See-through, held to its floors over anything (`toast_ghost`), edged in its level.
        settings::ToastDesign::Outline => ("toast_ghost", level, None),
        _ => ("toast", "toast_border", Some("toast_shadow")),
    }
}

/// A toast's box: its own lifted surface, border and shadow (`toast_colors`
/// in settings.rs), so it stands off whatever it's over on every theme, with
/// the person's `toast_corners` where `design` takes them. A banner's border
/// is the edge it faces the window across (`top`: its bottom).
fn toast_frame<E: Styled>(e: E, design: settings::ToastDesign, corners: settings::ToastCorners, level: &'static str, top: bool) -> E {
    let (fill, edge, shadow) = toast_frame_colors(design, level);
    let e = e.bg(c(fill)).border_color(c(edge)).rounded(settings::z(design.radius(corners)));
    let e = match design {
        settings::ToastDesign::Banner if top => e.border_b_1(),
        settings::ToastDesign::Banner => e.border_t_1(),
        _ => e.border_1(),
    };
    let Some(shadow) = shadow.map(c) else { return e };
    let near = Hsla { a: shadow.a * 0.6, ..shadow };
    e.shadow(vec![
        BoxShadow { color: shadow, offset: point(px(0.0), settings::z(8.0)), blur_radius: settings::z(24.0), spread_radius: px(0.0), inset: false },
        BoxShadow { color: near, offset: point(px(0.0), settings::z(1.0)), blur_radius: settings::z(3.0), spread_radius: px(0.0), inset: false },
    ])
}

/// The window's top chrome as painted so far this frame, `(top, bottom)` in
/// window px, with one more node's painted bounds: the root starts it where
/// the layout begins (below a system title strip the window draws), and a
/// `window_drag` node starting within it (a title, tab or brand bar, or a tab
/// bar under a title bar: nodes paint parents first, in order) takes its
/// bottom past its own. A drag node a third of the window or taller is a
/// panel, not a bar. Hidden nodes aren't painted, so they never count.
fn top_chrome(chrome: (f32, f32), b: Bounds<Pixels>, root: bool, drag: bool, viewport: Size<Pixels>) -> (f32, f32) {
    let bottom = chrome.1;
    if root {
        let t = f32::from(b.top());
        return (t, t);
    }
    if drag && f32::from(b.top()) <= bottom + 1.0 && b.size.height < viewport.height / 3.0 {
        return (chrome.0, bottom.max(f32::from(b.bottom())));
    }
    chrome
}

/// Where a toast stack starts (px at zoom 1 from its edge): past `chrome`
/// (a top stack's title and tab bars, `top_chrome`), the margin moved by
/// `toast_offset.y`, the window's edge at most, so the whole stack moves
/// together. Under the update pill (it holds the bottom-right corner, and a
/// centred stack can reach it), it starts above the pill whatever the offset.
fn toast_start(chrome: f32, offset_y: f32, above_pill: bool) -> f32 {
    let start = (chrome + TOAST_MARGIN + offset_y).max(0.0);
    if above_pill { (start + UPDATE_PILL_HEIGHT + TOAST_GAP).max(TOAST_MARGIN + UPDATE_PILL_HEIGHT + TOAST_GAP) } else { start }
}

/// Where a banner sits (px at zoom 1 from its edge): flush with it, past a
/// top banner's chrome, moved in by `toast_offset.y`; above the update pill
/// (a bottom banner spans it) whatever the offset.
fn banner_start(chrome: f32, offset_y: f32, above_pill: bool) -> f32 {
    let start = (chrome + offset_y).max(0.0);
    if above_pill { start.max(TOAST_MARGIN + UPDATE_PILL_HEIGHT + TOAST_GAP) } else { start }
}

/// `el` across the window, `along` px (at zoom 1) from its top or bottom edge.
fn at_banner_spot(el: AnyElement, top: bool, along: f32) -> AnyElement {
    let spot = div().absolute().left_0().right_0();
    let along = settings::z(along);
    let spot = if top { spot.top(along) } else { spot.bottom(along) };
    spot.child(el).into_any_element()
}

/// Each toast's distance (px at zoom 1) from its stack's edge, for `heights`
/// in the order opened: the nearest `start` from the edge, then `TOAST_GAP`
/// apart. A bottom stack keeps its oldest nearest the edge, a top stack its
/// newest. Also where one more would go.
fn toast_stack(heights: &[f32], start: f32, top: bool) -> (Vec<f32>, f32) {
    let mut at = vec![0.0; heights.len()];
    let mut along = start;
    let mut place = |i: usize| {
        at[i] = along;
        along += heights[i] + TOAST_GAP;
    };
    if top { (0..heights.len()).rev().for_each(&mut place) } else { (0..heights.len()).for_each(&mut place) }
    (at, along)
}

/// `el` where `toast_position` puts a toast, `along` px (at zoom 1) from its
/// top or bottom edge (`toast_start` has applied `toast_offset.y`), its side
/// moved in by `toast_offset.x`.
fn at_toast_spot(el: AnyElement, s: &settings::Settings, along: f32) -> AnyElement {
    use settings::ToastSide;
    let position = s.toast_position;
    let side = settings::z((TOAST_MARGIN + s.toast_offset.x).max(0.0));
    let along = settings::z(along);
    let spot = match position.side() {
        ToastSide::Left => div().absolute().left(side),
        ToastSide::Right => div().absolute().right(side),
        // Full width, so the toast centres without knowing its own width.
        ToastSide::Center => div().absolute().left_0().right_0().flex().justify_center(),
    };
    let spot = if position.top() { spot.top(along) } else { spot.bottom(along) };
    spot.child(el).into_any_element()
}

impl AppWindow {
    /// "Herdr 0.2.0 is ready · Restart": the frame's, so no layout or mod can hide it.
    /// What's waiting, in words: the pill shows it until clicked or dismissed.
    fn update_label() -> Option<String> {
        let core = crate::update::core();
        let core = core.ready.filter(|_| !core.updating);
        match (crate::update::ready(), core) {
            (Some(a), Some(c)) => Some(format!("Herdr {a} and herdr-core {c} are ready")),
            (Some(a), None) => Some(format!("Herdr {a} is ready")),
            (None, Some(c)) => Some(format!("herdr-core {c} is ready")),
            (None, None) => None,
        }
    }

    fn render_update_pill(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let app_version = crate::update::ready();
        let app_ready = app_version.is_some();
        let success = self.update_ready.is_none() && self.update_success;
        let label = self.update_ready.clone().or_else(|| success.then(|| format!("Updated to {}", env!("CARGO_PKG_VERSION"))))?;
        let dismissal = app_version.map(|v| format!("app:{v}")).unwrap_or_else(|| label.clone());
        if self.update_dismissed.as_ref() == Some(&dismissal) { return None; }
        let pill = div()
            .absolute()
            .right(settings::z(16.0))
            .bottom(settings::z(16.0))
            .h(settings::z(UPDATE_PILL_HEIGHT))
            .pl(settings::z(14.0))
            .pr(settings::z(6.0))
            .flex()
            .items_center()
            .gap(settings::z(10.0))
            .bg(c("popover"))
            .border_1()
            .border_color(c("border"))
            .rounded_full()
            .shadow_lg()
            .occlude()
            .text_size(settings::zt(13.0))
            .child(div().text_color(c("foreground")).child(label.clone()))
            .when(app_ready, |pill| pill.child(
                div()
                    .id("update-restart")
                    .h(settings::z(24.0))
                    .px(settings::z(10.0))
                    .flex()
                    .items_center()
                    .rounded_full()
                    .cursor_pointer()
                    .bg(c("accent"))
                    .text_color(c("accent_foreground"))
                    .font_weight(FontWeight::SEMIBOLD)
                    .hover(|s| s.opacity(0.9))
                    .child("Restart")
                    .on_click(cx.listener(|this, _, w, cx| this.run("app.update", json!({}), w, cx))),
            ))
            .child(div().id("update-notes").px(settings::z(6.0)).cursor_pointer().text_color(c("accent"))
                .child(if app_ready || success { "What changed" } else { "Open Updates" })
                .on_click(cx.listener(move |this, _, w, cx| {
                    if success { this.update_success = false; }
                    this.run("app.settings", json!({ "section": "updates" }), w, cx);
                })))
            .child(
                div()
                    .id("update-dismiss")
                    .size(settings::z(24.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor_pointer()
                    .text_color(c("dim"))
                    .hover(|s| s.bg(c("raised")).text_color(c("foreground")))
                    .child("×")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.update_dismissed = Some(dismissal.clone());
                        cx.notify();
                    })),
            );
        Some(pill.into_any_element())
    }
}

/// A view drawn again only when it changed: a terminal's output then
/// redraws that terminal, not every mod and pane in the window.
pub(crate) fn cached<V: Render>(view: &Entity<V>) -> AnyElement {
    view.clone().cached(StyleRefinement::default().size_full()).into_any_element()
}

pub(crate) fn shown_terminal(view: &Entity<TerminalView>, cx: &App) -> AnyElement {
    view.read(cx).mark_shown();
    cached(view)
}

fn remember_pane(history: &mut Vec<String>, id: &str) {
    if history.last().is_some_and(|last| last == id) {
        return;
    }
    history.retain(|pane| pane != id);
    history.push(id.to_owned());
}

/// Prefer the most recently focused pane along the nearest edge. Unvisited
/// panes fall back to overlap and layout order, ignoring subpixel split noise.
fn nearest(cur: Bounds<Pixels>, dir: &str, candidates: impl IntoIterator<Item = (String, Bounds<Pixels>)>, history: &[String]) -> Option<String> {
    let overlap = |a0: Pixels, a1: Pixels, b0: Pixels, b1: Pixels| f32::from(a1.min(b1) - a0.max(b0)).max(0.0);
    let cmp = |a: f32, b: f32| if (a - b).abs() < 1.0 { std::cmp::Ordering::Equal } else { a.total_cmp(&b) };
    candidates
        .into_iter()
        .filter_map(|(id, r)| {
            let (gap, ov) = match dir {
                "left" => (f32::from(cur.left() - r.right()), overlap(cur.top(), cur.bottom(), r.top(), r.bottom())),
                "right" => (f32::from(r.left() - cur.right()), overlap(cur.top(), cur.bottom(), r.top(), r.bottom())),
                "up" => (f32::from(cur.top() - r.bottom()), overlap(cur.left(), cur.right(), r.left(), r.right())),
                _ => (f32::from(r.top() - cur.bottom()), overlap(cur.left(), cur.right(), r.left(), r.right())),
            };
            let recent = history.iter().position(|pane| pane == &id);
            (gap > -2.0 && ov > 0.0).then_some((gap, recent, ov, id))
        })
        .reduce(|best, next| if cmp(next.0, best.0).then(best.1.cmp(&next.1)).then(cmp(best.2, next.2)).is_lt() { next } else { best })
        .map(|(_, _, _, id)| id)
}

/// How close a popup comes to the window's edges, px.
const POPUP_MARGIN: f32 = 8.0;

fn position_popup(at: Point<Pixels>, body: Stateful<Div>) -> Anchored {
    anchored()
        .position(point(at.x.max(px(POPUP_MARGIN)), at.y.max(px(POPUP_MARGIN))))
        .snap_to_window_with_margin(px(POPUP_MARGIN))
        .child(body)
}

/// An overlay's blocks get ids of their own (`<overlay>/<id>`), and its mods learn the overlay.
fn overlay_layout(mut n: Node, overlay: &str) -> Node {
    fn walk(n: &mut Node, overlay: &str, seq: &mut usize) {
        *seq += 1;
        n.id = if n.id.is_empty() { format!("ov{overlay}/{seq}") } else { format!("ov{overlay}/{}", n.id) };
        if let Kind::Block { content: Content::Mod { props, .. } } = &mut n.kind {
            if !props.is_object() {
                *props = json!({});
            }
            props["overlay"] = json!(overlay);
        }
        for c in n.children_mut().into_iter().flatten() {
            walk(c, overlay, seq);
        }
    }
    walk(&mut n, overlay, &mut 0);
    n
}

/// An agent working, or a program other than the shell in front (an agent
/// between turns isn't, even started from a shell).
fn busy(rec: &Value) -> bool {
    let program = |k: &str| rec["program"][k].as_bool() == Some(true);
    rec["status"] == "working" || !(program("shell") || program("agent"))
}

/// Set once the app starts quitting, the OS ending the session included: the
/// saved layout stays as it was.
pub static QUITTING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A folder's checkout and branch from its git status (`None` while unknown):
/// not a repo, or git couldn't say, is the folder itself.
/// `counts`: with ahead, behind and dirty (only `pane_header_info` shows them,
/// so without it a changed file doesn't redraw).
fn folder_info(status: &Value, cwd: &str, counts: bool) -> Option<model::FolderInfo> {
    match status {
        Value::Null => None,
        Value::Object(o) => {
            let n = |k: &str| o.get(k).and_then(Value::as_u64).filter(|_| counts).unwrap_or(0);
            Some(model::FolderInfo {
                top: o.get("root").and_then(Value::as_str).unwrap_or(cwd).to_owned(),
                branch: o.get("branch").and_then(Value::as_str).map(str::to_owned),
                ahead: n("ahead"),
                behind: n("behind"),
                dirty: ["staged", "unstaged", "untracked", "conflicted"].iter().any(|k| n(k) > 0),
            })
        }
        _ => Some(model::FolderInfo { top: cwd.into(), ..Default::default() }),
    }
}

fn terminal_block<'a>(chrome: &'a Node, focused: Option<&str>) -> Option<&'a Node> {
    let id = focused?;
    chrome.visible_blocks().into_iter().find(|n| n.id == id && n.terminal().is_some())
}

/// The terminal block `id` in a window's layout (an overlay with blocks).
fn overlay_terminal_block<'a>(overlays: &'a [Overlay], id: Option<&str>) -> Option<&'a Node> {
    let id = id?;
    overlays.iter().filter_map(|o| o.layout.as_ref()).find_map(|l| l.visible_blocks().into_iter().find(|n| n.id == id && n.terminal().is_some()))
}

#[derive(Debug, PartialEq)]
enum SwapBy<'a> {
    Direction(&'a str),
    Target(&'a str),
}

fn swap_with_neighbor(model: &mut Model, from: &str, explicit: bool, neighbor: Option<String>) -> Result<(), String> {
    if explicit && model.tab_of(from).and_then(|c| c.pane(from)).is_none() {
        return Err(format!("no pane {from:?}"));
    }
    if model.tab_of(from).is_some_and(|c| c.float(from).is_some()) {
        return Err("pane.swap direction moves tiled panes: move a floating one with pane.float_move, or swap it with pane.swap target".into());
    }
    let shown = model.active_tab().is_some_and(|c| c.shown().visible_blocks().iter().any(|n| n.id == from));
    if explicit && !shown {
        return Err(format!("pane.swap direction needs a pane in the shown tab, not {from:?}"));
    }
    if let Some(to) = neighbor
        && let Some(ctx) = model.tab_of(from).map(|c| c.id.clone())
        && let Some(c) = model.tab_mut(&ctx)
    {
        c.root.swap(from, &to);
    }
    Ok(())
}

fn swap_by(args: &Value) -> Result<SwapBy<'_>, String> {
    if args.get("direction").is_some_and(|d| !d.is_string() && !d.is_null()) {
        return Err("pane.swap direction must be a string (or null to omit it)".into());
    }
    match (AppWindow::arg(args, "direction"), AppWindow::arg(args, "target")) {
        (Some(_), Some(_)) => Err("pane.swap takes direction or target, not both".into()),
        (None, None) => Err("pane.swap needs direction or target".into()),
        (None, Some(target)) => Ok(SwapBy::Target(target)),
        (Some(dir @ ("left" | "right" | "up" | "down")), None) => Ok(SwapBy::Direction(dir)),
        (Some(dir), None) => Err(format!("pane.swap direction is left, right, up or down, not {dir:?}")),
    }
}

fn needs_model_pane(name: &str, args: &Value) -> bool {
    match name {
        "pane.split" | "pane.close" | "pane.rename" | "pane.set_icon" | "pane.zoom" | "pane.swap" | "pane.menu" | "terminal.show" => AppWindow::arg(args, "pane").is_none(),
        name if pane_floats::COMMANDS.contains(&name) => AppWindow::arg(args, "pane").is_none(),
        "pane.go" => true,
        "terminal.open" | "terminal.keep" => matches!(AppWindow::arg(args, "place"), Some("right" | "down")),
        _ => false,
    }
}

/// A failed implicit folder gets one retry in the machine's home, without local probes.
fn run_following_folder(mut folder: String, follow: bool, on: Option<&str>, mut run: impl FnMut(Option<String>) -> anyhow::Result<String>) -> anyhow::Result<(String, String)> {
    let result = run(Some(folder.clone()).filter(|f| !f.is_empty()));
    let address = match result {
        // A missing folder and a missing program both report spawn_failed. The holder's
        // timeout may still create a terminal, so never retry that ambiguous outcome.
        Err(e) if follow && !folder.is_empty() && crate::hc::error_kind(&e) == Some("spawn_failed") && !format!("{e:#}").contains("the holder did not answer") => {
            folder = if on.is_some() { String::new() } else { home() };
            run(Some(folder.clone()).filter(|f| !f.is_empty()))?
        }
        result => result?,
    };
    Ok((address, folder))
}

fn new_space_machine(args: &Value, person: bool, model: &Model) -> Option<String> {
    args.get("machine").and_then(Value::as_str)
        .or_else(|| if person { model.active_space().and_then(|s| s.machine.as_deref()) } else { None })
        .filter(|m| !m.is_empty()).map(str::to_owned)
}

/// Where the resumable programs are recorded (resume.rs).
fn resume_path() -> PathBuf {
    file("data/resume.json")
}

/// "Claude Code in space / tab, Codex in t4 and 2 more".
fn resumed_names(records: &[crate::resume::Record]) -> String {
    let mut names: Vec<String> = records.iter().take(3).map(|r| format!("{} in {}", r.name, r.place.as_deref().unwrap_or(&r.terminal))).collect();
    if records.len() > 3 {
        names.push(format!("{} more", records.len() - 3));
    }
    names.join(", ")
}

fn home() -> String {
    mods::home().to_string_lossy().into_owned()
}

/// Every mod folder by id, with their palette actions and terminal menu items
/// published for `modActions()` and `menuItems()`; and whether either changed.
fn scan_folders(scanned: Vec<Found>) -> (BTreeMap<String, Found>, bool) {
    let found: BTreeMap<String, Found> = scanned.into_iter().map(|f| (f.id.clone(), f)).collect();
    let actions = mods::publish_actions(Value::Array(mods::actions(found.values()).0));
    let items = mods::publish_menu_items(Value::Array(mods::menu_items(found.values()).0));
    (found, actions || items)
}

fn mod_overlay(dir: &std::path::Path) -> Value {
    let meta: Value = std::fs::read_to_string(dir.join("mod.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    meta.get("overlay").cloned().filter(Value::is_object).unwrap_or_else(|| json!({}))
}

/// More lines than any terminal keeps: `terminal.read` gives what it has.
/// Not `usize::MAX`: the core adds the screen's rows to it.
const ALL_LINES: u64 = 1_000_000_000;

fn copy_all_params(address: &str) -> Value {
    json!({ "terminal": address, "lines": ALL_LINES })
}

/// How long `terminal.copy_all` waits for the core: past its own stitch
/// deadline (15 s by default).
const COPY_ALL_READ_LIMIT: Duration = Duration::from_secs(30);

/// What `terminal.copy_all` puts on the clipboard, and its line count; `None`
/// when there's no text. The rows below the last text are blank screen, not output.
fn copied_text(read: &str) -> Option<(String, usize)> {
    let mut lines: Vec<&str> = read.lines().collect();
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    (!lines.is_empty()).then(|| (lines.join("\n"), lines.len()))
}

/// 1234 as "1,234".
fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, d) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(d);
    }
    out
}

/// `viewport` is the window's height in px and `scale` the panels' zoom
/// (`settings::ui_scale`): the menu gets `props.height`, the room inside the
/// popup it can scroll its items in, in its own px.
fn terminal_menu_spec(name: &str, defaults: &Value, tab: &str, target: Value, viewport: f32, scale: f32) -> Value {
    // The most: a menu is as tall as its items.
    let height = defaults.get("height").and_then(Value::as_f64).unwrap_or(420.0);
    let (viewport, scale) = (f64::from(viewport), f64::from(scale.max(0.01)));
    // Up to 1: a share of the window, as overlays size it.
    let most = if height > 1.0 { height } else { height * viewport / scale };
    // The popup snaps inside the window's margins and draws a 1 px border inside its height.
    let fits = (viewport - 2.0 * f64::from(POPUP_MARGIN)) / scale;
    let room = most.min(fits) - 2.0 / scale;
    json!({ "mod": name, "kind": "popup",
        "width": defaults.get("width").and_then(Value::as_f64).unwrap_or(220.0),
        "height": height,
        "props": { "tab": tab, "target": target, "height": room.max(0.0) } })
}

fn num_in(v: &Value, k: &str) -> f32 {
    v.get(k).and_then(Value::as_f64).unwrap_or(0.0) as f32
}

/// A floating mod's slot and copy (it's told its float id). `visible` and
/// `owned_mods` must build the same, or a kept copy is dropped.
fn float_mod(id: &str, name: &str, props: &Value) -> (String, Instance) {
    let mut props = if props.is_object() { props.clone() } else { json!({}) };
    props["float"] = json!(id);
    (floats::slot(id), Instance { name: name.to_owned(), props })
}

/// A pane header's slot and copy, told which pane it sits on (a mod pane has no terminal).
fn pane_header_mod(n: &Node, name: &str, props: &Value) -> (String, Instance) {
    let mut props = if props.is_object() { props.clone() } else { json!({}) };
    props["pane"] = json!(n.id);
    props["terminal"] = json!(n.terminal());
    (format!("{}#frame", n.id), Instance { name: name.to_owned(), props })
}

/// What became of a change to settings.json made from inside the app.
#[derive(Clone, Copy, Debug, PartialEq)]
enum PersonSaved {
    Saved,
    /// settings.json doesn't parse, so it was left alone.
    Broken,
    WriteFailed,
}

/// Applies `change` to `person`, then saves it with `save` unless settings.json
/// is `broken` (`AppWindow::write_person`).
fn change_person(person: &mut settings::Person, change: impl FnOnce(&mut settings::Person), broken: bool, save: impl FnOnce(&settings::Person) -> std::io::Result<()>) -> PersonSaved {
    change(person);
    if broken {
        return PersonSaved::Broken;
    }
    match save(person) {
        Ok(()) => PersonSaved::Saved,
        Err(e) => {
            tracing::warn!("cannot write settings.json: {e}");
            PersonSaved::WriteFailed
        }
    }
}

/// Every node shown on screen (hidden ones and inactive stack children skipped).
fn visible_nodes(n: &Node) -> Vec<&Node> {
    if n.hidden {
        return vec![];
    }
    let mut out = vec![n];
    match &n.kind {
        Kind::Stack { .. } => {
            if let Some(c) = n.active_child() {
                out.extend(visible_nodes(c));
            }
        }
        _ => {
            for c in n.children() {
                out.extend(visible_nodes(c));
            }
        }
    }
    out
}

fn fill_ids(n: &mut Node, m: &mut Model) {
    n.fill_ids(&mut || m.fresh_id("n"));
}

/// Puts a block along one side of the chrome.
fn dock(root: &mut Node, block: Node, place: &str, wrapper_id: String) {
    let axis = if matches!(place, "top" | "bottom") { Axis::Column } else { Axis::Row };
    let first = matches!(place, "left" | "top");
    if root.axis() == Some(axis) {
        let c = root.children_mut().unwrap();
        if first { c.insert(0, block) } else { c.push(block) }
        return;
    }
    let mut old = std::mem::replace(root, Node::new("", Kind::Row { children: vec![] }));
    let toast = old.toast.take();
    let terminal_menu = old.terminal_menu.take();
    let children = if first { vec![block, old] } else { vec![old, block] };
    let mut wrapper = Node::container(wrapper_id, axis, children);
    wrapper.toast = toast;
    wrapper.terminal_menu = terminal_menu;
    wrapper.divider = Some(Line { width: 1.0, color: "border".into() });
    *root = wrapper;
}

fn sized(slot: Div, n: &Node, axis: Axis, total_weight: f32) -> Div {
    let mut slot = slot;
    let row = axis == Axis::Row;
    match n.size {
        Some(Span::Px { px: v }) => {
            slot = slot.flex_none();
            slot = if row { slot.w(settings::z(v)).h_full() } else { slot.h(settings::z(v)).w_full() };
        }
        _ => {
            let w = pane_size::weight(n);
            let s = slot.style();
            s.flex_grow = Some(w / total_weight);
            s.flex_shrink = Some(1.0);
            s.flex_basis = Some(relative(0.).into());
            slot = if row { slot.h_full() } else { slot.w_full() };
        }
    }
    if let Some(min) = n.min {
        slot = if row { slot.min_w(settings::z(min)) } else { slot.min_h(settings::z(min)) };
    }
    if let Some(max) = n.max {
        slot = if row { slot.max_w(settings::z(max)) } else { slot.max_h(settings::z(max)) };
    }
    slot
}

/// What a terminal runs now (core's `foreground`), else what it started with.
fn running_argv(rec: &serde_json::Map<String, Value>) -> Option<&Value> {
    rec.get("foreground").and_then(|f| f.get("argv")).filter(|a| a.as_array().is_some_and(|a| !a.is_empty())).or_else(|| rec.get("argv"))
}

/// Whether the core's record still puts a terminal in this space and tab and
/// it still runs. A listing taken just before it ended still shows it running,
/// and labels outlive the program: putting it back would show a dead pane.
fn adoptable(record: &Value, space: &str, tab: Option<&str>, machine: Option<&str>) -> bool {
    let label = record.pointer("/labels/herdr.space").and_then(Value::as_str);
    let labeled_tab = record.pointer("/labels/herdr.tab").and_then(Value::as_str).filter(|s| !s.is_empty());
    record["state"] == "live"
        && label == Some(crate::mirror::local(space, machine))
        && labeled_tab == tab.map(|t| crate::mirror::local(t, machine))
}

fn program_of(report: &Value, argv: Option<&Value>) -> Value {
    let app = report.get("app").and_then(Value::as_str);
    let argv: Vec<String> = argv.and_then(Value::as_array).into_iter().flatten().filter_map(|a| a.as_str().map(str::to_owned)).collect();
    crate::programs::of(app, &argv)
}

fn apply_look(el: Div, n: &Node, focused: bool) -> Div {
    let mut el = el;
    let focus = n.focus.as_ref().filter(|_| focused);
    if let Some(bg) = focus.and_then(|f| f.background.as_ref()) {
        el = el.bg(c(bg));
    } else if let Some(bg) = &n.background {
        el = paint(el, bg);
    }
    if let Some(line) = pane_size::node_border(n, focused) {
        el = el.border(settings::z(line.width)).border_color(c(&line.color));
    }
    if let Some(r) = n.radius {
        el = el.rounded(settings::z(r));
    }
    if n.padding.is_some() {
        let (y, x) = pane_size::padding(n);
        el = el.py(settings::z(y)).px(settings::z(x));
    }
    shadow(el, n.shadow.as_deref())
}

/// A color or gradient paints the element; an image is a layer behind
/// whatever comes after it (call before adding content).
fn paint(el: Div, bg: &Paint) -> Div {
    match bg {
        Paint::Color(name) => el.bg(c(name)),
        Paint::Gradient { gradient: [a, b], angle } => el.bg(linear_gradient(*angle, linear_color_stop(c(a), 0.), linear_color_stop(c(b), 1.))),
        Paint::Image { image, fit } => {
            let fit = match fit {
                Fit::Cover => ObjectFit::Cover,
                Fit::Contain => ObjectFit::Contain,
            };
            el.child(img(mods::app_dir().join(image)).absolute().top_0().left_0().size_full().object_fit(fit))
        }
    }
}

fn shadow(el: Div, s: Option<&str>) -> Div {
    match s {
        Some("sm") => el.shadow_sm(),
        Some("md") => el.shadow_md(),
        Some("lg") => el.shadow_lg(),
        _ => el,
    }
}

/// How the window's text draws this frame: the panels' look, and the
/// terminals' for text a mod styles `terminal_text()`.
pub(crate) fn set_text_look(window: &mut Window) {
    let s = settings::get();
    window.set_glyph_thicken(s.ui_glyph_thicken());
    window.set_glyph_contrast(s.ui_text_contrast);
    window.set_glyph_subpixel(s.ui_text_rendering.subpixel());
    gpui::set_subpixel_bgr(s.subpixel_order == settings::SubpixelOrder::Bgr);
    window.set_font_weight_shift(s.ui_weight_shift());
    window.set_terminal_text(Some(crate::terminal::terminal_text()));
}

/// One of the window's two cached drawings (`AppWindow::lower`, `upper`).
pub(crate) struct Body {
    app: WeakEntity<AppWindow>,
    upper: bool,
}

impl Render for Body {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let upper = self.upper;
        self.app
            .update(cx, |app, cx| {
                if upper {
                    app.render_upper(window, cx)
                } else {
                    app.render_lower(window, cx)
                }
            })
            .unwrap_or_else(|_| Empty.into_any_element())
    }
}

impl AppWindow {
    /// What the window draws under the cursor trail: the layout, its panes
    /// and the window's frame; first the checks each draw makes.
    fn render_lower(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        crate::bench::count(&crate::bench::BODY);
        let _timed = crate::bench::timed(&crate::bench::WINDOW_US);
        // Terminals drawn this frame count as on screen; a frame that reuses
        // this drawing keeps them so.
        crate::terminal::next_frame();
        if self
            .model
            .active_tab()
            .is_some_and(|t| !t.floating.is_empty())
        {
            self.float_cell.set(crate::terminal::cell_size(window));
        }
        // A terminal the person clicked into becomes the focused pane.
        let clicked = self
            .terms
            .iter()
            .find(|(_, t)| t.focus.is_focused(window))
            .map(|(id, _)| id.clone());
        if clicked.is_some() && self.focused_block.take().is_some() {
            self.publish(cx);
        }
        // A window's terminal with the keyboard is what focused() reports; the pane under it stays.
        let in_window = overlay_terminal_block(&self.overlays, clicked.as_deref()).map(|n| n.id.clone());
        if in_window != self.window_terminal {
            self.window_terminal = in_window;
            self.publish(cx);
        }
        if let Some(id) = clicked
            && self.focused_pane.as_deref() != Some(id.as_str())
            && (self.model.tab_of(&id).is_some() || terminal_block(&self.chrome, Some(&id)).is_some())
        {
            self.model.focus_pane(&id);
            self.focused_pane = Some(id);
            self.synced_model_pane = self.model.focused_pane();
            self.publish(cx);
        }
        self.tick_floats(window);
        self.watch_cursor_trail(cx);
        self.apply_titlebar(window);
        if mods::publish_window(self.window_json(window)) {
            self.refresh_snapshot_mods(cx);
        }
        let focused = self.focused_path(window, cx);
        self.painting_main.set(true);
        let tree = self.render_node(&self.chrome, &focused, None, cx);
        self.painting_main.set(false);
        self.window_frame(tree, window, cx)
    }

    /// What the window draws over the cursor trail: overlays (windows,
    /// dialogs, popups, toasts) and what follows a drag.
    fn render_upper(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let _timed = crate::bench::timed(&crate::bench::WINDOW_US);
        let overlays = self.render_overlays(window.viewport_size(), cx);
        let tracker = self.float_tracker(cx);
        let win_tracker = self.window_tracker(cx);
        let pane_drag = self.pane_drag_layer(cx);
        div()
            .size_full()
            .relative()
            .children(overlays)
            .children(tracker)
            .children(win_tracker)
            .child(pane_drag)
            .into_any_element()
    }
}

impl Render for AppWindow {
    /// Every draw: the window's settings for the frame and its handlers, over
    /// the two drawings either side of the cursor trail. A frame the trail
    /// alone asked for (nothing else notified) reuses both; any other draws
    /// both again, as one drawing would be.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::bench::count(&crate::bench::WINDOW);
        let _timed = crate::bench::timed(&crate::bench::WINDOW_US);
        crate::bench::frame_start();
        window.set_rem_size(settings::ui_rem());
        set_text_look(window);
        window.set_veil(inactive_veil(window.is_window_active()));
        let (lower, upper) = (self.lower.entity_id(), self.upper.entity_id());
        let trail_only = window.only_view_notified(self.trail_layer.entity_id())
            && !window.view_dirty(lower)
            && !window.view_dirty(upper);
        if !trail_only {
            window.redraw_view(lower);
            window.redraw_view(upper);
        }
        let tree = AnyView::from(self.lower.clone())
            .cached(StyleRefinement::default().size_full())
            .reuse_nested();
        let trail = self.trail_layer.clone();
        let above = AnyView::from(self.upper.clone())
            .cached(
                StyleRefinement::default()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
            )
            .reuse_nested();
        div()
            .id("herdr")
            .track_focus(&self.focus)
            // `layout:<name>` key sections apply while that layout shows.
            .key_context(format!("Herdr layout = {}", self.layout_name).as_str())
            .size_full()
            .relative()
            .bg(c("background"))
            .text_color(c("foreground"))
            .font_family(settings::get().ui_font_family.clone())
            // The rem leaves the text scale out; the text under it takes it here.
            .text_size(settings::text_rems(1.0))
            .on_action(cx.listener(|this, a: &ShellAction, w, cx| {
                // Nothing selected: the copy chord goes to the program (#216).
                if a.id() == "terminal.copy" {
                    if !this.copy_key(w, cx) {
                        cx.propagate();
                    }
                    return;
                }
                // Off (or no terminal focused), the pane zoom keys are the program's.
                if a.id().starts_with("pane.zoom_text_") && !this.pane_zoom_keys_apply(w, cx) {
                    cx.propagate();
                    return;
                }
                this.run_action(a.id(), w, cx);
            }))
            .on_action(cx.listener(|this, a: &crate::keys::CommandBinding, w, cx| {
                match a.command() {
                    // As for string bindings: off, the pane zoom keys are the program's.
                    Ok((name, _)) if name.starts_with("pane.zoom_text_") && !this.pane_zoom_keys_apply(w, cx) => cx.propagate(),
                    Ok((name, args)) => {
                        #[cfg(target_os = "macos")]
                        if let Some(action) = crate::macos::action(&name) {
                            w.dispatch_action(action, cx);
                            return;
                        }
                        this.run_key(&name, args, w, cx);
                    }
                    Err(error) => this.command_error("Key binding", &error, w, cx),
                }
            }))
            // Ctrl (Cmd) alone shows links in the pane under the pointer, focused or not.
            .on_modifiers_changed(cx.listener(|this, _: &ModifiersChangedEvent, _, cx| {
                for t in this.terms.values() {
                    if t.view.read(cx).hovered() {
                        t.view.update(cx, |_, cx| cx.notify());
                    }
                }
            }))
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                this.follow_mouse_keys(window);
                if ev.keystroke.key == "escape" && this.pane_drag.is_some() {
                    this.cancel_pane_drag(cx);
                    cx.stop_propagation();
                }
            }))
            .capture_any_mouse_down(cx.listener(|this, ev: &MouseDownEvent, _, _| {
                this.pointer = ev.position;
                this.follow_mouse_cancel();
            }))
            // Out of the window: what the pointer passed on the way doesn't take the focus.
            .on_mouse_exit(cx.listener(|this, _: &MouseExitEvent, _, _| this.follow_mouse_cancel()))
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, window, cx| {
                this.follow_mouse(ev, window, cx);
                this.drag_window(ev, window);
                if this.drag.is_none() {
                    return;
                }
                if ev.pressed_button != Some(MouseButton::Left) {
                    this.drag = None;
                    this.save();
                    this.publish(cx);
                    return;
                }
                this.drag_to(ev.position);
                cx.notify();
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, window, cx| {
                this.drag_from = None;
                if this.drag.take().is_some() {
                    this.save();
                    // Mods see the new sizes (the sidebar picks its rail by width).
                    this.publish(cx);
                    this.flush_documents(false, window, cx);
                }
            }))
            .child(tree)
            .child(trail)
            .child(above)
            .child(crate::bench::frame_end())
    }
}

/// What covers the whole window: while it isn't active, with `unfocused_dim`
/// over the window, `unfocused_dim_color` (else the background) at that
/// alpha, above every panel, overlay and tooltip, so everything fades evenly.
fn inactive_veil(active: bool) -> Option<Hsla> {
    let s = settings::get();
    let toward = s.dim_color().unwrap_or_else(|| c("background"));
    (!active && s.unfocused_dim > 0.0 && s.unfocused_dim_scope == settings::DimScope::Window).then(|| Hsla { a: s.unfocused_dim.min(1.0), ..toward })
}

fn invalidate_asset(path: &std::path::Path, window: &mut Window, cx: &mut gpui::App) {
    let key = gpui_shell::AppAssets::cache_key(&path.to_string_lossy());
    invalidate_image(&gpui::Resource::Embedded(key.clone()), window, cx);
    window.invalidate_svg(&key);
}

fn invalidate_image(resource: &gpui::Resource, window: &mut Window, cx: &mut gpui::App) {
    if let Some(Ok(old)) = cx.cached_asset::<gpui::ImgResourceLoader>(resource) {
        cx.drop_image(old, Some(window));
    }
    cx.remove_asset::<gpui::ImgResourceLoader>(resource);
}

/// The changed paths, plus the files inside changed mod folders (a folder
/// renamed into place arrives as one event). Runs on the UI thread: skips what
/// the watcher ignores (hidden, `node_modules`) and stops at a mod-sized limit.
fn changed_asset_paths(home: &std::path::Path, changed: &[std::path::PathBuf]) -> Vec<std::path::PathBuf> {
    const MAX_FILES: usize = 2000;
    let mut paths: std::collections::BTreeSet<_> = changed.iter().cloned().collect();
    let mut seen = std::collections::HashSet::new();
    let mut pending: Vec<_> = changed.iter().filter(|p| p.starts_with("mods")).cloned().collect();
    while let Some(path) = pending.pop() {
        // Do not follow symlinks out of the mod folder (or into a cycle).
        if !seen.insert(path.clone()) || !home.join(&path).symlink_metadata().is_ok_and(|m| m.is_dir()) { continue; }
        let Ok(entries) = std::fs::read_dir(home.join(&path)) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || name == "node_modules" { continue; }
            let child = path.join(&*name);
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                pending.push(child);
            } else if paths.len() < MAX_FILES {
                paths.insert(child);
            } else {
                return paths.into_iter().collect();
            }
        }
    }
    paths.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use gpui::{div, point, px, size, Context, IntoElement, Render, TestAppContext, Window};
    use gpui::prelude::*;

    #[test]
    fn going_back_into_a_split_column_takes_its_top_pane_whatever_the_fractions() {
        let r = |x: f32, y: f32, w: f32, h: f32| gpui::Bounds::new(point(px(x), px(y)), size(px(w), px(h)));
        // An odd height splits into halves a fraction apart, the bottom one larger.
        let column = || vec![("top".to_owned(), r(0.0, 0.0, 400.0, 300.4)), ("bottom".to_owned(), r(0.0, 300.4, 400.0, 300.6))];
        assert_eq!(super::nearest(r(401.0, 0.0, 399.0, 601.0), "left", column(), &[]).as_deref(), Some("top"));
        // More overlap by a pixel or more still decides.
        assert_eq!(super::nearest(r(401.0, 250.0, 399.0, 351.0), "left", column(), &[]).as_deref(), Some("bottom"));
        // A visited pane wins even when an unvisited pane has more overlap.
        assert_eq!(super::nearest(r(401.0, 250.0, 399.0, 351.0), "left", column(), &["top".to_owned()]).as_deref(), Some("top"));
    }

    #[test]
    fn directional_round_trips_return_to_the_recent_pane_in_either_split() {
        let r = |x: f32, y: f32, w: f32, h: f32| gpui::Bounds::new(point(px(x), px(y)), size(px(w), px(h)));
        for (a, b, other, out, back) in [
            (r(0.0, 0.0, 400.0, 300.4), r(0.0, 300.4, 400.0, 300.6), r(401.0, 0.0, 399.0, 601.0), "right", "left"),
            (r(0.0, 0.0, 300.4, 400.0), r(300.4, 0.0, 300.6, 400.0), r(0.0, 401.0, 601.0, 399.0), "down", "up"),
        ] {
            let split = || vec![("a".to_owned(), a), ("b".to_owned(), b)];
            assert_eq!(super::nearest(other, back, split(), &[]).as_deref(), Some("a"));
            let mut history = Vec::new();
            for (id, bounds) in [("a", a), ("b", b), ("a", a)] {
                super::remember_pane(&mut history, id);
                assert_eq!(super::nearest(bounds, out, vec![("other".to_owned(), other)], &history).as_deref(), Some("other"));
                super::remember_pane(&mut history, "other");
                assert_eq!(super::nearest(other, back, split(), &history).as_deref(), Some(id));
                super::remember_pane(&mut history, id);
            }
            assert_eq!(history.len(), 3);
            // Recency cannot select a pane outside the requested direction.
            assert_eq!(super::nearest(other, out, split(), &history), None);
        }
    }

    #[test]
    fn terminal_folders_retry_only_definite_spawn_failures_in_home() {
        let failed = |message| anyhow::Error::new(crate::hc::RpcError(serde_json::json!({"message": message, "data": {"kind": "spawn_failed"}})));
        let mut folders = Vec::new();
        let result = super::run_following_folder("deleted".into(), true, None, |folder| {
            folders.push(folder.clone());
            if folder.as_deref() == Some("deleted") { Err(failed("invalid directory")) } else { Ok("t2".into()) }
        }).unwrap();
        assert_eq!(folders, vec![Some("deleted".into()), Some(super::home())]);
        assert_eq!(result, ("t2".into(), super::home()));
        let result = super::run_following_folder("/remote/deleted".into(), true, Some("remote"), |folder| {
            if folder.is_some() { Err(failed("invalid directory")) } else { Ok("remote/t2".into()) }
        }).unwrap();
        assert_eq!(result, ("remote/t2".into(), String::new()));
        for (follow, folder, message) in [(false, "explicit", "invalid directory"), (true, "", "invalid directory"), (true, "inherited", "the holder did not answer")] {
            let mut calls = 0;
            assert!(super::run_following_folder(folder.into(), follow, None, |_| { calls += 1; Err(failed(message)) }).is_err());
            assert_eq!(calls, 1);
        }
        let mut calls = 0;
        assert!(super::run_following_folder("inherited".into(), true, None, |_| { calls += 1; Err(anyhow::anyhow!("lost the core")) }).is_err());
        assert_eq!(calls, 1);
        let mut calls = 0;
        let result = super::run_following_folder("existing".into(), true, None, |_| { calls += 1; Ok("t1".into()) }).unwrap();
        assert_eq!(calls, 1);
        assert_eq!(result, ("t1".into(), "existing".into()));
        let mut calls = 0;
        let error = super::run_following_folder("deleted".into(), true, None, |_| { calls += 1; Err(failed(if calls == 1 { "first" } else { "second" })) }).unwrap_err();
        assert_eq!(calls, 2);
        assert!(error.to_string().contains("second"));
    }

    #[test]
    fn new_spaces_follow_the_persons_active_machine_but_keep_explicit_targets() {
        use serde_json::json;
        let mut model = crate::model::Model::default();
        assert_eq!(super::new_space_machine(&json!({}), true, &model), None);
        model.add_space("remote", "", "", crate::model::Tab::new("tab", "", "pane", "second/t1"));
        model.space_mut("remote").unwrap().machine = Some("second".into());
        for args in [json!({}), json!({"machine": null})] {
            assert_eq!(super::new_space_machine(&args, true, &model).as_deref(), Some("second"));
            assert_eq!(super::new_space_machine(&args, false, &model), None);
        }
        for person in [true, false] {
            assert_eq!(super::new_space_machine(&json!({"machine": "other"}), person, &model).as_deref(), Some("other"));
            assert_eq!(super::new_space_machine(&json!({"machine": ""}), person, &model), None);
        }
        model.add_space("local", "", "", crate::model::Tab::new("local-tab", "", "local-pane", "t1"));
        assert_eq!(super::new_space_machine(&json!({}), true, &model), None);
        model.active = Some("remote".into());
        assert_eq!(super::new_space_machine(&json!({}), true, &model).as_deref(), Some("second"));
    }

    #[gpui::test]
    fn image_invalidation_drops_cached_atlas_tiles_without_loading_missing_assets(cx: &mut TestAppContext) {
        use gpui::{ImgResourceLoader, Resource};
        let temp = if cfg!(target_os = "linux") { std::path::PathBuf::from("/var/tmp") } else { std::env::temp_dir() };
        let home = temp.join(format!("herdr-image-atlas-test-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        let file = home.join("image.gif");
        let mut encoder = image::codecs::gif::GifEncoder::new(std::fs::File::create(&file).unwrap());
        for color in [[255, 0, 0, 255], [0, 255, 0, 255]] {
            encoder.encode_frame(image::Frame::new(image::RgbaImage::from_pixel(2, 2, image::Rgba(color)))).unwrap();
        }
        drop(encoder);
        let resource = Resource::Path(file.into());
        cx.update(|cx| { cx.fetch_asset::<ImgResourceLoader>(&resource); });
        cx.background_executor.run_until_parked();
        let old = cx.update(|cx| cx.cached_asset::<ImgResourceLoader>(&resource)).unwrap().unwrap();
        let context = cx.add_empty_window();
        let displayed = old.clone();
        assert_eq!(old.frame_count(), 1);
        context.draw(point(px(0.0), px(0.0)), size(px(20.0), px(20.0)), move |_, _| {
            gpui::canvas(|_, _, _| (), move |bounds, _, window, _| {
                for frame in 0..displayed.frame_count() {
                    window.paint_image(bounds, bounds, gpui::Corners::default(), displayed.clone(), frame, false).unwrap();
                }
            }).size_full()
        });
        context.update(|window, cx| {
            assert!(window.has_image_atlas_entry(&old));
            super::invalidate_image(&resource, window, cx);
            assert!(!window.has_image_atlas_entry(&old));
            assert!(!cx.has_asset::<ImgResourceLoader>(&resource));
            let missing = Resource::Embedded("mods/missing.png".into());
            super::invalidate_image(&missing, window, cx);
            assert!(!cx.has_asset::<ImgResourceLoader>(&missing));
        });
        std::fs::remove_dir_all(home).unwrap();
    }

    #[gpui::test]
    fn still_image_cache_is_bounded_and_avoids_repeat_downloads(cx: &mut TestAppContext) {
        use gpui::{ImgResourceLoader, Resource, http_client::{AsyncBody, FakeHttpClient, Response}};
        use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]))
            .write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        let bytes = bytes.into_inner();
        let requests = Arc::new(AtomicUsize::new(0));
        let count = requests.clone();
        cx.update(|cx| cx.set_http_client(FakeHttpClient::create(move |_| {
            count.fetch_add(1, Ordering::SeqCst);
            let bytes = bytes.clone();
            async move { Ok(Response::builder().status(200).body(AsyncBody::from(bytes)).unwrap()) }
        })));
        let context = cx.add_empty_window();
        context.deactivate_window();
        let sources: Vec<_> = (0..130).map(|id| Resource::Uri(format!("http://images.test/{id}.png").into())).collect();
        for source in &sources {
            context.update(|_, cx| { cx.fetch_asset::<ImgResourceLoader>(source); });
            context.run_until_parked();
        }
        context.update(|window, cx| { window.draw(cx).clear(cx); });
        assert_eq!(context.update(|_, cx| sources.iter().filter(|source| cx.has_asset::<ImgResourceLoader>(source)).count()), 128);
        let retained = sources.last().unwrap();
        assert!(context.update(|_, cx| cx.fetch_asset::<ImgResourceLoader>(retained)).unwrap().is_ok());
        assert_eq!(requests.load(Ordering::SeqCst), 130);
        assert!(!context.update(|_, cx| cx.has_asset::<ImgResourceLoader>(&sources[0])));
        context.update(|window, cx| {
            cx.set_quit_mode(gpui::QuitMode::Explicit);
            window.remove_window();
        });
    }

    #[gpui::test]
    fn closing_last_window_releases_many_animation_sources_in_one_call(cx: &mut TestAppContext) {
        use gpui::{ImgResourceLoader, Resource, img};
        use std::sync::Arc;
        struct Images(Vec<Resource>);
        impl Render for Images {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().children(self.0.iter().enumerate().map(|(id, source)| {
                    img(gpui::ImageSource::Resource(source.clone())).id(id).size_4()
                }))
            }
        }
        let mut gif = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut gif);
            for color in [[255, 0, 0, 255], [0, 255, 0, 255]] {
                encoder.encode_frame(image::Frame::new(image::RgbaImage::from_pixel(2, 2, image::Rgba(color)))).unwrap();
            }
        }
        let temp = if cfg!(target_os = "linux") { std::path::PathBuf::from("/var/tmp") } else { std::env::temp_dir() };
        let dir = temp.join(format!("herdr-many-images-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sources: Vec<_> = (0..24).map(|id| {
            let path = dir.join(format!("{id}.gif"));
            std::fs::write(&path, &gif).unwrap();
            Resource::Path(path.into())
        }).collect();
        let (_, context) = cx.add_window_view(|_, _| Images(sources.clone()));
        context.deactivate_window();
        context.run_until_parked();
        let mut images = context.update(|window, _| window.displayed_images());
        assert_eq!(images.iter().filter(|image| image.frame_count() == 2).count(), 24);
        images.extend(context.update(|_, cx| sources.iter().map(|source| cx.cached_asset::<ImgResourceLoader>(source).unwrap().unwrap()).collect::<Vec<_>>()));
        let weak: Vec<_> = images.iter().map(Arc::downgrade).collect();
        drop(images);
        context.update(|window, cx| {
            cx.set_quit_mode(gpui::QuitMode::Explicit);
            window.remove_window();
        });
        assert!(weak.iter().all(|image| image.upgrade().is_none()), "one close must release every animation and its source regardless of hash order");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[gpui::test]
    fn image_tiles_wait_for_both_scenes_before_retiring(cx: &mut TestAppContext) {
        use std::sync::Arc;
        let context = cx.add_empty_window();
        let image = Arc::new(gpui::RenderImage::new((0..3).map(|red| {
            image::Frame::new(image::RgbaImage::from_pixel(2, 2, image::Rgba([red, 0, 0, 255])))
        }).collect::<smallvec::SmallVec<[_; 1]>>()));
        for frame in 0..3 {
            let painted = image.clone();
            context.draw(point(px(0.), px(0.)), size(px(20.), px(20.)), move |_, _| {
                gpui::canvas(|_, _, _| (), move |bounds, _, window, _| {
                    window.paint_image(bounds, bounds, gpui::Corners::default(), painted.clone(), frame, false).unwrap();
                }).size_full()
            });
            for previous in 0..=frame {
                assert_eq!(context.update(|window, _| window.has_image_frame_atlas_entry(&image, previous)), previous + 1 >= frame);
            }
        }
    }

    #[gpui::test]
    fn clipped_images_do_not_upload(cx: &mut TestAppContext) {
        use std::sync::Arc;
        let context = cx.add_empty_window();
        let image = Arc::new(gpui::RenderImage::new(smallvec::smallvec![
            image::Frame::new(image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255])))
        ]));
        for mask in [
            gpui::Bounds::new(point(px(50.), px(50.)), size(px(10.), px(10.))),
            gpui::Bounds::new(point(px(0.), px(0.)), size(px(0.), px(0.))),
        ] {
            for _ in 0..2 {
                let painted = image.clone();
                context.draw(point(px(0.), px(0.)), size(px(20.), px(20.)), move |_, _| {
                    gpui::canvas(|_, _, _| (), move |bounds, _, window, _| {
                        window.with_content_mask(Some(gpui::ContentMask { bounds: mask }), |window| {
                            window.paint_image(bounds, bounds, gpui::Corners::default(), painted.clone(), 0, false).unwrap();
                        });
                    }).size_full()
                });
                assert!(!context.update(|window, _| window.has_image_frame_atlas_entry(&image, 0)));
            }
        }
        let painted = image.clone();
        context.draw(point(px(0.), px(0.)), size(px(20.), px(20.)), move |_, _| {
            gpui::canvas(|_, _, _| (), move |bounds, _, window, _| {
                window.paint_image(bounds, bounds, gpui::Corners::default(), painted.clone(), 0, false).unwrap();
            }).size_full()
        });
        assert!(context.update(|window, _| window.has_image_frame_atlas_entry(&image, 0)));
    }

    #[gpui::test]
    fn background_images_stay_warm_and_release_animations(cx: &mut TestAppContext) {
        use gpui::{AssetSource as _, ImgResourceLoader, Resource, img};
        use std::sync::Arc;
        struct ImageProbe { source: Option<Resource>, animate: bool }
        impl Render for ImageProbe {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let mut element = div().size_full();
                if let Some(source) = &self.source {
                    element = element.child(img(gpui::ImageSource::Resource(source.clone())).size_4());
                    if self.animate {
                        element = element.child(img(gpui::ImageSource::Resource(source.clone())).id("animated").size_4());
                    }
                }
                element
            }
        }
        let temp = if cfg!(target_os = "linux") { std::path::PathBuf::from("/var/tmp") } else { std::env::temp_dir() };
        let home = temp.join(format!("herdr-image-release-test-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        let gif = home.join("animated.gif");
        let png = home.join("still.png");
        let mut encoder = image::codecs::gif::GifEncoder::new(std::fs::File::create(&gif).unwrap());
        for color in [[255, 0, 0, 255], [0, 255, 0, 255]] {
            encoder.encode_frame(image::Frame::new(image::RgbaImage::from_pixel(2, 2, image::Rgba(color)))).unwrap();
        }
        drop(encoder);
        image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255])).save(&png).unwrap();
        let assets = gpui_shell::AppAssets::new(home.clone());
        assert_eq!(
            assets.image_resource(&Resource::Path(gif.clone().into())),
            Resource::Path(gif.clone().into())
        );
        let (view, cx) = cx.add_window_view(|_, _| ImageProbe { source: None, animate: false });
        cx.deactivate_window();
        for file in [&gif, &png, &gif] {
            let resource = Resource::Path(file.clone().into());
            view.update(cx, |view, cx| { view.source = Some(resource.clone()); cx.notify(); });
            cx.run_until_parked();
            let image = cx.update(|_, cx| cx.cached_asset::<ImgResourceLoader>(&resource)).unwrap().unwrap();
            assert_eq!(image.frame_count(), 1);
            assert_eq!(&image.as_bytes(0).unwrap()[..4], &[0, 0, 255, 255]);
            assert!(cx.update(|window, _| window.has_image_frame_atlas_entry(&image, 0)));
            if file == &gif {
                view.update(cx, |view, cx| { view.animate = true; cx.notify(); });
                cx.run_until_parked();
                let animation = cx.update(|window, _| window.displayed_images().into_iter().find(|image| image.frame_count() == 2)).unwrap();
                assert_eq!(animation.as_bytes(0).unwrap().as_ptr(), image.as_bytes(0).unwrap().as_ptr());
                let weak_animation = Arc::downgrade(&animation);
                drop(animation);
                view.update(cx, |view, cx| { view.animate = false; cx.notify(); });
                cx.run_until_parked();
                assert!(weak_animation.upgrade().is_none(), "a still background must not keep animation pixels");
                assert!(cx.update(|window, _| window.has_image_frame_atlas_entry(&image, 0)));
            }
            let weak = Arc::downgrade(&image);
            drop(image);
            view.update(cx, |view, cx| { view.source = None; cx.notify(); });
            cx.run_until_parked();
            assert!(weak.upgrade().is_some(), "still pixels stay warm for layout switches");
            view.update(cx, |view, cx| { view.source = Some(resource.clone()); cx.notify(); });
            cx.update(|window, cx| { window.draw(cx).clear(cx); });
            let warm = cx.update(|_, cx| cx.cached_asset::<ImgResourceLoader>(&resource)).unwrap().unwrap();
            assert!(Arc::ptr_eq(&warm, &weak.upgrade().unwrap()));
            assert!(cx.update(|window, _| window.has_image_frame_atlas_entry(&warm, 0)));
            drop(warm);
        }
        // Leave both the source and animation unowned in the same close operation.
        view.update(cx, |view, cx| { view.animate = true; cx.notify(); });
        cx.run_until_parked();
        let images = cx.update(|window, _| window.displayed_images());
        let weak: Vec<_> = images.iter().map(Arc::downgrade).collect();
        assert!(images.iter().any(|image| image.frame_count() == 2));
        drop(images);
        cx.update(|window, cx| {
            cx.set_quit_mode(gpui::QuitMode::Explicit);
            window.remove_window();
        });
        assert!(weak.iter().all(|image| image.upgrade().is_none()), "closing the last window must prune in dependency order without another draw");
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn directory_changes_include_nested_assets() {
        let temp = if cfg!(target_os = "linux") { std::path::PathBuf::from("/var/tmp") } else { std::env::temp_dir() };
        let home = temp.join(format!("herdr-image-directory-test-{}", std::process::id()));
        std::fs::create_dir_all(home.join("mods/probe/nested")).unwrap();
        std::fs::write(home.join("mods/probe/nested/icon.svg"), "svg").unwrap();
        for ignored in ["node_modules/pkg", ".git"] {
            std::fs::create_dir_all(home.join("mods/probe").join(ignored)).unwrap();
            std::fs::write(home.join("mods/probe").join(ignored).join("logo.svg"), "svg").unwrap();
        }
        let changed = vec![std::path::PathBuf::from("mods/probe")];
        // A repeated root and a covered child are walked once.
        let events = [changed[0].clone(), changed[0].clone(), "mods/probe/nested".into()];
        let paths = super::changed_asset_paths(&home, &events);
        assert!(paths.contains(&std::path::PathBuf::from("mods/probe/nested/icon.svg")));
        assert!(paths.contains(&changed[0]));
        assert!(!paths.iter().any(|p| p.to_string_lossy().contains("node_modules") || p.to_string_lossy().contains(".git")));
        assert_eq!(paths.iter().filter(|p| p.ends_with("icon.svg")).count(), 1);
        std::fs::remove_dir_all(&home).unwrap();
        assert_eq!(super::changed_asset_paths(&home, &changed), changed);
    }

    #[test]
    fn asset_cache_keys_match_file_events_without_requiring_the_file() {
        use gpui_shell::AppAssets;
        let expected = AppAssets::cache_key("mods/probe/icon.svg");
        for alias in ["./mods/probe/icon.svg", "/mods/probe/icon.svg", "mods//probe/./icon.svg"] {
            assert_eq!(AppAssets::cache_key(alias), expected);
        }
        // Traversal remains invalid, rather than becoming a valid cache hit.
        assert_eq!(AppAssets::cache_key("mods/probe/../icon.svg").as_ref(), "mods/probe/../icon.svg");
        #[cfg(windows)]
        assert_eq!(AppAssets::cache_key(r"mods\probe\icon.svg"), expected);
    }

    #[test]
    fn svg_invalidation_removes_every_size_but_preserves_other_assets() {
        use gpui::{AtlasKey, DevicePixels, HeadlessAtlas, PlatformAtlas, RenderSvgParams, Size};
        let atlas = HeadlessAtlas::default();
        let key = |path: &str, size| AtlasKey::Svg(RenderSvgParams {
            path: path.to_owned().into(), size: Size::new(DevicePixels(size), DevicePixels(size)),
        });
        let keys = [key("icons/icon.svg", 16), key("icons/icon.svg", 32), key("icons/nested/icon.svg", 16), key("icons-other/icon.svg", 16), AtlasKey::Image(gpui::RenderImageParams { image_id: gpui::ImageId(1), frame_index: 0 })];
        let mut build = || Ok(Some((Size::new(DevicePixels(1), DevicePixels(1)), std::borrow::Cow::Borrowed(&[255u8][..]))));
        let tiles: Vec<_> = keys.iter().map(|key| atlas.get_or_insert_with(key.clone(), &mut build).unwrap().unwrap()).collect();
        atlas.remove_svg("icons");
        atlas.remove_svg("icons");
        for (index, key) in keys.iter().enumerate() {
            assert_eq!(atlas.contains(key), index >= 3);
            let tile = atlas.get_or_insert_with(key.clone(), &mut build).unwrap().unwrap();
            assert_eq!(tile == tiles[index], index >= 3);
        }
    }

    #[test]
    fn agent_commands_keep_arguments_out_of_shell_source() {
        let cmd = vec!["agent".to_owned(), "a 'quoted' task; $(not-a-command)\nnext line".to_owned(), "".to_owned()];
        for login in [false, true] {
            for sh in ["/bin/bash", "/bin/zsh", "/bin/sh", "/bin/dash", "/bin/fish"] {
                let run = super::unix_agent_argv(sh.into(), cmd.clone(), login);
                assert_eq!(run[0], sh);
                assert_eq!(run[1], if login { "-lic" } else { "-ic" });
                assert_eq!(run[3], sh);
                assert_eq!(run[4..], cmd);
                let expected = match (sh.ends_with("/fish"), login) {
                    (true, true) => "command $argv[2..]; exec $argv[1] -li",
                    (true, false) => "command $argv[2..]; exec $argv[1] -i",
                    (false, true) => "\"$@\"; exec \"$0\" -li",
                    (false, false) => "\"$@\"; exec \"$0\" -i",
                };
                assert_eq!(run[2], expected);
            }
        }
    }

    #[test]
    fn early_launch_failures_show_exit_code_and_last_output_lines() {
        let mut record = serde_json::json!({"reason":"exited", "exit_code":127, "created_ms":1000, "finished_ms":3000});
        assert_eq!(super::launch_failure(&record, "old\none\ntwo\nthree\nfour\nfive\n\n").as_deref(), Some("Exit code 127\none\ntwo\nthree\nfour\nfive"));
        assert_eq!(super::launch_failure(&record, "").as_deref(), Some("Exit code 127"));
        record["exit_code"] = 0.into();
        assert!(super::launch_failure(&record, "").is_none());
        record["exit_code"] = 1.into();
        record["finished_ms"] = 3001.into();
        assert!(super::launch_failure(&record, "").is_none());
        record["finished_ms"] = 999.into();
        assert!(super::launch_failure(&record, "").is_none());
        record["finished_ms"] = 2000.into();
        record["reason"] = "killed".into();
        assert!(super::launch_failure(&record, "").is_none());
        assert!(super::launch_failure(&serde_json::Value::Null, "").is_none());
    }

    #[gpui::test]
    fn traffic_light_dividers_start_below_the_toolbar(cx: &mut TestAppContext) {
        use std::{cell::Cell, rc::Rc};
        let mut layout: crate::layout::Node = serde_json::from_str(include_str!("../defaults/layouts/main.json")).unwrap();
        let context = cx.add_empty_window();
        for (macos, titlebar, width, zoom, expected) in [
            (true, crate::layout::Titlebar::Layout, 44.0, 1.0, 42.0),
            (true, crate::layout::Titlebar::Layout, 90.0, 0.75, 42.0),
            (true, crate::layout::Titlebar::Layout, 60.0, 1.5, 0.0),
            (true, crate::layout::Titlebar::Layout, 60.0, 1.0, 42.0),
            (true, crate::layout::Titlebar::Layout, 76.0, 1.0, 42.0),
            (true, crate::layout::Titlebar::Layout, 268.0, 1.0, 0.0),
            (false, crate::layout::Titlebar::Layout, 44.0, 1.0, 0.0),
            (true, crate::layout::Titlebar::System, 44.0, 1.0, 0.0),
        ] {
            layout.titlebar = Some(titlebar);
            layout.children_mut().unwrap()[0].size = Some(crate::layout::Size::Px { px: width });
            let top = super::traffic_light_divider_inset(&layout, crate::layout::Axis::Row, 0, macos, zoom);
            assert_eq!(top, expected);
            let bounds = Rc::new(Cell::new(gpui::Bounds::default()));
            let observed = bounds.clone();
            context.draw(point(px(0.0), px(0.0)), size(px(1.0), px(100.0)), move |_, _| {
                div().size_full().relative().flex().items_center().justify_center().child(
                    super::separator_line(true, 1.0, gpui::rgb(0xffffff).into(), top).child(
                        gpui::canvas(move |bounds, _, _| observed.set(bounds), |_, _, _, _| {}).size_full()
                    )
                )
            });
            assert_eq!(bounds.get().origin.y, px(expected));
            assert_eq!(bounds.get().size.height, px(100.0 - expected));
        }
        layout.traffic_lights = None;
        assert_eq!(super::traffic_light_divider_inset(&layout, crate::layout::Axis::Row, 0, true, 1.0), 0.0);
    }

    #[gpui::test]
    fn fitted_popups_use_their_rendered_size_at_the_window_edge(cx: &mut TestAppContext) {
        struct Popup {
            height: f32,
            edge: bool,
        }
        impl Render for Popup {
            fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let viewport = window.viewport_size();
                let at = if self.edge {
                    point(viewport.width - px(40.0), viewport.height - px(40.0))
                } else {
                    point(px(100.0), px(200.0))
                };
                div().size_full().child(super::position_popup(
                    at,
                    div()
                        .id("popup")
                        .debug_selector(|| "popup".into())
                        .w(px(220.0))
                        .max_h(viewport.height * 0.85)
                        .child(div().h(px(self.height))),
                ))
            }
        }
        let (view, cx) = cx.add_window_view(|_, _| Popup { height: 72.0, edge: false });
        for (height, edge) in [(72.0, false), (72.0, true), (300.0, true)] {
            view.update(cx, |view, cx| {
                view.height = height;
                view.edge = edge;
                cx.notify();
            });
            cx.run_until_parked();
            let bounds = cx.debug_bounds("popup").unwrap();
            assert_eq!(bounds.size, size(px(220.0), px(height)));
            if edge {
                let viewport = cx.update(|window, _| window.viewport_size());
                assert_eq!(bounds.bottom_right(), point(viewport.width - px(8.0), viewport.height - px(8.0)));
            } else {
                assert_eq!(bounds.origin, point(px(100.0), px(200.0)));
            }
        }
    }

    /// A toast's own `timeout` wins (the 1.5 s Copied); else its level's 5 s
    /// or 8 s, scaled by `toast_duration`, or none: until clicked.
    #[test]
    fn toast_lifetimes() {
        use super::toast_timeout;
        use crate::settings::ToastDuration;
        use serde_json::json;
        use std::time::Duration;
        let ms = |n: u64| Some(Duration::from_millis(n));
        let info = json!({ "title": "Saved" });
        let error = json!({ "title": "Failed", "level": "error" });
        let warning = json!({ "title": "Careful", "level": "warning" });
        let copied = json!({ "title": "Copied", "level": "success", "timeout": 1500 });
        for (duration, routine, urgent) in [
            (ToastDuration::Normal, ms(5000), ms(8000)),
            (ToastDuration::Short, ms(3000), ms(4800)),
            (ToastDuration::Long, ms(10000), ms(16000)),
            (ToastDuration::UntilDismissed, None, None),
        ] {
            assert_eq!([toast_timeout(&info, duration), toast_timeout(&error, duration), toast_timeout(&warning, duration)], [routine, urgent, urgent], "{duration:?}");
            assert_eq!(toast_timeout(&copied, duration), ms(1500), "{duration:?}: a toast's own time wins");
        }
    }

    #[test]
    fn toast_duration_and_mark_reach_new_toasts() {
        in_own_process("toast_duration_open");
    }

    /// The person's `toast_duration` decides when a toast opened now, or an
    /// alert joining one, goes (a toast already open keeps the time it was
    /// given); Copied keeps its 1.5 s, and shows even past five errors kept
    /// until clicked. Their `toast_mark` reaches the shipped toast mod in its
    /// props: its dot shows for stripe and dot only.
    #[gpui::test]
    fn toast_duration_open(cx: &mut gpui::TestAppContext) {
        use crate::model::Alert;
        use crate::settings::{ToastDuration, ToastMark};
        use serde_json::json;
        use std::time::{Duration, Instant};
        if !running_alone("toast_duration_open") {
            return;
        }
        let (view, cx) = open_window(cx, "toast", include_str!("../defaults/mods/toast/main.js"), json!({ "id": "pane", "type": "block" }));
        cx.run_until_parked();
        let set = |duration: ToastDuration, mark: ToastMark, cx: &mut gpui::VisualTestContext| view.update_in(cx, |this, window, cx| {
            this.person.toast_duration = duration;
            this.person.toast_mark = mark;
            this.apply_look(window, cx);
        });
        let lasts = |id: &str, cx: &mut gpui::VisualTestContext| {
            view.update(cx, |this, _| this.overlays.iter().find(|o| o.id == id).map(|o| o.expires.map(|at| at.saturating_duration_since(Instant::now())))).expect("the toast is open")
        };
        let open = |level: &str, cx: &mut gpui::VisualTestContext| {
            let id = crate::mods::new_overlay_id();
            view.update_in(cx, |this, window, cx| this.open_overlay(id.clone(), json!({ "kind": "toast", "notification": { "title": "t", "level": level } }), window, cx));
            cx.run_until_parked();
            lasts(&id, cx)
        };
        let close_all = |cx: &mut gpui::VisualTestContext| view.update_in(cx, |this, window, cx| {
            for id in this.overlays.iter().map(|o| o.id.clone()).collect::<Vec<_>>() {
                this.close_overlay(&id, window, cx);
            }
        });
        let about = |d: Option<Duration>, s: u64| d.is_some_and(|d| d <= Duration::from_secs(s) && d > Duration::from_secs(s) - Duration::from_secs(1));
        assert!(about(open("info", cx), 5), "unset: 5 s as ever");
        assert!(cx.debug_bounds("toast-dot").is_some(), "unset: the stripe's dot");
        assert!(about(open("error", cx), 8), "unset: 8 s for errors");
        close_all(cx);
        set(ToastDuration::Long, ToastMark::Dot, cx);
        assert!(about(open("info", cx), 10));
        assert!(cx.debug_bounds("toast-dot").is_some(), "dot: the dot");
        close_all(cx);
        cx.run_until_parked();
        set(ToastDuration::UntilDismissed, ToastMark::None, cx);
        assert_eq!(open("error", cx), None, "until clicked");
        assert!(cx.debug_bounds("toast-dot").is_none(), "none: no dot");
        view.update_in(cx, |this, window, cx| this.show_copied(window, cx));
        let copied = view.update(cx, |this, _| this.copied_toast.clone()).unwrap();
        assert!(lasts(&copied, cx).is_some_and(|d| d <= Duration::from_millis(1500)), "Copied stays brief");
        // Five errors kept until clicked don't starve a new toast: it shows, the oldest error goes.
        close_all(cx);
        let errors: Vec<String> = (0..5).map(|i| {
            let id = crate::mods::new_overlay_id();
            view.update_in(cx, |this, window, cx| this.open_overlay(id.clone(), json!({ "kind": "toast", "notification": { "title": format!("e{i}"), "level": "error" } }), window, cx));
            id
        }).collect();
        view.update_in(cx, |this, window, cx| this.show_copied(window, cx));
        let copied = view.update(cx, |this, _| this.copied_toast.clone()).unwrap();
        let shown = view.update(cx, |this, _| this.overlays.iter().filter(|o| o.kind == super::OverlayKind::Toast).map(|o| o.id.clone()).collect::<Vec<_>>());
        assert_eq!(shown, [&errors[1..], &[copied]].concat(), "Copied shows; the oldest error made room");
        // An alert joining a toast keeps it until clicked too.
        set(ToastDuration::Long, ToastMark::None, cx);
        let alert = |addr: &str, cx: &mut gpui::VisualTestContext| {
            view.update_in(cx, |this, window, cx| {
                this.person.sounds.enabled = false;
                this.live.insert(addr.into(), json!({ "name": format!("agent {addr}") }));
                this.alert(addr, Alert::Done, None, false, window, cx);
            });
            cx.run_until_parked();
        };
        alert("t1", cx);
        let joined = view.update(cx, |this, _| this.overlays.iter().rev().find(|o| o.alert.is_some()).map(|o| o.id.clone())).unwrap();
        assert!(about(lasts(&joined, cx), 10), "an alert toast follows the setting");
        set(ToastDuration::UntilDismissed, ToastMark::None, cx);
        alert("t2", cx);
        assert_eq!(lasts(&joined, cx), None, "a joining alert keeps it until clicked");
    }

    /// Every colour a toast's box and time-left bar draw, in every design
    /// and level, is a key of the theme's derived toast colours, under a dark
    /// and a light theme, and the two themes give it different values: a
    /// theme switch restyles every design.
    #[test]
    fn toast_frames_draw_their_theme_colours() {
        use crate::settings::{self, Settings, ToastDesign};
        for design in ToastDesign::ALL {
            settings::set(Settings { toast_design: design, ..Settings::default() });
            let themes = settings::themes();
            let [dark, light] = ["graphite", "paper"].map(|n| themes.iter().find(|(name, _)| name == n).map(|(_, t)| t.clone()).unwrap());
            for level in ["toast_info", "toast_success", "toast_warning", "toast_error"] {
                let (fill, edge, shadow) = super::toast_frame_colors(design, level);
                if design == ToastDesign::Outline {
                    assert!(dark[fill].len() == 9 && !dark[fill].ends_with("ff"), "the outline's backing lets the window through: {}", dark[fill]);
                }
                for key in [Some(fill), Some(edge), shadow, Some(level)].into_iter().flatten() {
                    assert!(key.starts_with("toast"), "{design:?}: {key} isn't a toast colour");
                    let (d, l) = (dark.get(key), light.get(key));
                    assert!(d.is_some() && l.is_some(), "{design:?}: {key} isn't derived");
                    assert_ne!(d, l, "{design:?}: {key} is the same on dark and light");
                }
            }
        }
        settings::set(Settings::default());
    }

    /// Each design's box: a pill one short line, an icon card taller with
    /// buttons for an action, a banner one line; the rest as the card.
    #[test]
    fn toast_boxes() {
        use super::{BANNER_HEIGHT, ICON_CARD_ACTIONS, ICON_CARD_HEIGHT, PILL_HEIGHT, TOAST_HEIGHT, TOAST_HEIGHT_WITH_BODY, TOAST_MAX_WIDTH, TOAST_MIN_WIDTH, toast_box};
        use crate::settings::ToastDesign;
        use serde_json::json;
        let (plain, body, action) = (json!({ "title": "t" }), json!({ "title": "t", "body": "b" }), json!({ "title": "t", "action": { "name": "agent.focus" } }));
        for design in [ToastDesign::Card, ToastDesign::Terminal, ToastDesign::Outline] {
            assert_eq!(toast_box(design, &plain), (TOAST_MIN_WIDTH, TOAST_MAX_WIDTH, TOAST_HEIGHT), "{design:?}");
            assert_eq!(toast_box(design, &body).2, TOAST_HEIGHT_WITH_BODY, "{design:?}");
        }
        assert_eq!(toast_box(ToastDesign::Pill, &body).2, PILL_HEIGHT, "a pill's body is its tooltip");
        assert_eq!(toast_box(ToastDesign::IconCard, &body).2, ICON_CARD_HEIGHT);
        assert_eq!(toast_box(ToastDesign::IconCard, &action).2, ICON_CARD_HEIGHT + ICON_CARD_ACTIONS);
        assert_eq!(toast_box(ToastDesign::IconCard, &json!({ "title": "t", "action": { "name": "" } })).2, ICON_CARD_HEIGHT, "no action, no buttons");
        assert_eq!(toast_box(ToastDesign::Banner, &body).2, BANNER_HEIGHT);
    }

    /// A banner sits flush on its edge, past a top banner's chrome, moved in
    /// by the offset; a bottom one clears the update pill.
    #[test]
    fn banners_start_on_their_edge() {
        use super::{TOAST_GAP, TOAST_MARGIN, UPDATE_PILL_HEIGHT, banner_start};
        assert_eq!(banner_start(0.0, 0.0, false), 0.0);
        assert_eq!(banner_start(40.0, 0.0, false), 40.0, "below the top chrome");
        assert_eq!(banner_start(40.0, 10.0, false), 50.0);
        assert_eq!(banner_start(0.0, -16.0, false), 0.0, "never past the edge");
        assert_eq!(banner_start(0.0, 0.0, true), TOAST_MARGIN + UPDATE_PILL_HEIGHT + TOAST_GAP, "above the pill");
        assert_eq!(banner_start(0.0, 200.0, true), 200.0);
    }

    #[test]
    fn layout_terminals_survive_stream_loss_until_confirmed_closed() {
        in_own_process("layout_terminal_stream_loss");
    }

    #[gpui::test]
    fn layout_terminal_stream_loss(cx: &mut gpui::TestAppContext) {
        use serde_json::json;
        if !running_alone("layout_terminal_stream_loss") {
            return;
        }
        let (view, cx) = open_window(cx, "neighbor", "", json!({
            "id": "root", "type": "row", "children": [
                { "id": "left", "type": "column", "children": [
                    { "id": "terminal", "type": "block", "content": { "terminal": "t1" } }
                ] },
                { "id": "right", "type": "column", "children": [
                    { "id": "neighbor", "type": "block", "content": { "mod": "neighbor" } }
                ] }
            ]
        }));
        cx.update(|window, cx| view.update(cx, |this, cx| {
            let saved = |this: &mut super::AppWindow| -> crate::layout::Node {
                this.save();
                serde_json::from_str(&std::fs::read_to_string(super::layout_path("test")).unwrap()).unwrap()
            };
            let original = this.chrome.clone();
            this.terms["terminal"].view.update(cx, |v, _| v.ended = Some("holder_lost".into()));
            this.terminal_ended("terminal", window, cx);
            assert_eq!(this.chrome, original);
            assert_eq!(saved(this), original, "saving while disconnected must retain the slot");
            this.live.insert("t1".into(), json!({"pid": 1, "state": "live"}));
            this.tend_terminals(false, window, cx);
            assert!(this.terms["terminal"].view.read(cx).ended.is_none(), "recovered terminal reattaches");

            let mut closed = original.clone();
            closed.remove_slot("terminal");
            this.terms["terminal"].view.update(cx, |v, _| v.ended = Some("exited".into()));
            this.terminal_ended("terminal", window, cx);
            assert_eq!(this.chrome, closed, "exit removes only the slot");
            assert_eq!(saved(this), closed);

            let error = |kind| Err(crate::hc::RpcError(json!({"data": {"kind": kind}})).into());
            for (result, stale, removed) in [
                (Ok(json!({"state": "finished", "reason": "holder_lost"})), false, true),
                (error("not_found"), false, false),
                (error("unavailable"), false, false),
                (Ok(json!({"state": "finished", "reason": "holder_lost"})), true, false),
                (Ok(json!({"state": "live", "reason": "holder_lost"})), false, false),
            ] {
                this.chrome = original.clone();
                this.sync(window, cx);
                let generation = this.core_generation;
                if stale { this.core_generation += 1; }
                this.checking_closed = true;
                this.checked_closed(generation, vec![("terminal".into(), "t1".into(), result)], window, cx);
                assert!(!this.checking_closed);
                let expected = if removed { &closed } else { &original };
                assert_eq!(&this.chrome, expected);
                assert_eq!(&saved(this), expected);
            }
        }));
    }

    #[test]
    fn each_toast_design_has_its_box_and_place() {
        in_own_process("toast_designs");
    }

    /// Switching `toast_design` redraws open toasts in their new box; banners
    /// span the window on the vertical half of `toast_position`, one at a
    /// time, the newest counting the rest; leaving banners shows them all again.
    #[gpui::test]
    fn toast_designs(cx: &mut gpui::TestAppContext) {
        use super::{BANNER_HEIGHT, ICON_CARD_ACTIONS, ICON_CARD_HEIGHT, PILL_HEIGHT, TOAST_HEIGHT};
        use crate::settings::{ToastDesign, ToastPosition};
        use serde_json::json;
        if !running_alone("toast_designs") {
            return;
        }
        let (view, cx) = open_window(cx, "toast", include_str!("../defaults/mods/toast/main.js"), json!({"id": "root", "type": "block"}));
        cx.run_until_parked();
        let design = |d: ToastDesign, position: ToastPosition, cx: &mut gpui::VisualTestContext| {
            view.update_in(cx, |this, window, cx| {
                this.person.toast_design = d;
                this.person.toast_position = position;
                this.apply_look(window, cx);
            });
            cx.run_until_parked();
        };
        let open = |id: &str, note: serde_json::Value, cx: &mut gpui::VisualTestContext| {
            view.update_in(cx, |this, window, cx| this.open_overlay(id.into(), json!({ "kind": "toast", "notification": note }), window, cx));
            cx.run_until_parked();
        };
        let at = |id: &str, cx: &mut gpui::VisualTestContext| cx.debug_bounds(format!("overlay-{id}").leak());
        let more = |cx: &mut gpui::VisualTestContext| view.update(cx, |this, _| this.toast_props().into_iter().map(|(id, p)| (id, p.get("toast_more").and_then(|n| n.as_u64()))).collect::<Vec<_>>());
        let viewport = cx.update(|window, _| window.viewport_size());
        open("a", json!({ "title": "Saved" }), cx);
        assert_eq!(at("a", cx).unwrap().size.height, px(TOAST_HEIGHT), "unset: the card");
        design(ToastDesign::Pill, ToastPosition::BottomRight, cx);
        assert_eq!(at("a", cx).unwrap().size.height, px(PILL_HEIGHT), "an open toast takes the new design");
        let mark = |cx: &mut gpui::VisualTestContext| view.update(cx, |this, _| this.toast_props()[0].1["toast_mark"].clone());
        assert_eq!(mark(cx), json!("dot"), "the props carry the mark the pill takes for the stripe");
        open("b", json!({ "title": "Agent finished", "body": "work / api", "level": "info", "action": { "name": "agent.focus", "args": { "terminal": "t1" } } }), cx);
        assert_eq!(at("b", cx).unwrap().size.height, px(PILL_HEIGHT));
        design(ToastDesign::IconCard, ToastPosition::BottomRight, cx);
        assert_eq!((at("a", cx).unwrap().size.height, at("b", cx).unwrap().size.height), (px(ICON_CARD_HEIGHT), px(ICON_CARD_HEIGHT + ICON_CARD_ACTIONS)), "buttons only for the action");
        assert!(cx.debug_bounds("toast-show").is_some() && cx.debug_bounds("toast-dismiss").is_some());
        assert!(cx.debug_bounds("toast-icon").is_some(), "the level icon");
        // Banners: the newest only, across the window on its bottom edge, saying one more.
        design(ToastDesign::Banner, ToastPosition::BottomRight, cx);
        assert!(at("a", cx).is_none(), "one banner at a time");
        let b = at("b", cx).unwrap();
        assert_eq!((b.size.width, b.size.height, b.origin.x, b.bottom()), (viewport.width, px(BANNER_HEIGHT), px(0.0), viewport.height));
        assert_eq!(more(cx), [("a".to_owned(), None), ("b".to_owned(), Some(1))]);
        assert!(cx.debug_bounds("toast-more").is_some());
        open("c", json!({ "title": "Copied" }), cx);
        assert_eq!(more(cx), [("a".to_owned(), None), ("b".to_owned(), None), ("c".to_owned(), Some(2))]);
        assert!(at("b", cx).is_none() && at("c", cx).is_some());
        // On top, below the chrome: the vertical half of the position.
        design(ToastDesign::Banner, ToastPosition::TopLeft, cx);
        let chrome = px(view.update(cx, |this, _| this.top_chrome.get().1));
        let c = at("c", cx).unwrap();
        assert_eq!((c.size.width, c.top()), (viewport.width, chrome));
        // Closing the shown one shows the next, which counts what's left.
        view.update_in(cx, |this, window, cx| this.close_overlay("c", window, cx));
        cx.run_until_parked();
        assert!(at("b", cx).is_some());
        assert_eq!(more(cx), [("a".to_owned(), None), ("b".to_owned(), Some(1))]);
        // Back to cards: all show, none counts.
        design(ToastDesign::Card, ToastPosition::BottomRight, cx);
        assert!(at("a", cx).is_some() && at("b", cx).is_some());
        assert_eq!(more(cx), [("a".to_owned(), None), ("b".to_owned(), None)]);
        assert_eq!(at("a", cx).unwrap().size.height, px(TOAST_HEIGHT));
    }

    #[test]
    fn a_waiting_banner_gets_its_whole_time_once_shown() {
        in_own_process("banner_waits");
    }

    /// Banners show the newest error or warning before any routine one; one
    /// waiting behind doesn't expire (the poll skips it) and gets its whole
    /// time when it comes forward, as does every one when toasts stop being banners.
    #[gpui::test]
    fn banner_waits(cx: &mut gpui::TestAppContext) {
        use crate::settings::ToastDesign;
        use serde_json::json;
        use std::time::{Duration, Instant};
        if !running_alone("banner_waits") {
            return;
        }
        let (view, cx) = open_window(cx, "toast", include_str!("../defaults/mods/toast/main.js"), json!({"id": "root", "type": "block"}));
        cx.run_until_parked();
        let design = |d: ToastDesign, cx: &mut gpui::VisualTestContext| {
            view.update_in(cx, |this, window, cx| {
                this.person.toast_design = d;
                this.apply_look(window, cx);
            });
            cx.run_until_parked();
        };
        let open = |id: &str, level: &str, cx: &mut gpui::VisualTestContext| {
            view.update_in(cx, |this, window, cx| this.open_overlay(id.into(), json!({ "kind": "toast", "notification": { "title": id, "level": level } }), window, cx));
            cx.run_until_parked();
        };
        let shown = |cx: &mut gpui::VisualTestContext| view.update(cx, |this, _| this.shown_banner().map(|o| o.id.clone()));
        let open_ids = |cx: &mut gpui::VisualTestContext| view.update(cx, |this, _| this.overlays.iter().map(|o| o.id.clone()).collect::<Vec<_>>());
        let left = |id: &str, cx: &mut gpui::VisualTestContext| view.update(cx, |this, _| this.overlays.iter().find(|o| o.id == id).and_then(|o| o.expires).map(|e| e.saturating_duration_since(Instant::now())));
        // Its time ran out long ago: the poll would close it if it were counting.
        let age = |id: &str, cx: &mut gpui::VisualTestContext| view.update(cx, |this, _| {
            let o = this.overlays.iter_mut().find(|o| o.id == id).unwrap();
            let total = o.expires.unwrap().saturating_duration_since(o.opened);
            o.opened = Instant::now() - total - Duration::from_secs(1);
            o.expires = Some(Instant::now() - Duration::from_secs(1));
        });
        let poll = |cx: &mut gpui::VisualTestContext| {
            view.update_in(cx, |this, window, cx| this.poll(window, cx));
            cx.run_until_parked();
        };
        design(ToastDesign::Banner, cx);
        open("error", "error", cx);
        open("info", "info", cx);
        assert_eq!(shown(cx).as_deref(), Some("error"), "an error before a newer routine banner");
        age("info", cx);
        poll(cx);
        assert!(open_ids(cx).contains(&"info".to_owned()), "a waiting banner doesn't expire");
        view.update_in(cx, |this, window, cx| this.close_overlay("error", window, cx));
        cx.run_until_parked();
        assert_eq!(shown(cx).as_deref(), Some("info"));
        assert!(left("info", cx).is_some_and(|d| d > Duration::from_millis(4500)), "its whole 5 s from now: {:?}", left("info", cx));
        // Two routine ones: the older waits, then gets its whole time.
        open("newer", "info", cx);
        assert_eq!(shown(cx).as_deref(), Some("newer"));
        age("info", cx);
        poll(cx);
        view.update_in(cx, |this, window, cx| this.close_overlay("newer", window, cx));
        cx.run_until_parked();
        assert!(left("info", cx).is_some_and(|d| d > Duration::from_millis(4500)));
        // Leaving banners: what waited counts from then.
        open("last", "info", cx);
        age("info", cx);
        design(ToastDesign::Card, cx);
        poll(cx);
        assert!(open_ids(cx).contains(&"info".to_owned()) && left("info", cx).is_some_and(|d| d > Duration::from_millis(4500)), "{:?}", left("info", cx));
    }

    #[test]
    fn the_time_left_bar_counts_down_and_stops() {
        in_own_process("toast_progress");
    }

    /// `toast_progress`: a bar along the toast's bottom as wide as the share
    /// of its time left, stepped by a timer that runs only while such a toast
    /// shows; none for a toast kept until clicked, nor with the setting off.
    #[gpui::test]
    fn toast_progress(cx: &mut gpui::TestAppContext) {
        use crate::settings::ToastDuration;
        use serde_json::json;
        use std::time::{Duration, Instant};
        if !running_alone("toast_progress") {
            return;
        }
        let (view, cx) = open_window(cx, "toast", include_str!("../defaults/mods/toast/main.js"), json!({"id": "root", "type": "block"}));
        cx.run_until_parked();
        let set = |progress: bool, duration: ToastDuration, cx: &mut gpui::VisualTestContext| {
            view.update_in(cx, |this, window, cx| {
                this.person.toast_progress = progress;
                this.person.toast_duration = duration;
                this.apply_look(window, cx);
            });
            cx.run_until_parked();
        };
        let open = |id: &str, cx: &mut gpui::VisualTestContext| {
            view.update_in(cx, |this, window, cx| this.open_overlay(id.into(), json!({ "kind": "toast", "notification": { "title": "Saved" } }), window, cx));
            cx.run_until_parked();
        };
        let ticking = |cx: &mut gpui::VisualTestContext| view.update(cx, |this, _| this.progress_tick.is_some());
        let bar = |id: &str, cx: &mut gpui::VisualTestContext| cx.debug_bounds(format!("progress-{id}").leak());
        let step = |cx: &mut gpui::VisualTestContext| {
            cx.executor().advance_clock(Duration::from_millis(600));
            cx.run_until_parked();
        };
        open("off", cx);
        assert!(bar("off", cx).is_none() && !ticking(cx), "unset: no bar, no timer");
        set(true, ToastDuration::Normal, cx);
        let toast = cx.debug_bounds("overlay-off").unwrap();
        let full = bar("off", cx).expect("on: an open toast shows its bar");
        assert!(ticking(cx));
        // Along the bottom inside the border, in from the sides by the corner radius (8 px rounded), so the clip never cuts it.
        let inside = toast.inset(px(1.0));
        let track = inside.size.width - px(16.0);
        assert!((full.bottom() - inside.bottom()).abs() <= px(1.0) && (full.left() - (inside.left() + px(8.0))).abs() <= px(1.0) && full.size.height == px(2.0), "{full:?} in {toast:?}");
        assert!(full.size.width > track * 0.9 && full.size.width <= track, "nearly all its time left: {full:?} of {toast:?}");
        // Half its time gone: half as wide.
        view.update(cx, |this, _| {
            let o = this.overlays.iter_mut().find(|o| o.id == "off").unwrap();
            o.opened = Instant::now() - Duration::from_millis(2500);
            o.expires = Some(Instant::now() + Duration::from_millis(2500));
        });
        step(cx);
        let half = bar("off", cx).unwrap();
        assert!((half.size.width - track * 0.5).abs() < px(6.0), "{half:?} of {toast:?}");
        // Gone with its toast; the timer stops at its next step.
        view.update_in(cx, |this, window, cx| this.close_overlay("off", window, cx));
        step(cx);
        assert!(!ticking(cx), "no toast, no timer");
        // Kept until clicked: no bar, no timer.
        set(true, ToastDuration::UntilDismissed, cx);
        open("kept", cx);
        assert!(bar("kept", cx).is_none() && !ticking(cx), "until clicked: nothing to count");
        // Off again: the timer stops with the bars.
        set(true, ToastDuration::Normal, cx);
        open("timed", cx);
        assert!(ticking(cx) && bar("timed", cx).is_some());
        set(false, ToastDuration::Normal, cx);
        step(cx);
        assert!(!ticking(cx) && bar("timed", cx).is_none());
        // In from the sides by the radius the design draws: half a pill's
        // height, none for the terminal's and the banner's square corners.
        use crate::settings::ToastDesign;
        let design = |d: ToastDesign, cx: &mut gpui::VisualTestContext| {
            view.update_in(cx, |this, window, cx| {
                for id in this.overlays.iter().map(|o| o.id.clone()).collect::<Vec<_>>() {
                    this.close_overlay(&id, window, cx);
                }
                this.person.toast_design = d;
                this.apply_look(window, cx);
            });
            cx.run_until_parked();
        };
        set(true, ToastDuration::Normal, cx);
        for (d, inset, edge) in [(ToastDesign::Pill, 16.0, 1.0), (ToastDesign::Terminal, 0.0, 1.0), (ToastDesign::Banner, 0.0, 0.0)] {
            design(d, cx);
            open("shape", cx);
            let (toast, bar) = (cx.debug_bounds("overlay-shape").unwrap(), bar("shape", cx).unwrap());
            assert!((bar.left() - (toast.left() + px(edge + inset))).abs() <= px(1.0), "{d:?}: {bar:?} in {toast:?}");
        }
        // A banner waiting behind one kept until clicked draws no bar, so it steps nothing.
        design(ToastDesign::Banner, cx);
        set(true, ToastDuration::UntilDismissed, cx);
        view.update_in(cx, |this, window, cx| {
            this.open_overlay("kept".into(), json!({ "kind": "toast", "notification": { "title": "Failed", "level": "error" } }), window, cx);
            this.show_copied(window, cx);
        });
        cx.run_until_parked();
        assert_eq!(view.update(cx, |this, _| this.shown_banner().map(|o| o.id.clone())).as_deref(), Some("kept"));
        assert!(view.update(cx, |this, _| this.copied_toast.is_some()), "Copied is open, waiting");
        step(cx);
        assert!(!ticking(cx), "a waiting Copied runs no timer");
    }

    #[test]
    fn a_toast_is_two_lines_high_only_with_a_body_the_mod_draws() {
        use super::{TOAST_HEIGHT, TOAST_HEIGHT_WITH_BODY, toast_height};
        use serde_json::json;
        for (body, two) in [
            (json!(null), false),
            (json!(""), false),
            (json!("  \n "), false),
            (json!("\u{feff} "), false),
            (json!(false), false),
            (json!(0), false),
            (json!("to the clipboard"), true),
            (json!(3), true),
            (json!(true), true),
            (json!(["a"]), true),
        ] {
            let want = if two { TOAST_HEIGHT_WITH_BODY } else { TOAST_HEIGHT };
            assert_eq!(toast_height(&json!({ "title": "t", "body": body })), want, "{body}");
        }
        assert_eq!(toast_height(&json!({ "title": "t" })), TOAST_HEIGHT);
    }

    #[test]
    fn toasts_are_as_wide_as_what_they_say() {
        in_own_process("toast_sizes");
    }

    #[gpui::test]
    fn toast_sizes(cx: &mut gpui::TestAppContext) {
        use super::{TOAST_HEIGHT, TOAST_HEIGHT_WITH_BODY, TOAST_MAX_WIDTH, TOAST_MIN_WIDTH};
        use serde_json::json;
        if !running_alone("toast_sizes") {
            return;
        }
        // The shipped mod, its root and text column keyed per toast so their bounds can be read.
        let shipped = include_str!("../defaults/mods/toast/main.js");
        let keyed = shipped.replace(r#".id("toast")"#, r#".id(`toast-${p.overlay}`)"#).replace("v_flex()\n        .flex_1()", "v_flex()\n        .id(`text-${p.overlay}`)\n        .flex_1()");
        // Two roots (the card's, and the other designs') and the card's text column.
        assert_eq!(keyed.matches("${p.overlay}").count(), 3, "the toast mod changed shape: key its roots and text column again");
        let (view, cx) = open_window(cx, "toast", &keyed, json!({"id": "root", "type": "block"}));
        let long = "A notification whose title goes on far longer than any toast is wide";
        let toasts = [
            ("short", json!({"title": "Copied"})),
            ("medium", json!({"title": "Terminal failed to start"})),
            ("long", json!({"title": long})),
            ("body", json!({"title": "Copied", "body": "to the clipboard"})),
        ];
        view.update_in(cx, |this, window, cx| {
            for (id, note) in &toasts {
                this.open_overlay((*id).into(), json!({"kind": "toast", "notification": note}), window, cx);
            }
            this.open_overlay("tall".into(), json!({"kind": "toast", "height": 90, "notification": {"title": "Set"}}), window, cx);
        });
        cx.run_until_parked();
        let [short, medium, long, body, tall] =
            ["overlay-short", "overlay-medium", "overlay-long", "overlay-body", "overlay-tall"].map(|s| cx.debug_bounds(s).unwrap_or_else(|| panic!("no {s}")));
        assert_eq!(short.size, size(px(TOAST_MIN_WIDTH), px(TOAST_HEIGHT)), "a word is the narrowest, one line high");
        assert!(medium.size.width > px(TOAST_MIN_WIDTH) && medium.size.width < px(TOAST_MAX_WIDTH), "fits its title: {:?}", medium.size);
        assert_eq!(long.size, size(px(TOAST_MAX_WIDTH), px(TOAST_HEIGHT)), "a long title is cut at the widest");
        assert_eq!(body.size.height, px(TOAST_HEIGHT_WITH_BODY));
        assert_eq!(tall.size.height, px(90.0), "an explicit height wins");
        // Stacked upward in the order opened, all on the same right edge.
        let stack = [short, medium, long, body, tall];
        for pair in stack.windows(2) {
            assert_eq!(pair[1].bottom(), pair[0].top() - px(8.0), "{stack:?}");
            assert_eq!(pair[1].right(), pair[0].right());
        }
        // The mod fills its pill inside the border, the text centred top to bottom.
        for (id, pill) in ["short", "medium", "long", "body", "tall"].iter().zip(stack) {
            let content = cx.debug_bounds(format!("toast-{id}").leak()).unwrap_or_else(|| panic!("no toast-{id}"));
            let text = cx.debug_bounds(format!("text-{id}").leak()).unwrap_or_else(|| panic!("no text-{id}"));
            assert_eq!(content, pill.inset(px(1.0)), "{id}: the content fills the pill inside its border");
            assert!((text.center().y - pill.center().y).abs() < px(0.5), "{id}: text centred: text {text:?}, pill {pill:?}");
        }
        // Every `toast_position`, with and without `toast_offset`: the stack
        // sits on its edge and side, bottom stacks grow up (the oldest
        // nearest the edge), top stacks down (the newest nearest it).
        use crate::settings::{ToastOffset, ToastPosition, ToastSide};
        let viewport = cx.update(|window, _| window.viewport_size());
        let near = |a: gpui::Pixels, b: gpui::Pixels| (a - b).abs() < px(0.5);
        for position in ToastPosition::ALL {
            for offset in [None, Some(ToastOffset { x: 10.0, y: 24.0 })] {
                view.update_in(cx, |this, window, cx| {
                    this.person.toast_position = position;
                    this.person.toast_offset = offset;
                    this.apply_look(window, cx);
                });
                cx.run_until_parked();
                let stack = ["short", "medium", "long", "body", "tall"].map(|id| cx.debug_bounds(format!("overlay-{id}").leak()).unwrap());
                let o = offset.unwrap_or_default();
                let (side, edge) = (px(16.0 + o.x), px(16.0 + o.y));
                let at = format!("{position:?} {offset:?}: {stack:?}");
                if position.top() {
                    // Past the window's top chrome: this layout's tab bar, `toasts_below_the_chrome`.
                    let chrome = px(view.update(cx, |this, _| this.top_chrome.get().1));
                    assert!(chrome > px(0.0), "{at}");
                    assert!(near(stack[4].top(), chrome + edge), "the newest below the top chrome, {at}");
                    for pair in stack.windows(2) {
                        assert!(near(pair[0].top(), pair[1].bottom() + px(8.0)), "older below newer, {at}");
                    }
                } else {
                    assert!(near(stack[0].bottom(), viewport.height - edge), "the oldest on the bottom edge, {at}");
                    for pair in stack.windows(2) {
                        assert!(near(pair[1].bottom(), pair[0].top() - px(8.0)), "newer above older, {at}");
                    }
                }
                for b in stack {
                    let lined = match position.side() {
                        ToastSide::Left => near(b.left(), side),
                        ToastSide::Right => near(b.right(), viewport.width - side),
                        ToastSide::Center => near(b.center().x, viewport.width / 2.0),
                    };
                    assert!(lined, "on its side, {at}");
                }
            }
        }
    }

    #[test]
    fn top_toasts_sit_below_the_title_and_tab_bars() {
        in_own_process("toasts_below_the_chrome");
    }

    /// A top stack starts the margin below the bars the layout draws along
    /// the window's top (its `window_drag` nodes, as painted), at any zoom
    /// and as soon as they change; with no bar there, at the margin.
    #[gpui::test]
    fn toasts_below_the_chrome(cx: &mut gpui::TestAppContext) {
        use crate::settings::ToastPosition;
        use serde_json::json;
        if !running_alone("toasts_below_the_chrome") {
            return;
        }
        let layout = json!({"id": "root", "type": "column", "children": [
            {"id": "bar", "type": "block", "size": {"px": 40}, "window_drag": true, "content": {"terminal": "t1"}},
            {"id": "body", "type": "block", "content": {"terminal": "t2"}},
        ]});
        let (view, cx) = open_window(cx, "toast", include_str!("../defaults/mods/toast/main.js"), layout);
        let near = |a: gpui::Pixels, b: gpui::Pixels| (a - b).abs() < px(0.5);
        for (position, zoom) in [(ToastPosition::TopRight, 1.0), (ToastPosition::TopCenter, 1.25), (ToastPosition::TopLeft, 1.0)] {
            view.update_in(cx, |this, window, cx| {
                this.person.toast_position = position;
                this.person.ui_zoom = zoom;
                this.apply_look(window, cx);
                this.open_overlay(format!("t{zoom}{position:?}"), json!({"kind": "toast", "notification": {"title": "Done"}}), window, cx);
            });
            cx.run_until_parked();
            let bar = view.update(cx, |this, _| this.bounds.borrow()["bar"]);
            let toast = cx.debug_bounds(format!("overlay-t{zoom}{position:?}").leak()).unwrap();
            let margin = px(16.0 * zoom);
            assert!(near(bar.size.height, px(40.0 * zoom)), "the bar is drawn at zoom {zoom}: {bar:?}");
            assert!(near(toast.top(), bar.bottom() + margin), "{position:?} at zoom {zoom}: toast {toast:?}, bar {bar:?}");
            view.update_in(cx, |this, window, cx| {
                let ids: Vec<String> = this.overlays.iter().map(|o| o.id.clone()).collect();
                ids.iter().for_each(|id| this.close_overlay(id, window, cx));
            });
        }
        // The bar grows with nothing else to redraw: the overlays draw after
        // the layout has placed it, so that frame already puts the toast below
        // the new bar and asks for no other.
        view.update_in(cx, |this, window, cx| {
            this.person.toast_position = ToastPosition::TopRight;
            this.person.ui_zoom = 1.0;
            this.apply_look(window, cx);
            this.open_overlay("grow".into(), json!({"kind": "toast", "notification": {"title": "Done"}}), window, cx);
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        let toast_top = |cx: &mut gpui::VisualTestContext| cx.debug_bounds("overlay-grow").unwrap().top();
        let before = view.update(cx, |this, _| this.bounds.borrow()["bar"]);
        assert!(near(toast_top(cx), before.bottom() + px(16.0)), "settled below the 40 px bar");
        view.update(cx, |this, cx| {
            if let Some(bar) = this.chrome.find_mut("bar") {
                bar.size = Some(crate::layout::Size::Px { px: 80.0 });
            }
            cx.notify();
        });
        cx.run_until_parked();
        let grown = view.update(cx, |this, _| this.bounds.borrow()["bar"]);
        assert!(near(grown.size.height, px(80.0)), "{grown:?}");
        assert!(
            near(toast_top(cx), grown.bottom() + px(16.0)),
            "the frame that grew the bar placed the toast below it"
        );
        assert_eq!(
            cx.update(|window, cx| window.simulate_next_frame(cx)),
            0,
            "and settled"
        );
        view.update_in(cx, |this, window, cx| this.close_overlay("grow", window, cx));
        // The bottom of the window keeps its margin, whatever the top draws.
        view.update_in(cx, |this, window, cx| {
            this.person.toast_position = ToastPosition::BottomRight;
            this.person.ui_zoom = 1.0;
            this.apply_look(window, cx);
            this.open_overlay("low".into(), json!({"kind": "toast", "notification": {"title": "Done"}}), window, cx);
        });
        cx.run_until_parked();
        let viewport = cx.update(|window, _| window.viewport_size());
        assert!(near(cx.debug_bounds("overlay-low").unwrap().bottom(), viewport.height - px(16.0)));
    }

    /// The window's top chrome: where the root starts (below a system title
    /// strip), then the lowest drag bar starting there; a bar further down,
    /// or one too tall to be a bar, doesn't count, nor does a plain node.
    #[test]
    fn the_top_chrome_is_the_lowest_bar_along_the_top() {
        use super::top_chrome;
        use gpui::Bounds;
        let at = |y: f32, h: f32| Bounds::new(point(px(0.0), px(y)), size(px(800.0), px(h)));
        let viewport = size(px(800.0), px(600.0));
        let root = top_chrome((0.0, 0.0), at(28.0, 572.0), true, false, viewport);
        assert_eq!(root, (28.0, 28.0), "the root starts it below the system strip");
        let painted = [(at(28.0, 42.0), true), (at(28.0, 46.0), true), (at(560.0, 40.0), true), (at(28.0, 572.0), true), (at(28.0, 300.0), false)];
        let chrome = painted.iter().fold(root, |c, (b, drag)| top_chrome(c, *b, false, *drag, viewport));
        assert_eq!(chrome, (28.0, 74.0), "the brand bar, the lowest at the top");
        // Bars stacked: a tab bar under a title bar counts; one below a gap doesn't.
        let stacked = [at(28.0, 30.0), at(58.0, 40.0), at(120.0, 30.0)].iter().fold(root, |c, b| top_chrome(c, *b, false, true, viewport));
        assert_eq!(stacked, (28.0, 98.0));
        assert_eq!(top_chrome((5.0, 90.0), at(0.0, 1000.0), true, false, viewport), (0.0, 0.0), "a new frame starts again");
    }

    /// A stack's spots: from `start`, each toast's height and the gap apart,
    /// the oldest first at the bottom and the newest first at the top.
    #[test]
    fn toasts_stack_away_from_their_edge() {
        use super::toast_stack;
        assert_eq!(toast_stack(&[40.0, 72.0, 40.0], 16.0, false), (vec![16.0, 64.0, 144.0], 192.0));
        assert_eq!(toast_stack(&[40.0, 72.0, 40.0], 16.0, true), (vec![144.0, 64.0, 16.0], 192.0));
        assert_eq!(toast_stack(&[], 58.0, false), (vec![], 58.0));
    }

    /// `toast_offset.y` moves the whole stack, to the edge at most, and never
    /// under the update pill.
    #[test]
    fn a_toast_offset_moves_the_stack_and_keeps_clear_of_the_pill() {
        use super::toast_start;
        assert_eq!(toast_start(0.0, 0.0, false), 16.0);
        assert_eq!(toast_start(0.0, 24.0, false), 40.0);
        assert_eq!(toast_start(0.0, -100.0, false), 0.0);
        assert_eq!(toast_start(0.0, 0.0, true), 58.0);
        assert_eq!(toast_start(0.0, 24.0, true), 82.0);
        assert_eq!(toast_start(0.0, -100.0, true), 58.0);
        assert_eq!(toast_start(42.0, 0.0, false), 58.0, "a top stack starts past the chrome");
        assert_eq!(toast_start(42.0, 10.0, false), 68.0);
    }

    #[test]
    fn mods_follow_ui_zoom_and_text_scale_live() {
        in_own_process("mod_ui_scales");
    }

    /// settings.json's ui_zoom grows a mod's px; ui_text_scale only its text.
    #[gpui::test]
    fn mod_ui_scales(cx: &mut gpui::TestAppContext) {
        use serde_json::json;
        if !running_alone("mod_ui_scales") {
            return;
        }
        let main_js = r#"import { View, div } from "gpui-kit";
export default class Scale extends View {
  render() {
    return div().flex().flex_col().items_start()
      .child(div().id("gap").w(40).h(10).flex_none())
      .child(div().id("rem-gap").w("2rem").h(10).flex_none())
      .child(div().id("label").flex_none().text_size(20).line_height(1).child("Hg"))
      .child(div().id("px-line").flex_none().text_size(20).line_height("24px").child("Hg"))
      .child(div().id("rem-text").flex_none().text_size("1rem").line_height(1).child("Hg"))
      .child(div().id("base-text").flex_none().text_base().line_height(1).child("Hg"));
  }
}"#;
        let (view, cx) = open_window(cx, "scale", main_js, json!({"id": "root", "type": "block", "content": {"mod": "scale"}}));
        // (zoom, ui_zoom, ui_text_scale): px grow by `spacing`, text by `text`.
        for ((zoom, ui_zoom, ui_text_scale), spacing, text) in [
            ((1.0, 1.0, 1.0), 1.0, 1.0),
            ((1.0, 1.2, 1.0), 1.2, 1.2),
            ((1.0, 1.0, 1.1), 1.0, 1.1),
            ((1.0, 1.2, 1.1), 1.2, 1.32),
            ((1.5, 1.0, 1.0), 1.5, 1.5),
            ((1.5, 1.2, 1.1), 1.8, 1.98),
            // zoom × ui_zoom stops at the largest zoom.
            ((3.0, 1.5, 1.0), 3.0, 3.0),
            ((1.0, 1.0, 1.0), 1.0, 1.0),
        ] {
            let person = crate::settings::Person { zoom, ui_zoom, ui_text_scale, ..crate::settings::Person::default() };
            crate::settings::write_person(&person).unwrap();
            view.update_in(cx, |this, window, cx| {
                this.watched.settings = None;
                this.reload_config(window, cx);
            });
            cx.run_until_parked();
            let (gap, label) = (cx.debug_bounds("gap").expect("no gap"), cx.debug_bounds("label").expect("no label"));
            let at = format!("zoom {zoom}, ui_zoom {ui_zoom}, ui_text_scale {ui_text_scale}");
            // Layout rounds to device pixels (half px here).
            let near = |id: &str, size: gpui::Pixels, want: f32| assert!((size - px(want)).abs() <= px(0.25), "{at}: {id} is {size:?}, not {want}");
            near("gap width", gap.size.width, 40.0 * spacing);
            near("gap height", gap.size.height, 10.0 * spacing);
            near("2rem", cx.debug_bounds("rem-gap").unwrap().size.width, 26.0 * spacing);
            near("text", label.size.height, 20.0 * text);
            near("a 24px line", cx.debug_bounds("px-line").unwrap().size.height, 24.0 * text);
            near("1rem text", cx.debug_bounds("rem-text").unwrap().size.height, 13.0 * text);
            near("text_base()", cx.debug_bounds("base-text").unwrap().size.height, 13.0 * text);
            assert_eq!(crate::settings::font_size(), 14.0 * zoom, "{at}: terminal text stays at zoom");
        }
    }

    #[test]
    fn mods_see_the_scale_of_the_screen_the_window_is_on() {
        in_own_process("mod_scale_factor");
    }

    /// `window().scale_factor` follows the window between screens, and the
    /// mods that read it redraw.
    #[gpui::test]
    fn mod_scale_factor(cx: &mut gpui::TestAppContext) {
        use serde_json::json;
        if !running_alone("mod_scale_factor") {
            return;
        }
        let main_js = r#"import { View, div } from "gpui-kit";
import { window } from "herdr";
export default class Scale extends View {
  render() {
    return div().child(div().id("scaled").w(10 * (window()?.scale_factor ?? 0)).h(10).flex_none());
  }
}"#;
        let (_view, cx) = open_window(cx, "scale", main_js, json!({"id": "root", "type": "block", "content": {"mod": "scale"}}));
        for scale in [2.0, 1.0, 2.0] {
            cx.simulate_scale_factor_change(scale);
            cx.run_until_parked();
            let width = cx.debug_bounds("scaled").expect("no scaled").size.width;
            assert_eq!(width, px(10.0 * scale), "at scale {scale}");
        }
    }

    #[test]
    fn a_mod_section_key_beats_the_kits_tab_from_the_first_start() {
        in_own_process("mod_section_tab");
    }

    /// Fetch's case: keys.json's `mod:<name>` binds tab inside a popup whose
    /// search field has the keyboard. From a fresh start (no keys.json reload
    /// after the mod loaded) tab goes to the mod as a command and the field
    /// keeps the keyboard; shift-tab, unbound there, still moves focus.
    #[gpui::test]
    fn mod_section_tab(cx: &mut gpui::TestAppContext) {
        use serde_json::json;
        if !running_alone("mod_section_tab") {
            return;
        }
        let main_js = r#"import { View, div } from "gpui-kit";
import { InputState, Input } from "gpui-base";
import { nextCommand } from "herdr";
export default class Picker extends View {
  init(_props, cx) {
    this.commands = 0;
    this.input = InputState.new({});
    this.input.focus();
    cx.spawn(async (cx) => {
      for (;;) { if (await nextCommand() === "next") this.commands += 1; cx.notify(); }
    });
  }
  render() {
    return div().w_full().h(200).child(Input.new(this.input).w(100).h(20))
      .child(div().id("commands").w(10 + 10 * this.commands).h(10).flex_none());
  }
}"#;
        std::fs::create_dir_all(crate::mods::app_dir()).unwrap();
        std::fs::write(crate::keys::path(), r#"{ "mod:picker": { "tab": "picker.next" } }"#).unwrap();
        let (view, cx) = open_window(cx, "picker", main_js, json!({ "id": "main", "type": "block", "content": { "tab": "active" } }));
        cx.update(|window, cx| view.update(cx, |this, cx| {
            this.model.add_space("s1", "one", "", crate::model::Tab::new("a", "a", "pa", "t1"));
            this.model.select_tab("a");
            this.sync(window, cx);
            this.dispatch("mod.open", &json!({ "name": "picker" }), true, window, cx);
        }));
        cx.run_until_parked();
        let commands = |cx: &mut gpui::VisualTestContext| cx.debug_bounds("commands").map(|b| (f32::from(b.size.width) as usize - 10) / 10);
        let in_input = |cx: &mut gpui::VisualTestContext| cx.update(|window, _| window.context_stack().last().is_some_and(|c| c.contains("Input")));
        assert!(in_input(cx), "the field has the keyboard");
        assert_eq!(commands(cx), Some(0));
        cx.simulate_keystrokes("tab");
        cx.run_until_parked();
        assert_eq!(commands(cx), Some(1), "tab reached the mod");
        assert!(in_input(cx), "tab left the field");
        cx.simulate_keystrokes("tab");
        cx.run_until_parked();
        assert_eq!(commands(cx), Some(2));
        cx.simulate_keystrokes("shift-tab");
        cx.run_until_parked();
        assert!(!in_input(cx), "shift-tab, bound by no section, still moves focus");
    }

    #[test]
    fn a_mods_on_key_down_never_sees_tab() {
        in_own_process("mod_tab_on_key_down");
    }

    #[test]
    fn a_mod_reads_tab_from_its_keys_section() {
        in_own_process("mod_tab_section_command");
    }

    /// The contract mods rely on: tab and shift-tab move focus between a mod's
    /// focusable elements and never reach `on_key_down`, while other keys do.
    #[gpui::test]
    fn mod_tab_on_key_down(cx: &mut gpui::TestAppContext) {
        if !running_alone("mod_tab_on_key_down") {
            return;
        }
        let cx = open_tab_counter(cx, None);
        cx.simulate_keystrokes("j");
        cx.run_until_parked();
        assert_eq!(tab_counter(cx, "keys"), Some(1), "j reaches on_key_down");
        assert!(!tab_counter_in_input(cx), "the root, not the field, has the keyboard");
        cx.simulate_keystrokes("tab");
        cx.run_until_parked();
        assert_eq!(tab_counter(cx, "tabs"), Some(0), "tab never reaches on_key_down");
        assert!(tab_counter_in_input(cx), "tab moves focus to the next focusable element");
        cx.simulate_keystrokes("shift-tab");
        cx.run_until_parked();
        assert_eq!(tab_counter(cx, "tabs"), Some(0), "shift-tab never reaches on_key_down either");
    }

    /// A mod that wants tab binds it in its `mod:<name>` section: it arrives
    /// as that command through `nextCommand()`, and focus stays put.
    #[gpui::test]
    fn mod_tab_section_command(cx: &mut gpui::TestAppContext) {
        if !running_alone("mod_tab_section_command") {
            return;
        }
        let cx = open_tab_counter(cx, Some(r#"{ "mod:tabcount": { "tab": "tabcount.view" } }"#));
        cx.simulate_keystrokes("tab");
        cx.run_until_parked();
        assert_eq!(tab_counter(cx, "views"), Some(1), "tab arrives as tabcount.view");
        assert_eq!(tab_counter(cx, "tabs"), Some(0), "not through on_key_down");
        assert!(!tab_counter_in_input(cx), "the bound tab doesn't move focus");
        cx.simulate_keystrokes("j");
        cx.run_until_parked();
        assert_eq!(tab_counter(cx, "keys"), Some(1), "j still reaches on_key_down");
    }

    /// A popup whose root holds the keyboard and counts tab, shift-tab and j in
    /// `on_key_down` and `view` commands; a field after it is the next tab stop.
    fn open_tab_counter<'a>(cx: &'a mut gpui::TestAppContext, keys: Option<&str>) -> &'a mut gpui::VisualTestContext {
        use serde_json::json;
        let main_js = r#"import { View, div } from "gpui-kit";
import { InputState, Input } from "gpui-base";
import { nextCommand } from "herdr";
export default class TabCount extends View {
  init(_props, cx) {
    this.tabs = 0;
    this.keys = 0;
    this.views = 0;
    this.input = InputState.new({});
    this.handle = cx.focus_handle();
    this.handle.focus();
    cx.spawn(async (cx) => {
      for (;;) { if (await nextCommand() === "view") this.views += 1; cx.notify(); }
    });
  }
  render() {
    return div().id("tabcount").track_focus(this.handle).w(300).h(200)
      .on_key_down((e, cx) => {
        if (e.keystroke === "tab" || e.keystroke === "shift-tab") { this.tabs += 1; cx.notify(); }
        else if (e.keystroke === "j") { this.keys += 1; cx.notify(); }
      })
      .child(Input.new(this.input).w(100).h(20))
      .child(div().id("tabs").w(10 + 10 * this.tabs).h(10).flex_none())
      .child(div().id("keys").w(10 + 10 * this.keys).h(10).flex_none())
      .child(div().id("views").w(10 + 10 * this.views).h(10).flex_none());
  }
}"#;
        std::fs::create_dir_all(crate::mods::app_dir()).unwrap();
        if let Some(keys) = keys {
            std::fs::write(crate::keys::path(), keys).unwrap();
        }
        let (view, cx) = open_window(cx, "tabcount", main_js, json!({ "id": "main", "type": "block", "content": { "tab": "active" } }));
        cx.update(|window, cx| view.update(cx, |this, cx| {
            this.model.add_space("s1", "one", "", crate::model::Tab::new("a", "a", "pa", "t1"));
            this.model.select_tab("a");
            this.sync(window, cx);
            this.dispatch("mod.open", &json!({ "name": "tabcount" }), true, window, cx);
        }));
        cx.run_until_parked();
        assert!(!tab_counter_in_input(cx), "the popup's root has the keyboard");
        cx
    }

    fn tab_counter(cx: &mut gpui::VisualTestContext, id: &'static str) -> Option<usize> {
        cx.debug_bounds(id).map(|b| (f32::from(b.size.width) as usize - 10) / 10)
    }

    fn tab_counter_in_input(cx: &mut gpui::VisualTestContext) -> bool {
        cx.update(|window, _| window.context_stack().last().is_some_and(|c| c.contains("Input")))
    }

    #[test]
    fn mod_palette_actions_follow_their_folders_and_mod_open_passes_props() {
        in_own_process("mod_actions");
    }

    /// Mod `extra` declares two good palette actions and a bad one; `acts`,
    /// opened by `mod.open` with props, draws how many `modActions()` lists and
    /// its props' `n`. Status names the bad action; editing `extra`'s mod.json
    /// and then removing `extra` change the open copy's list.
    #[gpui::test]
    fn mod_actions(cx: &mut gpui::TestAppContext) {
        use serde_json::json;
        if !running_alone("mod_actions") {
            return;
        }
        let extra = crate::mods::app_dir().join("mods/extra");
        std::fs::create_dir_all(&extra).unwrap();
        std::fs::write(extra.join("main.js"), "import { View, div } from \"gpui-kit\";\nexport default class Extra extends View { render() { return div(); } }\n").unwrap();
        std::fs::write(extra.join("mod.json"), json!({ "place": "none", "actions": [
            { "title": "One", "command": "mod.open" },
            { "title": "Two", "command": "tab.new" },
            { "title": "Bad", "command": "tab.fly" },
        ] }).to_string()).unwrap();
        let main_js = r#"import { View, div } from "gpui-kit";
import { modActions, props } from "herdr";
export default class Acts extends View {
  render() {
    return div().w(200).h(100)
      .child(div().id("actions").w(10 + 10 * modActions().filter((a) => a.mod === "extra").length).h(10).flex_none())
      .child(div().id("n").w(10 + 10 * (props().n ?? 0)).h(10).flex_none());
  }
}"#;
        let (view, cx) = open_window(cx, "acts", main_js, json!({ "id": "main", "type": "block", "content": { "tab": "active" } }));
        // Its own props default, which an open call with none must not count as asked for.
        std::fs::write(crate::mods::app_dir().join("mods/acts/mod.json"), r#"{ "place": "none", "overlay": { "props": { "n": 1 } } }"#).unwrap();
        let width = |cx: &mut gpui::VisualTestContext, id: &'static str| cx.debug_bounds(id).map(|b| (f32::from(b.size.width) as usize - 10) / 10);
        cx.update(|window, cx| view.update(cx, |this, cx| this.dispatch("mod.open", &json!({ "name": "acts", "props": { "n": 3 } }), true, window, cx)));
        cx.run_until_parked();
        assert_eq!(width(cx, "n"), Some(3), "mod.open's props reach props()");
        assert_eq!(width(cx, "actions"), Some(2), "extra's good actions are listed");
        let open = |cx: &mut gpui::VisualTestContext, props: serde_json::Value| {
            cx.update(|window, cx| view.update(cx, |this, cx| this.dispatch("mod.open", &json!({ "name": "acts", "props": props }), true, window, cx)));
            cx.run_until_parked();
        };
        open(cx, json!({ "n": 5 }));
        assert_eq!(width(cx, "n"), Some(5), "other props open it again with them, not close it");
        open(cx, json!({ "n": 5 }));
        assert_eq!(width(cx, "n"), None, "the same props close it");
        open(cx, json!({ "n": 3 }));
        assert_eq!(width(cx, "n"), Some(3));
        cx.update(|window, cx| view.update(cx, |this, cx| this.dispatch("mod.open", &json!({ "name": "acts" }), true, window, cx)));
        cx.run_until_parked();
        assert_eq!(width(cx, "n"), None, "no props closes it, whatever mod.json's own props say");
        // A layout overlay takes no props: asking again with any closes it.
        let lay = crate::mods::app_dir().join("mods/lay");
        std::fs::create_dir_all(&lay).unwrap();
        std::fs::write(lay.join("main.js"), "import { View, div } from \"gpui-kit\";\nexport default class Lay extends View { render() { return div(); } }\n").unwrap();
        std::fs::write(lay.join("mod.json"), r#"{ "place": "none", "overlay": { "layout": { "id": "x", "type": "block", "content": { "mod": "lay" } } } }"#).unwrap();
        let lay_open = |cx: &mut gpui::VisualTestContext, n: u32| {
            view.update_in(cx, |this, window, cx| {
                this.sync_folders(window, cx);
                this.dispatch("mod.open", &json!({ "name": "lay", "props": { "n": n } }), true, window, cx);
                this.overlays.iter().any(|o| o.id == "mod-lay")
            })
        };
        assert!(lay_open(cx, 1));
        assert!(!lay_open(cx, 2), "a layout overlay toggles closed");
        open(cx, json!({ "n": 3 }));
        let errors = view.update_in(cx, |this, window, cx| this.control(json!({ "status": true }), window, cx))["errors"].clone();
        assert!(errors.as_array().unwrap().iter().any(|e| e == "mod extra: actions[2]: no command \"tab.fly\""), "{errors}");
        // An edit keeps the folder: only the actions changed, and the open copy still redraws.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(extra.join("mod.json"), json!({ "place": "none", "actions": [{ "title": "One", "command": "mod.open" }] }).to_string()).unwrap();
        view.update_in(cx, |this, window, cx| this.sync_folders(window, cx));
        cx.run_until_parked();
        assert_eq!(width(cx, "actions"), Some(1), "an edited mod.json's actions are listed");
        std::fs::remove_dir_all(&extra).unwrap();
        view.update_in(cx, |this, window, cx| this.sync_folders(window, cx));
        cx.run_until_parked();
        assert_eq!(width(cx, "actions"), Some(0), "a removed mod's actions go, and the open copy redraws");
        let errors = view.update_in(cx, |this, window, cx| this.control(json!({ "status": true }), window, cx))["errors"].clone();
        assert!(!errors.to_string().contains("extra"), "{errors}");
    }

    #[test]
    fn focus_moves_are_shared_as_recent_places_and_overlays_are_not() {
        in_own_process("focus_history");
    }

    /// Tabs t (panes a, b) and u (pane d). A pane the person stays on is
    /// written to the core's `herdr.focus`; a quick hop through b isn't; a
    /// popup over the window changes nothing; a mod reading `recentPlaces()`
    /// redraws as the list grows, and places another client wrote show up.
    #[gpui::test]
    fn focus_history(cx: &mut gpui::TestAppContext) {
        use crate::focus_history::{NAME, SETTLE};
        use crate::model::{Tab, pane};
        use serde_json::{Value, json};
        use std::sync::{Arc, Mutex};
        if !running_alone("focus_history") {
            return;
        }
        cx.executor().allow_parking();
        let doc = Arc::new(Mutex::new((Value::Null, 0u64)));
        let store = doc.clone();
        let (core, _calls) = crate::hc::answering(move |method, params| {
            let mut doc = store.lock().unwrap();
            match method {
                "doc.get" if params["name"] == NAME => Ok(json!({ "value": doc.0, "version": doc.1 })),
                "doc.set" if params["name"] == NAME && params["version"] == doc.1 => {
                    *doc = (params["value"].clone(), doc.1 + 1);
                    Ok(json!({ "version": doc.1 }))
                }
                "doc.set" if params["name"] == NAME => Err(json!({ "message": "stale", "data": { "kind": "conflict" } })),
                _ => Err(json!({ "message": "not in this test" })),
            }
        });
        let main_js = r#"import { View, div } from "gpui-kit";
import { recentPlaces } from "herdr";
export default class Recent extends View {
  render() {
    return div().child(div().id("recent").w(10 * recentPlaces().length).h(10).flex_none());
  }
}"#;
        let layout = json!({ "id": "root", "type": "row", "children": [
            { "id": "main", "type": "block", "content": { "tab": "active" } },
            { "id": "side", "type": "block", "size": { "px": 300 }, "content": { "mod": "recent" } },
        ] });
        let (view, cx) = open_window_on(core, cx, "recent", main_js, layout);
        view.update_in(cx, |this, window, cx| {
            this.documents.entry(String::new()).or_default().available = true;
            let mut tab = Tab::new("t", "", "a", "ta");
            tab.root = crate::layout::Node::container("t", crate::layout::Axis::Row, vec![pane("a", "ta"), pane("b", "tb")]);
            this.model.add_space("s", "", "/tmp", tab);
            this.model.add_tab("s", Tab::new("u", "", "d", "td"));
            this.model.select_tab("t");
            this.model.focus_pane("a");
            this.sync(window, cx);
        });
        // A round runs off the UI thread: tick until it lands.
        let settle = |cx: &mut gpui::VisualTestContext| {
            std::thread::sleep(SETTLE + std::time::Duration::from_millis(50));
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                view.update(cx, |this, cx| {
                    this.documents_watching = true;
                    this.flush_focus(None, cx)
                });
                cx.run_until_parked();
                if !view.update(cx, |this, _| this.focus_history.pending) { break; }
                assert!(std::time::Instant::now() < deadline, "the write never finished");
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        };
        let stored = || -> Vec<String> {
            doc.lock().unwrap().0["places"].as_array().into_iter().flatten().map(|p| p["pane"].as_str().unwrap().to_owned()).collect()
        };
        let drawn = |cx: &mut gpui::VisualTestContext| cx.debug_bounds("recent").map_or(0.0, |b| f32::from(b.size.width) / 10.0) as usize;

        settle(cx);
        assert_eq!(stored(), ["a"]);
        let first = doc.lock().unwrap().0["places"][0].clone();
        assert_eq!((first["terminal"].as_str(), first["tab"].as_str(), first["space"].as_str()), (Some("ta"), Some("t"), Some("s")));
        assert_eq!(first["machine"], Value::Null);
        assert_eq!(drawn(cx), 1, "the mod redraws with the place");

        // b for a moment, then tab u: only u's pane counts.
        view.update_in(cx, |this, window, cx| {
            this.model.focus_pane("b");
            this.sync(window, cx);
            this.model.select_tab("u");
            this.sync(window, cx);
        });
        settle(cx);
        assert_eq!(stored(), ["d", "a"]);
        assert_eq!(drawn(cx), 2);

        // A popup over the window (as Fetch opens) is no place, and leaves the one under it.
        view.update_in(cx, |this, window, cx| {
            this.open_overlay(crate::mods::new_overlay_id(), json!({ "mod": "recent", "kind": "popup" }), window, cx);
        });
        settle(cx);
        assert!(view.update(cx, |this, _| this.overlays.iter().any(|o| o.kind == super::OverlayKind::Popup)));
        assert_eq!(stored(), ["d", "a"]);
        let version = doc.lock().unwrap().1;

        // Another client's place arrives through a doc event and merges in; nothing is rewritten for it.
        {
            let mut doc = doc.lock().unwrap();
            let mut places = doc.0["places"].as_array().cloned().unwrap();
            places.insert(0, json!({ "pane": "phone", "terminal": null, "tab": "t", "space": "s", "machine": null, "at": crate::model::now_ms() + 60_000, "client": "tui" }));
            *doc = (json!({ "v": 1, "places": places }), doc.1 + 1);
        }
        // This window's own write coming back is no change.
        view.update(cx, |this, cx| {
            this.flush_focus(Some(Some(version)), cx);
            assert!(!this.focus_history.stale && !this.focus_history.pending, "an echo isn't read again");
        });
        let version = doc.lock().unwrap().1;
        view.update(cx, |this, cx| this.flush_focus(Some(Some(version)), cx));
        settle(cx);
        assert_eq!(stored(), ["phone", "d", "a"]);
        assert_eq!(doc.lock().unwrap().1, version, "only the other client wrote");
        assert_eq!(drawn(cx), 3);
        let shown = view.update(cx, |this, _| this.focus_history.places());
        assert_eq!(shown[0].extra["client"], "tui", "fields other clients add are kept");
    }

    #[test]
    fn terminal_menu_is_a_root_layout_choice_that_roundtrips() {
        let mut root = super::default_layout("main");
        assert_eq!(root.terminal_menu, None);
        assert!(serde_json::to_value(&root).unwrap().get("terminal_menu").is_none());
        root.terminal_menu = Some("personal-menu".into());
        let restored: super::Node = serde_json::from_value(serde_json::to_value(&root).unwrap()).unwrap();
        assert_eq!(restored.terminal_menu.as_deref(), Some("personal-menu"));
    }

    #[test]
    fn copy_all_text_drops_the_blank_rows_below_the_last_text() {
        assert_eq!(super::copied_text("old\n  indented\n\nlast\n\n   \n"), Some(("old\n  indented\n\nlast".into(), 4)));
        assert_eq!(super::copied_text("one"), Some(("one".into(), 1)));
        assert_eq!(super::copied_text("\n \n"), None);
        assert_eq!(super::copied_text(""), None);
        assert_eq!((super::thousands(7), super::thousands(1234), super::thousands(1_234_567)), ("7".into(), "1,234".into(), "1,234,567".into()));
        // All the history above the screen, not just the screen (`hc read` without --lines).
        let params = super::copy_all_params("m/t7");
        assert_eq!(params["terminal"], "m/t7");
        assert!(params["lines"].as_u64().is_some_and(|n| n >= 1_000_000), "{params}");
    }

    #[test]
    fn pane_menu_copy_items_act_on_the_clicked_terminal() {
        in_own_process("pane_menu_copy");
    }

    /// Two terminals, t1 focused. A selection in t2 survives a right-click on
    /// it, the menu is told it's there and where t2 is, and the copy commands
    /// given `terminal: t2` act on t2, not on the focused t1.
    #[gpui::test]
    fn pane_menu_copy(cx: &mut gpui::TestAppContext) {
        use gpui::{Modifiers, MouseButton, point, px};
        use serde_json::json;
        if !running_alone("pane_menu_copy") {
            return;
        }
        let (core, calls) = crate::hc::answering(|method, params| match method {
            "terminal.read" => {
                let t = params["terminal"].as_str().unwrap_or_default();
                Ok(json!({ "text": format!("{t} history\n{t} screen\n\n   \n"), "complete": true, "how": "history" }))
            }
            _ => Err(json!({ "message": "not in this test" })),
        });
        let menu = r#"import { View, div } from "gpui-kit"; export default class Menu extends View { render() { return div().child("menu"); } }"#;
        let (view, cx) = open_window_on(core, cx, "menu", menu, json!({ "id": "root", "type": "row", "children": [
            { "id": "left", "type": "block", "content": { "terminal": "t1" } },
            { "id": "right", "type": "block", "content": { "terminal": "t2" } }
        ] }));
        // t2's shell marks its prompts (OSC 133): a finished `make`, a prompt after it.
        let prompt = "\x1b]133;D;0\x07\x1b]133;A\x07$ \x1b]133;B\x07";
        let text = |t: &str| match t {
            "t2" => format!("t2 says hello world\r\n{prompt}make\r\n\x1b]133;C\x07built t2\r\nok\r\n{prompt}").into_bytes(),
            _ => format!("{t} says hello world\r\n").into_bytes(),
        };
        view.update_in(cx, |this, window, cx| {
            window.activate_window();
            for id in this.overlays.iter().map(|o| o.id.clone()).collect::<Vec<_>>() {
                this.close_overlay(&id, window, cx);
            }
            this.live.insert("t1".into(), json!({ "address": "t1", "foreground": { "cwd": "/work/left" } }));
            this.live.insert("t2".into(), json!({ "address": "t2", "cwd": "/started" }));
            this.cwds.insert("t2".into(), "/work/right".into());
            for (pane, t) in [("left", "t1"), ("right", "t2")] {
                let term = this.terms[pane].view.clone();
                term.update(cx, |v, cx| v.ingest(vec![crate::hc::Stream::Data(text(t))], cx));
            }
            this.run_as("pane.focus", json!({ "terminal": "t1" }), false, window, cx);
        });
        cx.executor().advance_clock(std::time::Duration::from_millis(100));
        cx.run_until_parked();
        let none = Modifiers::default();
        let origin = view.update(cx, |this, _| this.bounds.borrow()["right"].origin);
        let (from, to) = (point(origin.x + px(4.), origin.y + px(10.)), point(origin.x + px(120.), origin.y + px(10.)));
        cx.simulate_mouse_down(from, MouseButton::Left, none);
        cx.simulate_mouse_move(to, Some(MouseButton::Left), none);
        cx.simulate_mouse_up(to, MouseButton::Left, none);
        cx.run_until_parked();
        let selected = |cx: &mut gpui::VisualTestContext| view.update(cx, |this, cx| this.terms["right"].view.read(cx).has_selection());
        assert!(selected(cx), "the drag selected nothing");
        view.update_in(cx, |this, window, cx| this.run_as("pane.focus", json!({ "terminal": "t1" }), false, window, cx));
        cx.write_to_clipboard(gpui::ClipboardItem::new_string("before".into()));
        let clipboard = |cx: &mut gpui::VisualTestContext| cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default();

        // The right-click opens the menu and leaves the selection.
        cx.simulate_mouse_down(to, MouseButton::Right, none);
        cx.simulate_mouse_up(to, MouseButton::Right, none);
        cx.run_until_parked();
        assert!(selected(cx), "the right-click cleared the selection");
        let target = view.update(cx, |this, _| {
            this.copies.values().find(|c| c.inst.name == "menu").map(|c| c.inst.props["target"].clone()).expect("the menu opened")
        });
        assert_eq!(target, json!({ "pane": "right", "terminal": "t2", "selected": true, "cwd": "/work/right", "last_output": true }));
        assert_eq!(clipboard(cx), "before", "opening the menu copied");

        let run = |name: &str, args: serde_json::Value, cx: &mut gpui::VisualTestContext| {
            let reply = view.update_in(cx, |this, window, cx| this.run_as(name, args, false, window, cx));
            cx.run_until_parked();
            reply
        };
        let toast = |cx: &mut gpui::VisualTestContext| view.update(cx, |this, _| {
            let id = this.copied_toast.clone().unwrap_or_default();
            this.copies.get(&format!("overlay:{id}")).map(|c| c.inst.props["title"].clone()).unwrap_or_default()
        });
        // The focused t1 has no selection: copy without a terminal copies nothing.
        run("terminal.copy", json!(null), cx);
        assert_eq!(clipboard(cx), "before");
        run("terminal.copy", json!({ "terminal": "t2" }), cx);
        assert!(clipboard(cx).starts_with("t2 says"), "{}", clipboard(cx));
        assert!(selected(cx), "a copy command keeps the selection");

        run("terminal.copy_path", json!({ "terminal": "t2" }), cx);
        assert_eq!(clipboard(cx), "/work/right");
        run("terminal.copy_path", json!({ "terminal": "t1" }), cx);
        assert_eq!(clipboard(cx), "/work/left", "without a live folder, the core's record of the foreground's");
        assert_eq!(toast(cx), "Copied path");

        run("terminal.copy_all", json!({ "terminal": "t2" }), cx);
        assert_eq!(clipboard(cx), "t2 history\nt2 screen");
        assert_eq!(toast(cx), "Copied 2 lines");
        let reads: Vec<_> = calls.lock().unwrap().iter().filter(|(m, _)| m == "terminal.read").map(|(_, p)| p.clone()).collect();
        assert_eq!(reads, [super::copy_all_params("t2")]);
        run("terminal.copy_all", json!(null), cx);
        assert_eq!(clipboard(cx), "t1 history\nt1 screen", "without a terminal, the focused one");

        // A full-screen program is read by scrolling it: a toast says so until it's done.
        view.update(cx, |this, cx| {
            this.terms["right"].view.update(cx, |v, cx| v.ingest(vec![crate::hc::Stream::Data(b"\x1b[?1049h".to_vec())], cx));
        });
        cx.run_until_parked();
        let titles = |cx: &mut gpui::VisualTestContext| view.update(cx, |this, _| {
            this.overlays.iter().filter_map(|o| this.copies.get(&format!("overlay:{}", o.id))).map(|c| c.inst.props["title"].clone()).collect::<Vec<_>>()
        });
        view.update_in(cx, |this, window, cx| this.run_as("terminal.copy_all", json!({ "terminal": "t2" }), false, window, cx));
        assert!(titles(cx).contains(&json!("Copying…")), "{:?}", titles(cx));
        cx.run_until_parked();
        assert_eq!(toast(cx), "Copied 2 lines");
        assert!(!titles(cx).contains(&json!("Copying…")), "{:?}", titles(cx));

        let missing = run("terminal.copy_all", json!({ "terminal": "t9" }), cx);
        assert!(missing.get("error").is_some(), "{missing}");

        // The last finished command's output, from the shell's marks; t1 has
        // none. t2 still shows the full-screen program: none until it's left.
        let on_program = run("terminal.copy_last_output", json!({ "terminal": "t2" }), cx);
        assert!(on_program.get("error").is_some(), "{on_program}");
        view.update(cx, |this, cx| {
            this.terms["right"].view.update(cx, |v, cx| v.ingest(vec![crate::hc::Stream::Data(b"\x1b[?1049l".to_vec())], cx));
        });
        cx.run_until_parked();
        run("terminal.copy_last_output", json!({ "terminal": "t2" }), cx);
        assert_eq!(clipboard(cx), "built t2\nok");
        assert_eq!(toast(cx), "Copied 2 lines");
        let unmarked = run("terminal.copy_last_output", json!(null), cx);
        assert!(unmarked["error"].as_str().is_some_and(|e| e.contains("OSC 133")), "{unmarked}");
        assert_eq!(clipboard(cx), "built t2\nok", "a failed copy left the clipboard alone");
        let missing = run("terminal.copy_last_output", json!({ "terminal": "t9" }), cx);
        assert!(missing.get("error").is_some(), "{missing}");

        // A pane header's right-click (pane.menu): the same menu for the pane,
        // its terminal and folder, nothing selected.
        view.update_in(cx, |this, window, cx| {
            for id in this.overlays.iter().map(|o| o.id.clone()).collect::<Vec<_>>() {
                this.close_overlay(&id, window, cx);
            }
        });
        run("pane.menu", json!({ "pane": "left" }), cx);
        let target = view.update(cx, |this, _| {
            this.copies.values().find(|c| c.inst.name == "menu").map(|c| c.inst.props["target"].clone()).expect("pane.menu opened the menu")
        });
        assert_eq!(target, json!({ "pane": "left", "terminal": "t1", "cwd": "/work/left", "last_output": false }));
    }

    #[test]
    fn copy_all_keeps_its_copying_toast_to_itself() {
        in_own_process("copying_toast");
    }

    /// `copy_all`'s "Copying…" toast lasts as long as the read may, a
    /// selection copied meanwhile says "Copied" beside it, and the read
    /// finishing takes away only its own toast.
    #[gpui::test]
    fn copying_toast(cx: &mut gpui::TestAppContext) {
        use super::{OverlayKind, COPY_ALL_READ_LIMIT};
        use serde_json::json;
        if !running_alone("copying_toast") {
            return;
        }
        let (view, cx) = open_window(cx, "unused", "", json!({ "id": "pane", "type": "block", "content": { "terminal": "t1" } }));
        cx.run_until_parked();
        let toasts = |cx: &mut gpui::VisualTestContext| {
            view.update(cx, |this, _| this.overlays.iter().filter(|o| o.kind == OverlayKind::Toast).map(|o| o.id.clone()).collect::<Vec<_>>())
        };
        let copying = cx.update(|window, cx| view.update(cx, |this, cx| this.show_copying(window, cx)));
        let lasts = view.update(cx, |this, _| this.overlays.iter().find(|o| o.id == copying).and_then(|o| o.expires).map(|at| at.saturating_duration_since(std::time::Instant::now())));
        assert!(lasts.is_some_and(|d| d > COPY_ALL_READ_LIMIT), "Copying… ends before the read may: {lasts:?}");

        cx.update(|window, cx| view.update(cx, |this, cx| this.show_copied(window, cx)));
        let selection = view.update(cx, |this, _| this.copied_toast.clone()).unwrap();
        assert_eq!(toasts(cx), [copying.clone(), selection.clone()], "a selection's Copied doesn't replace Copying…");

        // Another copy_all failing takes nothing of this one's.
        cx.update(|window, cx| view.update(cx, |this, cx| this.copied_all(None, Err(json!({ "title": "Nothing to copy" })), window, cx)));
        assert!(toasts(cx).starts_with(&[copying.clone(), selection.clone()]), "{:?}", toasts(cx));

        // This one failing: only its own toast goes.
        cx.update(|window, cx| view.update(cx, |this, cx| this.copied_all(Some(copying.clone()), Err(json!({ "title": "Couldn't copy the terminal", "level": "error" })), window, cx)));
        let left = toasts(cx);
        assert!(!left.contains(&copying) && left.contains(&selection), "{left:?}");
        assert_eq!(left.len(), 3, "the selection's, Nothing to copy, and why this one failed");
    }

    #[test]
    fn copy_on_select_says_copied_once_and_settings_presses_do_not() {
        in_own_process("copied_toast");
    }

    /// Only a copied mouse selection says "Copied": not a click in the
    /// terminal, and not presses in the Settings window drawn over it.
    #[gpui::test]
    fn copied_toast(cx: &mut gpui::TestAppContext) {
        use serde_json::json;
        if !running_alone("copied_toast") {
            return;
        }
        use gpui::{Modifiers, MouseButton};
        let (view, cx) = open_window(cx, "unused", "", json!({ "id": "pane", "type": "block", "content": { "terminal": "t1" } }));
        cx.run_until_parked();
        let line = "hello world ".repeat(20);
        view.update(cx, |this, cx| {
            let term = this.terms["pane"].view.clone();
            term.update(cx, |t, cx| t.ingest(vec![crate::hc::Stream::Data(format!("{line}\r\n{line}\r\n").into_bytes())], cx));
        });
        cx.executor().advance_clock(std::time::Duration::from_millis(100));
        cx.run_until_parked();
        let copied = |cx: &mut gpui::VisualTestContext| {
            view.update(cx, |this, _| this.overlays.iter().filter(|o| o.kind == super::OverlayKind::Toast).count())
        };
        let none = Modifiers::default();
        let origin = view.update(cx, |this, _| this.bounds.borrow()["pane"].origin);
        let (from, to) = (point(origin.x + px(12.), origin.y + px(10.)), point(origin.x + px(120.), origin.y + px(10.)));

        cx.simulate_click(from, none);
        cx.run_until_parked();
        assert_eq!(copied(cx), 0, "a click selected nothing, yet said Copied");

        cx.simulate_mouse_down(from, MouseButton::Left, none);
        cx.simulate_mouse_move(to, Some(MouseButton::Left), none);
        cx.simulate_mouse_up(to, MouseButton::Left, none);
        cx.run_until_parked();
        assert!(cx.read_from_clipboard().and_then(|c| c.text()).is_some_and(|t| t.contains("hello")), "the drag copied nothing");
        assert_eq!(copied(cx), 1, "a copied selection says so once");

        let first = view.update(cx, |this, _| this.copied_toast.clone());
        cx.simulate_mouse_down(from, MouseButton::Left, none);
        cx.simulate_mouse_move(to, Some(MouseButton::Left), none);
        cx.simulate_mouse_up(to, MouseButton::Left, none);
        cx.run_until_parked();
        assert_eq!(copied(cx), 1, "a second quick copy stacked another toast");
        let second = view.update(cx, |this, _| this.copied_toast.clone());
        assert_ne!(first, second, "the second copy didn't replace the first toast");

        cx.update(|window, cx| view.update(cx, |this, cx| {
            this.run_as("app.settings", json!({ "section": "themes" }), false, window, cx);
        }));
        cx.run_until_parked();
        // The slot tabs: presses that write no file, whose watcher would wake
        // the test from another thread.
        for tab in ["st-tt-light", "st-tt-dark", "st-tt-light"] {
            let at = cx.debug_bounds(tab).expect("no theme tab").center();
            cx.simulate_click(at, none);
            cx.run_until_parked();
        }
        assert_eq!(copied(cx), 1, "presses in the Settings window said Copied");
    }

    #[test]
    fn the_theme_grid_lays_out_only_the_cards_on_screen() {
        in_own_process("theme_grid");
    }

    /// 120 dark themes on the Settings window's Themes page: it opens on the
    /// picked one, lays out only the rows on screen, and a scroll lays out
    /// the rows it reaches without rendering the page again.
    #[gpui::test]
    fn theme_grid(cx: &mut gpui::TestAppContext) {
        use gpui::{ScrollDelta, ScrollWheelEvent};
        use serde_json::json;
        if !running_alone("theme_grid") {
            return;
        }
        let home = crate::mods::app_dir();
        std::fs::create_dir_all(home.join("themes")).unwrap();
        let names: Vec<String> = (0..120).map(|i| format!("grid-{i:03}")).collect();
        let ansi: Vec<String> = (0..16).map(|i| format!("#{:02x}4080", i * 16)).collect();
        for name in &names {
            std::fs::write(home.join(format!("themes/{name}.json")), json!({ "background": "#101418", "foreground": "#d0d0d0", "selection": "#304050", "ansi": ansi }).to_string()).unwrap();
        }
        std::fs::write(home.join("settings.json"), json!({ "appearance": "dark", "theme": { "dark": "grid-100", "light": "github-light" } }).to_string()).unwrap();
        let (view, cx) = open_window(cx, "unused", "", json!({ "id": "pane", "type": "block", "content": { "terminal": "t1" } }));
        cx.run_until_parked();
        let renders = |cx: &mut gpui::VisualTestContext| view.update(cx, |this, _| this.runtime.as_ref().unwrap().read_metrics().script_renders());
        let script_time = |cx: &mut gpui::VisualTestContext| view.update(cx, |this, _| this.runtime.as_ref().unwrap().read_metrics().script_render_time());
        // debug_bounds takes a static id: one per card, kept for the test's life.
        let ids: Vec<(&String, &'static str)> = names.iter().map(|n| (n, &*Box::leak(format!("st-t-{n}").into_boxed_str()))).collect();
        let laid_out = |cx: &mut gpui::VisualTestContext| ids.iter().filter(|(_, id)| cx.debug_bounds(id).is_some()).map(|(n, _)| (*n).clone()).collect::<Vec<_>>();
        let (before, took) = (renders(cx), script_time(cx));
        cx.update(|window, cx| view.update(cx, |this, cx| this.run_as("app.settings", json!({ "section": "themes" }), false, window, cx)));
        cx.run_until_parked();
        let (opened, open_renders) = (laid_out(cx), renders(cx) - before);
        eprintln!("theme grid: {} cards laid out and {open_renders} script renders ({:?} in script) opening", opened.len(), script_time(cx) - took);
        assert!(opened.iter().any(|n| n == "grid-100"), "opened away from the picked theme: {opened:?}");
        assert!(!opened.is_empty() && opened.len() <= 30, "{} of 120 cards laid out on opening", opened.len());
        let at = cx.debug_bounds("st-t-grid-100").unwrap().center();
        let before = renders(cx);
        for _ in 0..4 {
            cx.simulate_event(ScrollWheelEvent { position: at, delta: ScrollDelta::Pixels(point(px(0.), px(250.))), ..Default::default() });
            cx.run_until_parked();
        }
        let (scrolled, scroll_renders) = (laid_out(cx), renders(cx) - before);
        eprintln!("theme grid: {} cards laid out and {scroll_renders} script renders after 4 scrolls", scrolled.len());
        assert_eq!(scroll_renders, 0, "a scroll rendered the page");
        assert!(scrolled.len() <= 30 && scrolled.iter().all(|n| n.as_str() < "grid-100"), "after scrolling up: {scrolled:?}");
    }

    /// Runs `window::tests::<name>` alone in a test process of its own, on a
    /// fresh app home: a window takes process-wide things (the app home, the
    /// mods' request channel) that tests beside it would otherwise share.
    fn in_own_process(name: &str) {
        let home = std::path::PathBuf::from(format!("/var/tmp/herdr-window-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &format!("window::tests::{name}"), "--nocapture", "--test-threads=1"])
            .env("HERDR_APP_HOME", &home)
            .env("HERDR_TEST_WINDOW", name)
            .output()
            .unwrap();
        let _ = std::fs::remove_dir_all(&home);
        let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(out.status.success() && log.contains("1 passed"), "{log}");
    }

    /// In the process `in_own_process` started for `name`: whether to go on.
    fn running_alone(name: &str) -> bool {
        std::env::var("HERDR_TEST_WINDOW").as_deref() == Ok(name)
    }

    /// The window on a core that's never there (`hc::offline`), showing
    /// `layout` with `mod_name` (its `main.js`) in the home, past the first-run
    /// welcome (it lists machines through the core). No other mods: the
    /// shipped ones run programs on threads of their own, which the test
    /// executor refuses.
    fn open_window<'a>(cx: &'a mut gpui::TestAppContext, mod_name: &str, main_js: &str, layout: serde_json::Value) -> (gpui::Entity<super::AppWindow>, &'a mut gpui::VisualTestContext) {
        open_window_on(crate::hc::offline(), cx, mod_name, main_js, layout)
    }

    /// `open_window` on a given core (`hc::answering`).
    fn open_window_on<'a>(core: std::sync::Arc<crate::hc::Core>, cx: &'a mut gpui::TestAppContext, mod_name: &str, main_js: &str, layout: serde_json::Value) -> (gpui::Entity<super::AppWindow>, &'a mut gpui::VisualTestContext) {
        use serde_json::json;
        let home = crate::mods::app_dir();
        crate::mods::write_workspace().unwrap();
        let folder = home.join("mods").join(mod_name);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("mod.json"), r#"{ "place": "none" }"#).unwrap();
        std::fs::write(folder.join("main.js"), main_js).unwrap();
        std::fs::create_dir_all(home.join("layouts")).unwrap();
        std::fs::write(home.join("layouts/test.json"), layout.to_string()).unwrap();
        cx.update(crate::mods::init);
        let requests = crate::mods::install_policy();
        crate::mods::set_shared("layout".into(), json!("test"));
        crate::mods::set_shared("frame.welcomed".into(), json!("test"));
        // This version ran before: no What's New and no "Updated to" pill,
        // which would sit where bottom toasts go, whatever the version's notes.
        std::fs::write(home.join("update-notes.json"), json!({ "last_run": env!("CARGO_PKG_VERSION") }).to_string()).unwrap();
        let (view, cx) = cx.add_window_view(|window, cx| super::AppWindow::new(core, requests, window, cx));
        assert_eq!(view.update(cx, |this, _| this.layout_name.clone()), "test");
        (view, cx)
    }

    #[test]
    fn cursor_trail_runs_between_panes_then_ends() {
        in_own_process("cursor_trail");
    }

    /// Tab t: a | b | d, tab u: c. Off, a focus change keeps nothing. On,
    /// turning it on starts nothing; b to a starts a trail from b's cursor,
    /// which has a layer until its time is up and the next draw drops it.
    /// The same pane starts none. Closing the focused pane starts one from
    /// where it last drew its cursor. A trail whose pane is gone, or one
    /// running when the tab changes, ends at once; another tab starts none.
    #[gpui::test]
    fn cursor_trail(cx: &mut gpui::TestAppContext) {
        use crate::layout::{Axis, Node};
        use crate::model::{Tab, pane};
        use serde_json::json;
        if !running_alone("cursor_trail") {
            return;
        }
        let (view, cx) = open_window(cx, "unused", "", json!({ "id": "main", "type": "block", "content": { "tab": "active" } }));
        let mode = |trail: crate::settings::CursorTrail| {
            let before = crate::settings::get();
            crate::settings::set(crate::settings::Settings { cursor_trail: trail, cursor_trail_duration_ms: 1000, ..(*before).clone() });
        };
        view.update_in(cx, |this, window, cx| {
            this.documents.entry(String::new()).or_default().available = true;
            for id in this.overlays.iter().map(|o| o.id.clone()).collect::<Vec<_>>() {
                this.close_overlay(&id, window, cx);
            }
            let mut tab = Tab::new("t", "", "a", "ta");
            tab.root = Node::container("t", Axis::Row, vec![pane("a", "ta"), pane("b", "tb"), pane("d", "td")]);
            this.model.add_space("s", "", "/tmp", tab);
            this.model.add_tab("s", Tab::new("u", "", "c", "tc"));
            this.model.select_tab("t");
            this.model.focus_pane("a");
            this.sync(window, cx);
        });
        cx.run_until_parked();
        let draw = |cx: &mut gpui::VisualTestContext| {
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
        };
        let change = |cx: &mut gpui::VisualTestContext, f: &dyn Fn(&mut crate::model::Model)| {
            view.update_in(cx, |this, window, cx| {
                f(&mut this.model);
                this.sync(window, cx);
            });
            draw(cx);
        };
        let focus = |cx: &mut gpui::VisualTestContext, pane: &str| change(cx, &|m| { m.focus_pane(pane); });
        // (where the trail starts, toward which pane), and the (tab, pane) seen last.
        type State = (Option<(super::cursor_shapes::Quad, String)>, Option<(String, String)>);
        let state = |cx: &mut gpui::VisualTestContext| -> State {
            view.update(cx, |this, _| (this.trail.as_ref().and_then(|t| Some((t.from?, t.pane.clone()))), this.trail_seen.as_ref().map(|s| (s.tab.clone(), s.pane.clone()))))
        };
        let seen = |tab: &str, pane: &str| Some((tab.to_owned(), pane.to_owned()));
        let anchor = |cx: &mut gpui::VisualTestContext, pane: &str| super::cursor_shapes::quad(view.update(cx, |this, cx| this.terms[pane].view.read(cx).trail_end()).expect("drawn, so it has a cursor anchor").cursor);
        let expire = |cx: &mut gpui::VisualTestContext| view.update(cx, |this, _| {
            let trail = this.trail.as_mut().unwrap();
            trail.start = trail.start.checked_sub(std::time::Duration::from_secs(2)).unwrap();
        });
        let layer = |cx: &mut gpui::VisualTestContext| view.update(cx, |this, cx| this.trail_frame(cx).is_some());

        focus(cx, "b");
        assert_eq!(state(cx), (None, None), "off: nothing kept");

        mode(crate::settings::CursorTrail::Panes);
        draw(cx);
        assert_eq!(state(cx), (None, seen("t", "b")), "turning it on starts no trail");
        let b = anchor(cx, "b");

        focus(cx, "a");
        assert_eq!(state(cx), (Some((b, "a".into())), seen("t", "a")), "b to a starts a trail from b's cursor");
        assert!(layer(cx), "a live trail has a layer");
        let started = view.update(cx, |this, _| this.trail.as_ref().map(|t| t.start));
        focus(cx, "a");
        assert_eq!(view.update(cx, |this, _| this.trail.as_ref().map(|t| t.start)), started, "the same pane neither ends it nor starts another");
        expire(cx);
        draw(cx);
        assert_eq!(state(cx).0, None, "its time up, the next draw drops it");
        assert!(!layer(cx), "and has no layer, the one thing that asks for frames");

        // Steered: a move while a trail is drawn starts from what it drew.
        focus(cx, "b");
        assert!(layer(cx));
        let drawn = view.update(cx, |this, _| this.trail.as_ref().and_then(|t| t.drawn)).expect("a live trail notes what it drew");
        assert_ne!(drawn, anchor(cx, "a"), "the comet's tail, partway back to a");
        focus(cx, "a");
        assert_eq!(state(cx).0, Some((drawn, "a".into())), "b to a, mid-trail, starts from the trail as drawn, not b's cursor");
        expire(cx);
        draw(cx);

        focus(cx, "d");
        let d = anchor(cx, "d");
        expire(cx);
        draw(cx);
        change(cx, &|m| { m.remove_pane("d"); });
        let (trail, now) = state(cx);
        assert_eq!(trail.as_ref().map(|(from, _)| *from), Some(d), "closing d starts a trail from where d drew its cursor");
        assert_eq!(trail.map(|(_, to)| to), now.map(|(_, pane)| pane), "to the pane focused after");

        // Its pane's terminal going (a close the next draw hasn't seen yet) ends it.
        let to = state(cx).0.unwrap().1;
        view.update(cx, |this, _| this.terms.remove(&to));
        assert!(!layer(cx), "a trail whose terminal is gone ends");
        assert_eq!(state(cx).0, None);

        // Reduce motion: a running trail ends and none starts until it's off.
        focus(cx, "b");
        assert!(state(cx).0.is_some());
        cx.update(|_, cx| cx.set_reduce_motion(true));
        assert!(!layer(cx), "a running trail ends");
        assert_eq!(state(cx).0, None);
        focus(cx, "a");
        assert_eq!(state(cx).0, None, "none starts while motion is reduced");
        cx.update(|_, cx| cx.set_reduce_motion(false));
        focus(cx, "b");
        assert!(state(cx).0.is_some(), "and trails come back with it off");

        focus(cx, "b");
        focus(cx, "a");
        assert!(state(cx).0.is_some());
        change(cx, &|m| { m.select_tab("u"); });
        assert_eq!(state(cx), (None, seen("u", "c")), "a tab change ends a running trail and starts none");

        mode(crate::settings::CursorTrail::Off);
        draw(cx);
        assert_eq!(state(cx), (None, None), "off again forgets");
    }

    #[test]
    fn cursor_jumps_in_all_mode_draw_once_settled_and_never_mid_burst() {
        in_own_process("cursor_jumps");
    }

    /// Tab t: a | b, a focused, `cursor_trail: "all"` with a ripple. A jump in
    /// a's terminal draws nothing until the cursor has been still for the
    /// settle time, then one trail from where it settled before. A burst of
    /// moves (streaming output) draws nothing while it lasts and one effect
    /// after. A one-cell step settles with none; b's terminal, not focused,
    /// draws none; with "panes" a's jumps draw none.
    #[gpui::test]
    fn cursor_jumps(cx: &mut gpui::TestAppContext) {
        use crate::layout::{Axis, Node};
        use crate::model::{Tab, pane};
        use crate::settings::{Arrival, CursorTrail};
        use serde_json::json;
        use std::time::Duration;
        if !running_alone("cursor_jumps") {
            return;
        }
        let (view, cx) = open_window(cx, "unused", "", json!({ "id": "main", "type": "block", "content": { "tab": "active" } }));
        // Through the window's own settings, which a later look resolve keeps.
        let set = |cx: &mut gpui::VisualTestContext, trail: CursorTrail| {
            view.update_in(cx, |this, window, cx| {
                // 80 ms, so the test's 16 ms burst stays within it.
                (this.person.cursor_trail, this.person.cursor_arrival, this.person.cursor_trail_duration_ms, this.person.cursor_trail_settle_ms) = (trail, Arrival::Ripple, 1000, 80);
                this.apply_look(window, cx);
            });
        };
        view.update_in(cx, |this, window, cx| {
            this.documents.entry(String::new()).or_default().available = true;
            for id in this.overlays.iter().map(|o| o.id.clone()).collect::<Vec<_>>() {
                this.close_overlay(&id, window, cx);
            }
            let mut tab = Tab::new("t", "", "a", "ta");
            tab.root = Node::container("t", Axis::Row, vec![pane("a", "ta"), pane("b", "tb")]);
            this.model.add_space("s", "", "/tmp", tab);
            this.model.select_tab("t");
            this.model.focus_pane("a");
            this.sync(window, cx);
        });
        cx.run_until_parked();
        // Output to a pane's terminal, then a draw.
        let feed = |cx: &mut gpui::VisualTestContext, pane: &str, bytes: &str| {
            view.update(cx, |this, cx| {
                let term = this.terms[pane].view.clone();
                term.update(cx, |t, cx| {
                    t.feed(bytes.as_bytes(), cx);
                    cx.notify();
                });
            });
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
        };
        let wait = |cx: &mut gpui::VisualTestContext, ms: u64| {
            cx.executor().advance_clock(Duration::from_millis(ms));
            cx.run_until_parked();
        };
        let effect = |cx: &mut gpui::VisualTestContext| view.update(cx, |this, _| this.trail.as_ref().map(|t| (t.from.is_some(), t.arrival, t.pane.clone(), t.start)));
        let clear = |cx: &mut gpui::VisualTestContext| view.update(cx, |this, _| this.trail = None);
        assert!(view.update_in(cx, |this, window, cx| this.terms["a"].view.read(cx).owns_keyboard_focus(window, cx)), "a has the keyboard");
        set(cx, CursorTrail::All);

        feed(cx, "a", "\x1b[1;1H");
        wait(cx, 200);
        clear(cx);
        feed(cx, "a", "\x1b[10;30H");
        wait(cx, 40);
        assert_eq!(effect(cx), None, "still moving for under the settle time");
        wait(cx, 60);
        let first = effect(cx);
        assert_eq!(first.as_ref().map(|e| (e.0, e.1, e.2.clone())), Some((true, Arrival::Ripple, "a".into())), "settled: a trail and a ripple at a");

        // A burst: a move every 16 ms for 30 frames, then still.
        clear(cx);
        for i in 0..30 {
            feed(cx, "a", &format!("\x1b[{};{}H", 2 + i % 20, 5 + i));
            wait(cx, 16);
            assert_eq!(effect(cx), None, "nothing mid-burst ({i})");
        }
        wait(cx, 100);
        assert!(effect(cx).is_some(), "one effect once the burst stops");
        clear(cx);
        wait(cx, 500);
        assert_eq!(effect(cx), None, "and no other");

        // Scrolling back with the cursor in view moves its cell, not the cursor.
        feed(cx, "a", &"line\r\n".repeat(50));
        feed(cx, "a", "\x1b[5;1H");
        wait(cx, 200);
        clear(cx);
        let scroll = |cx: &mut gpui::VisualTestContext, rows: isize| {
            view.update(cx, |this, cx| this.terms["a"].view.update(cx, |t, cx| {
                t.scroll_back(rows);
                cx.notify();
            }));
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
        };
        scroll(cx, 5);
        wait(cx, 200);
        assert_eq!(effect(cx), None, "scrolled back: no jump");
        scroll(cx, -5);
        wait(cx, 200);
        assert_eq!(effect(cx), None, "and back down: none");

        // The default 10 ms settle, timed from the program's moves: output every
        // 5 ms drawn only every 50 ms (frames slower than the settle time) draws
        // nothing until it pauses, then once. Timed from frames, it would draw
        // at each of them.
        view.update_in(cx, |this, window, cx| {
            this.person.cursor_trail_settle_ms = 10;
            this.apply_look(window, cx);
        });
        feed(cx, "a", "\x1b[1;1H");
        wait(cx, 200);
        clear(cx);
        for i in 0..40u64 {
            // Output without a frame: the terminal isn't asked to draw.
            view.update(cx, |this, cx| {
                let term = this.terms["a"].view.clone();
                term.update(cx, |t, cx| t.feed(format!("\x1b[{};{}H", 1 + i % 20, 1 + i * 3 % 60).as_bytes(), cx));
            });
            wait(cx, 5);
            if i % 10 == 9 {
                cx.update(|window, _| window.refresh());
                cx.run_until_parked();
            }
            assert_eq!(effect(cx), None, "nothing while the program keeps moving it ({i})");
        }
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        wait(cx, 200);
        assert!(effect(cx).is_some(), "once it pauses");
        clear(cx);
        wait(cx, 300);
        assert_eq!(effect(cx), None, "and once only");
        view.update_in(cx, |this, window, cx| {
            this.person.cursor_trail_settle_ms = 80;
            this.apply_look(window, cx);
        });

        // Reduce motion: a settled jump draws nothing.
        feed(cx, "a", "\x1b[1;1H");
        wait(cx, 200);
        clear(cx);
        cx.update(|_, cx| cx.set_reduce_motion(true));
        feed(cx, "a", "\x1b[15;40H");
        wait(cx, 200);
        assert_eq!(effect(cx), None, "no jump trail or pulse while motion is reduced");
        cx.update(|_, cx| cx.set_reduce_motion(false));

        // A one-cell step is under the threshold.
        feed(cx, "a", "\x1b[1;1H");
        wait(cx, 200);
        clear(cx);
        feed(cx, "a", "\x1b[2;2H");
        wait(cx, 200);
        assert_eq!(effect(cx), None, "one cell is no jump");

        // A beam cursor: the trail starts from the beam as drawn (2 px at the
        // left of its cell), not the cell, and its end is the beam too.
        feed(cx, "a", "\x1b[6 q\x1b[1;1H");
        wait(cx, 200);
        clear(cx);
        feed(cx, "a", "\x1b[12;40H");
        wait(cx, 200);
        let (from, end) = view.update(cx, |this, cx| (this.trail.as_ref().and_then(|t| t.from), this.terms["a"].view.read(cx).trail_end()));
        let (from, end) = (from.expect("a trail from the beam"), end.unwrap());
        let width = |q: super::cursor_shapes::Quad| q[1].0 - q[0].0;
        assert_eq!(width(from), 2.0, "from the beam: {from:?}");
        assert_eq!(from[3].1 - from[0].1, f32::from(end.cell.size.height), "a cell high");
        assert_eq!((f32::from(end.cursor.size.width), end.shown), (2.0, true), "to the beam");
        feed(cx, "a", "\x1b[2 q");
        wait(cx, 200);
        clear(cx);

        // b doesn't have the keyboard: its cursor moving draws nothing.
        feed(cx, "b", "\x1b[1;1H");
        feed(cx, "b", "\x1b[20;40H");
        wait(cx, 200);
        assert_eq!(effect(cx), None, "not the focused pane");

        // Panes mode: in-pane jumps draw nothing.
        set(cx, CursorTrail::Panes);
        feed(cx, "a", "\x1b[1;1H");
        wait(cx, 200);
        feed(cx, "a", "\x1b[15;40H");
        wait(cx, 200);
        assert_eq!(effect(cx), None, "panes mode ignores jumps");
    }

    #[test]
    fn trail_frames_draw_the_trail_and_reuse_the_rest_of_the_window() {
        in_own_process("trail_frames");
    }

    /// Tab t: a | b, `cursor_trail: "panes"`. b to a starts a trail; each
    /// frame it asks for draws the window and paints the trail but reuses the
    /// window's layout (no layout draw), and the terminals still count as on
    /// screen, so their output keeps drawing. Anything else notifying the
    /// window during the trail draws the layout again, still reusing the
    /// terminals in it. Over, it paints no more, and the window draws its
    /// layout once more.
    #[gpui::test]
    fn trail_frames(cx: &mut gpui::TestAppContext) {
        use crate::bench::{BODY, TERMINAL, TRAIL, WINDOW};
        use crate::layout::{Axis, Node};
        use crate::model::{Tab, pane};
        use serde_json::json;
        use std::sync::atomic::Ordering::Relaxed;
        if !running_alone("trail_frames") {
            return;
        }
        let (view, cx) = open_window(
            cx,
            "unused",
            "",
            json!({ "id": "main", "type": "block", "content": { "tab": "active" } }),
        );
        let before = crate::settings::get();
        crate::settings::set(crate::settings::Settings {
            cursor_trail: crate::settings::CursorTrail::Panes,
            cursor_trail_duration_ms: 1000,
            ..(*before).clone()
        });
        view.update_in(cx, |this, window, cx| {
            this.documents.entry(String::new()).or_default().available = true;
            for id in this
                .overlays
                .iter()
                .map(|o| o.id.clone())
                .collect::<Vec<_>>()
            {
                this.close_overlay(&id, window, cx);
            }
            let mut tab = Tab::new("t", "", "a", "ta");
            tab.root = Node::container("t", Axis::Row, vec![pane("a", "ta"), pane("b", "tb")]);
            this.model.add_space("s", "", "/tmp", tab);
            this.model.select_tab("t");
            this.model.focus_pane("b");
            this.sync(window, cx);
        });
        cx.run_until_parked();
        let draw = |cx: &mut gpui::VisualTestContext| {
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
        };
        draw(cx);
        view.update_in(cx, |this, window, cx| {
            this.model.focus_pane("a");
            this.sync(window, cx);
        });
        draw(cx);
        assert!(
            view.update(cx, |this, _| this.trail.is_some()),
            "b to a starts a trail"
        );
        // The frames the trail asked for, each drawn.
        let frame = |cx: &mut gpui::VisualTestContext| {
            let asked = cx.update(|window, cx| window.simulate_next_frame(cx));
            cx.run_until_parked();
            asked
        };
        let counts = || {
            (
                WINDOW.load(Relaxed),
                BODY.load(Relaxed),
                TRAIL.load(Relaxed),
            )
        };
        let on_screen = |cx: &mut gpui::VisualTestContext| {
            view.update(cx, |this, cx| {
                ["a", "b"]
                    .iter()
                    .all(|p| this.terms[*p].view.read(cx).on_screen_this_frame())
            })
        };
        let start = counts();
        for _ in 0..3 {
            assert!(frame(cx) >= 1, "the live trail asks for the next frame");
        }
        let now = counts();
        assert_eq!(
            (now.0 - start.0, now.1 - start.1, now.2 - start.2),
            (3, 0, 3),
            "three window draws, each painting the trail and reusing the layout"
        );
        assert!(
            on_screen(cx),
            "the terminals the reused layout shows are still on screen"
        );

        let terminals = TERMINAL.load(Relaxed);
        view.update(cx, |_, cx| cx.notify());
        frame(cx);
        assert!(
            counts().1 > now.1,
            "the window notified: its layout draws again"
        );
        assert_eq!(
            TERMINAL.load(Relaxed),
            terminals,
            "reusing the terminals under it, which weren't"
        );
        assert!(on_screen(cx));

        view.update(cx, |this, _| {
            let trail = this.trail.as_mut().unwrap();
            trail.start = trail
                .start
                .checked_sub(std::time::Duration::from_secs(2))
                .unwrap();
        });
        frame(cx);
        assert!(
            view.update(cx, |this, _| this.trail.is_none()),
            "over, it ends"
        );
        let (painted, layouts) = (counts().2, counts().1);
        frame(cx);
        assert_eq!(counts().2, painted, "and paints no more");
        assert!(
            counts().1 > layouts,
            "the frame after its end draws the layout again"
        );
    }

    #[test]
    fn cursor_arrival_pulses_with_the_trail_off_or_on() {
        in_own_process("cursor_arrival");
    }

    /// Tab t: a | b. The arrival pulse alone (trail off) starts an effect at
    /// the new pane with no trail; with the trail on it comes with the trail
    /// in its style; the trail alone has no pulse; both off, nothing.
    #[gpui::test]
    fn cursor_arrival(cx: &mut gpui::TestAppContext) {
        use crate::layout::{Axis, Node};
        use crate::model::{Tab, pane};
        use crate::settings::{Arrival, CursorTrail, TrailStyle};
        use serde_json::json;
        if !running_alone("cursor_arrival") {
            return;
        }
        let (view, cx) = open_window(cx, "unused", "", json!({ "id": "main", "type": "block", "content": { "tab": "active" } }));
        let set = |trail: CursorTrail, style: TrailStyle, arrival: Arrival| {
            let before = crate::settings::get();
            crate::settings::set(crate::settings::Settings { cursor_trail: trail, cursor_trail_style: style, cursor_arrival: arrival, cursor_trail_duration_ms: 1000, ..(*before).clone() });
        };
        view.update_in(cx, |this, window, cx| {
            this.documents.entry(String::new()).or_default().available = true;
            for id in this.overlays.iter().map(|o| o.id.clone()).collect::<Vec<_>>() {
                this.close_overlay(&id, window, cx);
            }
            let mut tab = Tab::new("t", "", "a", "ta");
            tab.root = Node::container("t", Axis::Row, vec![pane("a", "ta"), pane("b", "tb")]);
            this.model.add_space("s", "", "/tmp", tab);
            this.model.select_tab("t");
            this.model.focus_pane("a");
            this.sync(window, cx);
        });
        cx.run_until_parked();
        let draw = |cx: &mut gpui::VisualTestContext| {
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
        };
        let focus = |cx: &mut gpui::VisualTestContext, pane: &str| {
            view.update_in(cx, |this, window, cx| {
                this.model.focus_pane(pane);
                this.sync(window, cx);
            });
            draw(cx);
        };
        // (has a trail's start, its style, the pulse, toward which pane), and whether it has a layer.
        let state = |cx: &mut gpui::VisualTestContext| view.update(cx, |this, _| this.trail.as_ref().map(|t| (t.from.is_some(), t.style, t.arrival, t.pane.clone())));
        let layer = |cx: &mut gpui::VisualTestContext| view.update(cx, |this, cx| this.trail_frame(cx).is_some());

        set(CursorTrail::Off, TrailStyle::Comet, Arrival::Ripple);
        draw(cx);
        focus(cx, "b");
        assert_eq!(state(cx), Some((false, None, Arrival::Ripple, "b".into())), "the pulse alone, at b");
        assert!(layer(cx), "it draws and asks for frames while it runs");

        set(CursorTrail::Panes, TrailStyle::Warp, Arrival::Ripple);
        draw(cx);
        focus(cx, "a");
        assert_eq!(state(cx), Some((true, Some(TrailStyle::Warp), Arrival::Ripple, "a".into())), "a warp from b with a ripple at a");

        set(CursorTrail::Panes, TrailStyle::Tail, Arrival::Off);
        draw(cx);
        focus(cx, "b");
        assert_eq!(state(cx), Some((true, Some(TrailStyle::Tail), Arrival::Off, "b".into())), "the trail alone");

        set(CursorTrail::Off, TrailStyle::Tail, Arrival::Off);
        draw(cx);
        assert_eq!(state(cx), None, "both off: nothing");
        focus(cx, "a");
        assert_eq!(state(cx), None);
    }

    #[test]
    fn pane_text_zoom_keys_zoom_the_focused_pane_or_reach_the_program() {
        in_own_process("pane_text_zoom");
    }

    /// Tab t: a | b, a focused. The pane zoom keys (Ctrl+=, -, 0 on macOS,
    /// bound in keys.json here elsewhere) zoom only a's text: b keeps its
    /// size and grid, the app's zoom stays, `panes()` has a's size. The app's
    /// zoom (Cmd+= on macOS) multiplies on top and resetting either keeps the
    /// other. With `pane_zoom_keys` off the keys go to a's program instead.
    #[gpui::test]
    fn pane_text_zoom(cx: &mut gpui::TestAppContext) {
        use crate::layout::{Axis, Node};
        use crate::model::{Tab, pane};
        use serde_json::json;
        if !running_alone("pane_text_zoom") {
            return;
        }
        if !cfg!(target_os = "macos") {
            let keys = json!({"global": {"ctrl-=": "pane.zoom_text_in", "ctrl--": "pane.zoom_text_out", "ctrl-0": "pane.zoom_text_reset", "ctrl-alt-=": "app.zoom_in", "ctrl-alt-0": "app.zoom_reset"}});
            std::fs::write(crate::keys::path(), keys.to_string()).unwrap();
        }
        let app_zoom_in = if cfg!(target_os = "macos") { "cmd-=" } else { "ctrl-alt-=" };
        let app_zoom_reset = if cfg!(target_os = "macos") { "cmd-0" } else { "ctrl-alt-0" };
        let (view, cx) = open_window(cx, "unused", "", json!({ "id": "main", "type": "block", "content": { "tab": "active" } }));
        view.update_in(cx, |this, window, cx| {
            this.documents.entry(String::new()).or_default().available = true;
            let mut tab = Tab::new("t", "", "a", "ta");
            tab.root = Node::container("t", Axis::Row, vec![pane("a", "ta"), pane("b", "tb")]);
            this.model.add_space("s", "", "/tmp", tab);
            this.model.select_tab("t");
            this.model.focus_pane("a");
            this.sync(window, cx);
        });
        cx.run_until_parked();
        // (a's text size, a's grid, b's text size, b's grid, a typed into, the app's zoom)
        let state = |cx: &mut gpui::VisualTestContext| view.update(cx, |this, cx| {
            let (a, b) = (this.terms["a"].view.read(cx), this.terms["b"].view.read(cx));
            (a.text_size(), a.dimensions(), b.text_size(), b.dimensions(), a.take_typed(), crate::settings::get().zoom)
        });
        let published = |pane: &str| crate::mods::published_panes().as_array().unwrap().iter().find(|p| p["pane"] == pane).map(|p| p["text_size"].clone()).unwrap();
        let key = |cx: &mut gpui::VisualTestContext, k: &str| {
            cx.simulate_keystrokes(k);
            cx.run_until_parked();
        };
        let start = state(cx);
        assert!(view.update_in(cx, |this, window, cx| this.terms["a"].view.read(cx).owns_keyboard_focus(window, cx)), "a has the keyboard");
        assert_eq!((start.0, start.2, start.4, start.5), (None, None, false, 1.0));

        key(cx, "ctrl-=");
        let one = state(cx);
        assert_eq!(one.0, Some(14.0 * 1.1), "a one tenth up");
        assert!(one.1.0 < start.1.0, "a reflowed: {:?} -> {:?}", start.1, one.1);
        assert_eq!((one.2, one.3), (None, start.3), "b untouched");
        assert_eq!((one.4, one.5), (false, 1.0), "the key zoomed, not typed; the app's zoom stays");
        assert_eq!((published("a"), published("b")), (json!(14.0f32 * 1.1), json!(null)));

        key(cx, app_zoom_in);
        let both = state(cx);
        assert_eq!(both.5, 1.1, "the app's zoom keys stay app-wide");
        assert_eq!(both.0, Some(14.0f32 * 1.1 * 1.1), "the app's zoom multiplies the pane's");
        assert_eq!(published("a"), json!(14.0f32 * 1.1 * 1.1));
        key(cx, app_zoom_reset);
        assert_eq!(state(cx).0, Some(14.0f32 * 1.1), "the app's reset keeps the pane's zoom");
        key(cx, app_zoom_in);
        key(cx, "ctrl-0");
        let reset = state(cx);
        assert_eq!((reset.0, reset.5), (None, 1.1), "the pane's reset keeps the app's zoom");
        key(cx, app_zoom_reset);
        assert_eq!(state(cx).1, start.1, "back to a's own grid");
        key(cx, "ctrl--");
        assert_eq!(state(cx).0, Some(14.0 * 0.9));
        key(cx, "ctrl-0");

        // A zoomed pane in a hidden tab is fitted at its own size.
        key(cx, "ctrl-=");
        let zoomed = state(cx).1;
        view.update_in(cx, |this, window, cx| {
            this.model.add_tab("s", Tab::new("u", "", "d", "td"));
            this.model.select_tab("u");
            this.sync(window, cx);
        });
        cx.run_until_parked();
        view.update_in(cx, |this, window, cx| {
            this.hidden_fit = super::pane_size::HiddenFit::Unknown;
            this.fit_hidden_tabs(window, cx);
            this.fit_hidden_tabs(window, cx);
            assert!(matches!(this.hidden_fit, super::pane_size::HiddenFit::Done(_)), "the hidden tab was fitted");
        });
        assert_eq!(state(cx).1, zoomed, "hidden, a keeps the zoomed grid");
        // Zoomed by command while hidden, its program gets the new grid without the tab showing.
        assert!(!view.update(cx, |this, cx| this.terms["a"].view.read(cx).on_screen_this_frame()), "a is off screen");
        let reply = view.update_in(cx, |this, window, cx| this.run_as("pane.zoom_text_in", json!({ "pane": "a" }), false, window, cx));
        assert!(reply.get("error").is_none(), "{reply}");
        cx.run_until_parked();
        let hidden = state(cx);
        assert_eq!(hidden.0, Some(14.0 * 1.2));
        assert!(hidden.1.0 < zoomed.0 && hidden.1.1 < zoomed.1, "hidden, a refits at once: {zoomed:?} -> {:?}", hidden.1);
        view.update_in(cx, |this, window, cx| {
            this.model.select_tab("t");
            this.model.focus_pane("a");
            this.sync(window, cx);
        });
        cx.run_until_parked();
        key(cx, "ctrl-0");
        assert_eq!(state(cx).0, None);
        let focus_a = |cx: &mut gpui::VisualTestContext| view.update_in(cx, |this, window, cx| {
            let focus = this.terms["a"].focus.clone();
            window.focus(&focus, cx);
        });

        // The keyboard in the palette (a mod), not a terminal: Ctrl+= isn't a
        // pane zoom; it goes on unhandled (the palette's field ignores it) and
        // no program gets it.
        view.update_in(cx, |this, window, cx| { this.run_as("app.palette", json!({}), false, window, cx); });
        cx.run_until_parked();
        assert!(!view.update_in(cx, |this, window, cx| this.terms.values().any(|t| t.view.read(cx).owns_keyboard_focus(window, cx))), "the palette has the keyboard");
        key(cx, "ctrl-=");
        let palette = state(cx);
        assert_eq!((palette.0, palette.4), (None, false), "with the palette focused, no pane zooms and nothing is typed");
        view.update_in(cx, |this, window, cx| { this.run_as("app.palette", json!({}), false, window, cx); });
        cx.run_until_parked();
        focus_a(cx);
        cx.run_until_parked();

        // A terminal swapped into a: the zoom goes to the view on screen and
        // panes() reports that view's size. All in one update: the offline
        // core's listing drops the made-up terminal at the next refresh. A
        // swapped-in view lives only while shown (hiding its tab drops it),
        // so there is no hidden one to refit.
        view.update_in(cx, |this, window, cx| {
            this.live.insert("tx".into(), json!({ "address": "tx" }));
            this.run_as("terminal.show", json!({ "pane": "a", "terminal": "tx" }), false, window, cx);
            this.sync(window, cx);
            // A mod that took the keyboard (a tab drag) gives it back to the
            // swapped-in view without ending the swap, as pane.focus would.
            let other = cx.focus_handle();
            window.focus(&other, cx);
            this.run_as("pane.refocus", json!({}), false, window, cx);
            assert_eq!(this.swapped.get("a").map(String::as_str), Some("tx"), "pane.refocus keeps what the pane shows");
            assert!(!other.is_focused(window), "pane.refocus takes the keyboard back");
            let reply =this.run_as("pane.zoom_text_in", json!({ "pane": "a" }), false, window, cx);
            assert!(reply.get("error").is_none(), "{reply}");
            // Made this frame, it isn't drawn yet (off screen), so the zoom
            // refits it there: to a's area at its new cell, not the hint it
            // attached with (120 × 40).
            let sv = this.terms[&format!("a{}", super::SWAP)].view.read(cx);
            assert!(!sv.on_screen_this_frame(), "the swapped-in view is off screen when zoomed");
            let area = this.shown_area(this.model.tab("t").unwrap(), "a").unwrap();
            assert_ne!(sv.dimensions(), (120, 40), "the swapped-in view was refitted");
            assert_eq!(sv.dimensions(), crate::terminal::grid_size(area, sv.cell()), "to a's area at its zoomed cell");
            assert!(sv.cell().width > crate::terminal::cell_size(window).width, "its cell is the zoomed one");
            let swapped = this.terms[&format!("a{}", super::SWAP)].view.read(cx).text_size();
            assert_eq!((swapped, this.terms["a"].view.read(cx).text_size()), (Some(14.0 * 1.1), None), "the swapped-in view zooms, a's own doesn't");
            this.publish(cx);
            assert_eq!(published("a"), json!(14.0f32 * 1.1), "panes() reports the view on screen");
            this.model.select_tab("u");
            this.sync(window, cx);
            assert!(!this.terms.contains_key(&format!("a{}", super::SWAP)), "hidden, the swapped-in view is gone");
            this.run_as("terminal.show", json!({ "pane": "a" }), false, window, cx);
            this.model.select_tab("t");
            this.model.focus_pane("a");
            this.sync(window, cx);
        });
        cx.run_until_parked();
        focus_a(cx);
        cx.run_until_parked();
        assert_eq!(state(cx).0, None);

        // An object-form binding ({"command": ...}) zooms as a string one does.
        let mut keys: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(crate::keys::path()).unwrap()).unwrap();
        keys["global"]["f6"] = json!({ "command": "pane.zoom_text_in" });
        keys["global"]["f7"] = json!({ "command": "pane.zoom_text_reset" });
        std::fs::write(crate::keys::path(), keys.to_string()).unwrap();
        view.update_in(cx, |this, window, cx| {
            this.watched.keys = None;
            this.reload_config(window, cx);
        });
        cx.run_until_parked();
        key(cx, "f6");
        assert_eq!(state(cx).0, Some(14.0 * 1.1), "f6, bound as an object, zooms");
        key(cx, "f7");
        assert_eq!(state(cx).0, None);

        let person = crate::settings::Person { pane_zoom_keys: false, ..crate::settings::Person::default() };
        crate::settings::write_person(&person).unwrap();
        view.update_in(cx, |this, window, cx| {
            this.watched.settings = None;
            this.reload_config(window, cx);
        });
        cx.run_until_parked();
        key(cx, "ctrl-=");
        let off = state(cx);
        assert_eq!(off.0, None, "off: no zoom");
        assert!(off.4, "off: Ctrl+= reached a's program");
        key(cx, "f6");
        let off = state(cx);
        assert_eq!(off.0, None, "off: an object binding doesn't zoom either");
        assert!(off.4, "off: f6 reached a's program");
    }

    #[test]
    fn a_tab_moves_to_another_space_with_its_panes_and_terminals() {
        in_own_process("tab_to_space");
    }

    /// Space s1: tab t (a | b tiled, f floating; b focused and zoomed) and
    /// tab u; space s2: tab v; space mac/r on another machine. tab.move_to_space
    /// takes t whole to s2 and shows it; its terminals keep running, labeled
    /// with their new space, and the document has it there. Wrong ids, its own
    /// space and another machine are errors that change nothing; moving a
    /// space's last tab closes the space and ends no terminal.
    #[gpui::test]
    fn tab_to_space(cx: &mut gpui::TestAppContext) {
        use crate::float_pane::FloatPane;
        use crate::layout::{Axis, Node};
        use crate::model::{Tab, pane};
        use serde_json::json;
        if !running_alone("tab_to_space") {
            return;
        }
        let (core, calls) = crate::hc::answering(|method, _| match method {
            "terminal.label" => Ok(json!({})),
            // A terminal still labeled with a space a move closed.
            "terminal.get" => Ok(json!({ "address": "tz", "state": "live", "keep": true, "labels": { "herdr.space": "s2", "herdr.tab": "v" } })),
            _ => Err(json!({ "message": "not in this test" })),
        });
        let (view, cx) = open_window_on(core, cx, "unused", "", json!({ "id": "main", "type": "block", "content": { "tab": "active" } }));
        let run = |cx: &mut gpui::VisualTestContext, command: &str, args: serde_json::Value| {
            let reply = view.update_in(cx, |this, window, cx| this.run_as(command, args, false, window, cx));
            cx.run_until_parked();
            reply
        };
        let calls_of = |method: &str| {
            let mut calls = calls.lock().unwrap();
            let (taken, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut *calls).into_iter().partition(|(m, _)| m == method);
            *calls = kept;
            let mut taken = taken.into_iter().map(|(_, p)| p).collect::<Vec<_>>();
            taken.sort_by_key(|p| p.to_string());
            taken
        };
        view.update_in(cx, |this, _, _| {
            for machine in ["", "mac"] {
                this.documents.entry(machine.into()).or_default().available = true;
            }
            let mut tab = Tab::new("t", "build", "a", "ta");
            tab.root = Node::container("t", Axis::Row, vec![pane("a", "ta"), pane("b", "tb")]);
            tab.floating.push(FloatPane { pane: pane("f", "tf"), rect: [0.1, 0.1, 0.4, 0.4], dock: None });
            this.model.add_space("s1", "", "/tmp", tab);
            this.model.add_tab("s1", Tab::new("u", "", "c", "tc"));
            this.model.add_space("s2", "", "/tmp", Tab::new("v", "", "d", "td"));
            this.model.add_space("mac/r", "", "/tmp", Tab::new("mac/w", "", "mac/e", "mac/te"));
            this.model.space_mut("mac/r").unwrap().machine = Some("mac".into());
            this.model.focus_pane("b");
            this.model.tab_mut("t").unwrap().zoomed = Some("b".into());
            for (a, space, tab) in [("ta", "s1", "t"), ("tb", "s1", "t"), ("tf", "s1", "t"), ("tc", "s1", "u"), ("td", "s2", "v")] {
                this.live.insert(a.into(), json!({ "address": a, "keep": true, "labels": { "herdr.space": space, "herdr.tab": tab } }));
            }
        });
        let before = view.update(cx, |this, _| this.model.tab("t").unwrap().clone());
        let tabs_of = |cx: &mut gpui::VisualTestContext| view.update(cx, |this, _| {
            this.model.spaces.iter().map(|s| format!("{}:{}", s.id, s.tabs.iter().map(|t| t.id.as_str()).collect::<Vec<_>>().join(","))).collect::<Vec<_>>()
        });

        assert_eq!(run(cx, "tab.move_to_space", json!({ "id": "t", "space": "s2" })), json!({ "ok": true }));
        assert_eq!(tabs_of(cx), ["s1:u", "s2:v,t", "mac/r:mac/w"]);
        view.update(cx, |this, _| {
            let t = this.model.tab("t").unwrap();
            assert_eq!((&t.root, &t.floating, &t.name, &t.zoomed), (&before.root, &before.floating, &before.name, &before.zoomed));
            assert_eq!((this.model.active.as_deref(), this.model.focused_pane().as_deref()), (Some("s2"), Some("b")), "shown where it went, focus kept");
            let shared = serde_json::to_value(crate::mirror::Document::capture(&this.model, None)).unwrap();
            assert_eq!(shared["spaces"][1]["tabs"][1]["id"], "t", "in the machine's document under s2: {shared}");
            assert_eq!(shared["spaces"][1]["tabs"][1]["floating"][0]["pane"]["id"], "f");
        });
        let labels = calls_of("terminal.label").into_iter().filter(|p| p["labels"].get("herdr.space").is_some()).collect::<Vec<_>>();
        assert_eq!(labels, ["ta", "tb", "tf"].map(|a| json!({ "terminal": a, "labels": { "herdr.space": "s2" } })), "the tab's terminals, floats too, and no other");
        assert!(calls_of("terminal.kill").is_empty());

        // The shown tab by default, at a position.
        assert_eq!(run(cx, "tab.move_to_space", json!({ "space": "s1", "index": 1 })), json!({ "ok": true }));
        assert_eq!(tabs_of(cx), ["s1:t,u", "s2:v", "mac/r:mac/w"]);
        assert_eq!(calls_of("terminal.label").iter().filter(|p| p["labels"]["herdr.space"] == "s1").count(), 3);

        // Errors: nothing moves, nothing is labeled.
        for (args, error) in [
            (json!({ "id": "a", "space": "s2" }), "no tab \"a\""),
            (json!({ "id": "t", "space": "nope" }), "no space \"nope\""),
            (json!({ "id": "t", "space": "u" }), "no space \"u\""),
            (json!({ "id": "t", "space": "s1" }), "already in space"),
            (json!({ "id": "t", "space": "mac/r" }), "another machine"),
            (json!({ "id": "mac/w", "space": "s2" }), "another machine"),
            (json!({ "id": "t", "space": "s2", "index": "2" }), "index is a position"),
            (json!({ "id": "t" }), "needs space"),
        ] {
            let reply = run(cx, "tab.move_to_space", args.clone());
            assert!(reply["error"].as_str().is_some_and(|e| e.contains(error)), "{args}: {reply}");
        }
        // While the machine's layout is unavailable, as for every structural command.
        view.update(cx, |this, _| {
            let slot = this.documents.get_mut("").unwrap();
            (slot.available, slot.error) = (false, None);
        });
        let reply = run(cx, "tab.move_to_space", json!({ "id": "t", "space": "s2" }));
        assert!(reply["error"].as_str().is_some_and(|e| e.contains("unavailable")), "{reply}");
        view.update(cx, |this, _| this.documents.get_mut("").unwrap().available = true);
        assert_eq!(tabs_of(cx), ["s1:t,u", "s2:v", "mac/r:mac/w"]);
        assert!(!calls_of("terminal.label").iter().any(|p| p["labels"].get("herdr.space").is_some()), "a failed move labels nothing");

        // A space's last tab: the space closes, its terminal runs on.
        assert_eq!(run(cx, "tab.move_to_space", json!({ "id": "v", "space": "s1" })), json!({ "ok": true }));
        assert_eq!(tabs_of(cx), ["s1:t,u,v", "mac/r:mac/w"]);
        assert!(calls_of("terminal.kill").is_empty() && view.update(cx, |this, _| this.model.pane_of("td").is_some()));
        assert_eq!(view.update(cx, |this, _| this.model.active.clone()).as_deref(), Some("s1"));
        assert!(calls_of("terminal.label").contains(&json!({ "terminal": "td", "labels": { "herdr.space": "s1" } })));

        // A terminal found without a pane, still labeled with the closed space
        // and the moved tab, joins that tab where it is now: no second s2 or v.
        view.update_in(cx, |this, window, cx| {
            this.live.insert("tz".into(), json!({ "address": "tz", "keep": true, "labels": { "herdr.space": "s2", "herdr.tab": "v" } }));
            this.reconcile(window, cx);
        });
        cx.run_until_parked();
        assert_eq!(view.update(cx, |this, _| this.model.pane_of("tz").and_then(|p| this.model.tab_of(&p).map(|t| t.id.clone()))).as_deref(), Some("v"));
        assert_eq!(tabs_of(cx), ["s1:t,u,v", "mac/r:mac/w"]);

        // Another machine's tab moves between its spaces, labeled with the id that machine knows.
        view.update(cx, |this, _| {
            this.model.add_space("mac/r2", "", "/tmp", Tab::new("mac/x", "", "mac/g", "mac/tg"));
            this.model.space_mut("mac/r2").unwrap().machine = Some("mac".into());
            this.live.insert("mac/te".into(), json!({ "address": "mac/te", "keep": true, "labels": { "herdr.space": "r", "herdr.tab": "w" } }));
        });
        assert_eq!(run(cx, "tab.move_to_space", json!({ "id": "mac/w", "space": "mac/r2" })), json!({ "ok": true }));
        assert_eq!(tabs_of(cx), ["s1:t,u,v", "mac/r2:mac/x,mac/w"]);
        assert!(calls_of("terminal.label").contains(&json!({ "terminal": "mac/te", "labels": { "herdr.space": "r2" } })));

        // A pane given to a terminal (terminal.keep) whose tab moved meanwhile lands in that tab, not parked.
        view.update_in(cx, |this, window, cx| {
            this.live.insert("tk".into(), json!({ "address": "tk", "keep": true, "labels": {} }));
            let change = super::TerminalChange::Keep { space: "gone".into(), tab: "u".into(), pane: "k".into(), name: String::new(), target: None, axis: Axis::Row };
            this.apply_terminal_change("tk".into(), change, Ok(()), window, cx);
        });
        assert_eq!(view.update(cx, |this, _| this.model.tab_of("k").map(|t| t.id.clone())).as_deref(), Some("u"));
    }

    #[test]
    fn floating_panes_draw_above_the_tab_and_take_focus_go_and_zoom() {
        in_own_process("floating_panes");
    }

    /// Tab t: a | d tiled, b and c floating side by side over d, e floating
    /// beside them. Floats draw where their rect says, above the tiles; a click
    /// focuses and raises one without reaching the tile below; `pane.go` moves
    /// between floats (the front one wins a tie); a zoomed float fills the tab;
    /// the float commands refuse a tiled pane; and the document and view bring
    /// it all back after a restart. A float's terminal is never adopted again.
    #[gpui::test]
    fn floating_panes(cx: &mut gpui::TestAppContext) {
        use crate::layout::{Axis, Node};
        use crate::model::{Model, Tab, pane};
        use gpui::Modifiers;
        use serde_json::json;
        if !running_alone("floating_panes") {
            return;
        }
        let (view, cx) = open_window(cx, "unused", "", json!({ "id": "main", "type": "block", "content": { "tab": "active" } }));
        let run = |cx: &mut gpui::VisualTestContext, command: &str, args: serde_json::Value| {
            let reply = view.update_in(cx, |this, window, cx| this.run_as(command, args, false, window, cx));
            cx.run_until_parked();
            reply
        };
        view.update_in(cx, |this, window, cx| {
            this.documents.entry(String::new()).or_default().available = true;
            for id in this.overlays.iter().map(|o| o.id.clone()).collect::<Vec<_>>() {
                this.close_overlay(&id, window, cx);
            }
            let mut tab = Tab::new("t", "", "a", "ta");
            tab.root = Node::container("t", Axis::Row, vec![pane("a", "ta"), pane("b", "tb"), pane("c", "tc"), pane("e", "te"), pane("d", "td")]);
            this.model.add_space("s", "", "/tmp", tab);
            this.model.select_tab("t");
            this.model.focus_pane("a");
            for (a, labels) in [("ta", "t"), ("tb", "t"), ("tc", "t"), ("td", "t"), ("te", "t")] {
                this.live.insert(a.into(), json!({ "address": a, "keep": true, "labels": { "herdr.space": "s", "herdr.tab": labels } }));
            }
        });
        for (id, rect) in [("b", [0.76, 0.1, 0.22, 0.3]), ("c", [0.76, 0.1, 0.22, 0.3]), ("e", [0.52, 0.1, 0.2, 0.3])] {
            assert_eq!(run(cx, "pane.float", json!({ "pane": id, "rect": rect })), json!({ "ok": true }), "{id}");
        }
        assert!(run(cx, "pane.float", json!({ "pane": "b" }))["error"].as_str().unwrap().contains("already floats"));
        assert!(run(cx, "pane.float_move:right", json!({ "pane": "a" }))["error"].as_str().unwrap().contains("doesn't float"));
        assert!(run(cx, "pane.raise", json!({ "pane": "a" }))["error"].is_string());
        cx.run_until_parked();
        let (area, b, e, d) = view.update(cx, |this, _| {
            let bounds = this.bounds.borrow();
            (bounds["float-layer:t"], bounds["b"], bounds["e"], bounds["d"])
        });
        let near = |a: f32, b: f32| (a - b).abs() <= 1.5;
        assert!(near(f32::from(b.left() - area.left()), 0.76 * f32::from(area.size.width)), "b where its rect says: {b:?} in {area:?}");
        assert!(d.contains(&b.center()), "b floats over d");

        // A click on e focuses and raises it; d, below, doesn't get it.
        assert_eq!(run(cx, "pane.lower", json!({ "pane": "e", "to": "back" })), json!({ "ok": true }));
        run(cx, "pane.focus", json!({ "pane": "a" }));
        view.update(cx, |this, _| assert_eq!(this.model.tab("t").unwrap().floating[0].pane.id, "e"));
        cx.simulate_click(e.center(), Modifiers::default());
        cx.run_until_parked();
        view.update(cx, |this, _| {
            assert_eq!(this.model.focused_pane().as_deref(), Some("e"), "the click focused e, not d below it");
            assert_eq!(this.model.tab("t").unwrap().floating.last().unwrap().pane.id, "e", "and brought it to the front");
            assert_eq!(this.terminal_pane_at(e.center()).as_deref(), Some("e"), "focus follows mouse sees the float, not d below it");
            // Nothing visited: only the stacking order settles the tie below.
            this.pane_history.retain(|p| p == "e");
        });

        // pane.go between floats: b and c tie, c is in front of b.
        run(cx, "pane.go", json!({ "direction": "right" }));
        view.update(cx, |this, _| assert_eq!(this.model.focused_pane().as_deref(), Some("c")));
        run(cx, "pane.go", json!({ "direction": "left" }));
        view.update(cx, |this, _| assert_eq!(this.model.focused_pane().as_deref(), Some("e"), "back to e, not d around them"));
        let order = |cx: &mut gpui::VisualTestContext| view.update(cx, |this, _| this.model.tab("t").unwrap().floating.iter().map(|f| f.pane.id.clone()).collect::<Vec<_>>());
        assert_eq!(order(cx), ["b", "c", "e"]);
        run(cx, "pane.raise", json!({ "pane": "b" }));
        assert_eq!(order(cx), ["c", "b", "e"], "one step forward");
        run(cx, "pane.lower", json!({ "pane": "e" }));
        assert_eq!(order(cx), ["c", "e", "b"], "one step back");
        run(cx, "pane.focus", json!({ "pane": "e" }));
        run(cx, "pane.float_cycle", json!({}));
        view.update(cx, |this, _| {
            let t = this.model.tab("t").unwrap();
            assert_eq!((t.focused.as_deref(), t.floating.last().map(|f| f.pane.id.as_str())), (Some("c"), Some("c")), "cycling brings the back one forward");
        });
        // Focus following the pointer onto a back float leaves the stacking as it was.
        let stacked = order(cx);
        view.update_in(cx, |this, window, cx| this.hover_focus(json!({ "pane": stacked[0] }), window, cx));
        cx.run_until_parked();
        assert_eq!(order(cx), stacked, "hover focuses without raising");
        view.update(cx, |this, _| assert_eq!(this.model.focused_pane().as_deref(), Some(stacked[0].as_str())));
        run(cx, "pane.focus", json!({ "pane": "c" }));

        // Zoom: a float fills the tab, and takes the keys; a zoomed tiled pane keeps the floats above it.
        run(cx, "pane.zoom", json!({ "pane": "b" }));
        cx.run_until_parked();
        view.update(cx, |this, _| {
            assert_eq!(this.model.focused_pane().as_deref(), Some("b"), "zooming an unfocused pane focuses it");
            let bounds = this.bounds.borrow();
            // Inside its 1 px border (no pane frame in this layout).
            let (b, w) = (f32::from(bounds["b"].size.width), f32::from(area.size.width));
            assert!((w - 2.0 - b).abs() <= 0.5, "the zoomed float fills the tab: {b} in {w}");
        });
        run(cx, "pane.zoom", json!({ "pane": "b" }));
        run(cx, "pane.zoom", json!({ "pane": "a" }));
        view.update(cx, |this, _| assert_eq!(this.model.tab("t").unwrap().shown_floats().count(), 3));
        run(cx, "pane.zoom", json!({ "pane": "a" }));
        run(cx, "pane.focus", json!({ "pane": "c" }));

        // Keyboard steps are cells of the float's own tab, shown or not.
        view.update_in(cx, |this, window, _| {
            use gpui::{Bounds, point, px, size};
            this.model.add_tab("s", Tab::new("u", "", "f", "tf"));
            this.model.float_pane("f", [0.1, 0.1, 0.5, 0.5]).unwrap();
            this.model.select_tab("t");
            let area = |w: f32, h: f32| Bounds::new(point(px(0.0), px(0.0)), size(px(w), px(h)));
            this.bounds.borrow_mut().insert("float-layer:t".into(), area(1000.0, 600.0));
            this.bounds.borrow_mut().insert("float-layer:u".into(), area(500.0, 300.0));
            let (shown, other) = (this.float_step("b", &json!({}), window).unwrap(), this.float_step("f", &json!({}), window).unwrap());
            assert!((other.0 - 2.0 * shown.0).abs() < 1e-9 && (other.1 - 2.0 * shown.1).abs() < 1e-9, "{shown:?} {other:?}");
            this.model.remove_tab("u");
        });

        // Moves stay inside; resizes keep the corner.
        run(cx, "pane.float_move", json!({ "pane": "b", "to": "bottom-right" }));
        run(cx, "pane.float_resize", json!({ "pane": "b", "direction": "right" }));
        view.update(cx, |this, _| assert_eq!(this.model.tab("t").unwrap().float("b").unwrap().rect, [0.5, 0.5, 0.5, 0.5], "already as wide as it fits"));

        // The orphan scan sees the floats' terminals as placed.
        view.update_in(cx, |this, window, cx| {
            this.reconcile(window, cx);
            assert!(this.changing_terminals.is_empty(), "no float terminal is adopted again: {:?}", this.changing_terminals);
            assert_eq!(this.model.tab("t").unwrap().panes().len(), 5);
        });

        // A restart: the document and this screen's view bring back the floats, their order and the focus.
        let (doc, saved) = view.update(cx, |this, _| (serde_json::to_string(&crate::mirror::Document::capture(&this.model, None)).unwrap(), serde_json::to_string(&crate::mirror::View::capture(&this.model)).unwrap()));
        let mut back = Model::default();
        serde_json::from_str::<crate::mirror::Document>(&doc).unwrap().replace(&mut back, None);
        serde_json::from_str::<crate::mirror::View>(&saved).unwrap().apply(&mut back);
        view.update(cx, |this, _| {
            let (t, u) = (this.model.tab("t").unwrap(), back.tab("t").unwrap());
            assert_eq!(u.floating, t.floating);
            assert_eq!((u.focused.as_deref(), u.tiled_count()), (Some("c"), 2));
        });

        // Docking puts each back; the tab reads a b c e d again.
        for id in ["e", "c", "b"] {
            assert_eq!(run(cx, "pane.dock", json!({ "pane": id })), json!({ "ok": true }), "{id}");
        }
        view.update(cx, |this, _| {
            let order: Vec<String> = crate::arrange::reading_order(&this.model.tab("t").unwrap().root).iter().map(|n| n.id.clone()).collect();
            assert_eq!(order, ["a", "b", "c", "e", "d"]);
        });
        // The header's float button runs pane.float_toggle for its pane: a click floats, the next docks.
        for floats in [true, false] {
            assert_eq!(run(cx, "pane.float_toggle", json!({ "pane": "b" })), json!({ "ok": true }));
            view.update(cx, |this, _| assert_eq!(this.model.tab("t").unwrap().float("b").is_some(), floats));
        }
        view.update(cx, |this, _| {
            let order: Vec<String> = crate::arrange::reading_order(&this.model.tab("t").unwrap().root).iter().map(|n| n.id.clone()).collect();
            assert_eq!(order, ["a", "b", "c", "e", "d"], "back where it was");
        });

        // An older build on the same machine adopted a float's terminal as a
        // tiled pane too. Read here, the float wins, and that read alone
        // writes nothing back, so the two builds don't rewrite each other.
        let doc: crate::mirror::Document = serde_json::from_value(json!({"spaces":[{"id":"s","tabs":[{"id":"t","root":
            {"id":"t","type":"row","children":[{"id":"a","type":"block","content":{"terminal":"ta"}},{"id":"old","type":"block","content":{"terminal":"tb"}}]},
            "floating":[{"pane":{"id":"b","type":"block","content":{"terminal":"tb"}},"rect":[0.2,0.2,0.6,0.6]}]}]}]})).unwrap();
        view.update_in(cx, |this, window, cx| {
            let slot = this.documents.entry(String::new()).or_default();
            (slot.connected, slot.available, slot.dirty, slot.version) = (true, true, false, 1);
            this.documents_watching = true;
            this.documents_updated(None, vec![(String::new(), crate::mirror_sync::Outcome::Read(doc.clone(), 2))], window, cx);
            assert!(this.model.tab("t").unwrap().find("old").is_none(), "the tiled copy is gone");
            this.flush_documents(false, window, cx);
            assert!(this.documents[""].pending.is_none(), "a read alone writes nothing");
            // Read again at the same version (any document event refreshes): still nothing to write.
            this.documents_updated(None, vec![(String::new(), crate::mirror_sync::Outcome::Read(doc, 2))], window, cx);
            this.flush_documents(false, window, cx);
            assert!(this.documents[""].pending.is_none(), "a re-read at the same version writes nothing either");
        });
    }

    #[test]
    fn arranging_panes_keeps_terminals_and_focus_and_cycles_modes() {
        in_own_process("arrange_panes");
    }

    /// Tab t: a | (b / c) with a mod pane m under c, b focused and zoomed.
    /// `pane.arrange` rebuilds it without touching a pane's terminal or mod,
    /// keeps b focused, unzooms, and `next` walks the modes from tiled.
    #[gpui::test]
    fn arrange_panes(cx: &mut gpui::TestAppContext) {
        use crate::layout::{Axis, Content, Node};
        use crate::model::{Tab, pane};
        use serde_json::json;
        if !running_alone("arrange_panes") {
            return;
        }
        let (view, cx) = open_window(cx, "unused", "", json!({ "id": "main", "type": "block", "content": { "tab": "active" } }));
        let run = |cx: &mut gpui::VisualTestContext, command: &str, args: serde_json::Value| {
            let reply = view.update_in(cx, |this, window, cx| this.run_as(command, args, false, window, cx));
            cx.run_until_parked();
            reply
        };
        view.update_in(cx, |this, _, _| {
            this.documents.entry(String::new()).or_default().available = true;
            let viewer = Node::block("m", Content::Mod { name: "unused".into(), props: json!({}) });
            let right = Node::container("r", Axis::Column, vec![pane("b", "tb"), pane("c", "tc"), viewer]);
            let mut tab = Tab::new("t", "", "a", "ta");
            tab.root = Node::container("t", Axis::Row, vec![pane("a", "ta"), right]);
            this.model.add_space("s", "", "/tmp", tab);
            this.model.add_tab("s", Tab::new("u", "", "d", "td"));
            this.model.select_tab("t");
            this.model.focus_pane("b");
            this.model.tab_mut("t").unwrap().zoomed = Some("b".into());
        });
        let contents = |cx: &mut gpui::VisualTestContext| view.update(cx, |this, _| {
            let mut all: Vec<(String, Option<Content>)> = this.model.tab("t").unwrap().root.panes().iter().map(|p| (p.id.clone(), p.content().cloned())).collect();
            all.sort_by(|x, y| x.0.cmp(&y.0));
            all
        });
        let before = contents(cx);
        for want in ["tiled", "bsp", "master", "columns", "rows", "tiled"] {
            let reply = run(cx, "pane.arrange", json!({ "mode": "next" }));
            assert_eq!(reply, json!({ "ok": true, "tab": "t", "mode": want }));
            assert_eq!(contents(cx), before, "{want}: every pane keeps its terminal or mod");
            view.update(cx, |this, _| {
                let tab = this.model.tab("t").unwrap();
                assert_eq!(tab.arrange.map(crate::arrange::Mode::name), Some(want));
                assert_eq!(this.world()["spaces"][0]["tabs"][0]["arrange"], json!(want), "herdr-app ls shows the mode");
                assert_eq!(tab.zoomed, None);
                assert_eq!(this.model.focused_pane().as_deref(), Some("b"), "{want}");
                assert_eq!(this.model.active_tab().map(|t| t.id.as_str()), Some("t"));
            });
        }
        // Master mode puts the focused pane first, on the left.
        run(cx, "pane.arrange", json!({ "mode": "master" }));
        view.update(cx, |this, _| assert_eq!(this.model.tab("t").unwrap().root.children()[0].id, "b"));
        // Another tab by id; its own mode memory.
        assert_eq!(run(cx, "pane.arrange", json!({ "mode": "next", "tab": "u" }))["mode"], "tiled");
        view.update(cx, |this, _| assert_eq!(this.model.tab("t").unwrap().arrange, Some(crate::arrange::Mode::Master)));
        assert!(run(cx, "pane.arrange", json!({ "mode": "spiral" }))["error"].as_str().unwrap().contains("not \"spiral\""));
        assert!(run(cx, "pane.arrange", json!({ "mode": "rows", "tab": "nope" }))["error"].is_string());
        assert!(run(cx, "pane.arrange", json!({}))["error"].as_str().unwrap().contains("needs mode"));
    }

    #[test]
    fn a_tab_with_a_mode_arranges_again_on_split_and_close_until_turned_off() {
        in_own_process("auto_arrange");
    }

    /// Tab t starts as one terminal pane a in columns. Mod panes split and
    /// close (no core needed); every split and close runs columns again, a
    /// dragged size lasts until the next one, bsp splits the focused pane the
    /// way asked and keeps its tree, `off` makes splits plain, and
    /// settings.json's `default_arrange` gives new tabs a mode.
    #[gpui::test]
    fn auto_arrange(cx: &mut gpui::TestAppContext) {
        use crate::arrange::Mode;
        use crate::layout::{Axis, Node, Size};
        use crate::model::Tab;
        use serde_json::json;
        if !running_alone("auto_arrange") {
            return;
        }
        let (view, cx) = open_window(cx, "unused", "", json!({ "id": "main", "type": "block", "content": { "tab": "active" } }));
        let run = |cx: &mut gpui::VisualTestContext, command: &str, args: serde_json::Value| {
            let reply = view.update_in(cx, |this, window, cx| this.run_as(command, args, false, window, cx));
            cx.run_until_parked();
            reply
        };
        view.update_in(cx, |this, _, _| {
            this.documents.entry(String::new()).or_default().available = true;
            this.model.add_space("s", "", "/tmp", Tab::new("t", "", "a", "ta"));
            this.model.select_tab("t");
        });
        // The tree with each pane named as the test knows it.
        let names = std::rc::Rc::new(std::cell::RefCell::new(vec![("a".to_owned(), "a".to_owned())]));
        let shape = |cx: &mut gpui::VisualTestContext, tab: &str| {
            fn draw(n: &Node, names: &[(String, String)]) -> String {
                match n.axis() {
                    Some(axis) => {
                        let inner: Vec<String> = n.children().iter().map(|c| draw(c, names)).collect();
                        format!("{}[{}]", if axis == Axis::Row { "R" } else { "C" }, inner.join(" "))
                    }
                    None => names.iter().find(|(id, _)| *id == n.id).map_or_else(|| n.id.clone(), |(_, name)| name.clone()),
                }
            }
            let names = names.borrow().clone();
            view.update(cx, |this, _| draw(&this.model.tab(tab).unwrap().root, &names))
        };
        let split = |cx: &mut gpui::VisualTestContext, direction: &str, name: &str| {
            let reply = run(cx, "pane.split", json!({ "direction": direction, "mod": "unused" }));
            let id = reply["pane"].as_str().unwrap_or_else(|| panic!("{reply}")).to_owned();
            names.borrow_mut().push((id.clone(), name.to_owned()));
            view.update(cx, |this, _| assert_eq!(this.model.focused_pane(), Some(id.clone()), "focus goes to the new pane {name}"));
            id
        };
        let focused = |cx: &mut gpui::VisualTestContext| view.update(cx, |this, _| this.model.focused_pane().unwrap());

        assert_eq!(run(cx, "pane.arrange", json!({ "mode": "columns" }))["mode"], "columns");
        let b = split(cx, "right", "b");
        assert_eq!(shape(cx, "t"), "R[a b]");
        // From a, down: still one row of columns, the new pane right after a.
        run(cx, "pane.focus", json!({ "pane": "a" }));
        let d = split(cx, "down", "d");
        assert_eq!(shape(cx, "t"), "R[a d b]");
        // A dragged divider stays while nothing comes or goes.
        view.update(cx, |this, _| this.model.tab_mut("t").unwrap().root.children_mut().unwrap()[0].size = Some(Size::Weight { weight: 2.5 }));
        run(cx, "pane.focus", json!({ "pane": b }));
        run(cx, "pane.swap", json!({ "direction": "left" }));
        assert_eq!(shape(cx, "t"), "R[a b d]", "a swap keeps the shape");
        view.update(cx, |this, _| assert_eq!(this.model.tab("t").unwrap().root.children()[0].size, Some(Size::Weight { weight: 2.5 })));
        // Closing evens the sizes and goes on in the swapped order.
        run(cx, "pane.close", json!({ "pane": d }));
        assert_eq!(shape(cx, "t"), "R[a b]");
        view.update(cx, |this, _| {
            let tab = this.model.tab("t").unwrap();
            assert!(tab.root.walk().iter().all(|n| n.size.is_none()));
            assert_eq!(tab.arrange, Some(Mode::Columns), "the mode stays");
        });
        assert_eq!(focused(cx), b, "focus stays where it was");
        // Bsp: the focused pane splits the way asked, and a close leaves the rest.
        assert_eq!(run(cx, "pane.arrange", json!({ "mode": "bsp" }))["mode"], "bsp");
        let c = split(cx, "down", "c");
        assert_eq!(shape(cx, "t"), "R[a C[b c]]");
        split(cx, "right", "e");
        assert_eq!(shape(cx, "t"), "R[a C[b R[c e]]]");
        run(cx, "pane.close", json!({ "pane": c }));
        assert_eq!(shape(cx, "t"), "R[a C[b e]]");
        view.update(cx, |this, _| assert_eq!(this.model.tab("t").unwrap().arrange, Some(Mode::Bsp)));
        // The arranged tree is the shared document's.
        view.update(cx, |this, _| {
            let shared = serde_json::to_value(crate::mirror::Document::capture(&this.model, None)).unwrap();
            assert_eq!(shared["spaces"][0]["tabs"][0]["root"], serde_json::to_value(&this.model.tab("t").unwrap().root).unwrap());
        });
        // Off: plain splits again, the tree left as it was.
        assert_eq!(run(cx, "pane.arrange", json!({ "mode": "off" })), json!({ "ok": true, "tab": "t", "mode": "off" }));
        assert_eq!(shape(cx, "t"), "R[a C[b e]]");
        view.update(cx, |this, _| assert_eq!(this.model.tab("t").unwrap().arrange, None));
        run(cx, "pane.focus", json!({ "pane": b }));
        split(cx, "right", "h");
        assert_eq!(shape(cx, "t"), "R[a C[R[b h] e]]");
        // `next` after off starts again from tiled.
        assert_eq!(run(cx, "pane.arrange", json!({ "mode": "next" }))["mode"], "tiled");

        // default_arrange: tabs made from now on start in it.
        std::fs::write(crate::settings::path(), r#"{ "default_arrange": "columns" }"#).unwrap();
        view.update_in(cx, |this, window, cx| this.reload_config(window, cx));
        let tab = run(cx, "tab.new", json!({ "mod": "unused" }));
        let tab = tab["tab"].as_str().unwrap_or_else(|| panic!("{tab}")).to_owned();
        view.update(cx, |this, _| {
            assert_eq!(this.model.tab(&tab).unwrap().arrange, Some(Mode::Columns));
            assert_eq!(this.model.tab("t").unwrap().arrange, Some(Mode::Tiled), "tabs already there keep theirs");
        });
        split(cx, "down", "f");
        split(cx, "down", "g");
        let first = view.update(cx, |this, _| this.model.tab(&tab).unwrap().root.panes()[0].id.clone());
        names.borrow_mut().push((first, "x".into()));
        assert_eq!(shape(cx, &tab), "R[x f g]");
    }

    #[test]
    fn a_script_toggling_follows_mouse_over_a_broken_settings_json_gets_an_error() {
        in_own_process("toggle_broken");
    }

    #[gpui::test]
    fn toggle_broken(cx: &mut gpui::TestAppContext) {
        use serde_json::json;
        if !running_alone("toggle_broken") {
            return;
        }
        let (view, cx) = open_window(cx, "unused", "", json!({ "id": "pane", "type": "block", "content": { "terminal": "t1" } }));
        std::fs::write(crate::settings::path(), "{ \"focus_follows_mouse\": ").unwrap();
        let reply = cx.update(|window, cx| view.update(cx, |this, cx| this.run_as("focus.toggle_follows_mouse", json!({}), false, window, cx)));
        assert_eq!(reply["error"], "Focus follows mouse: on (not saved: settings.json has errors)", "{reply}");
        assert_eq!(std::fs::read_to_string(crate::settings::path()).unwrap(), "{ \"focus_follows_mouse\": ", "left as it was");
    }

    #[test]
    fn a_failing_alert_command_says_why_once_and_posts_the_notification() {
        in_own_process("alert_errors");
    }

    /// Each reason one error toast however many alerts hit it, for a program
    /// that doesn't start and for a shell line that exits 127 later; a new
    /// `alert_command` gets its own say.
    #[gpui::test]
    fn alert_errors(cx: &mut gpui::TestAppContext) {
        use crate::settings::{AlertCommand, SystemNotifications};
        use serde_json::json;
        if !running_alone("alert_errors") {
            return;
        }
        cx.update(|cx| cx.set_app_identity("dev.herdr.test", "Herdr"));
        // The shell's exit comes back from the reaper's own thread.
        cx.executor().allow_parking();
        let (view, cx) = open_window(cx, "unused", "", json!({ "id": "pane", "type": "block", "content": { "terminal": "t1" } }));
        // Still running when the late notification comes: it posts only then.
        view.update(cx, |this, _| { this.live.insert("t1".into(), json!({ "address": "t1", "state": "live" })); });
        let toasts = |cx: &mut gpui::VisualTestContext| view.update(cx, |this, _| this.overlays.iter().filter(|o| o.kind == super::OverlayKind::Toast).count());
        let alert_with = |command: AlertCommand, cx: &mut gpui::VisualTestContext| {
            cx.update(|window, cx| view.update(cx, |this, cx| {
                this.person.sounds.enabled = false;
                this.person.system_notifications = SystemNotifications::Always;
                this.person.alert_command = Some(command);
                this.apply_look(window, cx);
                for _ in 0..2 {
                    this.alert("t1", crate::model::Alert::Done, None, false, window, cx);
                }
            }));
        };
        alert_with(AlertCommand::Argv(vec!["/nonexistent/herdr-notify".into()]), cx);
        assert_eq!(toasts(cx), 2, "one alert toast (the second alert joined it) and one error toast");
        assert_eq!(cx.shown_system_notifications().len(), 2, "both fell back");

        alert_with(AlertCommand::Shell("/nonexistent/herdr-notify -s done".into()), cx);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while cx.shown_system_notifications().len() < 4 {
            assert!(std::time::Instant::now() < deadline, "the shell's exit 127 never came back");
            std::thread::sleep(std::time::Duration::from_millis(20));
            cx.run_until_parked();
        }
        cx.run_until_parked();
        assert_eq!(toasts(cx), 3, "the alerts joined the same toast, one more error toast for the new command");
        view.update(cx, |this, _| {
            assert!(this.alert_command_errors.iter().any(|why| why.contains("command not found (exit 127)")), "{:?}", this.alert_command_errors);
            assert_eq!(this.alert_command_errors.len(), 1, "cleared when alert_command changed");
            assert!(this.notified.contains("t1"));
        });
    }

    #[test]
    fn an_alert_toast_names_one_agent_and_counts_more() {
        use super::{AlertToast, AlertedAgent};
        use crate::model::Alert;
        use serde_json::json;
        let agent = |n: usize| AlertedAgent { address: format!("t{n}"), name: format!("a{n}"), place: Some(format!("s / tab{n}")) };
        let one = AlertToast { alert: Alert::Done, agents: vec![agent(1)], joined: std::time::Instant::now() };
        assert_eq!(
            one.notification(),
            json!({ "title": "a1 finished", "body": "s / tab1", "level": "info", "action": { "name": "agent.focus", "args": { "terminal": "t1" } } }),
            "one agent's toast is as it always was"
        );
        let three = AlertToast { alert: Alert::Blocked, agents: (1..=3).rev().map(agent).collect(), ..one };
        let n = three.notification();
        assert_eq!((n["title"].as_str(), n["body"].as_str(), n["level"].as_str()), (Some("3 agents need you"), Some("a3, a2, a1"), Some("error")));
        assert_eq!(n["action"]["args"]["terminal"], "t3", "a click goes to the newest");
        let seven = AlertToast { alert: Alert::Error, agents: (1..=7).rev().map(agent).collect(), ..three };
        let n = seven.notification();
        assert_eq!((n["title"].as_str(), n["body"].as_str()), (Some("7 agents hit an error"), Some("a7, a6, a5, a4 and 3 more")));
    }

    #[test]
    fn alerts_close_together_share_a_toast_and_toasts_are_capped() {
        in_own_process("alert_toasts");
    }

    /// A wave of agents finishing is one toast, redrawn with each, not a toast
    /// each; another kind of alert, or one after the wave, gets its own; at
    /// most `TOAST_LIMIT` toasts show, the newest, except that errors and
    /// warnings outlast routine toasts.
    #[gpui::test]
    fn alert_toasts(cx: &mut gpui::TestAppContext) {
        use super::{ALERT_JOIN, OverlayKind, TOAST_LIMIT};
        use crate::model::Alert;
        use serde_json::json;
        if !running_alone("alert_toasts") {
            return;
        }
        // The shipped toast mod, its root keyed by its title so the test can see what it draws.
        let shipped = include_str!("../defaults/mods/toast/main.js");
        let keyed = shipped.replace(r#".id("toast")"#, r#".id(`toast-${p.title}`)"#);
        assert_ne!(keyed, shipped, "the toast mod changed shape: key its root again");
        let (view, cx) = open_window(cx, "toast", &keyed, json!({ "id": "pane", "type": "block" }));
        cx.run_until_parked();
        let alert = |addr: &str, kind: Alert, cx: &mut gpui::VisualTestContext| {
            cx.update(|window, cx| view.update(cx, |this, cx| {
                this.person.sounds.enabled = false;
                this.live.insert(addr.into(), json!({ "name": format!("agent {addr}") }));
                this.alert(addr, kind, None, false, window, cx);
            }));
            cx.run_until_parked();
        };
        let toasts = |cx: &mut gpui::VisualTestContext| {
            view.update(cx, |this, _| this.overlays.iter().filter(|o| o.kind == OverlayKind::Toast).map(|o| o.id.clone()).collect::<Vec<_>>())
        };

        alert("t1", Alert::Done, cx);
        assert_eq!(toasts(cx).len(), 1);
        assert!(cx.debug_bounds("toast-agent t1 finished").is_some(), "one agent: its own toast");
        let first = toasts(cx)[0].clone();
        for addr in ["t2", "t3", "t2"] {
            alert(addr, Alert::Done, cx);
        }
        assert_eq!(toasts(cx), vec![first.clone()], "the wave joined the first toast, no new mod loaded");
        assert!(cx.debug_bounds("toast-3 agents finished").is_some(), "the toast redrew with the count, each agent once");
        assert!(cx.debug_bounds("toast-agent t1 finished").is_none());
        let props = view.update(cx, |this, _| this.copies[&format!("overlay:{first}")].inst.props.clone());
        assert_eq!(props["body"], "agent t2, agent t3, agent t1", "newest first");
        assert_eq!(props["action"]["args"]["terminal"], "t2", "a click goes to the newest");
        assert_eq!(props["overlay"], first.as_str(), "it can still close itself");

        alert("t4", Alert::Blocked, cx);
        assert_eq!(toasts(cx).len(), 2, "needing the person is another kind of alert");
        let needs_you = toasts(cx)[1].clone();

        view.update(cx, |this, _| {
            for o in &mut this.overlays {
                if let Some(t) = &mut o.alert {
                    t.joined -= ALERT_JOIN;
                }
            }
        });
        alert("t5", Alert::Done, cx);
        assert_eq!(toasts(cx).len(), 3, "after the wave, a finish gets a toast of its own");

        cx.update(|window, cx| view.update(cx, |this, cx| {
            for i in 0..4 {
                this.open_overlay(format!("n{i}"), json!({ "kind": "toast", "notification": { "title": format!("note {i}") } }), window, cx);
            }
        }));
        cx.run_until_parked();
        let shown = toasts(cx);
        assert_eq!(shown.len(), TOAST_LIMIT);
        assert!(!shown.contains(&first) && shown.ends_with(&["n0".into(), "n1".into(), "n2".into(), "n3".into()]), "the oldest closed: {shown:?}");
        assert!(shown.contains(&needs_you), "routine toasts close before an older one that needs the person: {shown:?}");
        assert!(cx.debug_bounds("toast-3 agents finished").is_none(), "its mod went with it");
    }

    #[test]
    fn a_saved_agent_in_a_bare_shell_is_offered_and_resumed_only_there() {
        in_own_process("resume_offer");
    }

    /// After a start, a recorded agent whose terminal is a bare shell again
    /// gets one toast; its action types the command line into that terminal
    /// only, never into one running something else.
    #[gpui::test]
    fn resume_offer(cx: &mut gpui::TestAppContext) {
        use super::OverlayKind;
        use crate::resume::Record;
        use serde_json::json;
        if !running_alone("resume_offer") {
            return;
        }
        cx.executor().allow_parking();
        let (core, calls) = crate::hc::answering(|method, _| match method {
            "terminal.send" => Ok(json!({})),
            _ => Err(json!({ "message": "not in this test" })),
        });
        let (view, cx) = open_window_on(core, cx, "toast", include_str!("../defaults/mods/toast/main.js"), json!({ "id": "pane", "type": "block" }));
        cx.run_until_parked();
        let record = |terminal: &str, session: &str| Record {
            terminal: terminal.into(), place: Some(format!("s / {terminal}")), program: "claude".into(), name: "Claude Code".into(),
            cwd: "/w/it's".into(), argv: ["claude", "--model", "opus", "--resume", session].map(String::from).to_vec(),
            session: Some(session.into()), saved_ms: 1,
        };
        cx.update(|window, cx| view.update(cx, |this, cx| {
            this.live.insert("t1".into(), json!({ "address": "t1", "pid": 5, "foreground": { "pid": 5, "argv": ["-zsh"], "cwd": "/w" }, "labels": {} }));
            this.live.insert("t2".into(), json!({ "address": "t2", "pid": 6, "foreground": { "pid": 9, "argv": ["vim", "x"], "cwd": "/w" }, "labels": {} }));
            this.resume.insert(record("t1", "s1"));
            this.resume.insert(record("t2", "s2"));
            this.person.resume_agents = crate::settings::ResumeAgents::Ask;
            this.track_resume(window, cx);
            this.track_resume(window, cx);
        }));
        cx.run_until_parked();
        let toasts = |cx: &mut gpui::VisualTestContext| view.update(cx, |this, _| {
            this.overlays.iter().filter(|o| o.kind == OverlayKind::Toast).map(|o| this.copies[&format!("overlay:{}", o.id)].inst.props.clone()).collect::<Vec<_>>()
        });
        let shown = toasts(cx);
        assert_eq!(shown.len(), 1, "one offer, once: {shown:?}");
        assert_eq!((shown[0]["title"].as_str(), shown[0]["body"].as_str()), (Some("Resume 1 session"), Some("Claude Code in s / t1")));
        assert_eq!(shown[0]["action"], json!({ "name": "session.resume", "args": { "confirmed": true } }));
        let sends = || calls.lock().unwrap().iter().filter(|(m, _)| m == "terminal.send").cloned().collect::<Vec<_>>();
        assert!(sends().is_empty(), "nothing typed before the person says so");

        let saved = cx.update(|window, cx| view.update(cx, |this, cx| this.run_as("session.saved", json!({}), false, window, cx)));
        assert_eq!(saved["sessions"].as_array().map(|s| s.iter().map(|s| s["ready"].clone()).collect::<Vec<_>>()), Some(vec![json!(true), json!(false)]));
        assert!(saved["sessions"][1]["why"].as_str().is_some_and(|w| w.contains("Vim")), "{saved}");

        let unconfirmed = cx.update(|window, cx| view.update(cx, |this, cx| this.run_as("session.resume", json!({}), false, window, cx)));
        assert!(unconfirmed["error"].as_str().is_some_and(|e| e.contains("confirmed")), "{unconfirmed}");
        let done = cx.update(|window, cx| view.update(cx, |this, cx| this.run_as("session.resume", json!({ "confirmed": true }), false, window, cx)));
        assert_eq!(done["resumed"][0]["terminal"], "t1");
        assert_eq!(done["skipped"][0]["terminal"], "t2");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while sends().is_empty() {
            assert!(std::time::Instant::now() < deadline, "terminal.send never went");
            std::thread::sleep(std::time::Duration::from_millis(20));
            cx.run_until_parked();
        }
        assert_eq!(sends(), vec![("terminal.send".to_owned(), json!({ "terminal": "t1", "text": "cd '/w/it'\\''s' && claude --model opus --resume s1" }))]);
        let missing = cx.update(|window, cx| view.update(cx, |this, cx| this.run_as("session.resume", json!({ "terminal": "t7", "confirmed": true }), false, window, cx)));
        assert!(missing["error"].as_str().is_some_and(|e| e.contains("no saved session")), "{missing}");
    }

    #[test]
    fn the_toast_clock_runs_only_while_a_toast_is_up() {
        in_own_process("toast_clock");
    }

    /// No mod root wakes for toasts it doesn't have: the clock starts with a
    /// toast and stops once the last one has left.
    #[gpui::test]
    fn toast_clock(cx: &mut gpui::TestAppContext) {
        use serde_json::json;
        if !running_alone("toast_clock") {
            return;
        }
        let (view, cx) = open_window(cx, "probe", "import { View, div } from \"gpui-kit\";\nexport default class P extends View { render() { return div(); } }", json!({ "id": "probe", "type": "block", "content": { "mod": "probe" } }));
        cx.run_until_parked();
        let roots: Vec<_> = view.update(cx, |this, _| this.copies.values().filter_map(|c| c.root.clone()).collect());
        assert!(!roots.is_empty(), "no mod loaded");
        let running = |cx: &mut gpui::VisualTestContext| roots.iter().filter(|r| r.read_with(cx, |r, _| r.toast_clock_running())).count();
        assert_eq!(running(cx), 0, "no toasts, no clock");
        let root = roots[0].clone();
        cx.update(|window, cx| root.update(cx, |r, cx| r.push_toast(gpui_shell::ToastRequest::new("Saved"), window, cx)));
        assert_eq!(running(cx), 1, "a toast starts its root's clock, and only its");
        cx.executor().advance_clock(gpui_shell::ToastRequest::DEFAULT_TIMEOUT / 2);
        cx.run_until_parked();
        assert_eq!((running(cx), root.read_with(cx, |r, _| r.toast_count())), (1, 1), "still up, still ticking");
        cx.executor().advance_clock(gpui_shell::ToastRequest::DEFAULT_TIMEOUT * 2);
        cx.run_until_parked();
        assert_eq!(root.read_with(cx, |r, _| r.toast_count()), 0, "the toast timed out and left");
        assert_eq!(running(cx), 0, "and the clock stopped with it");
    }

    #[test]
    fn an_alert_carries_the_agents_own_message_from_the_listing() {
        in_own_process("alert_message");
    }

    /// The 7501 record in the core's listing reaches the notification: its
    /// `msg` over "space / tab", and `error` told apart from `blocked`.
    #[gpui::test]
    fn alert_message(cx: &mut gpui::TestAppContext) {
        use crate::settings::SystemNotifications;
        use serde_json::json;
        if !running_alone("alert_message") {
            return;
        }
        cx.update(|cx| cx.set_app_identity("dev.herdr.test", "Herdr"));
        let (view, cx) = open_window(cx, "unused", "", json!({ "id": "pane", "type": "block", "content": { "terminal": "t1" } }));
        view.update(cx, |this, _| this.model.add_space("p1", "api", "/src/api", crate::model::Tab::new("c1", "fix login", "x1", "t1")));
        let list = |status: serde_json::Value, cx: &mut gpui::VisualTestContext| {
            cx.update(|window, cx| view.update(cx, |this, cx| {
                this.person.sounds.enabled = false;
                this.person.system_notifications = SystemNotifications::Always;
                this.person.alert_command = None;
                this.apply_look(window, cx);
                this.window_active = false;
                this.terminals_listed(json!({ "terminals": [{ "address": "t1", "name": "Claude", "status": status }] }), window, cx);
            }));
        };
        list(json!({ "state": "working", "app": "claude-code" }), cx);
        list(json!({ "state": "blocked", "kind": "permission", "app": "claude-code", "msg": "approve Bash: make" }), cx);
        // Its program icon is made off the main thread first.
        cx.run_until_parked();
        let shown = cx.shown_system_notifications();
        let last = shown.last().expect("posted");
        assert_eq!((last.title.as_ref(), last.body.lines().next()), ("Claude needs you", Some("approve Bash: make")));
        list(json!({ "state": "working", "app": "claude-code" }), cx);
        list(json!({ "state": "error", "app": "claude-code" }), cx);
        cx.run_until_parked();
        let shown = cx.shown_system_notifications();
        assert_eq!(shown.last().map(|n| n.title.to_string()), Some("Claude hit an error".into()));
        assert!(!shown.last().unwrap().body.contains("approve"), "an old message doesn't linger");
    }

    #[test]
    fn the_dock_badge_follows_listings_and_the_setting() {
        in_own_process("dock_badge");
    }

    /// The badge counts the agents list's waiting rows as listings arrive, and
    /// settings.json's `dock_badge` takes it off and puts it back.
    #[gpui::test]
    fn dock_badge(cx: &mut gpui::TestAppContext) {
        use serde_json::json;
        if !running_alone("dock_badge") {
            return;
        }
        let (view, cx) = open_window(cx, "unused", "", json!({ "id": "pane", "type": "block", "content": { "terminal": "t1" } }));
        view.update(cx, |this, _| {
            this.model.add_space("p1", "api", "/src/api", crate::model::Tab::new("c1", "one", "x1", "t1"));
            this.model.add_space("p2", "web", "/src/web", crate::model::Tab::new("c2", "two", "x2", "t2"));
        });
        let list = |terminals: serde_json::Value, cx: &mut gpui::VisualTestContext| {
            cx.update(|window, cx| view.update(cx, |this, cx| {
                this.person.sounds.enabled = false;
                this.window_active = false;
                this.terminals_listed(json!({ "terminals": terminals }), window, cx);
                this.publish(cx);
            }));
        };
        let shown = |cx: &mut gpui::VisualTestContext| view.update(cx, |this, _| this.dock_badge.shown().to_owned());
        let agent = |address: &str, state: &str| json!({ "address": address, "state": "live", "status": { "state": state } });
        list(json!([agent("t1", "working"), agent("t2", "working")]), cx);
        assert_eq!(shown(cx), "");
        list(json!([agent("t1", "blocked"), agent("t2", "working")]), cx);
        assert_eq!(shown(cx), "1");
        let mut kid = agent("t3", "blocked");
        kid["parent"] = json!("t2");
        list(json!([agent("t1", "blocked"), agent("t2", "working"), kid]), cx);
        assert_eq!(shown(cx), "2", "a blocked sub-agent puts its parent's row in");
        let set = |on: bool, cx: &mut gpui::VisualTestContext| cx.update(|window, cx| view.update(cx, |this, cx| {
            this.person.dock_badge = on;
            this.apply_look(window, cx);
        }));
        set(false, cx);
        assert_eq!(shown(cx), "", "off takes it off at once");
        set(true, cx);
        assert_eq!(shown(cx), "2", "on puts the count back");
        list(json!([agent("t1", "working"), agent("t2", "working")]), cx);
        assert_eq!(shown(cx), "", "nothing waits: no badge");
    }

    #[test]
    fn a_needs_you_notification_stays_until_the_agent_is_unblocked() {
        in_own_process("sticky_needs_you");
    }

    /// Through the window's own listings: a "needs you" notification outlives
    /// the person looking at the agent, and goes once the agent unblocks or
    /// its terminal goes; with `notification_sticky: false`, seeing it is enough.
    #[gpui::test]
    fn sticky_needs_you(cx: &mut gpui::TestAppContext) {
        use crate::settings::SystemNotifications;
        use serde_json::json;
        if !running_alone("sticky_needs_you") {
            return;
        }
        cx.update(|cx| cx.set_app_identity("dev.herdr.test", "Herdr"));
        let (view, cx) = open_window(cx, "unused", "", json!({ "id": "pane", "type": "block", "content": { "terminal": "t1" } }));
        view.update(cx, |this, _| this.model.add_space("p1", "api", "/src/api", crate::model::Tab::new("c1", "one", "x1", "t1")));
        let set = |sticky: bool, cx: &mut gpui::VisualTestContext| cx.update(|window, cx| view.update(cx, |this, cx| {
            this.person.sounds.enabled = false;
            this.person.system_notifications = SystemNotifications::Always;
            this.person.notification_sticky = sticky;
            this.apply_look(window, cx);
        }));
        // The core's turn and the shared seen turn (`herdr.seen`) say whether
        // a finish was seen, also to a window that just started.
        let list_turns = |state: Option<&str>, turns: Option<(u64, u64)>, looking: bool, cx: &mut gpui::VisualTestContext| {
            let terminals = match (state, turns) {
                (Some(state), Some((turn, seen))) => json!([{ "address": "t1", "state": "live", "status": { "state": state }, "turn": turn, "labels": { "herdr.seen": seen.to_string() } }]),
                (Some(state), None) => json!([{ "address": "t1", "state": "live", "status": { "state": state } }]),
                (None, _) => json!([]),
            };
            cx.update(|window, cx| view.update(cx, |this, cx| {
                this.window_active = looking;
                this.terminals_listed(json!({ "terminals": terminals }), window, cx);
            }));
            cx.run_until_parked();
            cx.delivered_system_notifications().iter().map(|n| n.title.to_string()).collect::<Vec<_>>()
        };
        let list = |state: Option<&str>, looking: bool, cx: &mut gpui::VisualTestContext| list_turns(state, None, looking, cx);
        view.update(cx, |this, _| assert!(this.visible_addresses().contains(&"t1".to_owned()), "t1 is on screen: looking is the window being active"));
        let none: Vec<String> = Vec::new();
        set(true, cx);
        list(Some("working"), false, cx);
        assert_eq!(list(Some("blocked"), false, cx), ["An agent needs you"]);
        assert_eq!(list(Some("blocked"), true, cx), ["An agent needs you"], "seen: it stays");
        assert_eq!(list(Some("blocked"), false, cx), ["An agent needs you"]);
        assert_eq!(list(Some("working"), false, cx), none, "unblocked: withdrawn");
        assert_eq!(list(Some("blocked"), true, cx), none, "blocked while looking: nothing posted");
        list(Some("working"), false, cx);
        assert_eq!(list(Some("blocked"), false, cx), ["An agent needs you"], "blocked again");
        assert_eq!(list(None, false, cx), none, "its terminal gone: withdrawn");
        list(Some("working"), false, cx);
        list(Some("blocked"), false, cx);
        assert_eq!(list(Some("idle"), false, cx), ["An agent finished"], "unblocked unseen: one finished in its place");
        list(Some("working"), false, cx);

        // Herdr restarts while a notification is up: the core kept the agent,
        // and the new window adopts what may be up from its first listing.
        // A new window knows nothing of the terminals until the core lists them.
        let restart = |cx: &mut gpui::VisualTestContext| view.update(cx, |this, _| {
            this.notified = Default::default();
            this.listed_once = false;
            this.model.agents.clear();
            this.live.clear();
            this.written_seen.clear();
        });
        assert_eq!(list(Some("blocked"), false, cx), ["An agent needs you"]);
        restart(cx);
        assert_eq!(list(Some("blocked"), false, cx), ["An agent needs you"], "adopted, still up");
        assert_eq!(list(Some("blocked"), true, cx), ["An agent needs you"], "adopted as a needs-you: seen, it stays");
        assert_eq!(list(Some("working"), false, cx), none, "adopted, then unblocked: withdrawn");
        assert_eq!(list(Some("blocked"), false, cx), ["An agent needs you"]);
        restart(cx);
        assert_eq!(list(Some("working"), false, cx), none, "unblocked while Herdr was down: taken down on the first listing");
        assert_eq!(list(Some("error"), false, cx), ["An agent hit an error"]);
        restart(cx);
        assert_eq!(list(Some("error"), false, cx), ["An agent hit an error"], "an error's adopted");
        assert_eq!(list(Some("error"), true, cx), none, "adopted as an error: seen, it goes");
        list_turns(Some("working"), Some((1, 1)), false, cx);
        assert_eq!(list_turns(Some("idle"), Some((2, 1)), false, cx), ["An agent finished"]);
        restart(cx);
        assert_eq!(list_turns(Some("idle"), Some((2, 1)), false, cx), ["An agent finished"], "an unseen finish (turn 2, seen 1) is adopted");
        assert_eq!(list_turns(Some("idle"), Some((2, 1)), true, cx), none, "adopted as a finish: seen, it goes");
        list_turns(Some("working"), Some((2, 2)), false, cx);
        assert_eq!(list_turns(Some("idle"), Some((3, 2)), false, cx), ["An agent finished"]);
        restart(cx);
        assert_eq!(list_turns(Some("idle"), Some((3, 3)), false, cx), none, "a finish seen elsewhere meanwhile goes on the first listing");
        // A tick before the core's first listing has no terminals: it
        // mustn't count as the first listing.
        list(Some("working"), false, cx);
        assert_eq!(list(Some("blocked"), false, cx), ["An agent needs you"]);
        restart(cx);
        cx.update(|window, cx| view.update(cx, |this, cx| this.tend_terminals(false, window, cx)));
        assert_eq!(list(Some("working"), false, cx), none, "adoption waited for the real listing");

        set(false, cx);
        list(Some("working"), false, cx);
        assert_eq!(list(Some("blocked"), false, cx), ["An agent needs you"]);
        assert_eq!(list(Some("blocked"), true, cx), none, "not sticky: seen takes it down");
    }

    #[test]
    fn focus_follows_mouse_works_beside_an_open_window_but_not_through_it() {
        in_own_process("follow_beside_window");
    }

    #[gpui::test]
    fn follow_beside_window(cx: &mut gpui::TestAppContext) {
        use gpui::{point, px};
        use serde_json::json;
        if !running_alone("follow_beside_window") {
            return;
        }
        let (view, cx) = open_window(cx, "unused", "", json!({ "id": "pane", "type": "block", "content": { "terminal": "t1" } }));
        cx.update(|window, cx| view.update(cx, |this, cx| {
            this.person.focus_follows_mouse = true;
            this.open_overlay("w".into(), json!({ "kind": "window", "mod": "unused", "width": 200, "height": 200 }), window, cx);
        }));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| view.update(cx, |this, _| {
            assert!(!this.follow_gate(false, window).modal_open, "a window is not modal");
            let pane = this.bounds.borrow()["pane"];
            let slot = &this.overlays.iter().find(|o| o.id == "w").expect("the window").slot;
            let over = this.bounds.borrow().get(slot).copied().expect("the window's painted bounds");
            assert!(pane.contains(&over.center()));
            let beside = point(pane.left() + px(2.0), pane.top() + px(2.0));
            assert!(!over.contains(&beside));
            assert_eq!(this.terminal_pane_at(beside).as_deref(), Some("pane"), "the pane beside the window");
            assert_eq!(this.terminal_pane_at(over.center()), None, "the pane under the window");
        }));
        cx.update(|window, cx| view.update(cx, |this, cx| {
            this.open_overlay("d".into(), json!({ "kind": "dialog", "mod": "unused" }), window, cx);
            assert!(this.follow_gate(false, window).modal_open, "a dialog is");
        }));
    }

    #[test]
    fn showing_the_tab_again_gives_its_pane_the_keyboard() {
        in_own_process("tab_shown_again");
    }

    /// A home page in a stack beside the tab: a space made while it shows
    /// gets the keyboard once `layout.select` shows the tab again.
    #[gpui::test]
    fn tab_shown_again(cx: &mut gpui::TestAppContext) {
        use serde_json::json;
        if !running_alone("tab_shown_again") {
            return;
        }
        let (view, cx) = open_window(cx, "home", "", json!({
            "id": "center", "type": "stack", "active": "main", "children": [
                { "id": "main", "type": "block", "content": { "tab": "active" } },
                { "id": "home", "type": "block", "content": { "mod": "home" } }
            ]
        }));
        let select = |child: &str, cx: &mut gpui::VisualTestContext| {
            cx.update(|window, cx| view.update(cx, |this, cx| this.run("layout.select", json!({ "stack": "center", "child": child }), window, cx)));
            cx.run_until_parked();
        };
        select("home", cx);
        cx.update(|window, cx| view.update(cx, |this, cx| {
            this.model.add_space("sp1", "", "/tmp", crate::model::Tab::new("tb1", "", "p1", "t1"));
            this.sync(window, cx);
        }));
        cx.run_until_parked();
        let pane_focused = |cx: &mut gpui::VisualTestContext| cx.update(|window, cx| view.read(cx).pane_focus("p1", cx).unwrap().is_focused(window));
        assert!(!pane_focused(cx), "hidden, the pane can't hold the keyboard");
        select("main", cx);
        assert!(pane_focused(cx));
    }

    #[test]
    fn unnamed_panes_at_a_shell_show_an_automatic_name() {
        in_own_process("pane_auto_names");
    }

    /// `pane_auto_names` off: no automatic name anywhere. On: a bare shell's
    /// pane gets its stable name in panes() and as the rename prompt's start;
    /// a running command, a mod pane and a renamed pane don't.
    #[gpui::test]
    fn pane_auto_names(cx: &mut gpui::TestAppContext) {
        use serde_json::json;
        if !running_alone("pane_auto_names") {
            return;
        }
        let (view, cx) = open_window(cx, "unused", "", json!({
            "id": "root", "type": "block", "content": { "tab": "active" }
        }));
        let auto_name = |pane: &str| crate::mods::published_panes().as_array().unwrap().iter().find(|p| p["pane"] == pane).map(|p| p["auto_name"].clone());
        let prompt = |this: &mut super::AppWindow, pane: &str, window: &mut gpui::Window, cx: &mut gpui::Context<super::AppWindow>| {
            this.run("pane.rename", json!({ "pane": pane }), window, cx);
            let value = this.copies["overlay:rename"].inst.props["value"].clone();
            this.run("pane.rename", json!({ "pane": pane }), window, cx);
            value
        };
        cx.update(|window, cx| view.update(cx, |this, cx| {
            this.model.add_space("sp1", "", "/tmp", crate::model::Tab::new("tb1", "", "p1", "t1"));
            this.model.add_tab("sp1", crate::model::Tab::new("tb2", "", "p2", "t2"));
            this.live.insert("t1".into(), json!({ "address": "t1", "title": "~/src", "foreground": { "argv": ["-zsh"] }, "labels": {} }));
            this.live.insert("t2".into(), json!({ "address": "t2", "title": "~/src", "foreground": { "argv": ["cargo", "build"] }, "labels": {} }));
            this.sync(window, cx);
            this.publish(cx);
            assert_eq!(auto_name("p1"), Some(json!(null)), "off by default");
            assert_eq!(prompt(this, "p1", window, cx), json!(""));

            this.person.pane_auto_names = true;
            this.apply_look(window, cx);
            let expected = crate::pane_names::for_tab(["p1"])["p1"].clone();
            assert_eq!(auto_name("p1"), Some(json!(expected)), "turning it on publishes it");
            assert_eq!(auto_name("p2"), Some(json!(null)), "a running command names the pane");
            assert_eq!(prompt(this, "p1", window, cx), json!(expected), "Enter keeps the automatic name");

            // The window's own rename waits on the core's layout, which this one never has.
            assert!(this.model.rename_pane("p1", "build"));
            this.publish(cx);
            assert_eq!(auto_name("p1"), Some(json!(null)), "the person's name wins");
            assert_eq!(prompt(this, "p1", window, cx), json!("build"));
            assert!(this.model.rename_pane("p1", ""));
            this.publish(cx);
            assert_eq!(auto_name("p1"), Some(json!(expected)), "the same name again");

            this.person.pane_auto_names = false;
            this.apply_look(window, cx);
            assert_eq!(auto_name("p1"), Some(json!(null)), "turning it off takes it away");
            assert_eq!(prompt(this, "p1", window, cx), json!(""));
            assert_eq!(this.model.pane_name("p1"), None, "never written as the pane's name");
        }));
    }

    #[test]
    fn a_tabs_panes_keep_their_automatic_names_when_a_neighbour_starts_a_command() {
        in_own_process("pane_auto_names_in_a_tab");
    }

    /// Ids p0, p11709, p12850 and p14038 all hash to the same name. In tab t,
    /// the mod pane p11709 takes it, the shell p12850 the next and the shell
    /// p14038 the one after; p0 sits in another tab and leaves t alone. A
    /// command starting in p12850 hides its name but moves nobody's.
    #[gpui::test]
    fn pane_auto_names_in_a_tab(cx: &mut gpui::TestAppContext) {
        use crate::layout::{Axis, Content, Node};
        use crate::model::{Tab, pane};
        use serde_json::json;
        if !running_alone("pane_auto_names_in_a_tab") {
            return;
        }
        let (view, cx) = open_window(cx, "unused", "", json!({ "id": "main", "type": "block", "content": { "tab": "active" } }));
        let auto_name = |pane: &str| crate::mods::published_panes().as_array().unwrap().iter().find(|p| p["pane"] == pane).map(|p| p["auto_name"].clone());
        let (m, a, b) = ("p11709", "p12850", "p14038");
        let names = crate::pane_names::for_tab([m, a, b]);
        assert_ne!(names[b], crate::pane_names::for_tab([b])[b], "the ids collide");
        assert_ne!(names[b], crate::pane_names::for_tab([a, b])[b], "and the mod pane counts");
        cx.update(|window, cx| view.update(cx, |this, cx| {
            let viewer = Node::block(m, Content::Mod { name: "unused".into(), props: json!({}) });
            let mut tab = Tab::new("t", "", a, "ta");
            tab.root = Node::container("t", Axis::Row, vec![pane(a, "ta"), viewer, pane(b, "tb")]);
            this.model.add_space("s", "", "/tmp", tab);
            this.model.add_tab("s", Tab::new("u", "", "p0", "tc"));
            this.model.select_tab("t");
            for t in ["ta", "tb", "tc"] {
                this.live.insert(t.into(), json!({ "address": t, "foreground": { "argv": ["-zsh"] }, "labels": {} }));
            }
            this.sync(window, cx);
            this.person.pane_auto_names = true;
            this.apply_look(window, cx);
            assert_eq!(auto_name(m), Some(json!(null)), "a mod pane shows its mod");
            assert_eq!(auto_name(a), Some(json!(names[a])));
            assert_eq!(auto_name(b), Some(json!(names[b])));
            assert_eq!(auto_name("p0"), Some(json!(crate::pane_names::for_tab(["p0"])["p0"])), "another tab names its own");

            this.live.insert("ta".into(), json!({ "address": "ta", "foreground": { "argv": ["cargo", "build"] }, "labels": {} }));
            this.publish(cx);
            assert_eq!(auto_name(a), Some(json!(null)), "a running command names the pane");
            assert_eq!(auto_name(b), Some(json!(names[b])), "its neighbour keeps its name");
        }));
    }

    #[test]
    fn close_keys_dismiss_the_top_dialog_without_closing_the_tab() {
        in_own_process("close_keys_dismiss_dialog");
    }

    #[gpui::test]
    fn close_keys_dismiss_dialog(cx: &mut gpui::TestAppContext) {
        use serde_json::json;
        if !running_alone("close_keys_dismiss_dialog") {
            return;
        }
        let (view, cx) = open_window(cx, "unused", "", json!({
            "id": "root", "type": "block", "content": { "tab": "active" }
        }));
        cx.update(|window, cx| view.update(cx, |this, cx| {
            this.model.add_space("sp1", "", "/tmp", crate::model::Tab::new("tb1", "", "p1", "t1"));
            this.sync(window, cx);
            this.open_overlay("bottom".into(), json!({"kind": "dialog"}), window, cx);
            for kind in ["dialog", "popup"] {
                for name in ["tab.close", "pane.close"] {
                    let mut bindings = vec![None, Some(json!({}))];
                    if name == "tab.close" {
                        bindings.extend([Some(json!({"confirmed": true})), Some(json!({"id": null})), Some(json!({"id": null, "confirmed": true}))]);
                    } else {
                        bindings.push(Some(json!({"pane": null})));
                    }
                    for args in bindings {
                        this.open_overlay("top".into(), json!({"kind": kind}), window, cx);
                        if let Some(args) = args {
                            this.run_key(name, args, window, cx);
                        } else {
                            this.run_action(name, window, cx);
                        }
                        assert!(!this.overlays.iter().any(|o| o.id == "top"));
                        assert!(this.overlays.iter().any(|o| o.id == "bottom"));
                        assert_eq!(this.model.focused_pane().as_deref(), Some("p1"));
                        assert!(this.model.tab("tb1").is_some());
                    }
                }
            }
        }));
    }

    #[test]
    fn a_second_rename_press_closes_the_prompt() {
        in_own_process("rename_prompt_opens_once");
    }

    #[gpui::test]
    fn rename_prompt_opens_once(cx: &mut gpui::TestAppContext) {
        use serde_json::json;
        if !running_alone("rename_prompt_opens_once") {
            return;
        }
        let (view, cx) = open_window(cx, "unused", "", json!({
            "id": "root", "type": "block", "content": { "tab": "active" }
        }));
        cx.update(|window, cx| view.update(cx, |this, cx| {
            this.model.add_space("sp1", "", "/tmp", crate::model::Tab::new("tb1", "", "p1", "t1"));
            this.sync(window, cx);
            let prompts = |this: &super::AppWindow| this.overlays.iter().filter(|o| o.id == "rename").count();
            this.run("tab.rename", json!({}), window, cx);
            assert_eq!(prompts(this), 1);
            this.run("tab.rename", json!({}), window, cx);
            assert_eq!(prompts(this), 0, "a second press closes the prompt");
            this.run("tab.rename", json!({}), window, cx);
            assert_eq!(prompts(this), 1, "and a third opens it again");
        }));
    }

    #[test]
    fn moving_the_focus_keeps_framed_terminals_their_size() {
        in_own_process("focus_keeps_framed_sizes");
    }

    /// Tab t: a | b in a frame whose focus line is far wider than its border.
    /// Both terminals get the same grid whichever has the focus: each sits in
    /// by the wider line, the narrower one padded out.
    #[gpui::test]
    fn focus_keeps_framed_sizes(cx: &mut gpui::TestAppContext) {
        use crate::layout::{Axis, Node};
        use crate::model::{Tab, pane};
        use serde_json::json;
        if !running_alone("focus_keeps_framed_sizes") {
            return;
        }
        let frame = json!({ "lone": true, "border": { "width": 1, "color": "border" }, "focus": { "width": 30, "color": "focus" } });
        let (view, cx) = open_window(cx, "unused", "", json!({ "id": "main", "type": "block", "content": { "tab": "active", "pane": frame } }));
        let grids = |cx: &mut gpui::VisualTestContext, focus: &str| {
            view.update_in(cx, |this, window, cx| {
                this.model.focus_pane(focus);
                this.sync(window, cx);
            });
            cx.run_until_parked();
            view.update(cx, |this, cx| {
                let area = this.shown_area(this.model.tab("t").unwrap(), "a").unwrap();
                (this.terms["a"].view.read(cx).dimensions(), this.terms["b"].view.read(cx).dimensions(), area)
            })
        };
        view.update_in(cx, |this, window, cx| {
            this.documents.entry(String::new()).or_default().available = true;
            let mut tab = Tab::new("t", "", "a", "ta");
            tab.root = Node::container("t", Axis::Row, vec![pane("a", "ta"), pane("b", "tb")]);
            this.model.add_space("s", "", "/tmp", tab);
            this.model.select_tab("t");
            this.sync(window, cx);
        });
        let (a, b, area) = grids(cx, "a");
        assert_eq!(a, b, "a focused: the same grid for both");
        assert_eq!(grids(cx, "b"), (a, b, area), "b focused: nothing resized");
        view.update_in(cx, |_, window, _| assert_eq!(a, crate::terminal::grid_size(area, crate::terminal::cell_size(window)), "the grid fills the area a new terminal starts in"));
    }

    #[test]
    fn pane_refocus_hands_a_focused_mod_block_its_keyboard_back() {
        in_own_process("refocus_mod_block");
    }

    /// A tab drag takes the keyboard while a mod pane has the focus; the
    /// `pane.refocus` after the drop gives it back to that mod (its `focus`
    /// command), as closing a popup over it does.
    #[gpui::test]
    fn refocus_mod_block(cx: &mut gpui::TestAppContext) {
        use serde_json::json;
        if !running_alone("refocus_mod_block") {
            return;
        }
        let main_js = r#"import { View, div } from "gpui-kit";
import { nextCommand } from "herdr";
export default class Keeper extends View {
  init(_props, cx) {
    this.focuses = 0;
    cx.spawn(async (cx) => {
      for (;;) { if (await nextCommand() === "focus") this.focuses += 1; cx.notify(); }
    });
  }
  render() {
    return div().size_full().child(div().id("focuses").w(10 + 10 * this.focuses).h(10).flex_none());
  }
}"#;
        let (view, cx) = open_window(cx, "keeper", main_js, json!({ "id": "root", "type": "row", "children": [
            { "id": "main", "type": "block", "content": { "tab": "active" } },
            { "id": "side", "type": "block", "size": { "px": 300 }, "content": { "mod": "keeper" } },
        ] }));
        view.update_in(cx, |this, window, cx| {
            this.model.add_space("s1", "one", "", crate::model::Tab::new("a", "a", "pa", "t1"));
            this.sync(window, cx);
        });
        cx.run_until_parked();
        let focuses = |cx: &mut gpui::VisualTestContext| cx.debug_bounds("focuses").map(|b| (f32::from(b.size.width) as usize - 10) / 10);
        view.update_in(cx, |this, window, cx| this.focus_block("side", window, cx));
        cx.run_until_parked();
        assert_eq!(focuses(cx), Some(1), "the mod pane has the keyboard");
        view.update_in(cx, |this, window, cx| {
            let strip = cx.focus_handle();
            window.focus(&strip, cx);
            this.run_as("pane.refocus", json!({}), false, window, cx);
            assert!(this.focus.is_focused(window), "the window holds the keys for the mod");
            assert_eq!(this.focused_block.as_deref(), Some("side"));
        });
        cx.run_until_parked();
        assert_eq!(focuses(cx), Some(2), "pane.refocus handed the mod its keyboard back");
    }

    #[test]
    fn panes_and_tabs_show_their_automatic_or_chosen_icon() {
        in_own_process("pane_and_tab_icons");
    }

    /// `icon` on panes(), agents() and spaces()' tabs: null in letter mode
    /// (the program's icon, as before); a bare shell's automatic name's icon in
    /// glyph or emoji mode; a chosen one (pane.set_icon, tab.set_icon) always.
    /// A tab shows its focused pane's unless it has its own.
    #[gpui::test]
    fn pane_and_tab_icons(cx: &mut gpui::TestAppContext) {
        use crate::layout::{Axis, Node};
        use crate::model::{Tab, pane};
        use serde_json::{Value, json};
        if !running_alone("pane_and_tab_icons") {
            return;
        }
        let (view, cx) = open_window(cx, "unused", "", json!({ "id": "main", "type": "block", "content": { "tab": "active" } }));
        let published = |pane: &str, key: &str| crate::mods::published_panes().as_array().unwrap().iter().find(|p| p["pane"] == pane).map(|p| p[key].clone()).unwrap();
        let tab_icon = |_: &super::AppWindow, key: &str| crate::mods::published_spaces()[0]["tabs"][0][key].clone();
        cx.update(|window, cx| view.update(cx, |this, cx| {
            this.documents.entry(String::new()).or_default().available = true;
            let mut tab = Tab::new("t", "", "a", "ta");
            tab.root = Node::container("t", Axis::Row, vec![pane("a", "ta"), pane("b", "tb")]);
            this.model.add_space("s", "", "/tmp", tab);
            this.model.select_tab("t");
            this.live.insert("ta".into(), json!({ "address": "ta", "title": "~/src", "foreground": { "argv": ["-zsh"] }, "labels": {} }));
            this.live.insert("tb".into(), json!({ "address": "tb", "title": "~/src", "foreground": { "argv": ["cargo", "build"] }, "labels": {} }));
            this.sync(window, cx);
            this.person.pane_auto_names = true;
            this.person.space_avatar = Some("letter".into());
            this.apply_look(window, cx);
            let name = crate::pane_names::for_tab(["a", "b"])["a"].clone();
            assert_eq!(published("a", "auto_name"), json!(name));
            assert_eq!((published("a", "icon"), published("b", "icon")), (Value::Null, Value::Null), "letter mode: the program's icons, as before");
            assert_eq!(tab_icon(this, "icon"), Value::Null);

            for (mode, kind) in [("emoji", crate::pane_icons::Kind::Emoji), ("glyph", crate::pane_icons::Kind::Glyph), ("repo", crate::pane_icons::Kind::Glyph)] {
                this.person.space_avatar = Some(mode.into());
                this.apply_look(window, cx);
                let expected = crate::pane_icons::pane(None, Some(&name), &name, Some(kind));
                assert!(!expected.is_null());
                assert_eq!(published("a", "icon"), expected, "{mode}: the automatic name's icon, at once");
                assert_eq!(published("b", "icon"), Value::Null, "{mode}: a running command shows its program's");
                assert_eq!(tab_icon(this, "icon"), expected, "{mode}: the tab shows its focused pane's");
            }
            this.model.focus_pane("b");
            this.publish(cx);
            assert_eq!(tab_icon(this, "icon"), Value::Null, "focus moved: the tab follows its pane");

            let rocket = json!({ "kind": "glyph", "value": "\u{f135}" });
            assert_eq!(this.run_as("pane.set_icon", json!({ "pane": "b", "icon": rocket }), false, window, cx), json!({ "ok": true }));
            assert_eq!(published("b", "icon"), json!({ "kind": "glyph", "text": "\u{f135}" }), "a chosen icon wins over the program's");
            assert_eq!(published("b", "own_icon"), rocket);
            assert_eq!(published("a", "own_icon"), Value::Null);
            assert_eq!(tab_icon(this, "icon"), published("b", "icon"));
            this.person.space_avatar = Some("letter".into());
            this.apply_look(window, cx);
            assert_eq!(published("b", "icon"), json!({ "kind": "glyph", "text": "\u{f135}" }), "in letter mode too");

            assert_eq!(this.run_as("tab.set_icon", json!({ "id": "t", "icon": { "kind": "letter" } }), false, window, cx), json!({ "ok": true }));
            let cargo = published("b", "label");
            let letter = cargo.as_str().unwrap()[..1].to_uppercase();
            assert_eq!(tab_icon(this, "icon"), json!({ "kind": "letter", "text": letter }), "its own wins: an unnamed tab's letter is its focused pane's");
            assert_eq!(tab_icon(this, "own_icon"), json!({ "kind": "letter" }));
            this.run_as("tab.rename", json!({ "id": "t", "name": "deploy" }), false, window, cx);
            assert_eq!(tab_icon(this, "icon"), json!({ "kind": "letter", "text": "D" }), "a named tab's own letter");

            assert_eq!(this.run_as("pane.set_icon", json!({ "pane": "b", "icon": null }), false, window, cx), json!({ "ok": true }));
            assert_eq!((published("b", "icon"), published("b", "own_icon")), (Value::Null, Value::Null), "cleared: as before");
            this.run_as("tab.set_icon", json!({ "id": "t" }), false, window, cx);
            assert_eq!(tab_icon(this, "own_icon"), Value::Null);

            for bad in [json!({ "pane": "nope", "icon": { "kind": "letter" } }), json!({ "pane": "a", "icon": { "kind": "glyph" } }), json!({ "pane": "a", "icon": "x" })] {
                assert!(this.run_as("pane.set_icon", bad.clone(), false, window, cx).get("error").is_some(), "{bad}");
            }
            assert!(this.run_as("tab.set_icon", json!({ "id": "a", "icon": null }), false, window, cx).get("error").is_some(), "a pane is no tab");
            assert!(this.model.icons.is_empty());
        }));
    }

    #[test]
    fn pane_icons_follow_changes_not_frames() {
        in_own_process("icons_follow_changes");
    }

    /// Icons are resolved in publish, for the panes then published, never per frame.
    #[gpui::test]
    fn icons_follow_changes(cx: &mut gpui::TestAppContext) {
        use crate::layout::{Axis, Node};
        use crate::model::{Tab, pane};
        use serde_json::json;
        if !running_alone("icons_follow_changes") {
            return;
        }
        let (view, cx) = open_window(cx, "unused", "", json!({ "id": "main", "type": "block", "content": { "tab": "active" } }));
        view.update_in(cx, |this, window, cx| {
            this.documents.entry(String::new()).or_default().available = true;
            let mut tab = Tab::new("t", "", "a", "ta");
            tab.root = Node::container("t", Axis::Row, vec![pane("a", "ta"), pane("b", "tb")]);
            this.model.add_space("s", "", "/tmp", tab);
            this.model.select_tab("t");
            this.sync(window, cx);
            this.person.pane_auto_names = true;
            this.person.space_avatar = Some("emoji".into());
            this.apply_look(window, cx);
        });
        let resolved = || crate::pane_icons::RESOLVED.with(std::cell::Cell::get);
        cx.run_until_parked();
        let (before, frames) = (resolved(), crate::terminal::frame());
        for _ in 0..30 {
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
        }
        assert!(crate::terminal::frame() >= frames + 30, "30 frames drawn");
        assert_eq!(resolved(), before, "no icon is resolved for a frame");
        view.update_in(cx, |this, window, cx| {
            this.run("pane.set_icon", json!({ "pane": "a", "icon": { "kind": "emoji", "value": "🦊" } }), window, cx);
        });
        cx.run_until_parked();
        let after = resolved();
        assert!(after > before, "a choice resolves");
        assert_eq!((after - before) % 2, 0, "whole publishes: both panes each time");
        for _ in 0..30 {
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
        }
        assert_eq!(resolved(), after, "and frames after it none");
    }

    #[test]
    fn pane_menu_names_what_the_pane_shows() {
        in_own_process("pane_menu_shows");
    }

    /// `pane.menu` (a pane header's right-click) opens the menu for the
    /// terminal the pane shows, a swapped-in one too, and fails for a pane
    /// that isn't there.
    #[gpui::test]
    fn pane_menu_shows(cx: &mut gpui::TestAppContext) {
        use crate::model::Tab;
        use serde_json::json;
        if !running_alone("pane_menu_shows") {
            return;
        }
        let menu = r#"import { View, div } from "gpui-kit"; export default class Menu extends View { render() { return div().child("menu"); } }"#;
        let (view, cx) = open_window(cx, "menu", menu, json!({ "id": "main", "type": "block", "content": { "tab": "active" } }));
        let target = view.update_in(cx, |this, window, cx| {
            this.documents.entry(String::new()).or_default().available = true;
            this.model.add_space("s", "", "/tmp", Tab::new("t", "", "a", "ta"));
            this.model.select_tab("t");
            this.sync(window, cx);
            for id in this.overlays.iter().map(|o| o.id.clone()).collect::<Vec<_>>() {
                this.close_overlay(&id, window, cx);
            }
            this.live.insert("tx".into(), json!({ "address": "tx", "cwd": "/swapped" }));
            this.run_as("terminal.show", json!({ "pane": "a", "terminal": "tx" }), false, window, cx);
            this.sync(window, cx);
            let reply = this.run_as("pane.menu", json!({ "pane": "a" }), false, window, cx);
            assert!(reply.get("error").is_none(), "{reply}");
            let missing = this.run_as("pane.menu", json!({ "pane": "nope" }), false, window, cx);
            assert!(missing.get("error").is_some(), "an unknown pane fails: {missing}");
            assert_eq!(this.overlays.len(), 1, "and opens nothing");
            this.copies.values().find(|c| c.inst.name == "menu").map(|c| c.inst.props["target"].clone())
        });
        assert_eq!(target, Some(json!({ "pane": "a", "terminal": "tx", "cwd": "/swapped", "last_output": false })), "the swapped-in terminal, not the pane's own");
    }

    #[test]
    fn header_info_git_keys_count_once_turned_on() {
        in_own_process("header_info_git_keys");
    }

    /// Turning on a template with git counts reads the folders again at once
    /// (`apply_look`): `{branch}` to `{branch}{dirty}` fills `dirty` from the
    /// cached status with no other event.
    #[gpui::test]
    fn header_info_git_keys(cx: &mut gpui::TestAppContext) {
        use crate::model::Tab;
        use serde_json::json;
        if !running_alone("header_info_git_keys") {
            return;
        }
        let (view, cx) = open_window(cx, "unused", "", json!({ "id": "main", "type": "block", "content": { "tab": "active" } }));
        let place = crate::git_status::Place { on: None, folder: "/work/repo".into() };
        crate::git_status::set_for_test(&place, json!({ "root": "/work/repo", "branch": "main", "ahead": 0, "behind": 0, "staged": 0, "unstaged": 2, "untracked": 0, "conflicted": 0 }));
        let dirty = |template: &str, cx: &mut gpui::VisualTestContext| {
            view.update_in(cx, |this, window, cx| {
                this.person.pane_header_info = Some(template.into());
                this.apply_look(window, cx);
                this.model.folder_info.get("ta").map(|f| (f.branch.clone(), f.dirty))
            })
        };
        view.update_in(cx, |this, window, cx| {
            this.documents.entry(String::new()).or_default().available = true;
            this.model.add_space("s", "", "/work/repo", Tab::new("t", "", "a", "ta"));
            this.model.select_tab("t");
            this.sync(window, cx);
            this.live.insert("ta".into(), json!({ "address": "ta", "foreground": { "cwd": "/work/repo" } }));
            this.track_folders(cx);
        });
        assert_eq!(dirty("{branch}", cx), Some((Some("main".into()), false)), "no dirty key: not counted");
        assert_eq!(dirty("{branch}{dirty}", cx), Some((Some("main".into()), true)), "counted as soon as the template shows it");
        assert_eq!(dirty("{branch}", cx), Some((Some("main".into()), false)), "and dropped again");
    }

    #[test]
    fn header_info_follows_changes_and_the_minute_not_frames() {
        in_own_process("header_info_follows_changes");
    }

    /// `pane_header_info` is resolved in publish, for the agent panes then
    /// published: when the agent's state or request changes, and once a minute
    /// for `{elapsed}`, never per frame. Unset, nothing is resolved or timed.
    #[gpui::test]
    fn header_info_follows_changes(cx: &mut gpui::TestAppContext) {
        use crate::layout::{Axis, Node};
        use crate::model::{Tab, pane};
        use serde_json::{Value, json};
        if !running_alone("header_info_follows_changes") {
            return;
        }
        let (view, cx) = open_window(cx, "unused", "", json!({ "id": "main", "type": "block", "content": { "tab": "active" } }));
        view.update_in(cx, |this, window, cx| {
            this.documents.entry(String::new()).or_default().available = true;
            let mut tab = Tab::new("t", "", "a", "ta");
            tab.root = Node::container("t", Axis::Row, vec![pane("a", "ta"), pane("b", "tb")]);
            this.model.add_space("s", "", "/tmp", tab);
            this.model.select_tab("t");
            this.sync(window, cx);
        });
        let listed = |status: Value, cx: &mut gpui::VisualTestContext| {
            let terminals = json!({ "terminals": [{ "address": "ta", "argv": ["claude"], "status": status }, { "address": "tb", "argv": ["zsh"] }] });
            cx.update(|window, cx| view.update(cx, |this, cx| this.terminals_listed(terminals, window, cx)));
            cx.run_until_parked();
        };
        let template = |t: Option<&str>, cx: &mut gpui::VisualTestContext| {
            view.update_in(cx, |this, window, cx| {
                this.person.pane_header_info = t.map(str::to_owned);
                // Cards off: these counts are the header's alone (card_info_follows_changes has the cards').
                this.person.agents_card_info = Some(String::new());
                this.apply_look(window, cx);
            });
            cx.run_until_parked();
        };
        let resolved = || crate::header_info::RESOLVED.with(std::cell::Cell::get);
        let info = |pane: &str| crate::mods::published_panes().as_array().unwrap().iter().find(|p| p["pane"] == pane).unwrap()["header_info"].clone();
        let ticking = |cx: &mut gpui::VisualTestContext| view.update(cx, |this, _| this.header_tick.as_ref().map(|(at, _)| *at));
        let frames = |cx: &mut gpui::VisualTestContext| {
            let start = crate::terminal::frame();
            for _ in 0..30 {
                cx.update(|window, _| window.refresh());
                cx.run_until_parked();
            }
            assert!(crate::terminal::frame() >= start + 30, "30 frames drawn");
        };
        let minute = |cx: &mut gpui::VisualTestContext| {
            cx.executor().advance_clock(std::time::Duration::from_secs(61));
            cx.run_until_parked();
        };

        listed(json!({ "state": "working", "app": "claude-code" }), cx);
        let before = resolved();
        frames(cx);
        minute(cx);
        assert_eq!((resolved(), info("a"), ticking(cx)), (before, Value::Null, None), "unset: nothing resolved, nothing timed");

        template(Some("{ask} · {state} {elapsed}"), cx);
        assert_eq!(info("a")["segments"], json!([{ "sep": "", "text": "working <1m", "ask": false }]));
        assert_eq!(info("b"), Value::Null, "shells show nothing");
        let since = view.update(cx, |this, _| this.model.agents["ta"].since);
        assert_eq!(ticking(cx), Some(since + 60_000), "the minute mark is timed");

        let (before, drawn) = (resolved(), crate::terminal::frame());
        frames(cx);
        assert_eq!(resolved(), before, "no info is resolved for a frame");
        let drawn = crate::terminal::frame() - drawn - 30;

        listed(json!({ "state": "blocked", "kind": "permission", "msg": "approve Bash: make", "app": "claude-code" }), cx);
        assert_eq!(resolved() - before, 1, "one publish: the agent pane once");
        assert_eq!(info("a")["segments"][0], json!({ "sep": "", "text": "approve Bash: make", "ask": true }));
        assert_eq!(info("a")["text"], json!("approve Bash: make · blocked <1m"));

        let before = resolved();
        minute(cx);
        assert_eq!(resolved() - before, 1, "the minute publishes once");
        minute(cx);
        assert_eq!(resolved() - before, 2, "and again a minute on");
        eprintln!("header info: {drawn} window renders beyond 30 refreshes with the template set");

        // {subagents}: counted when a listing changes the agent's child records, not per frame.
        template(Some("{state} · {subagents}"), cx);
        let with_records = |records: Value, cx: &mut gpui::VisualTestContext| {
            let terminals = json!({ "terminals": [{ "address": "ta", "argv": ["claude"], "status": { "state": "working" }, "records": records }, { "address": "tb", "argv": ["zsh"] }] });
            cx.update(|window, cx| view.update(cx, |this, cx| this.terminals_listed(terminals, window, cx)));
            cx.run_until_parked();
        };
        with_records(json!({ "agent/a1": { "state": "working", "title": "Explore" } }), cx);
        assert_eq!(info("a")["segments"][1], json!({ "sep": " · ", "text": "1 sub-agent", "ask": false, "minor": true }));
        let before = resolved();
        frames(cx);
        assert_eq!(resolved(), before, "no sub-agents counted for a frame");
        with_records(json!({ "agent/a1": { "state": "working" }, "agent/b2": { "state": "blocked" } }), cx);
        assert_eq!(resolved() - before, 1, "a new record publishes once");
        assert_eq!(info("a")["segments"][1], json!({ "sep": " · ", "text": "2 sub-agents, 1 blocked", "ask": true }));
        with_records(Value::Null, cx);
        assert_eq!(info("a")["segments"].as_array().map(Vec::len), Some(1), "none left: the segment drops");

        template(None, cx);
        assert_eq!((info("a"), ticking(cx)), (Value::Null, None), "cleared: the timer stops");
    }

    #[test]
    fn card_info_follows_changes_and_the_minute_not_frames() {
        in_own_process("card_info_follows_changes");
    }

    /// `agents_card_info` is resolved in publish beside the header's, for each
    /// agent: by default (another machine's name, the space) on every publish
    /// that lists it, never per frame and with no timer; with `{elapsed}`, on
    /// the minute too; set empty, nothing. Header info off throughout.
    #[gpui::test]
    fn card_info_follows_changes(cx: &mut gpui::TestAppContext) {
        use crate::layout::{Axis, Node};
        use crate::model::{Tab, pane};
        use serde_json::{Value, json};
        if !running_alone("card_info_follows_changes") {
            return;
        }
        let (view, cx) = open_window(cx, "unused", "", json!({ "id": "main", "type": "block", "content": { "tab": "active" } }));
        view.update_in(cx, |this, window, cx| {
            this.documents.entry(String::new()).or_default().available = true;
            let mut tab = Tab::new("t", "", "a", "ta");
            tab.root = Node::container("t", Axis::Row, vec![pane("a", "ta"), pane("b", "tb")]);
            this.model.add_space("s", "Work", "/tmp", tab);
            this.model.select_tab("t");
            this.sync(window, cx);
        });
        let listed = |status: Value, cx: &mut gpui::VisualTestContext| {
            let terminals = json!({ "terminals": [{ "address": "ta", "argv": ["claude"], "status": status }, { "address": "tb", "argv": ["zsh"] }] });
            cx.update(|window, cx| view.update(cx, |this, cx| this.terminals_listed(terminals, window, cx)));
            cx.run_until_parked();
        };
        let template = |t: Option<&str>, cx: &mut gpui::VisualTestContext| {
            view.update_in(cx, |this, window, cx| {
                this.person.agents_card_info = t.map(str::to_owned);
                this.apply_look(window, cx);
            });
            cx.run_until_parked();
        };
        let resolved = || crate::header_info::RESOLVED.with(std::cell::Cell::get);
        let agents = || crate::mods::published_agents().as_array().cloned().unwrap_or_default();
        let card = |address: &str| agents().iter().find(|a| a["address"] == address).map(|a| a["card_info"].clone());
        let ticking = |cx: &mut gpui::VisualTestContext| view.update(cx, |this, _| this.header_tick.as_ref().map(|(at, _)| *at));
        let frames = |cx: &mut gpui::VisualTestContext| {
            for _ in 0..30 {
                cx.update(|window, _| window.refresh());
                cx.run_until_parked();
            }
        };
        let minute = |cx: &mut gpui::VisualTestContext| {
            cx.executor().advance_clock(std::time::Duration::from_secs(61));
            cx.run_until_parked();
        };

        listed(json!({ "state": "working", "app": "claude-code" }), cx);
        assert_eq!(
            card("ta"),
            Some(json!({ "text": "Work", "segments": [{ "sep": "", "text": "Work", "ask": false }], "no_space": null })),
            "unset: the default, the space here (no machine for this computer), and nothing without it"
        );
        assert_eq!(card("tb"), None, "a shell is no agent");
        let before = resolved();
        frames(cx);
        minute(cx);
        assert_eq!((resolved(), ticking(cx)), (before, None), "the default: nothing resolved for a frame, nothing timed");

        listed(json!({ "state": "blocked", "kind": "permission", "msg": "approve", "app": "claude-code" }), cx);
        assert_eq!(resolved() - before, 2, "a status change publishes once: the card, and again without its space");

        template(Some("{machine} · {state} {elapsed}"), cx);
        assert_eq!(card("ta").unwrap()["text"], json!("local · blocked <1m"));
        assert_eq!(card("ta").unwrap().get("no_space"), None, "no {{space}}: no no_space");
        let since = view.update(cx, |this, _| this.model.agents["ta"].since);
        assert_eq!(ticking(cx), Some(since + 60_000), "{{elapsed}} in the card times the minute mark");
        let before = resolved();
        frames(cx);
        assert_eq!(resolved(), before, "no card is resolved for a frame");
        minute(cx);
        assert_eq!(resolved() - before, 1, "the minute publishes once");

        // {subagents} in the card alone is counted.
        template(Some("{subagents}"), cx);
        let terminals = json!({ "terminals": [{ "address": "ta", "argv": ["claude"], "status": { "state": "working" }, "records": { "agent/a1": { "state": "working" } } }, { "address": "tb", "argv": ["zsh"] }] });
        cx.update(|window, cx| view.update(cx, |this, cx| this.terminals_listed(terminals, window, cx)));
        cx.run_until_parked();
        assert_eq!(card("ta").unwrap()["text"], json!("1 sub-agent"));

        template(Some(""), cx);
        assert_eq!((card("ta"), ticking(cx)), (Some(Value::Null), None), "set empty: no card info, the timer stops");
        let before = resolved();
        listed(json!({ "state": "working", "app": "claude-code" }), cx);
        assert_eq!(resolved(), before, "and nothing is resolved");
    }

    #[test]
    fn pane_labels_follow_changes_not_frames() {
        in_own_process("labels_follow_changes");
    }

    /// A pane's label is resolved when what it's made of changes (its title,
    /// the program running), for the panes then published, never per frame.
    #[gpui::test]
    fn labels_follow_changes(cx: &mut gpui::TestAppContext) {
        use crate::layout::{Axis, Node};
        use crate::model::{Tab, pane};
        use serde_json::json;
        if !running_alone("labels_follow_changes") {
            return;
        }
        let (view, cx) = open_window(cx, "unused", "", json!({ "id": "main", "type": "block", "content": { "tab": "active" } }));
        view.update_in(cx, |this, window, cx| {
            this.documents.entry(String::new()).or_default().available = true;
            let mut tab = Tab::new("t", "", "a", "ta");
            tab.root = Node::container("t", Axis::Row, vec![pane("a", "ta"), pane("b", "tb")]);
            this.model.add_space("s", "", "/tmp", tab);
            this.model.select_tab("t");
            this.sync(window, cx);
        });
        let listed = |program: &str, pid: Option<u64>, cx: &mut gpui::VisualTestContext| {
            let foreground = pid.map(|pid| json!({ "pid": pid, "argv": [program] }));
            let terminals = json!({ "terminals": [{ "address": "ta", "argv": [program], "foreground": foreground }, { "address": "tb", "argv": ["zsh"] }] });
            cx.update(|window, cx| view.update(cx, |this, cx| this.terminals_listed(terminals, window, cx)));
            cx.run_until_parked();
        };
        let title = |text: &str, cx: &mut gpui::VisualTestContext| {
            let term = view.update(cx, |this, _| this.terms["a"].view.clone());
            let osc = format!("\x1b]2;{text}\x07").into_bytes();
            term.update(cx, |t, cx| t.ingest(vec![crate::hc::Stream::Data(osc)], cx));
            cx.executor().advance_clock(std::time::Duration::from_millis(100));
            cx.run_until_parked();
        };
        let resolved = || crate::pane_label::RESOLVED.with(std::cell::Cell::get);
        let label = |cx: &mut gpui::VisualTestContext| view.update(cx, |this, _| this.labels["a"].clone());
        listed("claude", None, cx);
        title("\u{2733} Fix the login bug", cx);
        assert_eq!(label(cx), "Fix the login bug");

        let (before, frames) = (resolved(), crate::terminal::frame());
        for _ in 0..30 {
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
        }
        assert!(crate::terminal::frame() >= frames + 30, "30 frames drawn");
        assert_eq!(resolved(), before, "no label is resolved for a frame");

        title("claude-prompt-9acc2eb1-37f2-4d7e-9c1a-5b6f0e2d8a11.md + (/private/tmp/claude-501) - Nvim", cx);
        assert_eq!(label(cx), "Claude Code");
        let after_title = resolved();
        assert_eq!(after_title - before, 2, "one publish: both panes once");

        // Its session file names it: read off the main thread, then published.
        let home = crate::mods::app_dir();
        std::fs::create_dir_all(home.join(".claude/sessions")).unwrap();
        std::fs::write(home.join(".claude/sessions/4242.json"), r#"{"name":"vendor-insights"}"#).unwrap();
        // SAFETY: this test runs alone in its own process (`in_own_process`).
        unsafe { std::env::set_var("HOME", &home) };
        listed("claude", Some(4242), cx);
        assert_eq!(label(cx), "vendor-insights");
        let after_session = resolved();

        listed("nvim", None, cx);
        assert_eq!(label(cx), "Neovim");
        assert_eq!(resolved() - after_session, 2, "a new foreground: both panes once");
    }

    #[test]
    fn pane_ring_follows_keyboard_focus() {
        in_own_process("pane_ring_focus");
    }

    #[gpui::test]
    fn pane_ring_focus(cx: &mut gpui::TestAppContext) {
        if !running_alone("pane_ring_focus") {
            return;
        }
        let (view, cx) = open_window(cx, "unused", "", serde_json::json!({
            "id": "pane", "type": "block", "content": { "terminal": "t1" }
        }));
        cx.update(|window, cx| view.update(cx, |this, cx| {
            this.focused_pane = Some("pane".into());
            let pane = this.pane_focus("pane", cx).unwrap();
            pane.focus(window, cx);
            assert!(this.focused_path(window, cx).contains("pane"));
            let input = cx.focus_handle();
            input.focus(window, cx);
            assert!(this.focused_path(window, cx).is_empty());
            assert_eq!(this.focused_pane.as_deref(), Some("pane"));
            pane.focus(window, cx);
            assert!(this.focused_path(window, cx).contains("pane"));
        }));
        cx.deactivate_window();
        cx.update(|window, cx| view.update(cx, |this, cx| {
            assert!(this.focused_path(window, cx).contains("pane"));
        }));
    }

    #[test]
    fn find_focus_survives_tab_and_space_switches() {
        in_own_process("find_focus_switches");
    }

    #[gpui::test]
    fn find_focus_switches(cx: &mut gpui::TestAppContext) {
        use serde_json::json;
        if !running_alone("find_focus_switches") {
            return;
        }
        cx.update(gpui_base::init);
        let (view, cx) = open_window(cx, "unused", "", json!({
            "id": "main", "type": "block", "content": { "tab": "active" }
        }));
        cx.update(|window, cx| view.update(cx, |this, cx| {
            this.model.add_space("s1", "one", "", crate::model::Tab::new("a", "a", "pa", "t1"));
            this.model.add_tab("s1", crate::model::Tab::new("b", "b", "pb", "t2"));
            this.model.add_space("s2", "two", "", crate::model::Tab::new("c", "c", "pc", "t3"));
            this.model.select_tab("a");
            this.sync(window, cx);
            this.dispatch("terminal.search", &json!({}), true, window, cx);
        }));
        cx.run_until_parked();
        cx.simulate_keystrokes("n e e d l e");
        for (action, args) in [
            ("tab.select", json!({"id": "b"})),
            ("tab.select", json!({"id": "a"})),
            ("space.select", json!({"id": "s2"})),
            ("space.select", json!({"id": "s1"})),
        ] {
            cx.update(|window, cx| view.update(cx, |this, cx| {
                this.dispatch(action, &args, true, window, cx);
            }));
            cx.run_until_parked();
            if args["id"] == "a" || args["id"] == "s1" {
                cx.update(|window, cx| view.update(cx, |this, cx| {
                    let term = this.pane_term("pa").unwrap();
                    assert!(!term.focus.is_focused(window), "returning must not send a find query to the program");
                    assert!(this.pane_focus("pa", cx).unwrap().is_focused(window));
                    assert!(this.focused_path(window, cx).contains("pa"), "Find focus must keep the pane ring");
                }));
                cx.simulate_keystrokes("x");
            }
        }
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.update(|window, cx| view.update(cx, |this, _| {
            assert!(this.pane_term("pa").unwrap().focus.is_focused(window));
        }));
    }

    #[test]
    fn focus_follows_the_resting_pointer_between_terminal_panes() {
        in_own_process("follow_mouse");
    }

    /// Two terminal blocks, focus_follows_mouse on at 150 ms: the pane the
    /// pointer rests on takes the focus after the delay, and not while a button
    /// is held, an overlay is open, the window is inactive, or the keyboard
    /// moved the focus since the pointer last moved.
    #[gpui::test]
    fn follow_mouse(cx: &mut gpui::TestAppContext) {
        use gpui::{Modifiers, MouseButton, point, px};
        use serde_json::json;
        use std::time::Duration;
        if !running_alone("follow_mouse") {
            return;
        }
        let (view, cx) = open_window(cx, "unused", "", json!({ "id": "root", "type": "row", "children": [
            { "id": "left", "type": "block", "content": { "terminal": "t1" } },
            { "id": "right", "type": "block", "content": { "terminal": "t2" } }
        ] }));
        view.update_in(cx, |this, window, cx| {
            window.activate_window();
            // What's new opens on a first start, as a window overlay.
            for id in this.overlays.iter().map(|o| o.id.clone()).collect::<Vec<_>>() {
                this.close_overlay(&id, window, cx);
            }
            // Not through settings.json: the file watcher's thread would wake this executor.
            this.person.focus_follows_mouse = true;
            this.run_as("pane.focus", json!({ "terminal": "t1" }), false, window, cx);
        });
        cx.run_until_parked();
        // The focused pane, checked against the terminal holding the keyboard.
        let focused = |cx: &mut gpui::VisualTestContext| {
            cx.update(|window, cx| view.update(cx, |this, cx| {
                let pane = this.focused_pane.clone().unwrap_or_default();
                assert!(this.pane_focus(&pane, cx).is_some_and(|f| f.is_focused(window)), "{pane} doesn't have the keyboard");
                pane
            }))
        };
        let wait = |cx: &mut gpui::VisualTestContext, ms: u64| {
            cx.executor().advance_clock(Duration::from_millis(ms));
            cx.run_until_parked();
        };
        let (left, right) = view.update(cx, |this, _| {
            let b = this.bounds.borrow();
            (b["left"].center(), b["right"].center())
        });
        let nudge = |p: gpui::Point<gpui::Pixels>, d: f32| point(p.x + px(d), p.y + px(d));
        let none = Modifiers::default();
        assert_eq!(focused(cx), "left");

        // Resting on the other pane: not before the delay, then focused.
        cx.simulate_mouse_move(left, None, none);
        cx.simulate_mouse_move(right, None, none);
        wait(cx, 100);
        assert_eq!(focused(cx), "left", "focused before the delay");
        wait(cx, 60);
        assert_eq!(focused(cx), "right");

        // Leaving before the delay cancels it, back on the focused pane.
        cx.simulate_mouse_move(left, None, none);
        wait(cx, 100);
        cx.simulate_mouse_move(right, None, none);
        wait(cx, 300);
        assert_eq!(focused(cx), "right", "a cancelled wait fired");

        // A held button (a selection dragged across) never moves the focus,
        // and letting go over the other pane doesn't either until it moves.
        cx.simulate_mouse_down(right, MouseButton::Left, none);
        cx.simulate_mouse_move(left, Some(MouseButton::Left), none);
        wait(cx, 300);
        assert_eq!(focused(cx), "right", "moved the focus with a button held");
        cx.simulate_mouse_up(left, MouseButton::Left, none);
        wait(cx, 300);
        assert_eq!(focused(cx), "right", "moved the focus without a pointer move");

        // An open dialog holds it; one opening during the wait cancels it.
        // (A window overlay doesn't: `follow_beside_window`.)
        view.update_in(cx, |this, window, cx| this.open_overlay("held".into(), json!({ "kind": "dialog", "mod": "unused", "width": 10, "height": 10 }), window, cx));
        cx.simulate_mouse_move(nudge(left, 1.0), None, none);
        wait(cx, 300);
        view.update_in(cx, |this, window, cx| this.close_overlay("held", window, cx));
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| view.update(cx, |this, _| this.focused_pane.clone())).as_deref(), Some("right"), "moved the focus under a dialog");
        cx.simulate_mouse_move(nudge(left, 2.0), None, none);
        wait(cx, 100);
        view.update_in(cx, |this, window, cx| this.open_overlay("held".into(), json!({ "kind": "dialog", "mod": "unused", "width": 10, "height": 10 }), window, cx));
        wait(cx, 300);
        view.update_in(cx, |this, window, cx| this.close_overlay("held", window, cx));
        cx.run_until_parked();
        assert_eq!(focused(cx), "right", "a dialog opened during the wait didn't cancel it");

        // An inactive window holds it.
        cx.deactivate_window();
        cx.simulate_mouse_move(nudge(left, 3.0), None, none);
        wait(cx, 300);
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| view.update(cx, |this, _| this.focused_pane.clone())).as_deref(), Some("right"), "moved the focus of an inactive window");

        // The keyboard wins until the pointer moves: rest on left, then a key
        // focuses right with the pointer still there.
        cx.simulate_mouse_move(nudge(left, 4.0), None, none);
        wait(cx, 200);
        assert_eq!(focused(cx), "left");
        view.update_in(cx, |this, window, cx| this.run_as("pane.focus", json!({ "terminal": "t2" }), false, window, cx));
        wait(cx, 300);
        assert_eq!(focused(cx), "right", "the still pointer took the focus back");
        // A move within 4 px of where the key found it is still the keyboard's.
        cx.simulate_mouse_move(nudge(left, 6.0), None, none);
        wait(cx, 300);
        assert_eq!(focused(cx), "right", "a nudge after the key took the focus");
        cx.simulate_mouse_move(nudge(left, 9.0), None, none);
        wait(cx, 200);
        assert_eq!(focused(cx), "left", "the next pointer move didn't focus");

        // Only resting counts: moving on within the pane puts the wait off.
        cx.simulate_mouse_move(right, None, none);
        wait(cx, 100);
        cx.simulate_mouse_move(nudge(right, 3.0), None, none);
        wait(cx, 100);
        assert_eq!(focused(cx), "left", "the wait counted from entering the pane");
        wait(cx, 60);
        assert_eq!(focused(cx), "right");

        // At 0 ms, the first move onto a pane focuses it.
        view.update(cx, |this, _| this.person.focus_follows_mouse_delay_ms = 0);
        cx.simulate_mouse_move(left, None, none);
        cx.run_until_parked();
        assert_eq!(focused(cx), "left", "0 ms waited");
    }

    #[test]
    fn focus_follows_the_pointer_only_into_panes_still_shown() {
        in_own_process("follow_mouse_shown");
    }

    /// A tab's panes a | b | c and a second tab: the pointer rests on a, the
    /// keyboard zooms b (or shows the other tab), the pointer moves to where c
    /// is, the keyboard brings them back. Resting there focuses c; a, hidden
    /// meanwhile with the pointer's last word on it, never.
    #[gpui::test]
    fn follow_mouse_shown(cx: &mut gpui::TestAppContext) {
        use crate::layout::{Axis, Node};
        use crate::model::{Tab, pane};
        use gpui::{Modifiers, point, px};
        use serde_json::json;
        use std::time::Duration;
        if !running_alone("follow_mouse_shown") {
            return;
        }
        let (view, cx) = open_window(cx, "unused", "", json!({ "id": "main", "type": "block", "content": { "tab": "active" } }));
        view.update_in(cx, |this, window, cx| {
            window.activate_window();
            for id in this.overlays.iter().map(|o| o.id.clone()).collect::<Vec<_>>() {
                this.close_overlay(&id, window, cx);
            }
            this.person.focus_follows_mouse = true;
            let mut tab = Tab::new("t", "", "a", "ta");
            tab.root = Node::container("t", Axis::Row, vec![pane("a", "ta"), pane("b", "tb"), pane("c", "tc")]);
            this.model.add_space("s", "", "/tmp", tab);
            this.model.add_tab("s", Tab::new("u", "", "d", "td"));
            this.model.select_tab("t");
            this.run_as("pane.focus", json!({ "pane": "a" }), false, window, cx);
        });
        cx.run_until_parked();
        let focused = |cx: &mut gpui::VisualTestContext| cx.update(|_, cx| view.update(cx, |this, _| this.focused_pane.clone().unwrap_or_default()));
        let wait = |cx: &mut gpui::VisualTestContext, ms: u64| {
            cx.executor().advance_clock(Duration::from_millis(ms));
            cx.run_until_parked();
        };
        let keys = |cx: &mut gpui::VisualTestContext, command: &str, args: serde_json::Value| {
            view.update_in(cx, |this, window, cx| this.run_as(command, args, false, window, cx));
            cx.run_until_parked();
        };
        // Watches every step from here: a is never focused again.
        let step = |cx: &mut gpui::VisualTestContext, what: &str| {
            for _ in 0..4 {
                wait(cx, 50);
                assert_ne!(focused(cx), "a", "{what}: the hidden pane took the focus");
            }
        };
        let (a, c) = view.update(cx, |this, _| {
            let b = this.bounds.borrow();
            (b["a"].center(), b["c"].center())
        });
        let nudge = |p: gpui::Point<gpui::Pixels>, d: f32| point(p.x + px(d), p.y + px(d));
        let none = Modifiers::default();
        assert_eq!(focused(cx), "a");

        // Zoomed away.
        cx.simulate_mouse_move(a, None, none);
        cx.simulate_mouse_move(nudge(a, 2.0), None, none);
        wait(cx, 200);
        keys(cx, "pane.focus", json!({ "pane": "b" }));
        keys(cx, "pane.zoom", json!({}));
        cx.simulate_mouse_move(c, None, none);
        step(cx, "over b zoomed");
        keys(cx, "pane.zoom", json!({}));
        cx.simulate_mouse_move(nudge(c, 6.0), None, none);
        step(cx, "back from the zoom");
        assert_eq!(focused(cx), "c");

        // Another tab shown.
        keys(cx, "pane.focus", json!({ "pane": "a" }));
        cx.simulate_mouse_move(nudge(a, 6.0), None, none);
        cx.simulate_mouse_move(nudge(a, 9.0), None, none);
        wait(cx, 200);
        keys(cx, "pane.focus", json!({ "pane": "b" }));
        keys(cx, "tab.next", json!({}));
        assert_eq!(focused(cx), "d");
        cx.simulate_mouse_move(c, None, none);
        step(cx, "over the other tab");
        keys(cx, "tab.prev", json!({}));
        assert_eq!(focused(cx), "b");
        cx.simulate_mouse_move(nudge(c, 6.0), None, none);
        step(cx, "back from the other tab");
        assert_eq!(focused(cx), "c");
    }

    #[test]
    fn a_hidden_terminal_frees_its_drawing_and_draws_the_same_when_shown() {
        in_own_process("hidden_caches");
    }

    /// A terminal with colors, a selection and scrolled back, then its tab
    /// hidden: a quick flip back keeps its drawing caches; hidden a few
    /// seconds, it frees them; shown again, it draws exactly what it drew.
    #[gpui::test]
    fn hidden_caches(cx: &mut gpui::TestAppContext) {
        use crate::layout::{Axis, Node};
        use crate::model::{Tab, pane};
        use gpui::{Modifiers, MouseButton, point, px};
        use serde_json::json;
        use std::time::Duration;
        if !running_alone("hidden_caches") {
            return;
        }
        let (view, cx) = open_window(cx, "unused", "", json!({ "id": "main", "type": "block", "content": { "tab": "active" } }));
        view.update_in(cx, |this, window, cx| {
            for id in this.overlays.iter().map(|o| o.id.clone()).collect::<Vec<_>>() {
                this.close_overlay(&id, window, cx);
            }
            let mut tab = Tab::new("t", "", "a", "ta");
            tab.root = Node::container("t", Axis::Row, vec![pane("a", "ta")]);
            this.model.add_space("s", "", "/tmp", tab);
            this.model.add_tab("s", Tab::new("u", "", "d", "td"));
            this.model.select_tab("t");
            this.run_as("pane.focus", json!({ "pane": "a" }), false, window, cx);
        });
        cx.run_until_parked();
        let wait = |cx: &mut gpui::VisualTestContext, ms: u64| {
            cx.executor().advance_clock(Duration::from_millis(ms));
            cx.run_until_parked();
        };
        let keys = |cx: &mut gpui::VisualTestContext, command: &str| {
            view.update_in(cx, |this, window, cx| this.run_as(command, json!({}), false, window, cx));
            cx.run_until_parked();
        };
        let term = view.update(cx, |this, _| this.terms["a"].view.clone());
        let mut out = String::new();
        for i in 0..80 {
            out.push_str(&format!("\x1b[3{}mline {i} \x1b[1;4mbold\x1b[0m \x1b[44m blue \x1b[0m ─┼─ fi=>\r\n", i % 8));
        }
        out.push_str("\x1b[7mprompt\x1b[0m $ ");
        term.update(cx, |t, cx| t.ingest(vec![crate::hc::Stream::Data(out.into_bytes())], cx));
        wait(cx, 100);
        let origin = view.update(cx, |this, _| this.bounds.borrow()["a"].origin);
        let none = Modifiers::default();
        let (from, to) = (point(origin.x + px(12.), origin.y + px(30.)), point(origin.x + px(90.), origin.y + px(50.)));
        cx.simulate_mouse_down(from, MouseButton::Left, none);
        cx.simulate_mouse_move(to, Some(MouseButton::Left), none);
        cx.simulate_mouse_up(to, MouseButton::Left, none);
        cx.simulate_event(gpui::ScrollWheelEvent { position: to, delta: gpui::ScrollDelta::Lines(point(0., 3.)), ..Default::default() });
        wait(cx, 100);
        let drawing = |cx: &mut gpui::VisualTestContext| term.update(cx, |t, _| t.drawing());
        let (shown, released) = drawing(cx);
        assert!(!released && shown.contains("line ") && !shown.contains("selection None") && !shown.contains("prompt"), "not drawn colored, selected and scrolled back: {shown}");

        keys(cx, "tab.next");
        wait(cx, 1000);
        keys(cx, "tab.prev");
        wait(cx, 100);
        assert_eq!(drawing(cx), (shown.clone(), false), "a quick flip changed or released the drawing");

        keys(cx, "tab.next");
        wait(cx, 1000);
        assert!(!drawing(cx).1, "released within a second of hiding");
        wait(cx, 4000);
        assert!(drawing(cx).1, "hidden 5 s, the drawing caches are still held");

        keys(cx, "tab.prev");
        wait(cx, 100);
        assert_eq!(drawing(cx), (shown.clone(), false), "drawn again differently after the release");

        // Shown again, it watches again: hidden once more, it frees them once more.
        keys(cx, "tab.next");
        wait(cx, 5000);
        assert!(drawing(cx).1, "a second hiding kept the drawing caches");
        keys(cx, "tab.prev");
        wait(cx, 100);
        assert_eq!(drawing(cx), (shown, false));
    }

    /// Mods see window().active follow the focus, losing it too: a probe mod
    /// notes every value it renders with while the window is focused and
    /// blurred, and the poll that checks the focus runs in between.
    #[test]
    fn mods_see_the_window_lose_and_regain_focus() {
        in_own_process("focus_window");
    }

    #[gpui::test]
    fn focus_window(cx: &mut gpui::TestAppContext) {
        use serde_json::json;
        if !running_alone("focus_window") {
            return;
        }
        let probe = r#"import { View, div } from "gpui-kit";
import { window, shared, setShared } from "herdr";

export default class Probe extends View {
  render() {
    const active = window().active;
    if (active !== this.last) {
      this.last = active;
      setShared("probe.seen", [...(shared("probe.seen") ?? []), active]);
    }
    return div();
  }
}
"#;
        let layout = json!({ "id": "root", "type": "row", "children": [
            { "id": "probe", "type": "block", "content": { "mod": "active-probe" } }
        ] });
        let (_view, cx) = open_window(cx, "active-probe", probe, layout);
        // What the probe saw last, after the focus poll (every 400 ms) had its turn.
        let seen = |cx: &mut gpui::VisualTestContext| {
            cx.executor().advance_clock(std::time::Duration::from_secs(1));
            cx.run_until_parked();
            crate::mods::shared("probe.seen").and_then(|v| v.as_array()?.last().cloned())
        };
        for _ in 0..2 {
            cx.update(|window, _| window.activate_window());
            assert_eq!(seen(cx), Some(json!(true)), "focused: {:?}", crate::mods::shared("probe.seen"));
            cx.deactivate_window();
            assert_eq!(seen(cx), Some(json!(false)), "blurred: {:?}", crate::mods::shared("probe.seen"));
        }
    }

    #[test]
    fn terminal_text_dims_while_the_window_is_inactive() {
        in_own_process("unfocused_dim");
    }

    /// With `unfocused_dim` set over terminal text, blurring the window draws
    /// the terminal's text again, faded toward its background; focused again, it draws exactly
    /// what it drew before. Unset, blurring changes nothing.
    #[gpui::test]
    fn unfocused_dim(cx: &mut gpui::TestAppContext) {
        use crate::layout::{Axis, Node};
        use crate::model::{Tab, pane};
        use serde_json::json;
        use std::time::Duration;
        if !running_alone("unfocused_dim") {
            return;
        }
        let (view, cx) = open_window(cx, "unused", "", json!({ "id": "main", "type": "block", "content": { "tab": "active" } }));
        view.update_in(cx, |this, window, cx| {
            for id in this.overlays.iter().map(|o| o.id.clone()).collect::<Vec<_>>() {
                this.close_overlay(&id, window, cx);
            }
            let mut tab = Tab::new("t", "", "a", "ta");
            tab.root = Node::container("t", Axis::Row, vec![pane("a", "ta")]);
            this.model.add_space("s", "", "/tmp", tab);
            this.model.select_tab("t");
            this.run_as("pane.focus", json!({ "pane": "a" }), false, window, cx);
        });
        cx.run_until_parked();
        let term = view.update(cx, |this, _| this.terms["a"].view.clone());
        term.update(cx, |t, cx| t.ingest(vec![crate::hc::Stream::Data(b"\x1b[31mred \x1b[1mbold\x1b[0m plain \x1b[44m blue \x1b[0m".to_vec())], cx));
        let settle = |cx: &mut gpui::VisualTestContext| {
            cx.executor().advance_clock(Duration::from_millis(500));
            cx.run_until_parked();
        };
        let drawing = |cx: &mut gpui::VisualTestContext| term.update(cx, |t, _| t.drawing().0);
        let set_dim = |dim: f32, cx: &mut gpui::VisualTestContext| {
            view.update_in(cx, |this, window, cx| {
                this.person.unfocused_dim = dim;
                this.person.unfocused_dim_scope = crate::settings::DimScope::Text;
                this.apply_look(window, cx);
            });
        };
        cx.update(|window, _| window.activate_window());
        settle(cx);
        let bright = drawing(cx);
        assert!(bright.contains("red"), "not drawn: {bright}");

        cx.deactivate_window();
        settle(cx);
        assert_eq!(drawing(cx), bright, "unset, blurring changed the drawing");

        set_dim(0.5, cx);
        settle(cx);
        let dimmed = drawing(cx);
        assert_ne!(dimmed, bright, "blurred with unfocused_dim, the text was not dimmed");
        let bg = |d: &str| d.lines().flat_map(|l| l.split(" | ")).filter(|op| op.contains("bg ")).map(|op| op[op.find("bg ").unwrap()..].to_owned()).collect::<Vec<_>>();
        assert_eq!(bg(&dimmed), bg(&bright), "backgrounds are not dimmed");

        cx.update(|window, _| window.activate_window());
        settle(cx);
        assert_eq!(drawing(cx), bright, "focused again, the text is not at full brightness");
    }

    #[test]
    fn the_whole_window_dims_while_inactive() {
        in_own_process("unfocused_dim_window");
    }

    #[test]
    fn the_sidebar_shade_follows_focus() {
        in_own_process("sidebar_shade_focus");
    }

    /// `sidebar_shade` swaps `chrome` as the window gains and loses focus,
    /// with no poll in between: the theme everything reads changes at once.
    #[gpui::test]
    fn sidebar_shade_focus(cx: &mut gpui::TestAppContext) {
        use crate::settings::SidebarShade;
        use serde_json::json;
        if !running_alone("sidebar_shade_focus") {
            return;
        }
        let (view, cx) = open_window(cx, "unused", "", json!({ "id": "main", "type": "block", "content": { "tab": "active" } }));
        let chrome = |cx: &mut gpui::VisualTestContext| {
            cx.run_until_parked();
            crate::settings::theme()["chrome"].clone()
        };
        view.update_in(cx, |this, window, cx| {
            this.person.appearance = crate::settings::Appearance::Dark;
            this.person.sidebar_shade = SidebarShade::Active;
            this.apply_look(window, cx);
        });
        cx.update(|window, _| window.activate_window());
        let shaded = chrome(cx);
        cx.deactivate_window();
        let flat = chrome(cx);
        assert_ne!(shaded, flat, "active, the dark default theme's sidebar is a shade darker");
        assert_eq!(crate::settings::theme()["canvas"], shaded, "the shade sits level with canvas");
        cx.update(|window, _| window.activate_window());
        assert_eq!(chrome(cx), shaded, "focused again, shaded again");
        for (mode, active, inactive) in [(SidebarShade::Flat, &flat, &flat), (SidebarShade::Always, &shaded, &shaded)] {
            view.update_in(cx, |this, window, cx| {
                this.person.sidebar_shade = mode;
                this.apply_look(window, cx);
            });
            assert_eq!(&chrome(cx), active, "{mode:?} while active");
            let before = (crate::mods::revision("theme"), crate::settings::reshades());
            cx.deactivate_window();
            assert_eq!(&chrome(cx), inactive, "{mode:?} while inactive");
            cx.update(|window, _| window.activate_window());
            chrome(cx);
            assert_eq!(crate::mods::revision("theme"), before.0, "{mode:?}: focus changes bump nothing");
            assert_eq!(crate::settings::reshades(), before.1, "{mode:?}: focus changes resolve nothing");
        }
        view.update_in(cx, |this, window, cx| {
            this.person.sidebar_shade = SidebarShade::Active;
            this.person.colors.insert("chrome".into(), "#202020".into());
            this.apply_look(window, cx);
        });
        assert_eq!(chrome(cx), "#202020", "an own chrome wins while active");
        cx.deactivate_window();
        assert_eq!(chrome(cx), "#202020", "an own chrome wins while inactive");

        // A theme file on disk: focus changes resolve it from the palette last
        // read. With the home moved to an empty folder, any read of the file
        // would fail and fall back to Graphite. Writing the file below can't
        // reload the look behind the test's back: the watcher's 80 ms batching
        // timer runs on gpui's fake clock, which this test never advances.
        let file = crate::mods::app_dir().join("themes/focus-test.json");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, r##"{"background":"#1d2330","foreground":"#c8d0e0","selection":"#3a4660","ansi":["#000000","#e06c75","#98c379","#e5c07b","#61afef","#c678dd","#56b6c2","#abb2bf","#5c6370","#e06c75","#98c379","#e5c07b","#61afef","#c678dd","#56b6c2","#ffffff"]}"##).unwrap();
        view.update_in(cx, |this, window, cx| {
            this.person.colors.remove("chrome");
            this.person.theme = Some(crate::layout::ThemeChoice::One("focus-test".into()));
            this.apply_look(window, cx);
        });
        let flat = chrome(cx);
        assert_eq!(crate::settings::theme()["background"], "#1d2330", "the file's theme is showing");
        let home = crate::mods::app_dir();
        let empty = std::env::temp_dir().join(format!("herdr-focus-empty-{}", std::process::id()));
        std::fs::create_dir_all(&empty).unwrap();
        // SAFETY: this test runs alone in its own process (`in_own_process`).
        unsafe { std::env::set_var("HERDR_APP_HOME", &empty) };
        assert!(!crate::settings::theme_exists("focus-test"), "the theme file is out of reach");
        cx.update(|window, _| window.activate_window());
        let shaded = chrome(cx);
        assert_ne!(shaded, flat, "focus shades the file's theme");
        assert_eq!(crate::settings::theme()["background"], "#1d2330", "a focus change reads no theme file");
        cx.deactivate_window();
        assert_eq!(chrome(cx), flat);
        assert_eq!(crate::settings::theme()["background"], "#1d2330");
        unsafe { std::env::set_var("HERDR_APP_HOME", &home) };
        std::fs::remove_dir_all(&empty).unwrap();
    }

    /// With `unfocused_dim` over the window (the default scope), blurring lays
    /// one quad of the background at that alpha over the whole window, above
    /// every other primitive (a toast's included), and leaves the terminal's
    /// text as it was. Focused again, the quad is gone.
    #[gpui::test]
    fn unfocused_dim_window(cx: &mut gpui::TestAppContext) {
        use crate::layout::{Axis, Node};
        use crate::model::{Tab, pane};
        use serde_json::json;
        use std::time::Duration;
        if !running_alone("unfocused_dim_window") {
            return;
        }
        let (view, cx) = open_window(cx, "unused", "", json!({ "id": "main", "type": "block", "content": { "tab": "active" } }));
        view.update_in(cx, |this, window, cx| {
            for id in this.overlays.iter().map(|o| o.id.clone()).collect::<Vec<_>>() {
                this.close_overlay(&id, window, cx);
            }
            let mut tab = Tab::new("t", "", "a", "ta");
            tab.root = Node::container("t", Axis::Row, vec![pane("a", "ta")]);
            this.model.add_space("s", "", "/tmp", tab);
            this.model.select_tab("t");
            this.run_as("pane.focus", json!({ "pane": "a" }), false, window, cx);
            this.open_overlay("toast".into(), json!({ "kind": "toast", "notification": { "title": "On top?", "body": "", "level": "info" } }), window, cx);
            this.person.unfocused_dim = 0.4;
            this.apply_look(window, cx);
        });
        cx.run_until_parked();
        let term = view.update(cx, |this, _| this.terms["a"].view.clone());
        term.update(cx, |t, cx| t.ingest(vec![crate::hc::Stream::Data(b"\x1b[31mred\x1b[0m plain".to_vec())], cx));
        let settle = |cx: &mut gpui::VisualTestContext| {
            cx.executor().advance_clock(Duration::from_millis(500));
            cx.run_until_parked();
        };
        // The quads of this veil, and whether each is above every other primitive.
        let veils = |veil: super::Hsla, cx: &mut gpui::VisualTestContext| {
            cx.update(|window, _| {
                let viewport = window.viewport_size().scale(window.scale_factor());
                let orders = window.painted_orders();
                window.painted_quads().into_iter()
                    .filter(|q| q.background == gpui::Background::from(veil))
                    .map(|q| {
                        assert_eq!(q.bounds.origin, gpui::Point::default(), "the veil starts at the window's corner");
                        assert_eq!(q.bounds.size, viewport, "the veil covers the window");
                        orders.iter().filter(|o| **o >= q.order).count() == 1
                    })
                    .collect::<Vec<_>>()
            })
        };
        cx.update(|window, _| window.activate_window());
        settle(cx);
        assert!(view.update(cx, |this, _| this.overlays.iter().any(|o| o.id == "toast")), "the toast is open");
        let bright = term.update(cx, |t, _| t.drawing().0);
        assert!(bright.contains("red"), "not drawn: {bright}");
        let background = super::Hsla { a: 0.4, ..super::c("background") };
        let grey = super::Hsla { a: 0.4, ..crate::settings::parse_hex("#4d4d4d").unwrap() };
        assert_eq!(veils(background, cx), Vec::<bool>::new(), "active, nothing is veiled");

        cx.deactivate_window();
        settle(cx);
        assert_eq!(veils(background, cx), vec![true], "inactive, one veil of the background above everything");
        assert_eq!(term.update(cx, |t, _| t.drawing().0), bright, "the text isn't dimmed a second time");

        view.update_in(cx, |this, window, cx| {
            this.person.unfocused_dim_color = Some("#4d4d4d".into());
            this.apply_look(window, cx);
        });
        settle(cx);
        assert_eq!((veils(background, cx), veils(grey, cx)), (vec![], vec![true]), "the veil takes unfocused_dim_color");
        assert_eq!(term.update(cx, |t, _| t.drawing().0), bright);

        cx.update(|window, _| window.activate_window());
        settle(cx);
        assert_eq!(veils(grey, cx), Vec::<bool>::new(), "focused again, the veil is gone");
        assert_eq!(term.update(cx, |t, _| t.drawing().0), bright);
    }

    /// The veil is painted after the deferred draws, whatever their priority.
    #[gpui::test]
    fn the_veil_covers_deferred_draws(cx: &mut gpui::TestAppContext) {
        struct Layers;
        impl Render for Layers {
            fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                window.set_veil(Some(gpui::hsla(0.0, 0.0, 0.0, 0.25)));
                div().size_full().bg(gpui::red()).child(gpui::deferred(div().size_full().bg(gpui::blue())).with_priority(usize::MAX))
            }
        }
        let window = cx.add_window(|_, _| Layers);
        cx.run_until_parked();
        window.update(cx, |_, window, _| {
            let quads = window.painted_quads();
            let top = quads.iter().max_by_key(|q| q.order).unwrap();
            assert_eq!(top.background, gpui::Background::from(gpui::hsla(0.0, 0.0, 0.0, 0.25)));
            assert_eq!(window.painted_orders().iter().filter(|o| **o >= top.order).count(), 1);
            let blue = quads.iter().find(|q| q.background == gpui::Background::from(gpui::blue())).expect("the deferred draw is painted");
            assert!(blue.order < top.order);
        }).unwrap();
    }

    /// `terminal.popup` starts the program as `hc run --bg --no-parent` would
    /// (`--rm` unless it waits), peeks at it at once, and a waiter gets its
    /// exit code when it finishes, after which the terminal is removed.
    #[test]
    fn a_popup_runs_its_program_and_peeks_at_it() {
        in_own_process("popup_peeks");
    }

    #[gpui::test]
    fn popup_peeks(cx: &mut gpui::TestAppContext) {
        use serde_json::json;
        use std::collections::HashMap;
        if !running_alone("popup_peeks") {
            return;
        }
        let next = std::sync::atomic::AtomicU32::new(8);
        let (core, calls) = crate::hc::answering(move |method, params| match method {
                        "terminal.run" => {
                let n = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Ok(json!({ "address": format!("t{n}"), "created_ms": n }))
            }
            // The core turns one keep down: it comes back as a popup.
            "terminal.label" if params["terminal"] == "t28" || params["terminal"] == "t32" => {
                Err(json!({ "message": "the core said no" }))
            }
            "terminal.rm" | "terminal.kill" | "terminal.label" | "terminal.keep" => Ok(json!({})),
            _ => Err(json!({ "message": "not in this test" })),
        });
        let (view, cx) = open_window_on(core, cx, "unused", "", json!({ "id": "root", "type": "block", "content": { "mod": "unused" } }));
        let take = |method: &str| {
            let mut calls = calls.lock().unwrap();
            let (taken, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut *calls).into_iter().partition(|(m, _)| m == method);
            *calls = kept;
            // Background work runs in any order.
            let mut taken = taken.into_iter().map(|(_, p)| p["terminal"].as_str().map_or(p.clone(), |t| json!(t))).collect::<Vec<_>>();
            taken.sort_by_key(|p| p.to_string());
            taken
        };
        let peeked = |cx: &mut gpui::VisualTestContext| view.update(cx, |this, _| {
            let peek = this.overlays.iter().find(|o| o.id == "terminal-peek").expect("the peek overlay");
            (serde_json::to_value(peek.layout.as_ref().expect("its layout")).unwrap(), peek.size.width)
        });
        let peek_open = |cx: &mut gpui::VisualTestContext| view.update(cx, |this, _| this.overlays.iter().any(|o| o.id == "terminal-peek"));
        let popup = |args: serde_json::Value, cx: &mut gpui::VisualTestContext| cx.update(|window, cx| view.update(cx, |this, cx| this.run_as("terminal.popup", args, false, window, cx)));
        let now = std::time::Instant::now();

        assert_eq!(popup(json!({ "argv": ["htop"], "cwd": "/tmp" }), cx), json!({ "terminal": "t8" }));
        let run = take("terminal.run").pop().unwrap();
        assert_eq!((&run["argv"], &run["cwd"], &run["no_parent"], &run["rm"]), (&json!(["htop"]), &json!("/tmp"), &json!(true), &json!(true)));
        assert_eq!(run["labels"], json!({ super::POPUP_LABEL: std::process::id().to_string() }), "labelled with the app that owns it");
        let (layout, width) = peeked(cx);
        let blocks = layout.to_string();
        assert!(blocks.contains(r#"{"terminal":"t8"}"#), "{blocks}");
        assert_eq!(width, 0.8);
        assert!(view.update(cx, |this, _| this.waits.is_empty()), "nothing waits without wait");

        // Waiting: kept for the exit code, held output, the size asked for.
        // It replaces the popup still open, whose program ends.
        assert_eq!(popup(json!({ "argv": ["sh", "-c", "exit 7"], "wait": true, "hold": true, "width": 600 }), cx), json!({ "terminal": "t9" }));
        let run = take("terminal.run").pop().unwrap();
        assert_eq!(run["rm"], json!(false));
        let argv = run["argv"].as_array().unwrap();
        assert_eq!(argv[..2], [json!("sh"), json!("-c")]);
        assert!(!argv[2].as_str().unwrap().contains("read -"), "dash has no read -n: {}", argv[2]);
        assert_eq!(argv[3..], [json!("popup"), json!("sh"), json!("-c"), json!("exit 7")]);
        assert_eq!(run["cwd"], json!(super::home()));
        let (layout, width) = peeked(cx);
        assert!(layout.to_string().contains(r#"{"terminal":"t9"}"#), "the new popup replaces the last");
        assert_eq!(width, 600.0);
        cx.run_until_parked();
        assert_eq!((take("terminal.kill"), take("terminal.rm")), (vec![json!("t8")], vec![json!("t8")]), "the replaced popup's program ends");

        let (tx, rx) = std::sync::mpsc::channel();
        view.update(cx, |this, cx| {
            this.wait_for("t9", tx, cx);
            // Live, then finished with its code.
            this.live.insert("t9".into(), json!({}));
            this.settle_waits(&HashMap::new(), now, cx);
            this.live.clear();
            this.settle_waits(&HashMap::from([("t9".to_owned(), json!({ "exit_code": 7, "signal": null }))]), now, cx);
        });
        assert_eq!(rx.try_recv().unwrap(), json!({ "terminal": "t9", "exit_code": 7, "signal": null }));
        cx.run_until_parked();
        assert_eq!(take("terminal.rm"), [json!("t9")]);
        assert!(view.update(cx, |this, _| this.waits.is_empty()));

        // A peek of a terminal someone else started never ends it; a popup
        // ends when it closes, not only when the next one opens.
        let hide = |cx: &mut gpui::VisualTestContext| cx.update(|window, cx| view.update(cx, |this, cx| this.close_overlay("terminal-peek", window, cx)));
        let peek_t5 = |cx: &mut gpui::VisualTestContext| cx.update(|window, cx| view.update(cx, |this, cx| this.peek_terminal("t5", window, cx)));
        view.update(cx, |this, _| this.live.insert("t5".into(), json!({})));
        peek_t5(cx);
        popup(json!({ "argv": ["true"] }), cx);
        hide(cx);
        cx.run_until_parked();
        assert_eq!(
            (take("terminal.kill"), take("terminal.rm")),
            (vec![json!("t10")], vec![json!("t10")]),
            "t9 had ended, t5 is a peek, closed t10 ends"
        );
        popup(json!({ "argv": ["true"] }), cx);
        cx.run_until_parked();
        assert!(
            take("terminal.kill").is_empty(),
            "nothing left for the next popup to end"
        );

        // A plain peek after closing one has nothing more to end.
        hide(cx);
        peek_t5(cx);
        popup(json!({ "argv": ["true"], "wait": true }), cx);
        cx.run_until_parked();
        assert_eq!(
            (take("terminal.kill"), take("terminal.rm")),
            (vec![json!("t11")], vec![json!("t11")]),
            "closed t11 ends, the peeked t5 does not"
        );

        // A wait popup closed: its program ends, its terminal stays for the waiter.
        hide(cx);
        popup(json!({ "argv": ["sleep", "9"], "wait": true }), cx);
        cx.run_until_parked();
        assert_eq!((take("terminal.kill"), take("terminal.rm")), (vec![json!("t12")], vec![]), "t12 is kept for its waiter");
        take("terminal.run");

        // One that ends before anyone asks keeps its code (and terminal) for
        // the waiter; one removed without finishing answers null; unknown
        // ones are an error.
        view.update(cx, |this, cx| {
            this.live.insert("t13".into(), json!({}));
            this.settle_waits(&HashMap::from([("t12".to_owned(), json!({ "exit_code": 0, "signal": null }))]), now, cx);
            this.live.clear();
            this.settle_waits(&HashMap::new(), now, cx);
        });
        cx.run_until_parked();
        assert!(take("terminal.rm").is_empty(), "kept for their waiters");
        let answer = |address: &str, cx: &mut gpui::VisualTestContext| {
            let (tx, rx) = std::sync::mpsc::channel();
            view.update(cx, |this, cx| this.wait_for(address, tx, cx));
            rx.try_recv().unwrap()
        };
        assert_eq!(answer("t12", cx)["exit_code"], json!(0));
        cx.run_until_parked();
        assert_eq!(take("terminal.rm"), [json!("t12")], "removed once taken");
        assert!(answer("t12", cx)["error"].is_string(), "answered once");

        // t13 (no end) nobody claims: kept a minute, then removed.
        view.update(cx, |this, cx| this.expire_waits(now + super::WAIT_KEPT / 2, cx));
        cx.run_until_parked();
        assert!(take("terminal.rm").is_empty());
        view.update(cx, |this, cx| this.expire_waits(now + super::WAIT_KEPT, cx));
        cx.run_until_parked();
        assert_eq!(take("terminal.rm"), [json!("t13")]);
        assert!(view.update(cx, |this, _| this.waits.is_empty()));

        // A waiter that hung up takes nothing: the answer stays for another.
        popup(json!({ "argv": ["true"], "wait": true }), cx);
        let (tx, rx) = std::sync::mpsc::channel();
        view.update(cx, |this, cx| {
            this.wait_for("t14", tx, cx);
            drop(rx);
            this.live.insert("t14".into(), json!({}));
            this.settle_waits(&HashMap::new(), now, cx);
            this.live.clear();
            this.settle_waits(&HashMap::from([("t14".to_owned(), json!({ "exit_code": null, "signal": 15 }))]), now, cx);
        });
        assert_eq!(answer("t14", cx), json!({ "terminal": "t14", "exit_code": null, "signal": 15 }));

        // The CLI gave up before the window answered: nothing is left behind.
        let (tx, rx) = std::sync::mpsc::channel();
        drop(rx);
        cx.update(|window, cx| view.update(cx, |this, cx| {
            this.handle(crate::mods::Request::Control { cmd: json!({ "do": "terminal.popup", "args": { "argv": ["make"], "wait": true } }), reply: tx }, window, cx)
        }));
        cx.run_until_parked();
        assert!(view.update(cx, |this, _| !this.waits.contains_key("t15")));
        assert!(!peek_open(cx), "its popup closes");
        assert_eq!((take("terminal.kill"), take("terminal.rm")), (vec![json!("t15")], vec![json!("t14"), json!("t15")]));
        assert!(popup(json!({ "argv": [] }), cx)["error"].is_string());

        // A plain peek over an open popup (a link clicked in it) only hides
        // it, and a popup over that peek ends the hidden popup, not the peek.
        popup(json!({ "argv": ["htop"] }), cx);
        peek_t5(cx);
        popup(json!({ "argv": ["true"] }), cx);
        cx.run_until_parked();
        assert_eq!(take("terminal.kill"), [json!("t16")], "t16 was hidden by the peek, t5 is a peek");
        take("terminal.rm");

        // Addresses come back: a popup whose program ended on its own is
        // forgotten, and a later terminal at its address is never ended.
        let listed = |terminals: serde_json::Value, cx: &mut gpui::VisualTestContext| cx.update(|window, cx| view.update(cx, |this, cx| this.terminals_listed(json!({ "terminals": terminals }), window, cx)));
        listed(json!([{ "address": "t17", "created_ms": 17 }]), cx);
        listed(json!([]), cx);
        listed(json!([{ "address": "t17", "created_ms": 900 }]), cx);
        popup(json!({ "argv": ["true"] }), cx);
        cx.run_until_parked();
        assert!(take("terminal.kill").is_empty(), "t17 ended on its own; the t17 now is another terminal");

        // Taken over between two listings (a core restart): told by its time.
        listed(json!([{ "address": "t18", "created_ms": 18 }]), cx);
        listed(json!([{ "address": "t18", "created_ms": 901 }]), cx);
        popup(json!({ "argv": ["true"] }), cx);
        // Or before any listing showed it.
        view.update(cx, |this, _| this.live.insert("t19".into(), json!({ "address": "t19", "created_ms": 902 })));
        popup(json!({ "argv": ["true"] }), cx);
        cx.run_until_parked();
        assert!(take("terminal.kill").is_empty(), "t18 and t19 are other terminals now");

        // Still the same terminal: ended as before.
        listed(json!([{ "address": "t20", "created_ms": 20 }]), cx);
        popup(json!({ "argv": ["true"] }), cx);
        cx.run_until_parked();
        assert_eq!(take("terminal.kill"), [json!("t20")]);
        take("terminal.rm");

        // Every way a popup closes ends it: Esc (overlay.close), the bar's ✕
        // (the mod's closeOverlay), the backdrop (close_overlay), and closing
        // the peek of a link clicked in it, which had only hidden it.
        let ends = |cx: &mut gpui::VisualTestContext| {
            cx.run_until_parked();
            (take("terminal.kill"), take("terminal.rm"))
        };
        cx.update(|window, cx| {
            view.update(cx, |this, cx| {
                this.run_as("overlay.close", json!({}), false, window, cx)
            })
        });
        assert!(!peek_open(cx), "Esc closes it");
        assert_eq!(ends(cx), (vec![json!("t21")], vec![json!("t21")]), "Esc");
        popup(json!({ "argv": ["tail", "-F", "x"] }), cx);
        cx.update(|window, cx| {
            view.update(cx, |this, cx| {
                this.handle(
                    crate::mods::Request::CloseOverlay {
                        id: "terminal-peek".into(),
                    },
                    window,
                    cx,
                )
            })
        });
        assert_eq!(ends(cx), (vec![json!("t22")], vec![json!("t22")]), "✕");
        popup(json!({ "argv": ["tail", "-F", "x"], "hold": true }), cx);
        peek_t5(cx);
        assert!(ends(cx).0.is_empty(), "a link's peek only hides it");
        hide(cx);
        assert_eq!(
            ends(cx),
            (vec![json!("t23")], vec![json!("t23")]),
            "closing the link's peek ends the popup under it, not t5"
        );
        hide(cx);
        assert_eq!(ends(cx), (vec![], vec![]), "closing again ends nothing");
        peek_t5(cx);
        hide(cx);
        assert_eq!(
            ends(cx),
            (vec![], vec![]),
            "closing a peek of a terminal no popup started ends nothing"
        );

        // Closing the window or quitting: the last popup's program ends and
        // its terminal goes, waited on or not, and finished ones kept for a
        // waiter go too.
        popup(json!({ "argv": ["true"], "wait": true }), cx);
        view.update(cx, |this, cx| {
            this.settle_waits(
                &HashMap::from([("t24".to_owned(), json!({ "exit_code": 0, "signal": null }))]),
                now,
                cx,
            )
        });
        popup(json!({ "argv": ["sleep", "9"], "wait": true }), cx);
        take("terminal.run");
        assert_eq!(
            ends(cx),
            (vec![json!("t24")], vec![]),
            "the finished t24, replaced, stays for its waiter"
        );
        view.update(cx, |this, _| {
            this.end_popups_now(std::time::Instant::now() + std::time::Duration::from_secs(5))
        });
        assert_eq!(
            (take("terminal.kill"), take("terminal.rm")),
            (vec![json!("t25")], vec![json!("t24"), json!("t25")])
        );
        assert!(view.update(cx, |this, _| this.popped.is_none() && this.waits.is_empty()));
        assert_eq!(ends(cx), (vec![], vec![]), "nothing else in flight");

        // Started by an app that's gone (a crash, a forced quit): the first
        // listing ends it; later listings leave terminals alone.
        let dead = i32::MAX.to_string();
        let orphan = |address: &str| json!({ "address": address, "created_ms": 1, "labels": { super::POPUP_LABEL: dead } });
        view.update(cx, |this, _| this.reaped_orphans = false);
        listed(
            json!([
                orphan("t90"),
                { "address": "t91", "labels": { super::POPUP_LABEL: std::process::id().to_string() } },
                { "address": "t93", "keep": true, "labels": { "herdr.space": "s1", "herdr.tab": "tb1" } },
                { "address": "t94", "keep": true, "labels": { super::POPUP_LABEL: dead, "herdr.space": "s1" } },
                { "address": "t95", "labels": {} },
                { "address": "mac/t96", "labels": { super::POPUP_LABEL: dead } },
            ]),
            cx,
        );
        assert_eq!(
            ends(cx),
            (vec![json!("t90")], vec![json!("t90")]),
            "t91 is this app's, t93 a pane's, t94 a kept popup, t95 no popup, mac/t96 another machine's"
        );
        listed(json!([orphan("t92")]), cx);
        assert_eq!(ends(cx), (vec![], vec![]), "only at the first listing");

        // ↗ keep: the popup is the person's tab now, and closing the peek leaves it.
        popup(json!({ "argv": ["htop"] }), cx);
        take("terminal.run");
        view.update(cx, |this, _| this.live.insert("t26".into(), json!({ "address": "t26", "created_ms": 26, "labels": { super::POPUP_LABEL: "1" } })));
        let keep = |t: &str, cx: &mut gpui::VisualTestContext| {
            cx.update(|window, cx| {
                view.update(cx, |this, cx| {
                    this.run_as("terminal.keep", json!({ "terminal": t }), false, window, cx)
                })
            })
        };
        let popped_is = |t: &str, cx: &mut gpui::VisualTestContext| {
            view.update(cx, |this, _| {
                this.popped.as_ref().is_some_and(|p| p.address == t)
            })
        };
        // Turned down at once (no layout to keep it in): still the popup, still up.
        assert!(
            keep("t26", cx)["error"].is_string(),
            "no layout to keep it in yet"
        );
        assert!(
            popped_is("t26", cx) && peek_open(cx),
            "a keep that failed leaves the popup up"
        );
        assert_eq!(ends(cx), (vec![], vec![]), "a failed keep ends nothing");
        view.update(cx, |this, _| {
            this.documents.entry(String::new()).or_default().available = true;
            this.model
                .add_space("s", "", "/tmp", crate::model::Tab::new("tb", "", "a", "ta"));
            this.model.select_tab("tb");
        });
        let kept = keep("t26", cx);
        assert_eq!(kept["pending"], json!(true), "{kept}");
        assert!(!peek_open(cx), "the keep closes the peek of it");
        hide(cx);
        assert_eq!(
            ends(cx),
            (vec![], vec![]),
            "kept t26 (on its way into a pane) is never ended"
        );
        assert!(
            view.update(cx, |this, _| this.model.pane_of("t26").is_some()),
            "in a pane once the keep took"
        );
        // Its popup label goes once the keep took, not before (a failed keep can still be cleaned up).
        let t26: Vec<(String, serde_json::Value)> = calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, p)| {
                p["terminal"] == "t26"
                    && (m == "terminal.keep"
                        || (m == "terminal.label"
                            && (p["labels"].get("herdr.space").is_some()
                                || p["labels"].get(super::POPUP_LABEL).is_some())))
            })
            .cloned()
            .collect();
        let methods: Vec<&str> = t26.iter().map(|(m, _)| m.as_str()).collect();
        assert_eq!(
            methods,
            ["terminal.label", "terminal.keep", "terminal.label"],
            "{t26:?}"
        );
        assert!(
            t26[0].1["labels"].get(super::POPUP_LABEL).is_none(),
            "{t26:?}"
        );
        assert_eq!(
            t26[2].1["labels"],
            json!({ super::POPUP_LABEL: "" }),
            "kept, it's a popup no more"
        );
        take("terminal.label");
        take("terminal.keep");

        // A kept wait popup survives the window closing; its pane shows how it ended.
        popup(json!({ "argv": ["make"], "wait": true }), cx);
        view.update(cx, |this, _| {
            this.live
                .insert("t27".into(), json!({ "address": "t27", "created_ms": 27 }))
        });
        keep("t27", cx);
        cx.run_until_parked();
        take("terminal.run");
        take("terminal.label");
        view.update(cx, |this, _| {
            this.end_popups_now(std::time::Instant::now() + std::time::Duration::from_secs(5))
        });
        assert_eq!(
            ends(cx),
            (vec![], vec![]),
            "kept t27 and t26 are the person's"
        );

        // A keep the core turns down later: the popup comes back, and closing it ends it.
        popup(json!({ "argv": ["htop"] }), cx);
        view.update(cx, |this, _| {
            this.live
                .insert("t28".into(), json!({ "address": "t28", "created_ms": 28 }))
        });
        keep("t28", cx);
        assert!(!peek_open(cx));
        cx.run_until_parked();
        assert!(popped_is("t28", cx) && peek_open(cx), "back as the popup");
        assert!(view.update(cx, |this, _| this.model.pane_of("t28").is_none()));
        hide(cx);
        assert_eq!(ends(cx), (vec![json!("t28")], vec![json!("t28")]));

        // Shown in a pane some other way (swapped in, or kept by `hc keep`):
        // closing the popup leaves it.
        popup(json!({ "argv": ["htop"] }), cx);
        view.update(cx, |this, _| this.swapped.insert("a".into(), "t29".into()));
        hide(cx);
        popup(json!({ "argv": ["htop"] }), cx);
        view.update(cx, |this, _| {
            this.live.insert(
                "t30".into(),
                json!({ "address": "t30", "created_ms": 30, "keep": true }),
            )
        });
        hide(cx);
        assert_eq!(
            ends(cx),
            (vec![], vec![]),
            "t29 is swapped into a pane, t30 kept by the core"
        );

        // Keeping the terminal a link's peek shows over a popup: the peek
        // closes as any close does, and the hidden popup under it ends.
        popup(json!({ "argv": ["tail", "-F", "x"] }), cx);
        view.update(cx, |this, _| {
            this.live.insert("t5".into(), json!({ "address": "t5" }))
        });
        peek_t5(cx);
        assert_eq!(keep("t5", cx)["pending"], json!(true));
        assert!(!peek_open(cx));
        let (killed, removed) = ends(cx);
        assert_eq!(
            (killed, removed),
            (vec![json!("t31")], vec![json!("t31")]),
            "the hidden t31 ends, the kept t5 does not"
        );
        take("terminal.label");
        take("terminal.keep");

        // A keep turned down later while another popup is up: that one stays,
        // and the one whose keep failed, which nothing would show, ends.
        popup(json!({ "argv": ["htop"] }), cx);
        view.update(cx, |this, _| {
            this.live
                .insert("t32".into(), json!({ "address": "t32", "created_ms": 32 }))
        });
        assert_eq!(keep("t32", cx)["pending"], json!(true));
        popup(json!({ "argv": ["htop"] }), cx);
        assert_eq!(
            ends(cx),
            (vec![json!("t32")], vec![json!("t32")]),
            "t32's keep failed with t33 up"
        );
        assert!(
            popped_is("t33", cx) && peeked(cx).0.to_string().contains(r#"{"terminal":"t33"}"#),
            "t33 is still the popup shown"
        );
    }

    /// Which popup terminals are orphans: local, unkept, labelled with an app
    /// process that is not running (and not this one).
    #[test]
    fn orphan_popups_are_those_of_apps_that_are_gone() {
        use serde_json::json;
        let label = |pid: &str| json!({ super::POPUP_LABEL: pid });
        let live: std::collections::HashMap<String, serde_json::Value> = [
            ("t1", json!({ "labels": label("7") })),
            ("t2", json!({ "labels": label("8") })),
            ("t3", json!({ "labels": label("9") })),
            ("t4", json!({ "labels": label("7"), "keep": true })),
            ("mac/t5", json!({ "labels": label("7") })),
            ("t6", json!({ "labels": {} })),
            ("t7", json!({ "labels": label("") })),
            ("t8", json!({})),
        ]
        .into_iter()
        .map(|(a, r)| (a.to_owned(), r))
        .collect();
        let mut orphans: Vec<&String> = super::orphan_popups(&live, 9, |pid| pid == 8).collect();
        orphans.sort();
        assert_eq!(
            orphans,
            ["t1"],
            "t2 runs, t3 is this app, t4 was kept, mac/t5 is another machine's, the rest aren't popups"
        );
        assert!(super::process_running(std::process::id()));
        if cfg!(unix) {
            assert!(!super::process_running(i32::MAX as u32));
        }
    }

    include!("window/pane_size_tests.rs");
    include!("window/drag_tests.rs");

    /// The whole path, in a real window: a layout names the toast mod, a
    /// notification opens it with its props, and it can close itself (as one
    /// that hands notifications to the system does). Unknown: the shipped toast.
    #[test]
    fn a_layout_s_toast_mod_gets_every_notification() {
        in_own_process("toast_window");
    }

    #[gpui::test]
    fn toast_window(cx: &mut gpui::TestAppContext) {
        use serde_json::json;
        if !running_alone("toast_window") {
            return;
        }
        let forward = r#"import { View, div } from "gpui-kit";
import { props, closeOverlay, shared, setShared } from "herdr";

export default class SystemToast extends View {
  render() {
    const p = props();
    // A real one would exec("notify-send", [p.title, p.body]) here.
    setShared("toast-test.seen", [...(shared("toast-test.seen") ?? []), `${p.title}: ${p.body}`]);
    closeOverlay(p.overlay);
    return div();
  }
}
"#;
        let layout = json!({ "id": "root", "type": "row", "toast": "system-toast", "children": [] });
        let (view, cx) = open_window(cx, "system-toast", forward, layout);
        let notify = |cx: &mut gpui::VisualTestContext| {
            view.update_in(cx, |this, window, cx| {
                this.sync_folders(window, cx);
                let note = json!({ "kind": "toast", "notification": { "title": "Build finished", "body": "All green", "level": "success" } });
                let id = crate::mods::new_overlay_id();
                this.open_overlay(id.clone(), note, window, cx);
                (id.clone(), this.copies[&format!("overlay:{id}")].inst.name.clone())
            })
        };
        // Herdr's own notes ("Your edits are in …") may show up too.
        let shown = |id: &str, cx: &mut gpui::VisualTestContext| {
            cx.run_until_parked();
            view.update(cx, |this, _| this.overlays.iter().any(|o| o.id == id))
        };

        let (id, name) = notify(cx);
        assert_eq!(name, "system-toast");
        assert!(!shown(&id, cx), "the layout's mod closed the notification");
        let seen = crate::mods::shared("toast-test.seen").unwrap_or_default();
        assert!(seen.as_array().unwrap().contains(&json!("Build finished: All green")), "the mod got the notification's props: {seen}");

        view.update(cx, |this, _| this.chrome.toast = Some("no-such-mod".into()));
        let (id, name) = notify(cx);
        assert_eq!(name, "toast");
        assert!(shown(&id, cx), "an unknown mod falls back to the shipped toast, which stays");
    }

    #[test]
    fn docking_keeps_the_layouts_notification_and_menu_mods() {
        in_own_process("dock_root_choices");
    }

    #[gpui::test]
    fn dock_root_choices(cx: &mut gpui::TestAppContext) {
        use serde_json::json;
        if !running_alone("dock_root_choices") {
            return;
        }
        let layout = json!({
            "id": "root", "type": "row", "toast": "chosen", "terminal_menu": "chosen", "children": []
        });
        let (view, cx) = open_window(cx, "chosen", "", layout);
        let check_handlers = |cx: &mut gpui::VisualTestContext| {
            view.update_in(cx, |this, window, cx| {
                let id = crate::mods::new_overlay_id();
                this.open_overlay(id.clone(), json!({
                    "kind": "toast", "notification": { "title": "After docking", "body": "Still chosen" }
                }), window, cx);
                assert_eq!(this.copies[&format!("overlay:{id}")].inst.name, "chosen");
                this.close_overlay(&id, window, cx);
                this.open_menu("tab", json!({}), None, window, cx);
                let id = this.overlays.last().unwrap().id.clone();
                assert_eq!(this.copies[&format!("overlay:{id}")].inst.name, "chosen");
                this.close_overlay(&id, window, cx);
            });
        };
        check_handlers(cx);
        let folder = crate::mods::app_dir().join("mods/new-top");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("mod.json"), r#"{"place":"top"}"#).unwrap();
        std::fs::write(folder.join("main.js"), "").unwrap();
        view.update_in(cx, |this, window, cx| {
            this.sync_folders(window, cx);
            assert_eq!(this.chrome.axis(), Some(crate::layout::Axis::Column));
            assert!(this.chrome.walk().iter().any(|n| matches!(
                n.content(), Some(crate::layout::Content::Mod { name, .. }) if name == "new-top"
            )));
            this.save();
        });
        check_handlers(cx);
        view.update_in(cx, |this, window, cx| {
            this.load_layout("test");
            this.sync(window, cx);
        });
        check_handlers(cx);
    }

    #[test]
    fn the_polls_folder_look_finds_new_edited_and_removed_mods_off_the_main_thread() {
        in_own_process("folder_poll");
    }

    /// The poll's look at the mod folders reads them in the background and
    /// applies what it found on the next turn: a new mod appears (and is placed),
    /// an edited mod.json's actions are listed, a removed mod goes. A look the
    /// window starts meanwhile (a file event's) wins over the older answer.
    #[gpui::test]
    fn folder_poll(cx: &mut gpui::TestAppContext) {
        use serde_json::json;
        if !running_alone("folder_poll") {
            return;
        }
        let (view, cx) = open_window(
            cx,
            "base",
            "",
            json!({ "id": "root", "type": "row", "children": [] }),
        );
        let folder = crate::mods::app_dir().join("mods/later");
        let poll = |cx: &mut gpui::VisualTestContext| {
            let before = view.update_in(cx, |this, window, cx| {
                this.sync_folders_later(window, cx);
                this.folders.contains_key("later")
            });
            cx.run_until_parked();
            (
                before,
                view.update(cx, |this, _| {
                    (this.folders.contains_key("later"), this.folder_scan_running)
                }),
            )
        };
        let actions = || {
            crate::mods::published_actions()
                .as_array()
                .into_iter()
                .flatten()
                .filter(|a| a["mod"] == "later")
                .count()
        };

        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("mod.json"), r#"{"place":"top"}"#).unwrap();
        std::fs::write(folder.join("main.js"), "").unwrap();
        assert_eq!(
            poll(cx),
            (false, (true, false)),
            "found on the next turn, not while the poll runs"
        );
        assert!(
            view.update(cx, |this, _| this.chrome.walk().iter().any(|n| matches!(
                n.content(), Some(crate::layout::Content::Mod { name, .. }) if name == "later"
            ))),
            "a new mod goes where it asked"
        );

        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(
            folder.join("mod.json"),
            json!({ "place": "top", "actions": [{ "title": "Later", "command": "mod.open" }] })
                .to_string(),
        )
        .unwrap();
        assert_eq!(actions(), 0);
        let stamp = view.update(cx, |this, _| this.folders["later"].stamp);
        poll(cx);
        assert!(
            view.update(cx, |this, _| this.folders["later"].stamp) > stamp,
            "the edit is seen"
        );
        assert_eq!(actions(), 1, "the edited mod.json's actions are listed");

        // A file event's look starts while the poll's is out: the poll's older answer is dropped.
        std::fs::remove_dir_all(&folder).unwrap();
        view.update_in(cx, |this, window, cx| {
            this.sync_folders_later(window, cx);
            this.sync_folders(window, cx);
            assert!(
                !this.folders.contains_key("later"),
                "the file event's look applies at once"
            );
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(folder.join("main.js"), "").unwrap();
        });
        cx.run_until_parked();
        assert!(
            !view.update(cx, |this, _| this.folders.contains_key("later")),
            "the poll's look, begun before, is dropped"
        );
        assert!(
            !view.update(cx, |this, _| this.folder_scan_running),
            "and the next poll can look again"
        );
        assert_eq!(poll(cx), (false, (true, false)));

        std::fs::remove_dir_all(&folder).unwrap();
        assert_eq!(poll(cx), (true, (false, false)), "a removed mod goes");
        assert_eq!(actions(), 0, "and so do its actions");

        // The poll itself looks this way, on its 25th turn (each one without file events).
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("main.js"), "").unwrap();
        let polled = view.update_in(cx, |this, window, cx| {
            this.polls = 24;
            this.poll(window, cx);
            (this.folders.contains_key("later"), this.folder_scan_running)
        });
        assert_eq!(polled, (false, true), "the poll's look is out, not done on the main thread");
        cx.run_until_parked();
        assert!(view.update(cx, |this, _| this.folders.contains_key("later")));
    }

    #[test]
    fn toast_is_a_root_layout_choice_that_roundtrips() {
        let mut root = super::default_layout("main");
        assert_eq!(root.toast, None);
        assert!(serde_json::to_value(&root).unwrap().get("toast").is_none());
        root.toast = Some("system-toast".into());
        let restored: super::Node = serde_json::from_value(serde_json::to_value(&root).unwrap()).unwrap();
        assert_eq!(restored.toast.as_deref(), Some("system-toast"));
    }

    #[test]
    fn terminal_menu_preserves_clicked_target_and_uses_mod_dimensions() {
        use serde_json::json;
        let target = json!({ "pane": "p2", "terminal": "t8" });
        let spec = |defaults: serde_json::Value, viewport: f32, scale: f32| super::terminal_menu_spec("personal-menu", &defaults, "pane", target.clone(), viewport, scale);
        let stock = super::terminal_menu_spec("menu", &json!({}), "pane", target.clone(), 900.0, 1.0);
        assert_eq!((stock["width"].clone(), stock["height"].clone()), (json!(220.0), json!(420.0)));
        let custom = spec(json!({"kind": "dialog", "width": 310, "height": 120}), 900.0, 1.0);
        assert_eq!((custom["mod"].clone(), custom["kind"].clone(), custom["width"].clone(), custom["height"].clone()), (json!("personal-menu"), json!("popup"), json!(310.0), json!(120.0)));
        // The room inside the popup's 1 px border: the menu scrolls its items within it.
        assert_eq!(custom["props"], json!({"tab": "pane", "target": target, "height": 118.0}));
        assert_eq!(stock["props"]["height"], 418.0);
        let partial = spec(json!({"width": 0.4}), 900.0, 1.0);
        assert_eq!((partial["width"].clone(), partial["height"].clone()), (json!(0.4), json!(420.0)));
        // A short window: what fits between the 8 px margins, less the border.
        assert_eq!(stock_height(300.0, 1.0), 300.0 - 16.0 - 2.0);
        // Zoomed panels: the window holds fewer of the menu's px, and the border is 1 px of the window's.
        assert_eq!(stock_height(600.0, 2.0), (600.0 - 16.0) / 2.0 - 1.0);
        assert_eq!(stock_height(900.0, 2.0), 419.0, "420 still fits: only the border shrinks it");
        // A share of the window, in the menu's px.
        assert_eq!(spec(json!({"height": 0.5}), 800.0, 2.0)["props"]["height"], 800.0 * 0.5 / 2.0 - 1.0);
        assert_eq!(stock_height(10.0, 1.0), 0.0, "never below nothing");

        fn stock_height(viewport: f32, scale: f32) -> f64 {
            super::terminal_menu_spec("menu", &serde_json::json!({}), "pane", serde_json::json!({}), viewport, scale)["props"]["height"].as_f64().unwrap()
        }
    }

    #[test]
    fn composition_focus_only_uses_visible_terminal_blocks() {
        let chrome: super::Node = serde_json::from_value(serde_json::json!({
            "id": "composition", "type": "row", "children": [
                { "id": "left", "type": "block", "content": {"terminal": "t1"} },
                { "id": "right", "type": "block", "content": {"terminal": "other/t2"} },
                { "id": "hidden", "type": "block", "hidden": true, "content": {"terminal": "t3"} },
                { "id": "tabs", "type": "block", "content": {"tab": "active"} }
            ]
        })).unwrap();
        assert_eq!(super::terminal_block(&chrome, Some("left")).and_then(|n| n.terminal()), Some("t1"));
        assert_eq!(super::terminal_block(&chrome, Some("right")).and_then(|n| n.terminal()), Some("other/t2"));
        for id in [None, Some("hidden"), Some("tabs"), Some("source-pane"), Some("overlay:term")] {
            assert!(super::terminal_block(&chrome, id).is_none());
        }
    }

    #[test]
    fn pane_swap_refuses_a_direction_that_is_not_a_string() {
        use serde_json::json;
        for direction in [json!(false), json!(1), json!(["left"]), json!({"to": "left"})] {
            for args in [json!({"direction": direction}), json!({"direction": direction, "target": "p2"})] {
                assert_eq!(super::swap_by(&args), Err("pane.swap direction must be a string (or null to omit it)".into()), "{args}");
            }
        }
        assert_eq!(super::swap_by(&json!({"direction": null, "target": "p2"})), Ok(super::SwapBy::Target("p2")));
    }

    fn swap_model() -> super::Model {
        use crate::layout::Axis;
        let mut m = super::Model::default();
        m.add_space("s1", "api", "/src", super::Tab::new("c1", "", "x1", "t1"));
        m.split("x1", Axis::Row, "x2", "t2");
        m.add_tab("s1", super::Tab::new("c2", "", "x3", "t3"));
        m.select_tab("c1");
        m.focus_pane("x1");
        m
    }

    fn swap_state(m: &super::Model) -> (Vec<String>, Vec<String>, Option<String>) {
        let order = |tab: &str| m.tab(tab).unwrap().root.panes().iter().map(|n| n.id.clone()).collect();
        (order("c1"), order("c2"), m.focused_pane())
    }

    #[test]
    fn an_explicit_pane_swaps_with_its_neighbor_in_the_shown_tab() {
        let mut m = swap_model();
        assert_eq!(super::swap_with_neighbor(&mut m, "x2", true, Some("x1".into())), Ok(()));
        assert_eq!(swap_state(&m), (vec!["x2".into(), "x1".into()], vec!["x3".into()], Some("x1".into())));
        assert_eq!(super::swap_with_neighbor(&mut m, "x2", true, None), Ok(()));
        assert_eq!(swap_state(&m).0, ["x2", "x1"]);
    }

    #[test]
    fn an_explicit_pane_swap_by_direction_refuses_panes_it_cannot_place() {
        let mut m = swap_model();
        let container = m.tab("c1").unwrap().root.id.clone();
        let before = swap_state(&m);
        let cases = [
            ("nope".to_owned(), "no pane \"nope\"".to_owned()),
            (container.clone(), format!("no pane {container:?}")),
            ("x3".to_owned(), "pane.swap direction needs a pane in the shown tab, not \"x3\"".to_owned()),
        ];
        for (pane, error) in cases {
            assert_eq!(super::swap_with_neighbor(&mut m, &pane, true, Some("x1".into())), Err(error), "{pane}");
            assert_eq!(swap_state(&m), before, "{pane}");
        }
    }

    #[test]
    fn pane_swap_takes_exactly_one_of_direction_or_target() {
        use super::SwapBy;
        use serde_json::json;
        for dir in ["left", "right", "up", "down"] {
            assert_eq!(super::swap_by(&json!({"direction": dir})), Ok(SwapBy::Direction(dir)));
        }
        assert_eq!(super::swap_by(&json!({"target": "p2", "pane": "p1"})), Ok(SwapBy::Target("p2")));
        assert_eq!(super::swap_by(&json!({"direction": "left", "target": "p2"})), Err("pane.swap takes direction or target, not both".into()));
        for neither in [json!({}), json!(null), json!({"pane": "p1"}), json!({"direction": null, "target": null})] {
            assert_eq!(super::swap_by(&neither), Err("pane.swap needs direction or target".into()), "{neither}");
        }
        assert_eq!(super::swap_by(&json!({"direction": "sideways"})), Err("pane.swap direction is left, right, up or down, not \"sideways\"".into()));
    }

    #[test]
    fn composition_commands_never_default_to_a_source_pane() {
        use serde_json::json;
        for name in ["pane.split", "pane.close", "pane.zoom", "pane.go", "pane.swap", "terminal.show"] {
            assert!(super::needs_model_pane(name, &json!({})), "{name}");
            assert!(super::needs_model_pane(name, &json!({"pane": null})), "{name}");
        }
        for name in ["pane.split", "pane.close", "pane.zoom", "pane.swap", "terminal.show"] {
            assert!(!super::needs_model_pane(name, &json!({"pane": "explicit-pane"})), "{name}");
        }
        assert!(super::needs_model_pane("pane.go", &json!({"pane": "explicit-pane"})));
        for name in ["terminal.open", "terminal.keep"] {
            for place in ["right", "down"] {
                assert!(super::needs_model_pane(name, &json!({"place": place})), "{name} {place}");
            }
            assert!(!super::needs_model_pane(name, &json!({})), "{name}");
            assert!(!super::needs_model_pane(name, &json!({"place": "tab"})), "{name}");
        }
        for name in ["pane.focus", "terminal.copy", "terminal.paste", "terminal.paste_image", "terminal.right_click"] {
            assert!(!super::needs_model_pane(name, &json!({})), "{name}");
        }
    }

    #[test]
    fn folder_info_comes_from_the_git_status() {
        use serde_json::json;
        assert_eq!(super::folder_info(&serde_json::Value::Null, "/src/x", true), None, "not known yet");
        let none = super::folder_info(&json!(false), "/src/x", true).unwrap();
        assert_eq!((none.top.as_str(), none.branch), ("/src/x", None));
        let repo = super::folder_info(&json!({ "root": "/src", "branch": "main" }), "/src/x", true).unwrap();
        assert_eq!((repo.top.as_str(), repo.branch.as_deref()), ("/src", Some("main")));
        let detached = super::folder_info(&json!({ "root": "/src", "branch": null }), "/src/x", true).unwrap();
        assert_eq!(detached.branch, None);
        let status = json!({ "root": "/src", "branch": "main", "ahead": 2, "behind": 1, "staged": 0, "unstaged": 0, "untracked": 1, "conflicted": 0 });
        let counted = super::folder_info(&status, "/src/x", true).unwrap();
        assert_eq!((counted.ahead, counted.behind, counted.dirty), (2, 1, true));
        let clean = super::folder_info(&json!({ "root": "/src", "branch": "main", "ahead": 0, "behind": 0, "staged": 0, "unstaged": 0, "untracked": 0, "conflicted": 0 }), "/src/x", true).unwrap();
        assert!(!clean.dirty);
        assert_eq!(super::folder_info(&status, "/src/x", false).unwrap(), super::folder_info(&json!({ "root": "/src", "branch": "main" }), "/src/x", true).unwrap(), "without pane_header_info, as before");
    }

    #[test]
    fn busy_is_an_agent_working_or_a_program_in_front_of_the_shell() {
        let rec = |status: &str, shell: bool, agent: bool| {
            serde_json::json!({ "status": status, "program": { "shell": shell, "agent": agent } })
        };
        assert!(super::busy(&rec("working", false, true)), "an agent's turn");
        assert!(!super::busy(&rec("done", false, true)), "an agent started from a shell, its turn over");
        assert!(super::busy(&rec("", false, false)), "vim in front of the shell");
        assert!(!super::busy(&rec("", true, false)), "the shell's prompt");
    }

    #[test]
    fn shipped_layouts_parse_and_show_what_they_promise() {
        let main = super::default_layout("main");
        assert!(main.find("main").is_some() && main.find("sidebar").is_some());
        assert!(main.walk().iter().any(|n| matches!(n.content(), Some(super::Content::Context { .. }))));
        let terminals = super::default_layout("terminals");
        assert!(terminals.find("tabs").is_some());
        assert!(matches!(terminals.find("main").and_then(|n| n.content()), Some(super::Content::Context { .. })));
    }

    #[test]
    fn a_finished_terminal_is_not_put_back_in_its_tab() {
        let record = |state: &str| serde_json::json!({ "state": state, "labels": { "herdr.space": "sp1", "herdr.tab": "tb1" } });
        assert!(super::adoptable(&record("live"), "sp1", Some("tb1"), None));
        assert!(!super::adoptable(&record("finished"), "sp1", Some("tb1"), None));
        assert!(!super::adoptable(&record("live"), "sp2", Some("tb1"), None));
        assert!(!super::adoptable(&record("live"), "sp1", Some("tb2"), None));
    }

}
