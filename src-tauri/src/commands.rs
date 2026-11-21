//! Commands for the chrome UI and the palette (local pages), plus `content_event`, the only
//! command web pages in tabs may call (see `capabilities/content.json`).

use serde::Serialize;
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, Manager, State, Webview};

use crate::browser::{Browser, NEWTAB, UI, tab_id_of};
use crate::error::AppError;
use crate::mcp::McpHub;
use crate::palette;

pub fn toast(b: &Browser, msg: impl Into<String>) {
    let _ = b.0.app.emit_to(UI, "toast", msg.into());
}

fn active(b: &Browser) -> Result<u64, AppError> {
    b.active().ok_or_else(|| AppError::new("열린 탭이 없습니다.", "no active tab"))
}

fn copy(text: &str) -> Result<(), AppError> {
    let mut c = arboard::Clipboard::new().map_err(|e| AppError::new("클립보드를 열 수 없습니다.", e))?;
    c.set_text(text.to_string()).map_err(|e| AppError::new("클립보드에 복사하지 못했습니다.", e))
}

/// Chrome-family bookmark files found on this PC: (browser name, path).
fn chromium_bookmark_files() -> Vec<(&'static str, std::path::PathBuf)> {
    let Some(base) = dirs::data_local_dir() else { return Vec::new() };
    let candidates: &[(&str, &str)] = if cfg!(target_os = "macos") {
        &[
            ("Chrome", "Google/Chrome/Default/Bookmarks"),
            ("Edge", "Microsoft Edge/Default/Bookmarks"),
            ("Whale", "Naver/Whale/Default/Bookmarks"),
            ("Brave", "BraveSoftware/Brave-Browser/Default/Bookmarks"),
        ]
    } else {
        &[
            ("Chrome", "Google/Chrome/User Data/Default/Bookmarks"),
            ("Edge", "Microsoft/Edge/User Data/Default/Bookmarks"),
            ("Whale", "Naver/Naver Whale/User Data/Default/Bookmarks"),
            ("Brave", "BraveSoftware/Brave-Browser/User Data/Default/Bookmarks"),
        ]
    };
    candidates.iter().map(|(n, p)| (*n, base.join(p))).filter(|(_, p)| p.is_file()).collect()
}

pub fn import_bookmarks(b: &Browser) -> Result<String, AppError> {
    let files = chromium_bookmark_files();
    if files.is_empty() {
        return Ok("가져올 크롬/엣지/웨일 북마크 파일을 찾지 못했습니다.".into());
    }
    let mut parts = Vec::new();
    for (name, path) in files {
        let json = std::fs::read_to_string(&path).map_err(|e| AppError::new(format!("{name} 북마크를 읽지 못했습니다."), e))?;
        let n = b.0.store.import_chrome_bookmarks(&json)?;
        parts.push(format!("{name} {n}개"));
    }
    b.refresh_bookmark_flags();
    Ok(format!("북마크 가져오기: {}", parts.join(", ")))
}

fn mcp_token_path(app: &AppHandle) -> Result<std::path::PathBuf, AppError> {
    Ok(app.path().app_config_dir().map_err(|e| AppError::new("설정 폴더를 찾지 못했습니다.", e))?.join("mcp-token"))
}

pub fn mcp_port(b: &Browser) -> u16 {
    b.0.store.get("mcp_port").and_then(|p| p.parse().ok()).unwrap_or(crate::mcp::DEFAULT_PORT)
}

pub async fn mcp_start(app: &AppHandle) -> Result<(), AppError> {
    let b = app.state::<Browser>().inner().clone();
    let hub = app.state::<McpHub>().inner().clone();
    let t = crate::mcp::token(&mcp_token_path(app)?, false)?;
    hub.start(mcp_port(&b), t).await?;
    b.emit();
    Ok(())
}

/// Runs a shortcut / palette command.
pub async fn run_action(app: &AppHandle, action: &str) -> Result<(), AppError> {
    let b = app.state::<Browser>().inner().clone();
    // Anything but the palette toggles closes the palette first.
    if !matches!(action, "palette" | "search_tabs" | "search_history" | "search_bookmarks") && b.palette_open() {
        b.close_palette(action != "focus_address");
    }
    match action {
        "palette" => {
            if b.palette_open() {
                b.close_palette(true);
            } else {
                b.open_palette("", "")?;
            }
        }
        "search_tabs" => b.open_palette("@", "")?,
        "open_settings" => b.open_internal("settings")?,
        "open_bookmarks" => b.open_internal("bookmarks")?,
        "toggle_bookmark_bar" => {
            let on = b.0.store.get("bookmark_bar").as_deref() != Some("true");
            b.0.store.set("bookmark_bar", if on { "true" } else { "false" })?;
            b.emit_bookmark_bar();
        }
        "search_history" => b.open_palette("#", "")?,
        "search_bookmarks" => b.open_palette("*", "")?,
        "new_tab" => {
            b.new_tab(None, true)?;
            b.open_palette("", "")?;
        }
        "close_tab" => b.close_tab(active(&b)?)?,
        "reopen_tab" => b.reopen_closed()?,
        "duplicate_tab" => {
            let t = b.tab(active(&b)?).ok_or_else(|| AppError::new("탭이 없습니다.", ""))?;
            b.new_tab(if t.url == NEWTAB { None } else { Some(t.url) }, true)?;
        }
        "close_other_tabs" => {
            let keep = active(&b)?;
            for t in b.tabs() {
                if t.id != keep {
                    b.close_tab(t.id)?;
                }
            }
        }
        "focus_address" => {
            if let Some(ui) = app.get_webview(UI) {
                let _ = ui.set_focus();
                let _ = ui.emit_to(UI, "focus-address", ());
            }
        }
        "reload" => b.reload(active(&b)?, false)?,
        "hard_reload" => b.reload(active(&b)?, true)?,
        "stop" => b.stop(active(&b)?)?,
        "back" => b.back(active(&b)?)?,
        "forward" => b.forward(active(&b)?)?,
        "next_tab" => b.cycle(1)?,
        "prev_tab" => b.cycle(-1)?,
        "tab_last" => b.select_index(0, true)?,
        a if a.starts_with("tab_") => {
            if let Ok(n) = a[4..].parse::<usize>() {
                b.select_index(n.saturating_sub(1), false)?;
            }
        }
        "bookmark" => {
            let on = b.toggle_bookmark(active(&b)?)?;
            toast(&b, if on { "북마크에 추가했습니다." } else { "북마크에서 삭제했습니다." });
        }
        "zoom_in" | "zoom_out" | "zoom_reset" => {
            let z = b.zoom(active(&b)?, &action[5..])?;
            toast(&b, format!("확대/축소 {}%", (z * 100.0).round()));
        }
        "devtools" => {
            let wv = b.ensure_live(active(&b)?)?;
            if wv.is_devtools_open() {
                wv.close_devtools();
            } else {
                wv.open_devtools();
            }
        }
        "copy_url" => {
            let t = b.tab(active(&b)?).ok_or_else(|| AppError::new("탭이 없습니다.", ""))?;
            copy(&t.url)?;
            toast(&b, "주소를 복사했습니다.");
        }
        "import_chrome" => {
            let msg = import_bookmarks(&b)?;
            toast(&b, msg);
        }
        "clear_history" => {
            b.0.store.history_clear()?;
            toast(&b, "방문 기록을 지웠습니다.");
        }
        "mcp_copy" => {
            let t = crate::mcp::token(&mcp_token_path(app)?, false)?;
            copy(&crate::mcp::claude_command(mcp_port(&b), &t))?;
            let running = app.state::<McpHub>().status().running;
            toast(&b, if running { "Claude Code 연결 명령을 복사했습니다. 터미널에 붙여 넣으세요." } else { "연결 명령을 복사했습니다. (MCP 서버가 꺼져 있습니다: 명령 팔레트에서 켜세요)" });
        }
        "mcp_toggle" => {
            let hub = app.state::<McpHub>().inner().clone();
            if hub.status().running {
                hub.stop();
                b.0.store.set("mcp_enabled", "false")?;
                toast(&b, "MCP 서버를 껐습니다.");
            } else {
                b.0.store.set("mcp_enabled", "true")?;
                mcp_start(app).await?;
                toast(&b, format!("MCP 서버를 켰습니다: {}", crate::mcp::url(mcp_port(&b))));
            }
            b.emit();
        }
        other => return Err(AppError::new("알 수 없는 명령입니다.", other)),
    }
    Ok(())
}

// ---------- commands ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Init {
    state: crate::browser::BrowserState,
    bookmark_bar: Option<Vec<crate::browser::BarItem>>,
    keymap: Value,
    platform: &'static str,
}

#[tauri::command]
pub fn ui_init(b: State<'_, Browser>) -> Init {
    Init {
        state: b.snapshot(),
        bookmark_bar: b.bookmark_bar(),
        keymap: serde_json::from_str(&crate::keymap::json_map()).unwrap_or(json!({})),
        platform: if cfg!(target_os = "macos") { "mac" } else if cfg!(windows) { "windows" } else { "linux" },
    }
}

#[tauri::command]
pub async fn shortcut(app: AppHandle, action: String) -> Result<(), AppError> {
    run_action(&app, &action).await
}

/// From web pages in tabs. Only fixed kinds; the payload is never trusted as data.
#[tauri::command]
pub async fn content_event(app: AppHandle, webview: Webview, kind: String, action: String) -> Result<(), AppError> {
    let Some(id) = tab_id_of(webview.label()) else { return Ok(()) };
    let b = app.state::<Browser>().inner().clone();
    tracing::debug!(id, kind, action, active = ?b.active(), "content event");
    match kind.as_str() {
        "shortcut" if crate::keymap::bindings().iter().any(|k| k.action == action) => {
            // A key pressed in a background tab cannot happen; ignore scripted ones from them.
            if b.active() == Some(id) {
                run_action(&app, &action).await?;
            }
        }
        "url" => b.refresh_url(id),
        _ => {}
    }
    Ok(())
}

#[tauri::command]
pub async fn tab_new(b: State<'_, Browser>, url: Option<String>) -> Result<u64, AppError> {
    let engine = b.0.store.get("search_engine").unwrap_or_else(|| "google".into());
    let id = b.new_tab(url.filter(|u| !u.is_empty()).map(|u| palette::resolve(&u, &engine)), true)?;
    Ok(id)
}

#[tauri::command]
pub async fn tab_close(b: State<'_, Browser>, id: u64) -> Result<(), AppError> {
    b.close_tab(id)
}

#[tauri::command]
pub async fn tab_activate(b: State<'_, Browser>, id: u64) -> Result<(), AppError> {
    b.close_palette(false);
    b.activate(id)
}

#[tauri::command]
pub async fn tab_move(b: State<'_, Browser>, id: u64, to: usize) -> Result<(), AppError> {
    b.move_tab(id, to);
    Ok(())
}

/// Address bar Enter: URL or search words.
#[tauri::command]
pub async fn navigate(b: State<'_, Browser>, id: Option<u64>, input: String) -> Result<(), AppError> {
    let id = match id {
        Some(i) => i,
        None => active(&b)?,
    };
    b.navigate(id, &input)?;
    if let Some(wv) = b.webview(id) {
        let _ = wv.set_focus();
    }
    Ok(())
}

#[tauri::command]
/// Sync on purpose: runs in call order (async commands may overtake each other).
pub fn set_chrome_height(b: State<'_, Browser>, height: f64) -> Result<(), AppError> {
    b.set_chrome_height(height);
    Ok(())
}

#[tauri::command]
pub async fn palette_open(b: State<'_, Browser>, mode: String, query: String) -> Result<(), AppError> {
    b.open_palette(&mode, &query)
}

#[tauri::command]
pub async fn palette_close(b: State<'_, Browser>, refocus: bool) -> Result<(), AppError> {
    b.close_palette(refocus);
    Ok(())
}

#[tauri::command]
/// Sync on purpose: runs in call order (async commands may overtake each other).
pub fn palette_resize(b: State<'_, Browser>, height: f64) -> Result<(), AppError> {
    b.resize_palette(height);
    Ok(())
}

#[tauri::command]
pub async fn palette_search(b: State<'_, Browser>, query: String) -> Result<Vec<palette::Group>, AppError> {
    let engine = b.0.store.get("search_engine").unwrap_or_else(|| "google".into());
    Ok(palette::search(&query, &b.palette_tabs(), &b.0.store, &engine))
}

/// Opens a palette result. `new_tab`: Ctrl/⌘+Enter.
#[tauri::command]
pub async fn palette_run(app: AppHandle, kind: String, id: String, new_tab: bool) -> Result<(), AppError> {
    let b = app.state::<Browser>().inner().clone();
    if kind == "command" {
        b.close_palette(true);
        return run_action(&app, &id).await;
    }
    b.close_palette(false);
    match kind.as_str() {
        "tab" => {
            let tid: u64 = id.parse().map_err(|e| AppError::new("탭을 찾지 못했습니다.", e))?;
            b.activate(tid)?;
        }
        _ => {
            // bookmark / history / url / search: open the URL.
            match b.active().and_then(|a| b.tab(a)).filter(|_| !new_tab) {
                Some(t) => {
                    b.navigate_url(t.id, &id)?;
                    if let Some(wv) = b.webview(t.id) {
                        let _ = wv.set_focus();
                    }
                }
                None => {
                    b.new_tab(Some(id), true)?;
                }
            }
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn toggle_bookmark(b: State<'_, Browser>, id: Option<u64>) -> Result<bool, AppError> {
    let id = match id {
        Some(i) => i,
        None => active(&b)?,
    };
    b.toggle_bookmark(id)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpInfo {
    running: bool,
    url: String,
    command: String,
}

#[tauri::command]
pub async fn mcp_info(app: AppHandle) -> Result<McpInfo, AppError> {
    let b = app.state::<Browser>().inner().clone();
    let st = app.state::<McpHub>().status();
    let t = crate::mcp::token(&mcp_token_path(&app)?, false)?;
    Ok(McpInfo { running: st.running, url: crate::mcp::url(mcp_port(&b)), command: crate::mcp::claude_command(mcp_port(&b), &t) })
}
