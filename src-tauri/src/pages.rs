//! Commands for Kite's own pages in tabs (`kite://settings`, `kite://bookmarks`).
//! Allowed only for local pages in tab webviews (`capabilities/internal.json`); web pages
//! never get them, and tabs reach those pages only when Kite navigates there.

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::browser::Browser;
use crate::commands::{mcp_port, mcp_start, toast};
use crate::error::AppError;
use crate::mcp::McpHub;
use crate::store::Bookmark;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Shortcut {
    title: String,
    keys: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    search_engine: String,
    engines: Vec<(String, String)>,
    restore_session: bool,
    bookmark_bar: bool,
    mcp_enabled: bool,
    mcp_running: bool,
    mcp_port: u16,
    mcp_url: String,
    mcp_command: String,
    data_dir: String,
    version: String,
    shortcuts: Vec<Shortcut>,
    bookmark_count: usize,
}

fn token_path(app: &AppHandle) -> Result<std::path::PathBuf, AppError> {
    Ok(app.path().app_config_dir().map_err(|e| AppError::new("설정 폴더를 찾지 못했습니다.", e))?.join("mcp-token"))
}

fn current(app: &AppHandle) -> Result<Settings, AppError> {
    let b = app.state::<Browser>().inner().clone();
    let store = &b.0.store;
    let hub = app.state::<McpHub>();
    let port = mcp_port(&b);
    let token = crate::mcp::token(&token_path(app)?, false)?;
    Ok(Settings {
        search_engine: store.get("search_engine").unwrap_or_else(|| "google".into()),
        engines: [("google", "Google"), ("naver", "네이버"), ("duckduckgo", "DuckDuckGo"), ("bing", "Bing")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
        restore_session: store.get("restore_session").as_deref() != Some("false"),
        bookmark_bar: store.get("bookmark_bar").as_deref() == Some("true"),
        mcp_enabled: store.get("mcp_enabled").as_deref() != Some("false"),
        mcp_running: hub.status().running,
        mcp_port: port,
        mcp_url: crate::mcp::url(port),
        mcp_command: crate::mcp::claude_command(port, &token),
        data_dir: app.path().app_data_dir().map(|p| p.display().to_string()).unwrap_or_default(),
        version: env!("CARGO_PKG_VERSION").into(),
        shortcuts: crate::keymap::describe().into_iter().map(|(title, keys)| Shortcut { title, keys }).collect(),
        bookmark_count: store.bookmarks_all().len(),
    })
}

#[tauri::command]
pub async fn settings_get(app: AppHandle) -> Result<Settings, AppError> {
    current(&app)
}

/// Changes one setting (fixed set of keys) and applies it.
#[tauri::command]
pub async fn settings_set(app: AppHandle, key: String, value: String) -> Result<Settings, AppError> {
    let b = app.state::<Browser>().inner().clone();
    let store = &b.0.store;
    match key.as_str() {
        "search_engine" => {
            if !crate::palette::ENGINES.iter().any(|(n, _)| *n == value) {
                return Err(AppError::new("알 수 없는 검색엔진입니다.", value));
            }
            store.set("search_engine", &value)?;
        }
        "restore_session" => store.set("restore_session", if value == "true" { "true" } else { "false" })?,
        "bookmark_bar" => {
            store.set("bookmark_bar", if value == "true" { "true" } else { "false" })?;
            b.emit_bookmark_bar();
        }
        "mcp_enabled" => {
            let on = value == "true";
            store.set("mcp_enabled", if on { "true" } else { "false" })?;
            let hub = app.state::<McpHub>().inner().clone();
            if on {
                mcp_start(&app).await?;
            } else {
                hub.stop();
                b.emit();
            }
        }
        "mcp_port" => {
            let port: u16 = value.trim().parse().ok().filter(|p| *p >= 1024).ok_or_else(|| AppError::new("포트는 1024~65535 사이 숫자여야 합니다.", value.clone()))?;
            store.set("mcp_port", &port.to_string())?;
            if app.state::<McpHub>().status().running {
                mcp_start(&app).await?;
                toast(&b, format!("MCP 서버를 포트 {port}에서 다시 시작했습니다. Claude Code 연결 명령도 바뀝니다."));
            }
        }
        other => return Err(AppError::new("바꿀 수 없는 설정입니다.", other)),
    }
    current(&app)
}

/// New MCP token (agents registered with the old one stop working until re-added).
#[tauri::command]
pub async fn mcp_renew_token(app: AppHandle) -> Result<Settings, AppError> {
    crate::mcp::token(&token_path(&app)?, true)?;
    if app.state::<McpHub>().status().running {
        mcp_start(&app).await?;
    }
    current(&app)
}

#[tauri::command]
pub fn copy_text(text: String) -> Result<(), AppError> {
    let mut c = arboard::Clipboard::new().map_err(|e| AppError::new("클립보드를 열 수 없습니다.", e))?;
    c.set_text(text).map_err(|e| AppError::new("클립보드에 복사하지 못했습니다.", e))
}

/// `history` | `site_data` (cookies, cache, storage of every site).
#[tauri::command]
pub async fn clear_data(app: AppHandle, what: String) -> Result<String, AppError> {
    let b = app.state::<Browser>().inner().clone();
    match what.as_str() {
        "history" => {
            b.0.store.history_clear()?;
            Ok("방문 기록을 지웠습니다.".into())
        }
        "site_data" => {
            // One shared data store: clearing through any tab clears it for all.
            let wv = b.tabs().iter().find_map(|t| b.webview(t.id)).ok_or_else(|| AppError::new("열린 탭이 없습니다.", "no webview"))?;
            wv.clear_all_browsing_data().map_err(|e| AppError::new("사이트 데이터를 지우지 못했습니다.", e))?;
            Ok("쿠키와 사이트 데이터를 지웠습니다. 로그인된 사이트에서 로그아웃됩니다.".into())
        }
        other => Err(AppError::new("알 수 없는 항목입니다.", other)),
    }
}

#[tauri::command]
pub fn bookmarks_all(b: State<'_, Browser>) -> Vec<Bookmark> {
    b.0.store.bookmarks_all()
}

/// Adds (`id` None) or edits a bookmark.
#[tauri::command]
pub async fn bookmark_save(b: State<'_, Browser>, id: Option<i64>, url: String, title: String, folder: String) -> Result<(), AppError> {
    let url = crate::palette::as_url(&url).ok_or_else(|| AppError::new("주소가 올바르지 않습니다.", url.clone()))?;
    match id {
        Some(id) => b.0.store.bookmark_update(id, &url, &title, &folder)?,
        None => {
            if !b.0.store.bookmark_add(&url, title.trim(), &folder)? {
                return Err(AppError::new("같은 주소의 북마크가 이미 있습니다.", url));
            }
        }
    }
    b.refresh_bookmark_flags();
    Ok(())
}

#[tauri::command]
pub async fn bookmark_delete(b: State<'_, Browser>, id: i64) -> Result<(), AppError> {
    b.0.store.bookmark_delete(id)?;
    b.refresh_bookmark_flags();
    Ok(())
}

#[tauri::command]
pub async fn bookmarks_import(app: AppHandle) -> Result<String, AppError> {
    crate::commands::import_bookmarks(app.state::<Browser>().inner())
}
