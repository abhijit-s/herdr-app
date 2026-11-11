//! `herdr-app do <command> [--arg value ... | json]`, `herdr-app ls`, `herdr-app commands`,
//! `herdr-app status` and `herdr-app popup`: drive a running window from a shell
//! (agents, tests) over `$HERDR_APP_HOME/herdr-app.sock`, one JSON line each way.

use crate::mods::{self, Request};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(windows)]
use uds_windows::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::Duration;

fn socket() -> PathBuf {
    mods::app_dir().join("herdr-app.sock")
}

/// Serves the socket for this window. A live socket means another window owns this home.
pub fn listen(tx: async_channel::Sender<Request>) {
    let path = socket();
    if UnixStream::connect(&path).is_ok() {
        tracing::warn!("another Herdr is serving {}; `herdr-app do` goes there", path.display());
        return;
    }
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => return tracing::warn!("cannot serve {}: {e}", path.display()),
    };
    let _ = std::thread::Builder::new().name("herdr-app-control".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            let tx = tx.clone();
            std::thread::spawn(move || serve(stream, tx));
        }
    });
}

/// How long a request may take the window to answer. `terminal.popup` starts
/// its terminal on the UI thread, and that call alone may take the core's 10 s.
fn answer_within(cmd: &Value) -> Duration {
    if cmd.get("shot").is_some() {
        Duration::from_secs(30)
    } else if cmd.get("do").and_then(Value::as_str) == Some("terminal.popup") {
        POPUP_ANSWER_WITHIN
    } else {
        Duration::from_secs(5)
    }
}

const POPUP_ANSWER_WITHIN: Duration = Duration::from_secs(15);

fn serve(stream: UnixStream, tx: async_channel::Sender<Request>) {
    let mut line = String::new();
    if BufReader::new(&stream).read_line(&mut line).is_err() {
        return;
    }
    let reply = match serde_json::from_str::<Value>(&line) {
        Ok(cmd) => {
            // `{"wait": terminal}` answers when that terminal ends, however long it runs.
            let deadline = cmd.get("wait").is_none().then(|| std::time::Instant::now() + answer_within(&cmd));
            let (reply_tx, reply_rx) = std::sync::mpsc::channel();
            let _ = tx.send_blocking(Request::Control { cmd, reply: reply_tx });
            match await_reply(&stream, &reply_rx, deadline) {
                Awaited::Answer(reply) => reply,
                // Dropping `reply_rx` tells the window nobody took the answer.
                Awaited::HungUp => return,
                Awaited::TooLate => json!({ "error": "the window did not answer" }),
            }
        }
        Err(e) => json!({ "error": format!("bad request: {e}") }),
    };
    let _ = (&stream).write_all(format!("{reply}\n").as_bytes());
}

enum Awaited {
    Answer(Value),
    HungUp,
    TooLate,
}

/// The window's answer, unless the client hangs up (Ctrl-C) or the deadline passes first.
fn await_reply(stream: &UnixStream, reply: &std::sync::mpsc::Receiver<Value>, deadline: Option<std::time::Instant>) -> Awaited {
    use std::sync::mpsc::RecvTimeoutError;
    loop {
        let slice = deadline.map_or(HANG_UP_CHECK, |d| d.saturating_duration_since(std::time::Instant::now()).min(HANG_UP_CHECK));
        match reply.recv_timeout(slice) {
            Ok(answer) => return Awaited::Answer(answer),
            Err(RecvTimeoutError::Disconnected) => return Awaited::TooLate,
            Err(RecvTimeoutError::Timeout) if hung_up(stream) => return Awaited::HungUp,
            Err(RecvTimeoutError::Timeout) if deadline.is_some_and(|d| std::time::Instant::now() >= d) => return Awaited::TooLate,
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

const HANG_UP_CHECK: Duration = Duration::from_millis(500);

/// The client closed its socket. One that only shut its writing half
/// (`printf … | nc -U`) is still reading: POLLHUP comes only once nothing
/// can be written to it (macOS reports it only with POLLOUT asked for).
#[cfg(unix)]
fn hung_up(stream: &UnixStream) -> bool {
    use std::os::fd::AsRawFd;
    let mut fd = libc::pollfd { fd: stream.as_raw_fd(), events: libc::POLLOUT, revents: 0 };
    // SAFETY: one valid pollfd, and a zero timeout: it never blocks.
    let ready = unsafe { libc::poll(&mut fd, 1, 0) };
    ready > 0 && fd.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0
}

/// A client sends one line, then only reads: anything readable now is its end of the stream.
#[cfg(windows)]
fn hung_up(stream: &UnixStream) -> bool {
    use std::io::{ErrorKind, Read};
    if stream.set_nonblocking(true).is_err() {
        return false;
    }
    let mut reader = stream;
    let gone = match reader.read(&mut [0u8; 1]) {
        Ok(n) => n == 0,
        Err(e) => !matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted),
    };
    let _ = stream.set_nonblocking(false);
    gone
}

/// One Herdr per home: a second launch brings the running window to the
/// front and quits. True when one is running (even one too busy to answer:
/// a second window would ring every alert twice).
pub fn raise_running() -> bool {
    raise_running_at(&socket())
}

fn raise_running_at(path: &std::path::Path) -> bool {
    let Ok(mut stream) = UnixStream::connect(path) else { return false };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    if stream.write_all(format!("{}\n", json!({ "raise": true })).as_bytes()).is_ok() {
        let _ = BufReader::new(&stream).read_line(&mut String::new());
    }
    true
}

/// The client side; returns the process exit code.
pub fn client(args: &[String]) -> i32 {
    if args == ["commands"] {
        println!("{}", serde_json::to_string_pretty(&crate::commands::listing()).unwrap());
        return 0;
    }
    let cmd = match args {
        [verb, rest @ ..] if verb == "shot" && rest.len() <= 1 => {
            let path = PathBuf::from(rest.first().map(String::as_str).unwrap_or("herdr-shot.png"));
            let path = match std::path::absolute(path) {
                Ok(path) => path,
                Err(e) => return fail(&format!("cannot resolve screenshot path: {e}")),
            };
            if path.extension().is_none_or(|ext| !ext.eq_ignore_ascii_case("png")) {
                return fail("screenshot path must end in .png");
            }
            json!({ "shot": path })
        },
        [verb] if verb == "status" => json!({ "status": true }),
        [verb] if verb == "ls" => json!({ "ls": true }),
        [verb, message] if verb == "feedback" => json!({ "do": "app.feedback", "args": { "message": message, "from": "agent" } }),
        [verb, command, rest @ ..] if verb == "do" => match parse_args(rest) {
            Ok(a) => json!({ "do": command, "args": a }),
            Err(e) => return fail(&e),
        },
        _ => return fail("usage: herdr-app ls | herdr-app commands | herdr-app status | herdr-app shot [file.png] | herdr-app do <command> [--arg value ... | json]"),
    };
    let mut stream = match UnixStream::connect(socket()) {
        Ok(s) => s,
        Err(_) if args.first().is_some_and(|v| v == "feedback") => {
            let body = crate::feedback::draft(&args[1], "", 0, "agent", "other");
            println!("{}", serde_json::to_string_pretty(&body).unwrap_or_default());
            eprintln!("Herdr must be open to review and send this feedback. Nothing was sent; core version and layout will reflect the open window.");
            return 1;
        }
        Err(e) => return fail(&format!("no Herdr window at {}: {e}", socket().display())),
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(35)));
    let mut line = String::new();
    if stream.write_all(format!("{cmd}\n").as_bytes()).is_err() || BufReader::new(&stream).read_line(&mut line).is_err() {
        return fail("the window hung up");
    }
    let reply: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
    if args.first().is_some_and(|v| v == "shot") {
        if let Some(error) = reply.get("error") {
            return fail(error.as_str().unwrap_or("screenshot failed"));
        }
        let Some(path) = reply.get("path").and_then(Value::as_str) else {
            return fail("the window returned no screenshot path");
        };
        println!("{path}");
    } else {
        println!("{}", serde_json::to_string_pretty(&reply).unwrap_or_default());
    }
    if reply.get("error").is_some() { 1 } else { 0 }
}

const POPUP_USAGE: &str = "\
Usage: herdr-app popup [--cwd DIR] [--hold] [--wait] [--size WxH] [--] <command> [args...]

Runs a command on this computer in a popup over the shown tab (terminal.popup).
The popup closes when the command ends; closing it (✕, Esc) ends the command.

  --cwd DIR    start in DIR (default: here)
  --hold       keep the popup up after the command ends, until a key
  --wait       wait for the command to end and exit with its exit code
  --size WxH   the popup's size: each side a fraction of the window up to 1
               (0.6x0.5), or px from 50 (900x600); 1.5 is neither
  --           ends the options, for a command with its own flags

Exits 2 for bad usage, 1 when the window is not open or the popup failed.
";

/// Sides up to 1 are fractions of the window, larger ones px: between 1 and
/// this, a side is more likely a mistyped fraction (1.5) than a tiny popup.
const MIN_POPUP_PX: f64 = 50.0;

#[derive(Debug, PartialEq)]
struct Popup {
    argv: Vec<String>,
    cwd: Option<String>,
    hold: bool,
    wait: bool,
    size: Option<(f64, f64)>,
}

/// `herdr-app popup`'s arguments; `Ok(None)` asks for the usage.
fn parse_popup(args: &[String]) -> Result<Option<Popup>, String> {
    let mut p = Popup { argv: Vec::new(), cwd: None, hold: false, wait: false, size: None };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--hold" => p.hold = true,
            "--wait" => p.wait = true,
            "--cwd" => p.cwd = Some(it.next().ok_or("--cwd needs a folder")?.clone()),
            "--size" => {
                let value = it.next().ok_or("--size needs WxH")?;
                let side = |n: &str| n.parse::<f64>().ok().filter(|n| *n > 0.0 && (*n <= 1.0 || *n >= MIN_POPUP_PX));
                let size = value.split_once('x').and_then(|(w, h)| Some((side(w)?, side(h)?)));
                p.size = Some(size.ok_or_else(|| format!("--size is WxH, each a fraction up to 1 or px from {MIN_POPUP_PX}, not {value:?}"))?);
            }
            "--" => {
                p.argv = it.cloned().collect();
                break;
            }
            option if option.starts_with('-') => return Err(format!("unknown option {option}")),
            _ => {
                p.argv = std::iter::once(arg).chain(it).cloned().collect();
                break;
            }
        }
    }
    if p.argv.is_empty() {
        return Err("no command to run".into());
    }
    Ok(Some(p))
}

/// `herdr-app popup`: the process exit code (the command's, with `--wait`).
pub fn popup(args: &[String]) -> i32 {
    let p = match parse_popup(args) {
        Ok(Some(p)) => p,
        Ok(None) => {
            print!("{POPUP_USAGE}");
            return 0;
        }
        Err(e) => {
            eprint!("herdr-app popup: {e}\n\n{POPUP_USAGE}");
            return 2;
        }
    };
    let cwd = p.cwd.or_else(|| std::env::current_dir().ok().map(|d| d.display().to_string()));
    let mut args = json!({ "argv": p.argv, "cwd": cwd, "hold": p.hold, "wait": p.wait });
    if let Some((width, height)) = p.size {
        (args["width"], args["height"]) = (json!(width), json!(height));
    }
    // Just past the server's limit, so its "did not answer" arrives first.
    let shown = match ask(&json!({ "do": "terminal.popup", "args": args }), Some(POPUP_ANSWER_WITHIN + Duration::from_secs(2))) {
        Ok(reply) if reply.get("error").is_none() => reply,
        Ok(reply) => return popup_failed(reply["error"].as_str().unwrap_or("the popup failed")),
        Err(e) => return popup_failed(&e),
    };
    if !p.wait {
        return 0;
    }
    let Some(terminal) = shown["terminal"].as_str() else { return popup_failed("the window named no terminal") };
    match ask(&json!({ "wait": terminal }), None) {
        Ok(reply) if reply.get("error").is_none() => exit_status(&reply),
        Ok(reply) => popup_failed(reply["error"].as_str().unwrap_or("waiting failed")),
        Err(e) => popup_failed(&e),
    }
}

/// A shell's `$?`: the exit code, else 128 + the signal that killed it, else 1.
fn exit_status(ended: &Value) -> i32 {
    let code = ended["exit_code"].as_i64().or_else(|| ended["signal"].as_i64().map(|s| 128 + s));
    code.and_then(|c| i32::try_from(c).ok()).unwrap_or(1)
}

fn popup_failed(message: &str) -> i32 {
    eprintln!("herdr-app popup: {message}");
    1
}

/// One request to the window and its reply; `None` waits as long as it takes.
fn ask(cmd: &Value, timeout: Option<Duration>) -> Result<Value, String> {
    let mut stream = UnixStream::connect(socket()).map_err(|e| format!("no Herdr window at {}: {e}", socket().display()))?;
    let _ = stream.set_read_timeout(timeout);
    let mut line = String::new();
    if stream.write_all(format!("{cmd}\n").as_bytes()).is_err() || BufReader::new(&stream).read_line(&mut line).map_or(true, |n| n == 0) {
        return Err("the window hung up".into());
    }
    serde_json::from_str(&line).map_err(|e| format!("bad reply from the window: {e}"))
}

/// `--name api --argv '["pi"]'` (values that parse as JSON are JSON), or one JSON object.
fn parse_args(rest: &[String]) -> Result<Value, String> {
    match rest {
        [] => Ok(Value::Null),
        [raw] if raw.trim_start().starts_with('{') => serde_json::from_str(raw).map_err(|e| format!("args are not JSON: {e}")),
        _ => {
            let mut out = serde_json::Map::new();
            let mut it = rest.iter();
            while let Some(flag) = it.next() {
                let key = flag.strip_prefix("--").ok_or_else(|| format!("expected --<arg>, got {flag:?}"))?;
                let value = it.next().ok_or_else(|| format!("--{key} needs a value"))?;
                let v = serde_json::from_str::<Value>(value).ok().filter(|v| !v.is_string()).unwrap_or_else(|| Value::String(value.clone()));
                out.insert(key.to_owned(), v);
            }
            Ok(Value::Object(out))
        }
    }
}

/// Encode before opening the destination: GPU errors must not truncate a good shot.
pub fn save_shot(window: &gpui::Window, path: &std::path::Path) -> anyhow::Result<()> {
    let image = window.render_to_image()?;
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, image.width(), image.height());
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
        encoder.write_header()?.write_image_data(image.as_raw())?;
    }
    // A sibling temporary file keeps publication on the same filesystem.
    let temporary = path.with_file_name(format!(".herdr-shot-{}.png", std::process::id()));
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temporary)?;
    let result = (|| -> anyhow::Result<()> {
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn fail(message: &str) -> i32 {
    eprintln!("herdr-app: {message}");
    2
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn popup_reads_its_options_then_the_command() {
        let parse = |args: &[&str]| parse_popup(&args.iter().map(|a| a.to_string()).collect::<Vec<_>>());
        let popup = |argv: &[&str]| Popup { argv: argv.iter().map(|a| a.to_string()).collect(), cwd: None, hold: false, wait: false, size: None };
        assert_eq!(parse(&["htop"]), Ok(Some(popup(&["htop"]))));
        // The command's own flags are its own, after the command or after --.
        assert_eq!(parse(&["git", "log", "--oneline"]), Ok(Some(popup(&["git", "log", "--oneline"]))));
        assert_eq!(parse(&["--", "-x", "--wait"]), Ok(Some(popup(&["-x", "--wait"]))));
        assert_eq!(
            parse(&["--cwd", "/tmp", "--hold", "--wait", "--size", "900x0.5", "make", "test"]),
            Ok(Some(Popup { cwd: Some("/tmp".into()), hold: true, wait: true, size: Some((900.0, 0.5)), ..popup(&["make", "test"]) }))
        );
        let size = |value: &str| parse(&["--size", value, "x"]).map(|p| p.unwrap().size);
        assert_eq!(size("1x50"), Ok(Some((1.0, 50.0))), "the whole width, 50 px tall");
        assert_eq!(parse(&["-h"]), Ok(None));
        assert_eq!(parse(&["--wait", "--help", "x"]), Ok(None));
        for bad in [&[][..], &["--wait"], &["--"], &["--nope", "x"], &["--cwd"], &["--size", "80", "x"], &["--size", "0x1", "x"], &["--size", "ax2", "x"], &["--size", "1.5x0.5", "x"], &["--size", "0.5x49", "x"]] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_waited_popup_exits_as_a_shell_would() {
        assert_eq!(exit_status(&json!({ "terminal": "t1", "exit_code": 7, "signal": null })), 7);
        assert_eq!(exit_status(&json!({ "terminal": "t1", "exit_code": null, "signal": 15 })), 143, "SIGTERM");
        assert_eq!(exit_status(&json!({ "terminal": "t1", "exit_code": null, "signal": null })), 1, "removed before it ended");
    }

    /// Ctrl-C on `herdr-app popup --wait` hangs up: the server stops waiting
    /// and drops the reply channel, which tells the window nobody will take it.
    #[test]
    fn a_client_that_hangs_up_stops_the_wait() {
        let (client, server) = UnixStream::pair().unwrap();
        let (tx, rx) = async_channel::unbounded();
        let served = std::thread::spawn(move || serve(server, tx));
        (&client).write_all(b"{\"wait\":\"t9\"}\n").unwrap();
        let Ok(Request::Control { reply, .. }) = rx.recv_blocking() else { panic!("no control request") };
        drop(client);
        let started = std::time::Instant::now();
        served.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(3), "noticed within a check or two");
        assert!(reply.send(json!({})).is_err(), "nobody takes the answer");
    }

    /// `printf … | nc -U` shuts its writing half after the line and keeps
    /// reading: that is no hang-up, and the answer still reaches it.
    #[cfg(unix)]
    #[test]
    fn a_client_that_only_stops_writing_still_gets_its_answer() {
        let (client, server) = UnixStream::pair().unwrap();
        (&client).write_all(b"{}\n").unwrap();
        client.shutdown(std::net::Shutdown::Write).unwrap();
        assert!(!hung_up(&server), "half-closed");
        let (tx, rx) = async_channel::unbounded();
        let served = std::thread::spawn(move || serve(server, tx));
        let Ok(Request::Control { reply, .. }) = rx.recv_blocking() else { panic!("no control request") };
        // Slower than a hang-up check or two.
        std::thread::sleep(HANG_UP_CHECK * 3);
        reply.send(json!({ "ok": true })).expect("still waited for");
        let mut line = String::new();
        BufReader::new(&client).read_line(&mut line).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&line).unwrap(), json!({ "ok": true }));
        served.join().unwrap();
        let (open, other) = UnixStream::pair().unwrap();
        assert!(!hung_up(&open));
        drop(other);
        assert!(hung_up(&open), "closed");
    }

    #[test]
    fn the_server_waits_longer_for_a_popup_than_the_core_may_take() {
        assert!(answer_within(&json!({ "do": "terminal.popup", "args": {} })) > Duration::from_secs(10), "Core::call's limit");
        assert_eq!(answer_within(&json!({ "do": "tab.new" })), Duration::from_secs(5));
    }

    #[test]
    fn a_second_launch_raises_the_running_window() {
        let dir = std::env::temp_dir().join(format!("herdr-app-raise-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("herdr-app.sock");
        // Nothing running: this launch opens its own window.
        assert!(!raise_running_at(&path));
        // One running: it gets asked to come to the front.
        let listener = UnixListener::bind(&path).unwrap();
        let asked = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).unwrap();
            (&stream).write_all(b"{\"ok\":true}\n").unwrap();
            serde_json::from_str::<Value>(&line).unwrap()
        });
        assert!(raise_running_at(&path));
        assert_eq!(asked.join().unwrap(), json!({ "raise": true }));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
