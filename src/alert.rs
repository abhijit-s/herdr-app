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

/// What `send` did.
#[derive(Debug)]
pub enum Sent {
    Posted,
    /// The command started; why it failed comes here if it exits non-zero.
    Ran(async_channel::Receiver<String>),
    /// The command didn't start (why): the system notification went instead.
    Fallback(String),
}

/// Sends the alert: the person's command when they set one, else (or when it
/// doesn't start) the system notification.
pub fn send(note: &Note, command: Option<&AlertCommand>, cx: &App) -> Sent {
    let sent = match command.and_then(|c| Some((c, command_argv(c)?))) {
        Some((c, argv)) => match run(&argv, label(c), note) {
            Ok(failed) => return Sent::Ran(failed),
            Err(error) => Sent::Fallback(error),
        },
        None => Sent::Posted,
    };
    cx.show_system_notification(system_notification(note));
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
            assert!(matches!(send(&note(Alert::Done), None, cx), Sent::Posted));
            assert!(matches!(send(&note(Alert::Blocked), None, cx), Sent::Posted));
        });
        assert_eq!(cx.delivered_system_notifications(), [system_notification(&note(Alert::Blocked))]);
        let ran = cx.update(|cx| send(&note(Alert::Done), Some(&AlertCommand::Argv(vec!["/usr/bin/true".into()])), cx));
        let Sent::Ran(failed) = ran else { panic!("{ran:?}") };
        assert!(failed.recv_blocking().is_err(), "a command that succeeds reports nothing");
        assert_eq!(cx.shown_system_notifications().len(), 2);
        // A command that doesn't start: the system notification instead, and why.
        let missing = AlertCommand::Argv(vec!["/nonexistent/herdr-notify".into()]);
        let sent = cx.update(|cx| send(&note(Alert::Done), Some(&missing), cx));
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
        // A shell line, as `alert_command` runs one: the shell waits on its own child.
        let argv = command_argv(&AlertCommand::Shell("sleep 30 & echo $!; wait".into())).unwrap();
        let mut child = command(&argv, &note(Alert::Done)).stdout(Stdio::piped()).spawn().unwrap();
        let mut line = String::new();
        std::io::BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
        let sleep: libc::pid_t = line.trim().parse().unwrap();
        let started = Instant::now();
        let status = reap(child, Duration::from_millis(300)).expect("reaped");
        assert!(!status.success(), "killed, not finished");
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
}
