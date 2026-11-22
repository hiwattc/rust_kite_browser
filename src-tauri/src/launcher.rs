//! Global launcher: a system-wide shortcut (default ⌥Space on macOS, Ctrl+Alt+Space on
//! Windows) shows Kite's spotlight in a small floating window over whatever app is in front,
//! like Spotlight or Everything. Picking a result opens it in Kite and brings Kite forward;
//! Esc or clicking elsewhere hides it and leaves the other app in front.
//!
//! The window loads the same palette page (`palette.html?launcher`); the palette commands
//! notice the `launcher` webview and size/close this window instead of the in-window overlay.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Emitter, LogicalSize, Manager, WebviewUrl, WebviewWindow};
#[cfg(target_os = "macos")]
use tauri::LogicalPosition;
#[cfg(not(target_os = "macos"))]
use tauri::{PhysicalPosition, PhysicalSize};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

use crate::browser::Browser;
use crate::error::AppError;

pub const LABEL: &str = "launcher";
const WIDTH: f64 = 680.0;

pub const DEFAULT_SHORTCUT: &str = if cfg!(target_os = "macos") { "Alt+Space" } else { "Ctrl+Alt+Space" };

/// The shortcut registered now (to unregister it when it changes).
static CURRENT: Mutex<Option<String>> = Mutex::new(None);
/// Kite was the active app when the launcher came up (then hiding it stays in Kite).
static KITE_WAS_ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn enabled(b: &Browser) -> bool {
    b.0.store.get("launcher_enabled").as_deref() != Some("false")
}

pub fn shortcut(b: &Browser) -> String {
    b.0.store.get("launcher_shortcut").filter(|s| !s.trim().is_empty()).unwrap_or_else(|| DEFAULT_SHORTCUT.into())
}

/// Registers the saved shortcut (or none when turned off). Error: invalid or taken by another app.
pub fn apply(app: &AppHandle) -> Result<(), AppError> {
    let b = app.state::<Browser>().inner().clone();
    let gs = app.global_shortcut();
    let old = CURRENT.lock().unwrap().take();
    if let Some(old) = old {
        crate::diag::mark(format!("launcher: unregister {old}"));
        let r = gs.unregister(old.as_str());
        tracing::info!(shortcut = %old, ok = r.is_ok(), "launcher shortcut unregistered");
    }
    if !enabled(&b) {
        return Ok(());
    }
    let sc = shortcut(&b);
    crate::diag::mark(format!("launcher: register {sc}"));
    gs.on_shortcut(sc.as_str(), |app, _, e| {
        if e.state == ShortcutState::Pressed {
            toggle(app);
        }
    })
    .map_err(|e| AppError::new(format!("전역 단축키 {}를 등록하지 못했습니다. 다른 앱이 쓰고 있을 수 있습니다. 설정에서 다른 조합을 고르세요.", label(&sc)), e))?;
    tracing::info!(shortcut = %sc, "launcher shortcut registered");
    *CURRENT.lock().unwrap() = Some(sc);
    // Made now (hidden), so the first press shows a ready page, not a blank window. From a
    // background task: building a webview window on the main thread (inside an event-loop
    // callback) can deadlock on Windows; from here Tauri hands it to the main thread itself.
    let a = app.clone();
    tauri::async_runtime::spawn(async move {
        if window(&a).is_none() {
            crate::diag::mark("launcher: create window");
            match create(&a) {
                Ok(_) => tracing::info!("launcher window created"),
                Err(e) => tracing::warn!(error = %e.detail, "launcher window"),
            }
        }
    });
    Ok(())
}

/// A shortcut people can set: modifiers + one key, or a function key (F1–F24) on its own. A plain
/// letter alone would swallow that letter everywhere.
pub fn check(sc: &str) -> Result<(), AppError> {
    let parts: Vec<&str> = sc.split('+').map(str::trim).collect();
    let key = parts.last().copied().unwrap_or("");
    let is_fn = key.len() >= 2 && key[..1].eq_ignore_ascii_case("f") && key[1..].parse::<u8>().is_ok_and(|n| (1..=24).contains(&n));
    if parts.len() == 1 && !is_fn {
        return Err(AppError::new("펑션키(F1~F24)가 아니면 Ctrl·Alt·Shift·⌘ 같은 키와 함께 지정하세요.", sc));
    }
    sc.parse::<tauri_plugin_global_shortcut::Shortcut>().map(|_| ()).map_err(|e| AppError::new("단축키 형식이 올바르지 않습니다.", e))
}

/// `Alt+Space` → `⌥ Space` (macOS) / `Alt+Space`.
pub fn label(sc: &str) -> String {
    if !cfg!(target_os = "macos") {
        return sc.to_string();
    }
    sc.split('+')
        .map(|k| match k.to_ascii_lowercase().as_str() {
            "alt" | "option" => "⌥".to_string(),
            "cmd" | "command" | "super" | "meta" => "⌘".to_string(),
            "ctrl" | "control" => "⌃".to_string(),
            "shift" => "⇧".to_string(),
            "space" => " Space".to_string(),
            other => other.trim_start_matches("key").to_uppercase(),
        })
        .collect()
}

fn window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(LABEL)
}

fn create(app: &AppHandle) -> Result<WebviewWindow, AppError> {
    let w = tauri::WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("palette.html?launcher".into()))
        .title("Kite 검색")
        .inner_size(WIDTH, 120.0)
        .decorations(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible_on_all_workspaces(true)
        .shadow(true)
        .visible(false)
        .focused(true)
        .zoom_hotkeys_enabled(false)
        .browser_extensions_enabled(true)
        .build()
        .map_err(|e| AppError::new("검색 창을 만들지 못했습니다.", e))?;
    let a = app.clone();
    w.on_window_event(move |e| {
        // Like Spotlight: gone when you click elsewhere.
        if let tauri::WindowEvent::Focused(false) = e {
            hide(&a, false);
        }
    });
    Ok(w)
}

/// Shows the launcher (or hides it when it is already up).
pub fn toggle(app: &AppHandle) {
    let a = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(w) = window(&a)
            && w.is_visible().unwrap_or(false)
        {
            tracing::info!("launcher: shortcut → hide");
            hide(&a, true);
            return;
        }
        tracing::info!("launcher: shortcut → show");
        if window(&a).is_some() {
            if let Err(e) = show(&a) {
                tracing::warn!(error = %e.detail, "launcher");
            }
            return;
        }
        // Not made yet: make it off the main thread (see `apply`), then show it there.
        let a2 = a.clone();
        tauri::async_runtime::spawn(async move {
            crate::diag::mark("launcher: create window (first press)");
            if let Err(e) = create(&a2) {
                tracing::warn!(error = %e.detail, "launcher window");
                return;
            }
            let a3 = a2.clone();
            let _ = a2.run_on_main_thread(move || {
                if let Err(e) = show(&a3) {
                    tracing::warn!(error = %e.detail, "launcher");
                }
            });
        });
    });
}

fn show(app: &AppHandle) -> Result<(), AppError> {
    let b = app.state::<Browser>().inner().clone();
    KITE_WAS_ACTIVE.store(b.wins().into_iter().filter_map(|n| b.window(n)).any(|w| w.is_focused().unwrap_or(false)), Ordering::SeqCst);
    let w = window(app).ok_or_else(|| AppError::new("검색 창이 아직 준비되지 않았습니다.", "no launcher window"))?;
    place(app, &w);
    let _ = w.emit_to(LABEL, "palette-open", serde_json::json!({ "mode": "", "query": "" }));
    let _ = w.show();
    let _ = w.set_focus();
    // From another app, macOS finishes activating Kite a moment later and may make a browser
    // window key then: take the keyboard back once activation has settled.
    for ms in [60u64, 180] {
        let w = w.clone();
        let a = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(ms));
            let _ = a.run_on_main_thread(move || {
                if w.is_visible().unwrap_or(false) && !w.is_focused().unwrap_or(false) {
                    let _ = w.set_focus();
                }
            });
        });
    }
    Ok(())
}

/// Upper part of the screen the mouse is on, like Spotlight.
#[cfg(target_os = "macos")]
fn place(app: &AppHandle, w: &WebviewWindow) {
    // Points, top-left origin (what window positions use on macOS). Monitors of different
    // scales do not line up in physical pixels, so everything here stays in points.
    let Ok(monitors) = app.available_monitors() else { return };
    let rects: Vec<(f64, f64, f64, f64)> = monitors
        .iter()
        .map(|m| {
            let p = m.position().to_logical::<f64>(m.scale_factor());
            let s = m.size().to_logical::<f64>(m.scale_factor());
            (p.x, p.y, s.width, s.height)
        })
        .collect();
    // AppKit's mouse location: points, origin at the bottom left of the main screen.
    let main_h = app.primary_monitor().ok().flatten().map(|m| m.size().to_logical::<f64>(m.scale_factor()).height).unwrap_or(0.0);
    let loc = objc2_app_kit::NSEvent::mouseLocation();
    let (cx, cy) = (loc.x, main_h - loc.y);
    let Some(&(x, y, mw, mh)) = rects.iter().find(|(x, y, w, h)| cx >= *x && cx < x + w && cy >= *y && cy < y + h).or(rects.first()) else { return };
    let width = WIDTH.min(mw - 40.0);
    let _ = w.set_size(LogicalSize::new(width, 120.0));
    let _ = w.set_position(LogicalPosition::new(x + (mw - width) / 2.0, y + mh * 0.22));
}

/// Windows: the virtual desktop is in physical pixels, cursor and monitors alike.
#[cfg(not(target_os = "macos"))]
fn place(app: &AppHandle, w: &WebviewWindow) {
    let monitor = app.cursor_position().ok().and_then(|p| app.monitor_from_point(p.x, p.y).ok().flatten()).or_else(|| app.primary_monitor().ok().flatten());
    let Some(m) = monitor else { return };
    let scale = m.scale_factor();
    let (mx, my) = (m.position().x as f64, m.position().y as f64);
    let (mw, mh) = (m.size().width as f64, m.size().height as f64);
    let width = (WIDTH * scale).min(mw - 40.0 * scale);
    let _ = w.set_size(PhysicalSize::new(width, 120.0 * scale));
    let _ = w.set_position(PhysicalPosition::new(mx + (mw - width) / 2.0, my + mh * 0.22));
}

/// Hides the launcher. `back`: Esc or the shortcut again; when Kite was not the app in front,
/// it steps aside so the previous app is in front again.
pub fn hide(app: &AppHandle, back: bool) {
    let Some(w) = window(app) else { return };
    if !w.is_visible().unwrap_or(false) {
        return;
    }
    let _ = w.hide();
    #[cfg(target_os = "macos")]
    if back && !KITE_WAS_ACTIVE.load(Ordering::SeqCst) {
        let _ = app.hide();
    }
    #[cfg(not(target_os = "macos"))]
    let _ = back;
}

/// Something was picked: Kite's window comes forward to show it.
pub fn picked(app: &AppHandle) {
    let b = app.state::<Browser>().inner().clone();
    KITE_WAS_ACTIVE.store(true, Ordering::SeqCst);
    if let Some(w) = window(app) {
        let _ = w.hide();
    }
    if let Some(w) = b.window(b.focused()) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// Sizes the launcher to the palette's content.
pub fn resize(app: &AppHandle, h: f64) {
    if let Some(w) = window(app) {
        let width = w.inner_size().ok().and_then(|s| w.scale_factor().ok().map(|f| s.to_logical::<f64>(f).width)).unwrap_or(WIDTH);
        let _ = w.set_size(LogicalSize::new(width, h.clamp(60.0, 640.0)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_people_can_set() {
        for ok in ["F9", "f12", "Alt+Space", "Ctrl+Alt+Space", "Cmd+Shift+K", "Ctrl+F9"] {
            assert!(check(ok).is_ok(), "{ok}");
        }
        for bad in ["A", "Space", "F25", "Ctrl+", "Alt+Nope"] {
            assert!(check(bad).is_err(), "{bad}");
        }
        assert_eq!(label("F9"), "F9");
    }
}
