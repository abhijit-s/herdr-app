//! System notifications mods post (`systemNotify`). Each carries the posting
//! mod's name in its tag, so a click goes back to that mod, also a click on one
//! from an earlier run (it can start Herdr): that one is held until a mod with
//! that name loads.
//!
//! Fork: an agent notification's look (`systemNotify({ agent })`: the person's
//! notification sounds, marks and program icons), the toast titles, and
//! `system.notify` for programs.

use std::cell::RefCell;
use std::path::PathBuf;

use gpui::{App, SystemNotification, SystemNotificationSound};
use serde_json::Value;

use crate::model::Alert;
use crate::notification_icon::Source;
use crate::settings::{Settings, SystemNotifications};

const PREFIX: &str = "herdr-mod/";

/// The tag the system sees: a mod's own tag, scoped to the mod, so two mods
/// never replace each other's notifications. Mod names are folder names, so
/// they hold no `/`.
pub fn system_tag(mod_name: &str, tag: &str) -> String {
    format!("{PREFIX}{mod_name}/{tag}")
}

fn parse(system_tag: &str) -> Option<(&str, &str)> {
    let (name, tag) = system_tag.strip_prefix(PREFIX)?.split_once('/')?;
    (!name.is_empty()).then_some((name, tag))
}

pub fn notification(mod_name: &str, tag: &str, title: String, body: String) -> SystemNotification {
    SystemNotification { tag: system_tag(mod_name, tag).into(), title: title.into(), body: body.into(), actions: Vec::new(), ..Default::default() }
}

/// What Settings > Alerts sends on request: the person is there when macOS
/// asks for permission, so no mod's notification is the one lost to the
/// question.
pub fn test_notification() -> SystemNotification {
    SystemNotification {
        tag: "herdr-test".into(),
        title: "Herdr can reach you here".into(),
        body: "Mods that post system notifications show like this.".into(),
        actions: Vec::new(),
        ..Default::default()
    }
}

thread_local! {
    /// Where each loaded copy's clicks go: (mod, sender).
    static LISTENERS: RefCell<Vec<(String, async_channel::Sender<String>)>> = const { RefCell::new(Vec::new()) };
    /// Clicks for a mod not loaded yet: (mod, tag).
    static HELD: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
}

/// Clicks on the notifications `mod_name` posted, for one loaded copy. Clicks
/// held for that mod go to it now.
pub fn listen(mod_name: &str) -> async_channel::Receiver<String> {
    let (tx, rx) = async_channel::unbounded();
    HELD.with_borrow_mut(|held| {
        held.retain(|(name, tag)| name != mod_name || tx.try_send(tag.clone()).is_err());
    });
    LISTENERS.with_borrow_mut(|listeners| {
        listeners.retain(|(_, tx)| !tx.is_closed());
        listeners.push((mod_name.to_owned(), tx));
    });
    rx
}

/// Delivers a click to every loaded copy of the mod that posted it, or holds
/// it until one loads. Returns whether a mod posted it.
pub fn clicked(system_tag: &str) -> bool {
    let Some((name, tag)) = parse(system_tag) else { return false };
    let delivered = LISTENERS.with_borrow_mut(|listeners| {
        listeners.retain(|(_, tx)| !tx.is_closed());
        let mut any = false;
        for (n, tx) in listeners.iter() {
            if n == name {
                any |= tx.try_send(tag.to_owned()).is_ok();
            }
        }
        any
    });
    if !delivered {
        HELD.with_borrow_mut(|held| held.push((name.to_owned(), tag.to_owned())));
    }
    true
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

/// Fork: `systemNotify({ agent: { app, state } })`, a notification about an
/// agent, which sounds and looks as the person's notification settings say for
/// that state.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentLook {
    /// What the agent reported as its `app`, for its glyph and program icon.
    pub app: Option<String>,
    pub alert: Alert,
}

impl AgentLook {
    /// From `systemNotify`'s `agent`; absent or null is none.
    pub fn of(agent: Option<&Value>) -> Result<Option<AgentLook>, String> {
        let Some(agent) = agent.filter(|a| !a.is_null()) else { return Ok(None) };
        let alert = match agent.get("state").and_then(Value::as_str) {
            Some("blocked") => Alert::Blocked,
            Some("error") => Alert::Error,
            Some("done") => Alert::Done,
            _ => return Err("systemNotify: agent.state must be \"blocked\", \"error\" or \"done\"".into()),
        };
        let app = agent.get("app").and_then(Value::as_str).filter(|a| !a.is_empty()).map(str::to_owned);
        Ok(Some(AgentLook { app, alert }))
    }
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
    /// With `notification_icons`, its program's icon (notification_icon.rs).
    pub fn of(agent: &AgentLook, settings: &Settings) -> Look {
        let sounds = &settings.notification_sounds;
        let sound = match agent.alert {
            Alert::Blocked => &sounds.blocked,
            Alert::Error => &sounds.error,
            Alert::Done => &sounds.done,
        };
        let icon = if settings.notification_icons { agent.app.as_deref().and_then(Source::program) } else { None };
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

/// Gives a mod's notification about `agent` its sound and marks, and returns
/// the program icon to attach (`show`).
pub fn dress(notification: &mut SystemNotification, agent: &AgentLook, look: Look) -> Option<Source> {
    if look.marks {
        notification.title = format!("{} {}", agent_glyph(agent.app.as_deref()), notification.title).into();
        let mark = state_mark(agent.alert);
        notification.body = if notification.body.is_empty() { mark.into() } else { format!("{mark} {}", notification.body).into() };
    }
    notification.sound = look.sound;
    look.icon
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
    tracing::debug!(tag, title = %notification.title, "system notification posted");
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
    tracing::debug!(tag, "system notification withdrawn");
    WAITING.with(|w| w.borrow_mut().remove(tag));
    cx.dismiss_system_notification(tag);
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

    #[test]
    fn a_tag_names_the_posting_mod() {
        assert_eq!(system_tag("toast", "agent:t1"), "herdr-mod/toast/agent:t1");
        assert_eq!(parse("herdr-mod/toast/agent:t1"), Some(("toast", "agent:t1")));
        assert_eq!(parse("herdr-mod/toast/a/b"), Some(("toast", "a/b")));
        assert_eq!(parse("herdr-mod/toast/"), Some(("toast", "")));
        assert_eq!(parse("herdr-mod//x"), None);
        assert_eq!(parse("herdr-test"), None);
    }

    #[test]
    fn a_click_goes_to_the_mod_that_posted_it() {
        std::thread::spawn(|| {
            let toast = listen("toast");
            let other = listen("tower");
            assert!(clicked(&system_tag("toast", "t1")));
            assert_eq!(toast.try_recv().unwrap(), "t1");
            assert!(other.is_empty());
            assert!(!clicked("herdr-test"), "not a mod's");
            assert!(toast.is_empty() && other.is_empty());
        })
        .join()
        .unwrap();
    }

    #[test]
    fn a_click_from_an_earlier_run_waits_for_its_mod() {
        std::thread::spawn(|| {
            // Clicked in Notification Center, which started Herdr.
            assert!(clicked(&system_tag("toast", "t2")));
            let other = listen("tower");
            assert!(other.is_empty());
            let toast = listen("toast");
            assert_eq!(toast.try_recv().unwrap(), "t2");
            // Delivered once: a later copy doesn't get it again.
            assert!(listen("toast").is_empty());
            // A copy that unloaded doesn't take a click from the next one.
            drop(toast);
            let next = listen("toast");
            assert!(clicked(&system_tag("toast", "t3")));
            assert_eq!(next.try_recv().unwrap(), "t3");
        })
        .join()
        .unwrap();
    }

    fn claude(alert: Alert) -> AgentLook {
        AgentLook { app: Some("claude-code".into()), alert }
    }

    /// The settings with no program icons (their files are the person's).
    fn plain() -> Settings {
        Settings { notification_icons: false, ..Settings::default() }
    }

    fn sounds(v: serde_json::Value) -> Settings {
        let person: Person = serde_json::from_value(serde_json::json!({ "notification_sounds": v })).unwrap();
        Settings::compose(&person, &crate::layout::Look::default(), false)
    }

    #[test]
    fn an_agent_look_is_read_from_systemnotify() {
        use serde_json::json;
        assert_eq!(AgentLook::of(None), Ok(None));
        assert_eq!(AgentLook::of(Some(&Value::Null)), Ok(None));
        assert_eq!(AgentLook::of(Some(&json!({ "app": "claude-code", "state": "error" }))), Ok(Some(claude(Alert::Error))));
        assert_eq!(AgentLook::of(Some(&json!({ "app": "", "state": "done" }))), Ok(Some(AgentLook { app: None, alert: Alert::Done })));
        for bad in [json!({}), json!({ "state": "working" }), json!("blocked")] {
            assert!(AgentLook::of(Some(&bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn each_state_plays_its_own_sound() {
        use SystemNotificationSound::*;
        let s = Settings { notification_icons: false, ..sounds(serde_json::json!({ "done": "Glass", "blocked": "Purr", "error": "Basso" })) };
        let look = |alert| Look::of(&claude(alert), &s).sound;
        assert_eq!(look(Alert::Done), Named("Glass".into()));
        assert_eq!(look(Alert::Blocked), Named("Purr".into()));
        assert_eq!(look(Alert::Error), Named("Basso".into()));
        assert_eq!(Look::of(&claude(Alert::Done), &plain()).sound, Silent, "silent unless set");
        let mut n = notification("notifications", "agent:t7", "Claude hit an error".into(), String::new());
        dress(&mut n, &claude(Alert::Error), Look { sound: Named("Basso".into()), ..Look::default() });
        assert_eq!(n.sound, Named("Basso".into()));
    }

    #[test]
    fn marks_put_the_agents_glyph_on_the_title_and_the_states_on_the_body() {
        let marked = || Look { marks: true, ..Look::default() };
        let mut n = notification("notifications", "agent:t7", "Claude needs you".into(), "approve Bash: make\nherdr / main".into());
        dress(&mut n, &claude(Alert::Blocked), marked());
        assert_eq!((n.title.as_ref(), n.body.as_ref()), ("🤖 Claude needs you", "✋ approve Bash: make\nherdr / main"));
        let mut n = notification("notifications", "agent:t8", "Codex hit an error".into(), "herdr / main".into());
        dress(&mut n, &AgentLook { app: Some("codex".into()), alert: Alert::Error }, marked());
        assert_eq!((n.title.as_ref(), n.body.as_ref()), ("🧠 Codex hit an error", "🔴 herdr / main"));
        let mut bare = notification("notifications", "agent:t7", "Claude finished".into(), String::new());
        dress(&mut bare, &claude(Alert::Done), marked());
        assert_eq!(bare.body.as_ref(), "✅");
        let mut unmarked = notification("notifications", "agent:t7", "Claude finished".into(), String::new());
        dress(&mut unmarked, &claude(Alert::Done), Look::default());
        assert_eq!((unmarked.title.as_ref(), unmarked.body.as_ref()), ("Claude finished", ""), "no marks unless asked");
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
        assert_eq!(Look::of(&claude(Alert::Done), &plain()).icon, None, "notification_icons off");
        let on = Look::of(&claude(Alert::Done), &Settings { notification_icons: true, ..plain() });
        assert!(matches!(on.icon, Some(Source::Program(ref shown)) if shown["key"] == "claude"), "{on:?}");
        let mut n = notification("notifications", "agent:t7", "Claude finished".into(), String::new());
        assert!(dress(&mut n, &claude(Alert::Done), on).is_some());
        assert_eq!(n.image, None, "the image comes later, off the main thread");
    }

    /// The icon's file work happens off the main thread: nothing shows until
    /// the background executor has run, then the notification shows with its
    /// copy. A post or dismissal under the same tag meanwhile wins.
    #[gpui::test]
    fn an_icon_is_made_off_the_main_thread_and_a_newer_post_wins(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| cx.set_app_identity("dev.herdr.test", "Herdr"));
        let png = std::env::temp_dir().join(format!("herdr-alert-icon-{}.png", std::process::id()));
        std::fs::write(&png, b"png").unwrap();
        let tag = system_tag("notifications", "agent:t7");
        let notification = |title: &str| SystemNotification { tag: tag.clone().into(), title: title.to_owned().into(), ..Default::default() };
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
            dismiss(&tag, cx);
        });
        cx.run_until_parked();
        assert_eq!(cx.shown_system_notifications().len(), 2);
        let _ = std::fs::remove_file(&png);
    }

    /// The `notifications` mod reads these through `settings()`; keys the app
    /// no longer reads (`alert_command`) are ignored, not refused.
    #[test]
    fn the_notification_settings_parse_and_stay_out_of_settings_json_by_default() {
        let person: Person = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!((person.system_notifications, person.dock_bounce, person.notification_sticky), (SystemNotifications::Background, false, true));
        let saved = serde_json::to_value(&person).unwrap();
        for key in ["system_notifications", "dock_bounce", "notification_sticky"] {
            assert!(saved.get(key).is_none(), "{key} written at its default");
        }
        for (word, mode) in [("always", SystemNotifications::Always), ("off", SystemNotifications::Off), ("background", SystemNotifications::Background)] {
            let person: Person = serde_json::from_value(serde_json::json!({ "system_notifications": word })).unwrap();
            assert_eq!(person.system_notifications, mode);
        }
        let old: Person = serde_json::from_value(serde_json::json!({ "alert_command": "notify -s done", "dock_bounce": true, "notification_sticky": false })).unwrap();
        assert!(old.dock_bounce && !old.notification_sticky);
        let mods = Settings::compose(&old, &crate::layout::Look::default(), false).for_mods();
        assert_eq!((mods["system_notifications"].clone(), mods["notification_sticky"].clone()), (serde_json::json!("background"), serde_json::json!(false)));
    }

    #[test]
    fn notification_sticky_is_on_unless_set_off() {
        let person: Person = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(person.notification_sticky && Settings::default().notification_sticky);
        assert!(serde_json::to_value(&person).unwrap().get("notification_sticky").is_none(), "written at its default");
        let off: Person = serde_json::from_value(serde_json::json!({ "notification_sticky": false })).unwrap();
        assert!(!Settings::compose(&off, &crate::layout::Look::default(), false).notification_sticky);
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
        assert_eq!(parse(&first), None, "not a mod's tag");
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
