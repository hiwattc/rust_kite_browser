//! Diagnostics for when Kite misbehaves where no console is attached (installed Windows builds):
//!
//! - a log file next to the app data (`kite.log`; the run before is kept as `kite.prev.log`);
//! - a main-thread watchdog: another thread asks the main thread to answer every second; when it
//!   has not for a few seconds, the log gets what the main thread did last (`mark`) and how many
//!   window events arrived per second (an event loop feeding itself shows up as a storm).

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::Duration;

use tauri::AppHandle;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const KEEP_MARKS: usize = 40;
const STALL_MS: i64 = 4000;

static MARKS: Mutex<VecDeque<(i64, String)>> = Mutex::new(VecDeque::new());
static PONG: AtomicI64 = AtomicI64::new(0);
static EVENTS: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Same folder as Tauri's app data dir (`%APPDATA%\com.kite.browser`, `~/Library/Application
/// Support/com.kite.browser`); known before the app is built.
pub fn log_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("com.kite.browser"))
}

/// Logs to stdout and to `kite.log` (no colors there). `RUST_LOG` overrides the level.
pub fn init_logging() {
    let filter = || tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "kite_lib=info".into());
    let file = log_dir().and_then(|d| {
        std::fs::create_dir_all(&d).ok()?;
        let path = d.join("kite.log");
        let _ = std::fs::rename(&path, d.join("kite.prev.log"));
        std::fs::File::create(&path).ok()
    });
    let stdout = tracing_subscriber::fmt::layer().with_filter(filter());
    let registry = tracing_subscriber::registry().with(stdout);
    match file {
        Some(f) => {
            let layer = tracing_subscriber::fmt::layer().with_ansi(false).with_writer(Mutex::new(f)).with_filter(filter());
            let _ = registry.with(layer).try_init();
        }
        None => {
            let _ = registry.try_init();
        }
    }
    tracing::info!(version = env!("CARGO_PKG_VERSION"), os = std::env::consts::OS, "Kite starting");
}

/// Notes a step the main thread is taking (kept in a small ring, logged on a stall).
pub fn mark(what: impl Into<String>) {
    let mut m = MARKS.lock().unwrap();
    if m.len() >= KEEP_MARKS {
        m.pop_front();
    }
    m.push_back((now_ms(), what.into()));
}

/// A window event arrived (counted for the storm check).
pub fn event() {
    EVENTS.fetch_add(1, Ordering::Relaxed);
}

/// Starts the watchdog thread.
pub fn watchdog(app: AppHandle) {
    PONG.store(now_ms(), Ordering::Relaxed);
    let _ = std::thread::Builder::new().name("main-watchdog".into()).spawn(move || {
        let mut stalled_since: Option<i64> = None;
        let mut last_events = 0u64;
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let _ = app.run_on_main_thread(|| PONG.store(now_ms(), Ordering::Relaxed));
            let events = EVENTS.load(Ordering::Relaxed);
            let rate = events - last_events;
            last_events = events;
            if rate > 300 {
                tracing::warn!(per_second = rate, recent = %recent(8), "window event storm");
            }
            let quiet = now_ms() - PONG.load(Ordering::Relaxed);
            if quiet > STALL_MS {
                if stalled_since.is_none() {
                    stalled_since = Some(now_ms());
                    tracing::error!(ms = quiet, last_steps = %recent(KEEP_MARKS), "main thread stalled (UI not responding)");
                }
            } else if let Some(t) = stalled_since.take() {
                tracing::warn!(ms = now_ms() - t + STALL_MS, "main thread answered again");
            }
        }
    });
}

fn recent(n: usize) -> String {
    let m = MARKS.lock().unwrap();
    let now = now_ms();
    m.iter().rev().take(n).map(|(t, w)| format!("[-{}ms] {w}", now - t)).collect::<Vec<_>>().join(" | ")
}
