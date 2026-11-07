//! Alerts outside the window: a system notification, or the person's own
//! `alert_command` in its place. The toast and the sounds stay in window.rs.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use gpui::{App, SystemNotification, SystemNotificationSound};
use serde_json::Value;

use crate::model::Alert;
use crate::notification_icon::Source;
use crate::settings::{AlertCommand, Settings, SystemNotifications};

/// One alert, as the notification and the command see it.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub address: String,
    pub alert: Alert,
    /// The status word the agent left.
    pub prev: Option<String>,
    pub app: Option<String>,
    pub name: String,
    /// "space / tab", as the toast shows it.
    pub place: Option<String>,
    /// What the agent's 7501 report said with it.
    pub said: Said,
}

/// The agent's own words from its 7501 report, as given; empty when it said
/// nothing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Said {
    /// permission, question or auth.
    pub kind: String,
    pub title: String,
    /// Its own summary, e.g. "approve Bash: make".
    pub message: String,
}

impl Said {
    /// From the terminal's 7501 record in the core's listing.
    pub fn of(report: Option<&serde_json::Value>) -> Said {
        let field = |key: &str| report.and_then(|r| r.get(key)).and_then(serde_json::Value::as_str).unwrap_or_default().to_owned();
        Said { kind: field("kind"), title: field("title"), message: field("msg") }
    }
}

impl Note {
    pub fn title(&self) -> String {
        title(self.alert, &self.name)
    }

    fn state(&self) -> &'static str {
        match self.alert {
            Alert::Blocked => "blocked",
            Alert::Error => "error",
            Alert::Done => "done",
        }
    }
}

/// "<name> needs you", "<name> hit an error" or "<name> finished".
pub fn title(alert: Alert, name: &str) -> String {
    match alert {
        Alert::Blocked => format!("{name} needs you"),
        Alert::Error => format!("{name} hit an error"),
        Alert::Done => format!("{name} finished"),
    }
}

/// The title of one toast for `count` agents with the same alert.
pub fn merged_title(alert: Alert, count: usize) -> String {
    match alert {
        Alert::Blocked => format!("{count} agents need you"),
        Alert::Error => format!("{count} agents hit an error"),
        Alert::Done => format!("{count} agents finished"),
    }
}

/// Whether an alert leaves the window. `app_active`: Herdr's window is key in
/// the app in front. `looking`: that agent is on screen in it.
pub fn should_notify(mode: SystemNotifications, app_active: bool, looking: bool) -> bool {
    match mode {
        SystemNotifications::Off => false,
        _ if looking => false,
        SystemNotifications::Background => !app_active,
        SystemNotifications::Always => true,
    }
}

const TAG_PREFIX: &str = "herdr-agent:";

/// One notification per agent: a newer alert replaces the older one.
pub fn tag(address: &str) -> String {
    format!("{TAG_PREFIX}{address}")
}

pub fn address_of(tag: &str) -> Option<&str> {
    tag.strip_prefix(TAG_PREFIX).filter(|a| !a.is_empty())
}

/// How an agent's system notification sounds and looks, from the settings.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Look {
    pub sound: SystemNotificationSound,
    /// The agent's glyph before the title and the state's mark before the body.
    pub marks: bool,
    /// The agent's program icon, made into the notification's image off the
    /// main thread (`show`).
    pub icon: Option<Source>,
}

impl Look {
    /// For posting `note`: with `notification_icons`, its program's icon
    /// (notification_icon.rs).
    pub fn of(note: &Note, settings: &Settings) -> Look {
        let sounds = &settings.notification_sounds;
        let sound = match note.alert {
            Alert::Blocked => &sounds.blocked,
            Alert::Error => &sounds.error,
            Alert::Done => &sounds.done,
        };
        let icon = if settings.notification_icons { note.app.as_deref().and_then(Source::program) } else { None };
        Look { sound: sound.clone(), marks: settings.notification_marks, icon }
    }
}

/// Each agent's glyph by its programs.json key (or its `app` with -code,
/// -cli or -agent left off), as the herdr-app-notify script had them.
const AGENT_GLYPHS: &[(&str, &str)] = &[
    ("claude", "🤖"),
    ("codex", "🧠"),
    ("gemini", "✨"),
    ("cursor", "🖱️"),
    ("copilot", "🚀"),
    ("aider", "🛠️"),
    ("crush", "💜"),
    ("opencode", "📂"),
    ("amp", "⚡"),
];
const AGENT_GLYPH_OTHER: &str = "🤖";

/// The glyph before an agent notification's title.
pub fn agent_glyph(app: Option<&str>) -> &'static str {
    let Some(app) = app.filter(|a| !a.is_empty()) else { return AGENT_GLYPH_OTHER };
    let key = crate::programs::of(Some(app), &[]).get("key").and_then(Value::as_str).map(str::to_owned);
    let stem = ["-code", "-cli", "-agent"].iter().fold(app, |a, suffix| a.strip_suffix(suffix).unwrap_or(a));
    let glyph = |name: &str| AGENT_GLYPHS.iter().find(|(n, _)| *n == name).map(|(_, g)| *g);
    key.as_deref().and_then(glyph).or_else(|| glyph(stem)).unwrap_or(AGENT_GLYPH_OTHER)
}

/// The mark before an agent notification's body.
pub fn state_mark(alert: Alert) -> &'static str {
    match alert {
        Alert::Blocked => "✋",
        Alert::Error => "🔴",
        Alert::Done => "✅",
    }
}

pub fn system_notification(note: &Note, look: &Look) -> SystemNotification {
    let (mut title, mut body) = (note.title(), body(note));
    if look.marks {
        title = format!("{} {title}", agent_glyph(note.app.as_deref()));
        body = if body.is_empty() { state_mark(note.alert).to_owned() } else { format!("{} {body}", state_mark(note.alert)) };
    }
    SystemNotification {
        tag: tag(&note.address).into(),
        title: title.into(),
        body: body.into(),
        sound: look.sound.clone(),
        ..Default::default()
    }
}

/// Posts `note` as its system notification, looking as the settings say.
pub fn post(note: &Note, settings: &Settings, cx: &App) {
    let look = Look::of(note, settings);
    show(system_notification(note, &look), look.icon, cx);
}

thread_local! {
    /// Notifications waiting for their image, by tag: the newest one's number.
    static WAITING: std::cell::RefCell<std::collections::HashMap<String, u64>> = Default::default();
}

/// Shows `notification`, with `icon` made into its image off the main thread
/// first (it reads, draws and copies files). A notification posted or
/// dismissed under the same tag meanwhile wins: the late one isn't shown.
pub fn show(mut notification: SystemNotification, icon: Option<Source>, cx: &App) {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tag = notification.tag.to_string();
    let Some(icon) = icon else {
        WAITING.with(|w| w.borrow_mut().remove(&tag));
        return cx.show_system_notification(notification);
    };
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    WAITING.with(|w| w.borrow_mut().insert(tag.clone(), n));
    let image = cx.background_executor().spawn(async move { icon.copy() });
    cx.spawn(async move |cx| {
        notification.image = image.await;
        let newest = WAITING.with(|w| {
            let mut w = w.borrow_mut();
            let newest = w.get(&tag) == Some(&n);
            if newest {
                w.remove(&tag);
            }
            newest
        });
        if newest {
            cx.update(|cx| cx.show_system_notification(notification));
        } else if let Some(image) = notification.image {
            let _ = std::fs::remove_file(image);
        }
    })
    .detach();
}

/// Takes back the notification under `tag`, and one still waiting for its image.
pub fn dismiss(tag: &str, cx: &App) {
    WAITING.with(|w| w.borrow_mut().remove(tag));
    cx.dismiss_system_notification(tag);
}

/// The agent's own message when it gave one, over "space / tab".
fn body(note: &Note) -> String {
    let lines = [note.said.message.trim(), note.place.as_deref().unwrap_or_default()];
    lines.into_iter().filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}

/// The program and its arguments; `None` when there is nothing to run.
pub fn command_argv(command: &AlertCommand) -> Option<Vec<String>> {
    match command {
        AlertCommand::Shell(line) if line.trim().is_empty() => None,
        AlertCommand::Shell(line) => Some(vec!["/bin/sh".into(), "-c".into(), line.clone()]),
        AlertCommand::Argv(argv) if argv.first().is_none_or(|p| p.is_empty()) => None,
        AlertCommand::Argv(argv) => Some(argv.clone()),
    }
}

/// The environment `alert_command` gets (defaults/AGENTS.md lists it).
pub fn command_env(note: &Note) -> Vec<(&'static str, String)> {
    let mut env = vec![("HERDR_AGENT_STATE", note.state().to_owned())];
    if let Some(prev) = &note.prev {
        env.push(("HERDR_AGENT_PREV_STATE", prev.clone()));
    }
    env.extend([
        ("HERDR_AGENT_APP", note.app.clone().unwrap_or_default()),
        ("HERDR_AGENT_NAME", note.name.clone()),
        ("HERDR_AGENT_ADDRESS", note.address.clone()),
        ("HERDR_AGENT_PLACE", note.place.clone().unwrap_or_default()),
        ("HERDR_ALERT_TITLE", note.title()),
        ("HERDR_AGENT_MESSAGE", note.said.message.clone()),
        ("HERDR_AGENT_KIND", note.said.kind.clone()),
        ("HERDR_AGENT_TITLE", note.said.title.clone()),
    ]);
    env
}

/// What `send` did.
#[derive(Debug)]
pub enum Sent {
    Posted,
    /// The command started; why it failed comes here if it exits non-zero.
    Ran(async_channel::Receiver<String>),
    /// The command didn't start (why): the system notification went instead.
    Fallback(String),
}

/// Sends the alert: the person's `alert_command` when they set one, else (or
/// when it doesn't start) the system notification.
pub fn send(note: &Note, settings: &Settings, cx: &App) -> Sent {
    let sent = match settings.alert_command.as_ref().and_then(|c| Some((c, command_argv(c)?))) {
        Some((c, argv)) => match run(&argv, label(c), note) {
            Ok(failed) => return Sent::Ran(failed),
            Err(error) => Sent::Fallback(error),
        },
        None => Sent::Posted,
    };
    post(note, settings, cx);
    sent
}

/// The command as the person wrote it, for the error toast: a shell line
/// rather than `/bin/sh`.
fn label(command: &AlertCommand) -> &str {
    match command {
        AlertCommand::Shell(line) => line,
        AlertCommand::Argv(argv) => argv.first().map_or("", String::as_str),
    }
}

/// How long an `alert_command` may run before it is killed.
const ALERT_COMMAND_LIMIT: Duration = Duration::from_secs(30);

/// The process `run` starts: in the home folder (Herdr's own is `/` when
/// opened from the Finder), in its own process group, so a hung one is
/// killed with everything it started.
fn command(argv: &[String], note: &Note) -> Command {
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .envs(command_env(note))
        .current_dir(crate::mods::home())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    own_group(&mut cmd);
    cmd
}

/// Why an exit counts as a failure, or `None` when it doesn't. A shell line
/// always starts, so a missing program only shows here (127).
fn exit_failure(label: &str, code: Option<i32>) -> Option<String> {
    match code? {
        0 => None,
        127 => Some(format!("alert_command {label:?} failed: command not found (exit 127)")),
        126 => Some(format!("alert_command {label:?} failed: cannot run (exit 126)")),
        code => Some(format!("alert_command {label:?} exited with code {code}")),
    }
}

/// Fire and forget: a thread waits for the program so it leaves no zombie,
/// and the window never waits for it. Why it didn't start, if not; once
/// started, why it failed arrives on the receiver.
fn run(argv: &[String], label: &str, note: &Note) -> Result<async_channel::Receiver<String>, String> {
    match command(argv, note).spawn() {
        Ok(child) => {
            let (tx, rx) = async_channel::bounded(1);
            let label = label.to_owned();
            std::thread::spawn(move || {
                let Some(status) = reap(child, ALERT_COMMAND_LIMIT) else { return };
                if let Some(why) = exit_failure(&label, status.code()) {
                    tracing::warn!("{why}");
                    let _ = tx.try_send(why);
                } else if !status.success() {
                    tracing::warn!("alert_command {label:?} {status}");
                }
            });
            Ok(rx)
        }
        Err(error) => {
            let error = format!("alert_command {:?} did not start: {error}", argv[0]);
            tracing::warn!("{error}");
            Err(error)
        }
    }
}

fn own_group(cmd: &mut Command) {
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(cmd, 0);
    #[cfg(not(unix))]
    let _ = cmd;
}

/// Kills `child` and, on Unix, the process group it leads (`own_group`).
fn kill(child: &mut std::process::Child) {
    #[cfg(unix)]
    if let Ok(group) = libc::pid_t::try_from(child.id()) {
        // SAFETY: killpg takes plain integers; a group already gone is ESRCH.
        unsafe { libc::killpg(group, libc::SIGKILL) };
    }
    let _ = child.kill();
}

/// Waits for `child` up to `limit`, then kills it with its group: a hung
/// command must not keep a thread (and the programs) forever.
fn reap(mut child: std::process::Child, limit: Duration) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100).min(deadline.saturating_duration_since(Instant::now()))),
            Ok(None) => {
                tracing::warn!("alert_command still running after {}s: killed", limit.as_secs());
                kill(&mut child);
                return child.wait().ok();
            }
            Err(_) => return None,
        }
    }
}

/// `system.notify`: a notification any program posts as Herdr.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Notify {
    pub title: String,
    pub body: String,
    pub subtitle: String,
    pub sound: SystemNotificationSound,
    /// A terminal address or an agent's name: a click focuses it.
    pub agent: Option<String>,
    /// A newer notification with the same id replaces the older one.
    pub id: Option<String>,
    /// Posted even while Herdr is in front with `system_notifications: background`.
    pub urgent: bool,
    /// An image file shown with it.
    pub icon: Option<PathBuf>,
}

/// Longer text is cut short with "…"; the notification center cuts it sooner.
const NOTIFY_TITLE_MAX: usize = 200;
const NOTIFY_SUBTITLE_MAX: usize = 200;
const NOTIFY_BODY_MAX: usize = 2000;
const NOTIFY_ID_MAX: usize = 64;
const NOTIFY_AGENT_MAX: usize = 200;
const NOTIFY_ICON_PATH_MAX: usize = 1024;
/// macOS's own limit for an image attachment.
const NOTIFY_ICON_BYTES_MAX: u64 = 10 * 1024 * 1024;
const NOTIFY_ICON_TYPES: &[&str] = &["png", "jpg", "jpeg", "gif", "heic"];

/// `system.notify`'s arguments, checked: an error says which one and why.
pub fn parse_notify(args: &Value) -> Result<Notify, String> {
    let text = |key: &str| -> Result<Option<&str>, String> {
        match args.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s)),
            Some(_) => Err(format!("system.notify: {key} must be a string")),
        }
    };
    let title = clean(text("title")?.unwrap_or_default(), false, NOTIFY_TITLE_MAX);
    if title.is_empty() {
        return Err("system.notify: title is required".into());
    }
    let body = clean(text("body")?.unwrap_or_default(), true, NOTIFY_BODY_MAX);
    let subtitle = clean(text("subtitle")?.unwrap_or_default(), false, NOTIFY_SUBTITLE_MAX);
    let sound = match args.get("sound") {
        None | Some(Value::Null) => SystemNotificationSound::Default,
        Some(v) => crate::settings::sound(v).map_err(|why| format!("system.notify: sound {why}"))?,
    };
    let id = text("id")?.map(|id| {
        let ok = (1..=NOTIFY_ID_MAX).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-'));
        if ok { Ok(id.to_owned()) } else { Err(format!("system.notify: id {id:?} must be 1 to {NOTIFY_ID_MAX} of A-Z a-z 0-9 . _ : -")) }
    }).transpose()?;
    let agent = text("agent")?.map(|agent| {
        let ok = (1..=NOTIFY_AGENT_MAX).contains(&agent.chars().count()) && !agent.chars().any(is_unsafe_char);
        if ok { Ok(agent.to_owned()) } else { Err(format!("system.notify: agent must be a terminal address or a name, 1 to {NOTIFY_AGENT_MAX} characters without control characters")) }
    }).transpose()?;
    let urgent = match args.get("urgent") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err("system.notify: urgent must be true or false".into()),
    };
    let icon = text("icon")?.map(notify_icon).transpose()?;
    Ok(Notify { title, body, subtitle, sound, agent, id, urgent, icon })
}

/// An `icon` path: an absolute path to an image file macOS can attach.
fn notify_icon(path: &str) -> Result<PathBuf, String> {
    let fail = |why: &str| Err(format!("system.notify: icon {path:?} {why}"));
    if path.len() > NOTIFY_ICON_PATH_MAX || path.chars().any(is_unsafe_char) {
        return fail("is not a usable path");
    }
    let file = PathBuf::from(path);
    if !file.is_absolute() {
        return fail("must be an absolute path");
    }
    let kind = file.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    if !NOTIFY_ICON_TYPES.contains(&kind.as_str()) {
        return fail(&format!("must be one of {}", NOTIFY_ICON_TYPES.join(", ")));
    }
    match std::fs::metadata(&file) {
        Ok(m) if m.is_file() && m.len() <= NOTIFY_ICON_BYTES_MAX => Ok(file),
        Ok(m) if m.is_file() => fail("is over 10 MB"),
        _ => fail("is not a file"),
    }
}

/// Control characters (a newline kept where `multiline`), and the direction
/// overrides that make text read other than it is, are left out; past `max`
/// characters it ends in "…".
fn clean(text: &str, multiline: bool, max: usize) -> String {
    let text = text.replace("\r\n", "\n");
    let kept: String = text
        .trim()
        .chars()
        .map(|c| if c == '\t' || (c == '\n' && !multiline) { ' ' } else { c })
        .filter(|c| (*c == '\n' && multiline) || !is_unsafe_char(*c))
        .collect();
    if kept.chars().count() <= max {
        return kept;
    }
    let mut cut: String = kept.chars().take(max - 1).collect();
    cut.push('…');
    cut
}

fn is_unsafe_char(c: char) -> bool {
    c.is_control() || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

const NOTIFY_TAG_PREFIX: &str = "herdr-notify:";

/// `system.notify`'s tag: its id, else a new one each time.
pub fn notify_tag(id: Option<&str>) -> String {
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    match id {
        Some(id) => format!("{NOTIFY_TAG_PREFIX}{id}"),
        None => {
            let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
            format!("{NOTIFY_TAG_PREFIX}~{at}-{n}")
        }
    }
}

pub fn notify_notification(notify: &Notify, tag: &str) -> SystemNotification {
    SystemNotification {
        tag: tag.to_owned().into(),
        title: notify.title.clone().into(),
        subtitle: notify.subtitle.clone().into(),
        body: notify.body.clone().into(),
        sound: notify.sound.clone(),
        ..Default::default()
    }
}

/// Where `system.notify` goes: nowhere (`system_notifications: off`), the
/// system's notification center, or the window's own toast (`background`
/// while Herdr is in front, unless urgent).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NotifyTo {
    Nowhere,
    System,
    Toast,
}

pub fn notify_to(mode: SystemNotifications, app_active: bool, urgent: bool) -> NotifyTo {
    match mode {
        SystemNotifications::Off => NotifyTo::Nowhere,
        SystemNotifications::Background if app_active && !urgent => NotifyTo::Toast,
        _ => NotifyTo::System,
    }
}

/// Which agent a `system.notify` click focuses, by tag: the newest few.
#[derive(Debug, Default)]
pub struct NotifyClicks(std::collections::VecDeque<(String, String)>);

/// How many clickable `system.notify` notifications are remembered.
const NOTIFY_CLICKS_KEPT: usize = 64;

impl NotifyClicks {
    /// A notification posted under `tag`: it focuses `agent` when clicked, or
    /// nothing (it replaced one that did).
    pub fn posted(&mut self, tag: &str, agent: Option<&str>) {
        self.0.retain(|(t, _)| t != tag);
        if let Some(agent) = agent {
            if self.0.len() == NOTIFY_CLICKS_KEPT {
                self.0.pop_front();
            }
            self.0.push_back((tag.to_owned(), agent.to_owned()));
        }
    }

    /// The agent a clicked notification focuses.
    pub fn clicked(&mut self, tag: &str) -> Option<String> {
        let at = self.0.iter().position(|(t, _)| t == tag)?;
        self.0.remove(at).map(|(_, agent)| agent)
    }
}

/// Whether a posted notification can show: not where the platform knows it
/// can't (macOS: not run from an app bundle, or notifications denied in
/// System Settings), so `system.notify` fails and its caller can fall back.
pub fn can_post(cx: &App) -> bool {
    cx.system_notifications_allowed() != Some(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Person;

    fn note(alert: Alert) -> Note {
        Note {
            address: "local:t7".into(),
            alert,
            prev: Some("working".into()),
            app: Some("claude-code".into()),
            name: "Claude".into(),
            place: Some("herdr / main".into()),
            said: Said::default(),
        }
    }

    /// The settings with no program icons (their files are the person's).
    fn plain() -> Settings {
        Settings { notification_icons: false, ..Settings::default() }
    }

    fn said(message: &str) -> Said {
        Said { kind: "permission".into(), title: "fix the build".into(), message: message.into() }
    }

    #[test]
    fn notifies_only_when_away_unless_always_and_never_while_looking() {
        use SystemNotifications::*;
        let cases = [
            // mode, app_active, looking, notify
            (Background, false, false, true),
            (Background, true, false, false),
            (Background, false, true, false),
            (Always, true, false, true),
            (Always, false, false, true),
            (Always, true, true, false),
            (Off, false, false, false),
            (Off, true, false, false),
        ];
        for (mode, active, looking, want) in cases {
            assert_eq!(should_notify(mode, active, looking), want, "{mode:?} active={active} looking={looking}");
        }
    }

    #[test]
    fn the_setting_parses_and_stays_out_of_settings_json_by_default() {
        let person: Person = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!((person.system_notifications, person.dock_bounce, person.alert_command.clone()), (SystemNotifications::Background, false, None));
        let saved = serde_json::to_value(&person).unwrap();
        for key in ["system_notifications", "dock_bounce", "alert_command"] {
            assert!(saved.get(key).is_none(), "{key} written at its default");
        }
        for (word, mode) in [("always", SystemNotifications::Always), ("off", SystemNotifications::Off), ("background", SystemNotifications::Background)] {
            let person: Person = serde_json::from_value(serde_json::json!({ "system_notifications": word })).unwrap();
            assert_eq!(person.system_notifications, mode);
        }
        let shell: Person = serde_json::from_value(serde_json::json!({ "alert_command": "notify -s done", "dock_bounce": true })).unwrap();
        assert_eq!(shell.alert_command, Some(AlertCommand::Shell("notify -s done".into())));
        assert!(shell.dock_bounce);
        let argv: Person = serde_json::from_value(serde_json::json!({ "alert_command": ["notify", "-s"] })).unwrap();
        assert_eq!(argv.alert_command, Some(AlertCommand::Argv(vec!["notify".into(), "-s".into()])));
    }

    #[test]
    fn the_notification_says_what_the_toast_says_tagged_by_agent() {
        let n = system_notification(&note(Alert::Blocked), &Look::default());
        assert_eq!((n.tag.as_ref(), n.title.as_ref(), n.body.as_ref()), ("herdr-agent:local:t7", "Claude needs you", "herdr / main"));
        let done = system_notification(&Note { place: None, ..note(Alert::Done) }, &Look::default());
        assert_eq!((done.title.as_ref(), done.body.as_ref()), ("Claude finished", ""));
        assert_eq!(address_of(&n.tag), Some("local:t7"));
        assert_eq!(address_of("herdr-agent:"), None);
        assert_eq!(address_of("other:local:t7"), None);
    }

    #[test]
    fn the_command_gets_argv_and_the_alert_environment() {
        assert_eq!(command_argv(&AlertCommand::Shell("notify -s \"$HERDR_AGENT_STATE\"".into())), Some(vec!["/bin/sh".into(), "-c".into(), "notify -s \"$HERDR_AGENT_STATE\"".into()]));
        assert_eq!(command_argv(&AlertCommand::Argv(vec!["/usr/local/bin/notify".into(), "-q".into()])), Some(vec!["/usr/local/bin/notify".into(), "-q".into()]));
        assert_eq!(command_argv(&AlertCommand::Shell("  ".into())), None);
        assert_eq!(command_argv(&AlertCommand::Argv(Vec::new())), None);
        assert_eq!(command_argv(&AlertCommand::Argv(vec![String::new()])), None);

        let env = command_env(&note(Alert::Blocked));
        let expected: Vec<(&str, String)> = vec![
            ("HERDR_AGENT_STATE", "blocked".into()),
            ("HERDR_AGENT_PREV_STATE", "working".into()),
            ("HERDR_AGENT_APP", "claude-code".into()),
            ("HERDR_AGENT_NAME", "Claude".into()),
            ("HERDR_AGENT_ADDRESS", "local:t7".into()),
            ("HERDR_AGENT_PLACE", "herdr / main".into()),
            ("HERDR_ALERT_TITLE", "Claude needs you".into()),
            ("HERDR_AGENT_MESSAGE", String::new()),
            ("HERDR_AGENT_KIND", String::new()),
            ("HERDR_AGENT_TITLE", String::new()),
        ];
        assert_eq!(env, expected);
        let bare = command_env(&Note { prev: None, app: None, place: None, ..note(Alert::Done) });
        assert_eq!(bare.iter().find(|(k, _)| *k == "HERDR_AGENT_STATE").map(|(_, v)| v.as_str()), Some("done"));
        assert!(bare.iter().all(|(k, _)| *k != "HERDR_AGENT_PREV_STATE"));
    }

    #[test]
    fn the_agents_own_words_reach_the_command_and_the_notification_as_given() {
        let quoted = "approve Bash: echo \"$HOME\" 'x' $(id); `id`";
        let n = Note { said: said(quoted), ..note(Alert::Blocked) };
        let env = command_env(&n);
        let get = |key: &str| env.iter().find(|(k, _)| *k == key).map(|(_, v)| v.as_str());
        assert_eq!(get("HERDR_AGENT_MESSAGE"), Some(quoted));
        assert_eq!(get("HERDR_AGENT_KIND"), Some("permission"));
        assert_eq!(get("HERDR_AGENT_TITLE"), Some("fix the build"));
        assert_eq!(system_notification(&n, &Look::default()).body.as_ref(), format!("{quoted}\nherdr / main"));
        let alone = Note { place: None, said: said("  done: 3 files  "), ..note(Alert::Done) };
        assert_eq!(system_notification(&alone, &Look::default()).body.as_ref(), "done: 3 files");
        let report = serde_json::json!({ "state": "blocked", "kind": "question", "title": "t", "msg": "which one?" });
        assert_eq!(Said::of(Some(&report)), Said { kind: "question".into(), title: "t".into(), message: "which one?".into() });
        assert_eq!(Said::of(Some(&serde_json::json!("blocked"))), Said::default(), "a bare word says nothing more");
        assert_eq!(Said::of(None), Said::default());
    }

    #[test]
    fn an_error_is_told_apart_from_blocked() {
        let n = note(Alert::Error);
        assert_eq!(command_env(&n)[0], ("HERDR_AGENT_STATE", "error".to_owned()));
        assert_eq!(n.title(), "Claude hit an error");
        assert!(Alert::Error.needs_you() && Alert::Blocked.needs_you() && !Alert::Done.needs_you());
    }

    #[test]
    fn a_command_runs_with_the_environment() {
        let out = std::env::temp_dir().join(format!("herdr-alert-command-{}", std::process::id()));
        let _ = std::fs::remove_file(&out);
        let line = format!("printf '%s|%s|%s' \"$HERDR_AGENT_STATE\" \"$HERDR_AGENT_ADDRESS\" \"$HERDR_ALERT_TITLE\" > '{}'", out.display());
        let shell = AlertCommand::Shell(line.clone());
        run(&command_argv(&shell).unwrap(), &line, &note(Alert::Done)).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !out.exists() || std::fs::read_to_string(&out).unwrap().is_empty() {
            assert!(std::time::Instant::now() < deadline, "the command never ran");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "done|local:t7|Claude finished");
        let _ = std::fs::remove_file(&out);
    }

    #[gpui::test]
    fn send_posts_one_notification_per_agent_or_runs_the_command(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| cx.set_app_identity("dev.herdr.test", "Herdr"));
        cx.update(|cx| {
            assert!(matches!(send(&note(Alert::Done), &plain(), cx), Sent::Posted));
            assert!(matches!(send(&note(Alert::Blocked), &plain(), cx), Sent::Posted));
        });
        assert_eq!(cx.delivered_system_notifications(), [system_notification(&note(Alert::Blocked), &Look::default())]);
        let command = |c: AlertCommand| Settings { alert_command: Some(c), ..plain() };
        let ran = cx.update(|cx| send(&note(Alert::Done), &command(AlertCommand::Argv(vec!["/usr/bin/true".into()])), cx));
        let Sent::Ran(failed) = ran else { panic!("{ran:?}") };
        assert!(failed.recv_blocking().is_err(), "a command that succeeds reports nothing");
        assert_eq!(cx.shown_system_notifications().len(), 2);
        // A command that doesn't start: the system notification instead, and why.
        let missing = AlertCommand::Argv(vec!["/nonexistent/herdr-notify".into()]);
        let sent = cx.update(|cx| send(&note(Alert::Done), &command(missing), cx));
        assert!(matches!(&sent, Sent::Fallback(why) if why.contains("/nonexistent/herdr-notify")), "{sent:?}");
        assert_eq!(cx.shown_system_notifications().len(), 3);
        cx.update(|cx| cx.dismiss_system_notification(&tag("local:t7")));
        assert!(cx.delivered_system_notifications().is_empty());
    }

    #[test]
    fn a_shell_line_that_fails_says_why_once_it_exits() {
        let failed = |line: &str| {
            let shell = AlertCommand::Shell(line.into());
            run(&command_argv(&shell).unwrap(), line, &note(Alert::Done)).expect("a shell always starts").recv_blocking()
        };
        let missing = failed("/nonexistent/herdr-notify -s done").expect("reported");
        assert_eq!(missing, "alert_command \"/nonexistent/herdr-notify -s done\" failed: command not found (exit 127)");
        assert_eq!(failed("exit 3").expect("reported"), "alert_command \"exit 3\" exited with code 3");
        assert!(failed("true").is_err(), "success reports nothing");
        assert_eq!(exit_failure("x", Some(126)).as_deref(), Some("alert_command \"x\" failed: cannot run (exit 126)"));
        assert_eq!(exit_failure("x", None), None, "killed by a signal: only logged");
    }

    #[cfg(unix)]
    #[test]
    fn a_hung_alert_command_is_killed_after_its_limit_with_what_it_started() {
        use std::io::BufRead;
        use std::os::unix::process::ExitStatusExt;
        // A shell line, as `alert_command` runs one: the shell waits on a child
        // of its own. The child is a subshell looping on short sleeps, so a stray
        // kill of a `sleep` (pkill, killall) only ends one of them; it stops by
        // itself after 30 s, so a kill that never comes fails the test rather
        // than hanging it. `wait $!`, not a bare `wait`, which returns 0 however
        // its child ended.
        let argv = command_argv(&AlertCommand::Shell("(i=0; while [ $i -lt 30 ]; do sleep 1; i=$((i+1)); done) & echo $!; wait $!".into())).unwrap();
        let mut child = command(&argv, &note(Alert::Done)).stdout(Stdio::piped()).spawn().unwrap();
        let mut line = String::new();
        std::io::BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
        let sleep: libc::pid_t = line.trim().parse().unwrap();
        let started = Instant::now();
        let status = reap(child, Duration::from_millis(300)).expect("reaped");
        // SAFETY: signal 0 only asks whether the process is there.
        let alive = unsafe { libc::kill(sleep, 0) } == 0;
        assert_eq!(
            status.signal(),
            Some(libc::SIGKILL),
            "the limit killed the shell: got {status} after {:?}, its child {sleep} {}",
            started.elapsed(),
            if alive { "not yet reaped" } else { "already gone" },
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        // SAFETY: signal 0 only asks whether the process is there.
        let gone = || unsafe { libc::kill(sleep, 0) } != 0;
        let deadline = Instant::now() + Duration::from_secs(3);
        while !gone() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(gone(), "the shell's child {sleep} outlived it");
        let quick = Command::new("true").spawn().unwrap();
        assert!(reap(quick, Duration::from_secs(5)).unwrap().success());
    }

    fn sounds(v: serde_json::Value) -> Settings {
        let person: Person = serde_json::from_value(serde_json::json!({ "notification_sounds": v })).unwrap();
        Settings::compose(&person, &crate::layout::Look::default(), false)
    }

    #[test]
    fn each_state_plays_its_own_sound() {
        use SystemNotificationSound::*;
        let s = sounds(serde_json::json!({ "done": "Glass", "blocked": "Purr", "error": "Basso" }));
        let look = |alert| Look::of(&note(alert), &Settings { notification_icons: false, ..s.clone() }).sound;
        assert_eq!(look(Alert::Done), Named("Glass".into()));
        assert_eq!(look(Alert::Blocked), Named("Purr".into()));
        assert_eq!(look(Alert::Error), Named("Basso".into()));
        assert_eq!(Look::of(&note(Alert::Done), &plain()).sound, Silent, "silent unless set");
        let n = system_notification(&note(Alert::Error), &Look { sound: Named("Basso".into()), ..Look::default() });
        assert_eq!(n.sound, Named("Basso".into()));
    }

    #[test]
    fn marks_put_the_agents_glyph_on_the_title_and_the_states_on_the_body() {
        let marked = Look { marks: true, ..Look::default() };
        let n = system_notification(&Note { said: said("approve Bash: make"), ..note(Alert::Blocked) }, &marked);
        assert_eq!((n.title.as_ref(), n.body.as_ref()), ("🤖 Claude needs you", "✋ approve Bash: make\nherdr / main"));
        let codex = Note { app: Some("codex".into()), name: "Codex".into(), ..note(Alert::Error) };
        let n = system_notification(&codex, &marked);
        assert_eq!((n.title.as_ref(), n.body.as_ref()), ("🧠 Codex hit an error", "🔴 herdr / main"));
        let bare = system_notification(&Note { place: None, ..note(Alert::Done) }, &marked);
        assert_eq!(bare.body.as_ref(), "✅");
        let plain_look = system_notification(&note(Alert::Done), &Look::default());
        assert_eq!(plain_look.title.as_ref(), "Claude finished", "no marks unless asked");
        // By programs.json's key (github-copilot is copilot's), else the app
        // without -code, -cli or -agent, else the robot.
        let glyphs: Vec<_> = [Some("claude-code"), Some("gemini-cli"), Some("github-copilot"), Some("cursor-agent"), Some("aider"), Some("someone-new"), Some(""), None]
            .into_iter()
            .map(agent_glyph)
            .collect();
        assert_eq!(glyphs, ["🤖", "✨", "🚀", "🖱️", "🛠️", "🤖", "🤖", "🤖"]);
        assert!(AGENT_GLYPHS.iter().all(|(k, _)| agent_glyph(Some(k)) != AGENT_GLYPH_OTHER || *k == "claude"));
    }

    #[test]
    fn the_icon_comes_from_the_program_unless_notification_icons_is_off() {
        assert_eq!(Look::of(&note(Alert::Done), &plain()).icon, None, "notification_icons off");
        let on = Look::of(&note(Alert::Done), &Settings { notification_icons: true, ..plain() });
        assert!(matches!(on.icon, Some(Source::Program(ref shown)) if shown["key"] == "claude"), "{on:?}");
        assert_eq!(system_notification(&note(Alert::Done), &on).image, None, "the image comes later, off the main thread");
    }

    /// The icon's file work happens off the main thread: nothing shows until
    /// the background executor has run, then the notification shows with its
    /// copy. A post or dismissal under the same tag meanwhile wins.
    #[gpui::test]
    fn an_icon_is_made_off_the_main_thread_and_a_newer_post_wins(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| cx.set_app_identity("dev.herdr.test", "Herdr"));
        let png = std::env::temp_dir().join(format!("herdr-alert-icon-{}.png", std::process::id()));
        std::fs::write(&png, b"png").unwrap();
        let notification = |title: &str| SystemNotification { tag: tag("local:t7").into(), title: title.to_owned().into(), ..Default::default() };
        cx.update(|cx| show(notification("first"), Source::file(&png), cx));
        assert!(cx.shown_system_notifications().is_empty(), "posting didn't wait for the file work");
        cx.run_until_parked();
        let shown = cx.shown_system_notifications();
        let image = shown[0].image.clone().expect("a copy of the image");
        assert!(image != png && std::fs::read(&image).unwrap() == b"png", "{image:?}");
        let _ = std::fs::remove_file(&image);

        // A newer one without an image shows at once, and the older one's late image is dropped.
        cx.update(|cx| {
            show(notification("older"), Source::file(&png), cx);
            show(notification("newer"), None, cx);
        });
        cx.run_until_parked();
        let titles: Vec<String> = cx.shown_system_notifications().iter().map(|n| n.title.to_string()).collect();
        assert_eq!(titles, ["first", "newer"]);

        // Dismissed while its image is made: never shown.
        cx.update(|cx| {
            show(notification("dismissed"), Source::file(&png), cx);
            dismiss(&tag("local:t7"), cx);
        });
        cx.run_until_parked();
        assert_eq!(cx.shown_system_notifications().len(), 2);
        let _ = std::fs::remove_file(&png);
    }

    #[test]
    fn notify_arguments_are_checked_and_cleaned() {
        use serde_json::json;
        let n = parse_notify(&json!({ "title": "Build", "body": "line one\r\nline two\u{7}", "subtitle": "ci\nmain", "sound": "Glass", "agent": "local:t3", "id": "build.main", "urgent": true })).unwrap();
        assert_eq!(n, Notify {
            title: "Build".into(),
            body: "line one\nline two".into(),
            subtitle: "ci main".into(),
            sound: SystemNotificationSound::Named("Glass".into()),
            agent: Some("local:t3".into()),
            id: Some("build.main".into()),
            urgent: true,
            icon: None,
        });
        let least = parse_notify(&json!({ "title": "  hi  " })).unwrap();
        assert_eq!((least.title.as_str(), least.sound, least.urgent), ("hi", SystemNotificationSound::Default, false));
        assert_eq!(parse_notify(&json!({ "title": "x", "sound": "none" })).unwrap().sound, SystemNotificationSound::Silent);
        assert_eq!(parse_notify(&json!({ "title": "x", "sound": false })).unwrap().sound, SystemNotificationSound::Silent);
        // Direction overrides and control characters don't pass through.
        assert_eq!(parse_notify(&json!({ "title": "a\u{202E}b\u{1b}[31mc\td" })).unwrap().title, "ab[31mc d");
        // Too long: cut short.
        let long = parse_notify(&json!({ "title": "t".repeat(500), "body": "b".repeat(5000) })).unwrap();
        assert_eq!((long.title.chars().count(), long.body.chars().count()), (NOTIFY_TITLE_MAX, NOTIFY_BODY_MAX));
        assert!(long.title.ends_with('…'));
        let fails = [
            (json!({}), "title is required"),
            (json!({ "title": " \n " }), "title is required"),
            (json!({ "title": 3 }), "title must be a string"),
            (json!({ "title": "x", "id": "a b" }), "id \"a b\""),
            (json!({ "title": "x", "id": "x".repeat(65) }), "id"),
            (json!({ "title": "x", "id": "" }), "id"),
            (json!({ "title": "x", "sound": "../../x" }), "sound"),
            (json!({ "title": "x", "sound": 1 }), "sound"),
            (json!({ "title": "x", "agent": "a\nb" }), "agent"),
            (json!({ "title": "x", "agent": "" }), "agent"),
            (json!({ "title": "x", "urgent": "yes" }), "urgent must be"),
            (json!({ "title": "x", "icon": "relative.png" }), "absolute"),
            (json!({ "title": "x", "icon": "/etc/hosts" }), "must be one of"),
            (json!({ "title": "x", "icon": "/nonexistent/i.png" }), "not a file"),
        ];
        for (args, why) in fails {
            let error = parse_notify(&args).expect_err(&args.to_string());
            assert!(error.starts_with("system.notify: ") && error.contains(why), "{args}: {error}");
        }
        let icon = std::env::temp_dir().join(format!("herdr-notify-icon-{}.png", std::process::id()));
        std::fs::write(&icon, b"png").unwrap();
        assert_eq!(parse_notify(&json!({ "title": "x", "icon": icon })).unwrap().icon, Some(icon.clone()));
        let _ = std::fs::remove_file(&icon);
    }

    #[test]
    fn notify_goes_where_system_notifications_says() {
        use SystemNotifications::*;
        let cases = [
            // mode, app_active, urgent, to
            (Off, false, true, NotifyTo::Nowhere),
            (Background, false, false, NotifyTo::System),
            (Background, true, false, NotifyTo::Toast),
            (Background, true, true, NotifyTo::System),
            (Always, true, false, NotifyTo::System),
        ];
        for (mode, active, urgent, to) in cases {
            assert_eq!(notify_to(mode, active, urgent), to, "{mode:?} active={active} urgent={urgent}");
        }
    }

    #[gpui::test]
    fn a_notify_with_the_same_id_replaces_the_older_one(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| cx.set_app_identity("dev.herdr.test", "Herdr"));
        let post = |title: &str, id: Option<&str>, cx: &mut gpui::TestAppContext| {
            let notify = parse_notify(&serde_json::json!({ "title": title, "id": id })).unwrap();
            let tag = notify_tag(notify.id.as_deref());
            cx.update(|cx| cx.show_system_notification(notify_notification(&notify, &tag)));
            tag
        };
        let first = post("build 1", Some("build"), cx);
        let second = post("build 2", Some("build"), cx);
        assert_eq!(first, second);
        let (a, b) = (post("ping", None, cx), post("ping", None, cx));
        assert_ne!(a, b, "without an id each one stands");
        let delivered: Vec<String> = cx.delivered_system_notifications().iter().map(|n| n.title.to_string()).collect();
        assert_eq!(delivered, ["build 2", "ping", "ping"]);
        assert_eq!(address_of(&first), None, "not an agent alert's tag");
    }

    #[test]
    fn a_notify_click_focuses_its_agent_until_replaced() {
        let mut clicks = NotifyClicks::default();
        clicks.posted("herdr-notify:build", Some("local:t3"));
        clicks.posted("herdr-notify:other", None);
        assert_eq!(clicks.clicked("herdr-notify:other"), None);
        clicks.posted("herdr-notify:deploy", Some("local:t4"));
        clicks.posted("herdr-notify:deploy", None);
        assert_eq!(clicks.clicked("herdr-notify:deploy"), None, "replaced by one without an agent");
        assert_eq!(clicks.clicked("herdr-notify:build").as_deref(), Some("local:t3"));
        assert_eq!(clicks.clicked("herdr-notify:build"), None, "once");
        for n in 0..NOTIFY_CLICKS_KEPT + 5 {
            clicks.posted(&format!("herdr-notify:{n}"), Some("t"));
        }
        assert_eq!(clicks.0.len(), NOTIFY_CLICKS_KEPT);
        assert_eq!(clicks.clicked("herdr-notify:0"), None, "the oldest are forgotten");
    }

    #[test]
    fn notification_sounds_marks_and_icons_parse() {
        use SystemNotificationSound::*;
        let all = |s: Settings| [s.notification_sounds.blocked, s.notification_sounds.error, s.notification_sounds.done];
        assert_eq!(all(Settings::default()), [Silent, Silent, Silent]);
        assert_eq!(all(sounds(serde_json::json!("Glass"))), [Named("Glass".into()), Named("Glass".into()), Named("Glass".into())]);
        assert_eq!(all(sounds(serde_json::json!("default"))), [Default, Default, Default]);
        assert_eq!(all(sounds(serde_json::json!(true))), [Default, Default, Default]);
        assert_eq!(all(sounds(serde_json::json!(false))), [Silent, Silent, Silent]);
        assert_eq!(all(sounds(serde_json::json!("none"))), [Silent, Silent, Silent]);
        assert_eq!(all(sounds(serde_json::json!({ "blocked": "Purr", "done": false }))), [Named("Purr".into()), Silent, Silent]);
        for bad in [serde_json::json!({ "finished": "Glass" }), serde_json::json!("../Glass"), serde_json::json!(3), serde_json::json!({ "done": [] }), serde_json::json!(".hidden")] {
            let s = sounds(bad.clone());
            assert_eq!(all(s.clone()), [Silent, Silent, Silent], "{bad}");
            assert!(s.problems().iter().any(|p| p.starts_with("notification_sounds ") && p.ends_with("notifications stay silent")), "{bad}: {:?}", s.problems());
        }
        assert!(sounds(serde_json::json!("Glass")).problems().iter().all(|p| !p.starts_with("notification_sounds")));

        let person: Person = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!((person.notification_sounds.clone(), person.notification_marks, person.notification_icons), (None, false, true));
        let saved = serde_json::to_value(&person).unwrap();
        for key in ["notification_sounds", "notification_marks", "notification_icons"] {
            assert!(saved.get(key).is_none(), "{key} written at its default");
        }
        let set: Person = serde_json::from_value(serde_json::json!({ "notification_marks": true, "notification_icons": false })).unwrap();
        let s = Settings::compose(&set, &crate::layout::Look::default(), false);
        assert!(s.notification_marks && !s.notification_icons);
        // The composed settings travel as JSON (to mods) and back.
        let back: Settings = serde_json::from_value(serde_json::to_value(sounds(serde_json::json!({ "done": "Glass", "error": "default" }))).unwrap()).unwrap();
        assert_eq!(all(back), [Silent, Default, Named("Glass".into())]);
    }
}
