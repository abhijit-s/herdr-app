//! Alerts outside the window: a system notification, or the person's own
//! `alert_command` in its place. The toast and the sounds stay in window.rs.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use gpui::{App, SystemNotification};

use crate::model::Alert;
use crate::settings::{AlertCommand, SystemNotifications};

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
}

impl Note {
    pub fn title(&self) -> String {
        match self.alert {
            Alert::Blocked => format!("{} needs you", self.name),
            Alert::Done => format!("{} finished", self.name),
        }
    }

    fn state(&self) -> &'static str {
        match self.alert {
            Alert::Blocked => "blocked",
            Alert::Done => "done",
        }
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

pub fn system_notification(note: &Note) -> SystemNotification {
    SystemNotification {
        tag: tag(&note.address).into(),
        title: note.title().into(),
        body: note.place.clone().unwrap_or_default().into(),
        actions: Vec::new(),
    }
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
    ]);
    env
}

/// Sends the alert: the person's command when they set one, else the system
/// notification. Returns whether a system notification was posted, so it can
/// be dismissed later.
pub fn send(note: &Note, command: Option<&AlertCommand>, cx: &App) -> bool {
    match command {
        Some(command) => {
            run(command, note);
            false
        }
        None => {
            cx.show_system_notification(system_notification(note));
            true
        }
    }
}

/// How long an `alert_command` may run before it is killed.
const ALERT_COMMAND_LIMIT: Duration = Duration::from_secs(30);

/// Fire and forget: a thread waits for the program so it leaves no zombie,
/// and the window never waits for it.
fn run(command: &AlertCommand, note: &Note) {
    let Some(argv) = command_argv(command) else { return };
    let spawned = Command::new(&argv[0])
        .args(&argv[1..])
        .envs(command_env(note))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match spawned {
        Ok(child) => {
            std::thread::spawn(move || reap(child, ALERT_COMMAND_LIMIT));
        }
        Err(error) => tracing::warn!("alert_command {:?} did not start: {error}", argv[0]),
    }
}

/// Waits for `child` up to `limit`, then kills it: a hung command must not
/// keep a thread (and the program) forever.
fn reap(mut child: std::process::Child, limit: Duration) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100).min(deadline.saturating_duration_since(Instant::now()))),
            Ok(None) => {
                tracing::warn!("alert_command still running after {}s: killed", limit.as_secs());
                let _ = child.kill();
                return child.wait().ok();
            }
            Err(_) => return None,
        }
    }
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
        }
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
        let n = system_notification(&note(Alert::Blocked));
        assert_eq!((n.tag.as_ref(), n.title.as_ref(), n.body.as_ref()), ("herdr-agent:local:t7", "Claude needs you", "herdr / main"));
        let done = system_notification(&Note { place: None, ..note(Alert::Done) });
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
        ];
        assert_eq!(env, expected);
        let bare = command_env(&Note { prev: None, app: None, place: None, ..note(Alert::Done) });
        assert_eq!(bare.iter().find(|(k, _)| *k == "HERDR_AGENT_STATE").map(|(_, v)| v.as_str()), Some("done"));
        assert!(bare.iter().all(|(k, _)| *k != "HERDR_AGENT_PREV_STATE"));
    }

    #[test]
    fn a_command_runs_with_the_environment() {
        let out = std::env::temp_dir().join(format!("herdr-alert-command-{}", std::process::id()));
        let _ = std::fs::remove_file(&out);
        let line = format!("printf '%s|%s|%s' \"$HERDR_AGENT_STATE\" \"$HERDR_AGENT_ADDRESS\" \"$HERDR_ALERT_TITLE\" > '{}'", out.display());
        run(&AlertCommand::Shell(line), &note(Alert::Done));
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
            assert!(send(&note(Alert::Done), None, cx));
            assert!(send(&note(Alert::Blocked), None, cx));
        });
        assert_eq!(cx.delivered_system_notifications(), [system_notification(&note(Alert::Blocked))]);
        cx.update(|cx| assert!(!send(&note(Alert::Done), Some(&AlertCommand::Argv(vec!["/usr/bin/true".into()])), cx)));
        assert_eq!(cx.shown_system_notifications().len(), 2);
        cx.update(|cx| cx.dismiss_system_notification(&tag("local:t7")));
        assert!(cx.delivered_system_notifications().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_hung_alert_command_is_killed_after_its_limit() {
        let child = Command::new("sleep").arg("30").spawn().unwrap();
        let started = Instant::now();
        let status = reap(child, Duration::from_millis(300)).expect("reaped");
        assert!(!status.success(), "killed, not finished");
        assert!(started.elapsed() < Duration::from_secs(5));
        let quick = Command::new("true").spawn().unwrap();
        assert!(reap(quick, Duration::from_secs(5)).unwrap().success());
    }
}
