//! With HERDR_APP_STARTUP_LOG set, each startup step's first occurrence goes to
//! stderr as `[startup] <ms since main> <step> <terminal or ->`, so a slow start
//! can be timed on the installed app without a special build.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

struct Log {
    start: Instant,
    seen: Mutex<HashSet<(String, &'static str)>>,
}

static LOG: OnceLock<Option<Log>> = OnceLock::new();

fn log() -> Option<&'static Log> {
    LOG.get_or_init(|| {
        std::env::var_os("HERDR_APP_STARTUP_LOG").map(|_| Log { start: Instant::now(), seen: Mutex::new(HashSet::new()) })
    })
    .as_ref()
}

pub fn enabled() -> bool {
    log().is_some()
}

/// Starts the clock; call first thing in `main`.
pub fn begin() {
    mark("-", "main");
}

/// Notes `step` for `who` (a terminal's address, or "-") the first time only.
pub fn mark(who: &str, step: &'static str) {
    let Some(log) = log() else { return };
    if log.seen.lock().unwrap().insert((who.to_owned(), step)) {
        eprintln!("[startup] {:9.1}ms {step:16} {who}", log.start.elapsed().as_secs_f64() * 1000.0);
    }
}
