//! Optional CLI runtime logging.
//!
//! Enabled with `--debug`/`--verbose` or a non-empty `DSHL_LOG` env var.
//! All progress/flow/process lines funnel through [`crate::progress::log`],
//! which mirrors them to stderr here when enabled, so a terminal run
//! (`cargo r -- --debug`) shows the same timeline the UI shows.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

static ENABLED: AtomicBool = AtomicBool::new(false);
static START: OnceLock<Instant> = OnceLock::new();

pub fn set_enabled(value: bool) {
    ENABLED.store(value, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Print a runtime log line to stderr when debug logging is on.
pub fn emit(message: &str) {
    if !enabled() {
        return;
    }
    let start = START.get_or_init(Instant::now);
    let elapsed = start.elapsed();
    eprintln!("[dshl +{:>7.2}s] {}", elapsed.as_secs_f32(), message);
}
