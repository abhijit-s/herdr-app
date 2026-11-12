//! HERDR: process-wide frame counters, read by the app's bench file
//! (`HERDR_APP_BENCH`). Relaxed atomics, so counting costs nothing that shows.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// Frames sent to the GPU, drawn or not.
pub static PRESENTS: AtomicU64 = AtomicU64::new(0);
/// Presents that turned spinners at or after their step was due.
pub static SPIN_STEPS: AtomicU64 = AtomicU64::new(0);
/// How late those presents were, summed, in microseconds of the spin clock.
pub static SPIN_LATE_US: AtomicU64 = AtomicU64::new(0);
/// The latest of them since the counter was last taken.
pub static SPIN_LATE_MAX_US: AtomicU64 = AtomicU64::new(0);
/// Times a window's display link started (macOS: each start spawns a
/// CoreVideo thread).
pub static LINK_STARTS: AtomicU64 = AtomicU64::new(0);

/// Counts one present; `late_ms` is how late a due spinner step was.
pub fn presented(late_ms: Option<f64>) {
    PRESENTS.fetch_add(1, Relaxed);
    if let Some(late) = late_ms {
        let us = (late.max(0.0) * 1000.0) as u64;
        SPIN_STEPS.fetch_add(1, Relaxed);
        SPIN_LATE_US.fetch_add(us, Relaxed);
        SPIN_LATE_MAX_US.fetch_max(us, Relaxed);
    }
}

/// Counts one display link start.
pub fn link_started() {
    LINK_STARTS.fetch_add(1, Relaxed);
}
