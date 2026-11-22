//! Keeps session cookies (no expiry date) across restarts, like Chrome's "continue where you
//! left off": many sites (Naver among them) keep a login only in session cookies, which the
//! engine drops when Kite quits. While session restore is on, Kite saves them shortly after
//! page loads (and every minute) and puts them back before the first tab loads.
//!
//! The file sits next to `kite.db` (`session-cookies.json`, 0600 on Unix): the same protection
//! as the engine's own cookie store, which keeps persistent login cookies on disk as well.
//! Turning session restore off or clearing site data deletes it.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::webview::cookie::{self, SameSite};
use tauri::{AppHandle, Manager};

use crate::browser::Browser;
use crate::store::now_ms;

const FILE: &str = "session-cookies.json";
/// Save this long after the last page load (logins finish with a page load).
const QUIET_MS: i64 = 2_000;
const EVERY_MS: i64 = 60_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Saved {
    name: String,
    value: String,
    domain: String,
    path: String,
    secure: bool,
    http_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    same_site: Option<String>,
}

static DIRTY_AT: AtomicI64 = AtomicI64::new(0);
static STARTED: AtomicBool = AtomicBool::new(false);
static LAST: Mutex<String> = Mutex::new(String::new());

fn path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join(FILE))
}

pub fn enabled(b: &Browser) -> bool {
    b.0.store.get("restore_session").as_deref() != Some("false")
}

fn to_saved(c: &cookie::Cookie<'_>) -> Option<Saved> {
    // Only session cookies: persistent ones are kept by the engine itself.
    if !matches!(c.expires(), None | Some(cookie::Expiration::Session)) {
        return None;
    }
    let domain = raw_domain(c)?;
    Some(Saved {
        name: c.name().to_string(),
        value: c.value().to_string(),
        domain,
        path: c.path().unwrap_or("/").to_string(),
        secure: c.secure().unwrap_or(false),
        http_only: c.http_only().unwrap_or(false),
        same_site: match c.same_site() {
            Some(SameSite::Lax) => Some("lax".into()),
            Some(SameSite::Strict) => Some("strict".into()),
            _ => None,
        },
    })
}

/// The cookie's domain as the engine gave it. `Cookie::domain()` drops a leading dot, but that
/// dot matters: `.naver.com` is sent to every subdomain, `naver.com` (host-only) is not. The
/// raw value is still inside the cookie, only `Debug` shows it (pinned by the tests).
fn raw_domain(c: &cookie::Cookie<'_>) -> Option<String> {
    let d = c.domain()?;
    let dotted = format!("{c:?}").contains(&format!("domain: Some(Concrete(\".{d}\"))"));
    Some(if dotted { format!(".{d}") } else { d.to_string() })
}

fn to_cookie(s: &Saved) -> cookie::Cookie<'static> {
    // One extra dot: `domain()` (which the engine gets) strips one, keeping `.naver.com`.
    let domain = if s.domain.starts_with('.') { format!(".{}", s.domain) } else { s.domain.clone() };
    let mut b = cookie::Cookie::build((s.name.clone(), s.value.clone())).domain(domain).path(s.path.clone());
    // Only when set: macOS (wry) passes `Secure: "FALSE"` on, and a present key means secure.
    if s.secure {
        b = b.secure(true);
    }
    if s.http_only {
        b = b.http_only(true);
    }
    match s.same_site.as_deref() {
        Some("lax") => b = b.same_site(SameSite::Lax),
        Some("strict") => b = b.same_site(SameSite::Strict),
        _ => {}
    }
    b.build()
}

fn write_private(p: &PathBuf, text: &str) -> std::io::Result<()> {
    let tmp = p.with_extension("json.tmp");
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        f.write_all(text.as_bytes())?;
    }
    #[cfg(not(unix))]
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, p)
}

/// Reads the session cookies now and writes them if they changed. Never on the main thread
/// (reading cookies waits for the engine; on Windows that deadlocks the UI thread).
fn save(b: &Browser) {
    let Some(p) = path(&b.0.app) else { return };
    if !enabled(b) {
        forget(&b.0.app);
        return;
    }
    let Some(wv) = b.ui_webview() else { return };
    let cookies = match wv.cookies() {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!(error = %e, "reading cookies failed");
            return;
        }
    };
    let mut list: Vec<Saved> = cookies.iter().filter_map(to_saved).collect();
    list.sort_by(|a, b| (&a.domain, &a.path, &a.name).cmp(&(&b.domain, &b.path, &b.name)));
    let Ok(text) = serde_json::to_string(&list) else { return };
    let mut last = LAST.lock().unwrap();
    if *last == text {
        return;
    }
    match write_private(&p, &text) {
        Ok(()) => {
            tracing::debug!(count = list.len(), "session cookies saved");
            *last = text;
        }
        Err(e) => tracing::warn!(error = %e, "saving session cookies failed"),
    }
}

/// Puts the saved session cookies back. Call before any tab loads, off the main thread.
pub fn restore(b: &Browser) {
    let Some(p) = path(&b.0.app) else { return };
    if !enabled(b) {
        forget(&b.0.app);
        return;
    }
    let Ok(text) = std::fs::read_to_string(&p) else { return };
    let Ok(list) = serde_json::from_str::<Vec<Saved>>(&text) else { return };
    let Some(wv) = b.ui_webview() else { return };
    let mut n = 0;
    for s in &list {
        match wv.set_cookie(to_cookie(s)) {
            Ok(()) => n += 1,
            Err(e) => tracing::debug!(error = %e, domain = %s.domain, "restoring a cookie failed"),
        }
    }
    *LAST.lock().unwrap() = text;
    tracing::info!(restored = n, "session cookies restored");
}

/// Deletes the saved file (session restore turned off, site data cleared).
pub fn forget(app: &AppHandle) {
    if let Some(p) = path(app) {
        let _ = std::fs::remove_file(p);
    }
    LAST.lock().unwrap().clear();
}

/// A page finished loading: save soon.
pub fn touch() {
    DIRTY_AT.store(now_ms(), Ordering::Relaxed);
}

/// Starts the saver thread (once).
pub fn start(b: Browser) {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::Builder::new()
        .name("session-cookies".into())
        .spawn(move || {
            let mut saved_at = now_ms();
            loop {
                std::thread::sleep(Duration::from_secs(1));
                let now = now_ms();
                let dirty = DIRTY_AT.load(Ordering::Relaxed);
                let quiet = dirty > saved_at && now - dirty >= QUIET_MS;
                if quiet || now - saved_at >= EVERY_MS {
                    saved_at = now;
                    save(&b);
                }
            }
        })
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_session_cookies_round_trip() {
        let session = cookie::Cookie::build(("NID_SES", "abc")).domain(".naver.com").path("/").secure(true).http_only(true).same_site(SameSite::Lax).build();
        let s = to_saved(&session).unwrap();
        assert_eq!(s, Saved { name: "NID_SES".into(), value: "abc".into(), domain: ".naver.com".into(), path: "/".into(), secure: true, http_only: true, same_site: Some("lax".into()) });
        let back = to_cookie(&s);
        assert_eq!((back.name(), back.value(), back.domain(), back.http_only(), back.expires()), ("NID_SES", "abc", Some(".naver.com"), Some(true), None));
        // Host-only stays host-only.
        let host = cookie::Cookie::build(("a", "b")).domain("www.naver.com").build();
        assert_eq!(to_saved(&host).unwrap().domain, "www.naver.com");
        let back = to_cookie(&to_saved(&host).unwrap());
        assert_eq!((back.domain(), back.secure(), back.http_only()), (Some("www.naver.com"), None, None));
        let persistent = cookie::Cookie::build(("SID", "x")).domain(".google.com").expires(cookie::time::OffsetDateTime::now_utc() + cookie::time::Duration::days(30)).build();
        assert!(to_saved(&persistent).is_none());
    }
}
