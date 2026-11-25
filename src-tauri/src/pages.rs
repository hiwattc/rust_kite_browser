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
    /// Global shortcut for the floating spotlight (`launcher.rs`), on or off.
    launcher_enabled: bool,
    launcher_shortcut: String,
    launcher_label: String,
    launcher_default: String,
    launcher_default_label: String,
    /// Color theme id (`default` = the original look, following the OS light/dark).
    theme: String,
    /// Section to scroll to once (`kite://extensions`).
    section: Option<String>,
}

fn token_path(app: &AppHandle) -> Result<std::path::PathBuf, AppError> {
    Ok(app.path().app_config_dir().map_err(|e| AppError::new("설정 폴더를 찾지 못했습니다.", e))?.join("mcp-token"))
}

/// Color themes (`src/shared/theme.ts` has their names and colors; `base.css` their tokens).
pub const THEMES: &[&str] = &["default", "ocean", "forest", "sunset", "sakura", "sand", "midnight", "nordic", "grape", "mocha", "graphite"];

fn theme_of(store: &crate::store::Store) -> String {
    store.get("theme").filter(|t| THEMES.contains(&t.as_str())).unwrap_or_else(|| "default".into())
}

/// The color theme, for every Kite page as it starts.
#[tauri::command]
pub fn theme_get(b: State<'_, Browser>) -> String {
    theme_of(&b.0.store)
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
        theme: theme_of(store),
        launcher_enabled: crate::launcher::enabled(&b),
        launcher_shortcut: crate::launcher::shortcut(&b),
        launcher_label: crate::launcher::label(&crate::launcher::shortcut(&b)),
        launcher_default: crate::launcher::DEFAULT_SHORTCUT.into(),
        launcher_default_label: format!("기본값: {}", crate::launcher::label(crate::launcher::DEFAULT_SHORTCUT)),
        section: None,
    })
}

#[tauri::command]
pub async fn settings_get(app: AppHandle) -> Result<Settings, AppError> {
    let mut s = current(&app)?;
    let store = &app.state::<Browser>().0.store;
    s.section = store.get("settings_section").filter(|v| !v.is_empty());
    if s.section.is_some() {
        store.set("settings_section", "")?;
    }
    Ok(s)
}

/// Changes one setting (fixed set of keys) and applies it.
#[tauri::command]
pub async fn settings_set(app: AppHandle, key: String, value: String) -> Result<Settings, AppError> {
    crate::diag::mark(format!("settings: {key}"));
    tracing::info!(%key, "setting changed");
    let b = app.state::<Browser>().inner().clone();
    let store = &b.0.store;
    match key.as_str() {
        "search_engine" => {
            if !crate::palette::ENGINES.iter().any(|(n, _)| *n == value) {
                return Err(AppError::new("알 수 없는 검색엔진입니다.", value));
            }
            store.set("search_engine", &value)?;
        }
        "launcher_enabled" => {
            let on = value == "true";
            store.set("launcher_enabled", if on { "true" } else { "false" })?;
            if let Err(e) = crate::launcher::apply(&app) {
                // Not registered: saved as off too, so the switch never shows a state that is not real.
                if on {
                    store.set("launcher_enabled", "false")?;
                }
                return Err(e);
            }
        }
        "launcher_shortcut" => {
            let old = crate::launcher::shortcut(&b);
            let v = if value.trim().is_empty() { crate::launcher::DEFAULT_SHORTCUT.to_string() } else { value.trim().to_string() };
            crate::launcher::check(&v)?;
            store.set("launcher_shortcut", &v)?;
            if let Err(e) = crate::launcher::apply(&app) {
                // Keep the one that worked.
                store.set("launcher_shortcut", &old)?;
                let _ = crate::launcher::apply(&app);
                return Err(e);
            }
        }
        "theme" => {
            if !THEMES.contains(&value.as_str()) {
                return Err(AppError::new("알 수 없는 테마입니다.", value));
            }
            store.set("theme", &value)?;
            b.emit_theme(&value);
        }
        "restore_session" => {
            store.set("restore_session", if value == "true" { "true" } else { "false" })?;
            if value != "true" {
                crate::session_cookies::forget(&app);
            }
        }
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
        "passwords_enabled" => store.set("passwords_enabled", if value == "true" { "true" } else { "false" })?,
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
            crate::session_cookies::forget(&app);
            b.0.store.favicons_clear()?;
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

/// Drag in the bookmark manager: a folder (and its contents) into another one; returns the new path.
#[tauri::command]
pub async fn bookmark_folder_move(b: State<'_, Browser>, from: String, parent: String) -> Result<String, AppError> {
    let to = b.0.store.folder_move(&from, &parent)?;
    b.refresh_bookmark_flags();
    Ok(to)
}

/// Every folder, empty ones too.
#[tauri::command]
pub async fn bookmark_folders(b: State<'_, Browser>) -> Result<Vec<String>, AppError> {
    Ok(b.0.store.folders_all())
}

#[tauri::command]
pub async fn bookmark_folder_create(b: State<'_, Browser>, path: String) -> Result<String, AppError> {
    let p = b.0.store.folder_create(&path)?;
    b.emit_bookmark_bar();
    Ok(p)
}

#[tauri::command]
pub async fn bookmark_folder_rename(b: State<'_, Browser>, path: String, name: String) -> Result<String, AppError> {
    let p = b.0.store.folder_rename(&path, &name)?;
    b.refresh_bookmark_flags();
    Ok(p)
}

/// Deletes a folder with its subfolders and bookmarks; returns how many bookmarks went.
#[tauri::command]
pub async fn bookmark_folder_delete(b: State<'_, Browser>, path: String) -> Result<usize, AppError> {
    let n = b.0.store.folder_delete(&path)?;
    b.refresh_bookmark_flags();
    Ok(n)
}

#[tauri::command]
pub async fn bookmark_folder_order(b: State<'_, Browser>) -> Result<Vec<String>, AppError> {
    Ok(b.0.store.folder_order())
}

#[tauri::command]
pub async fn bookmark_folder_order_set(b: State<'_, Browser>, order: Vec<String>) -> Result<(), AppError> {
    b.0.store.set_folder_order(&order)?;
    b.emit_bookmark_bar();
    Ok(())
}

#[tauri::command]
pub async fn bookmarks_import(app: AppHandle) -> Result<String, AppError> {
    crate::commands::import_bookmarks(app.state::<Browser>().inner())
}

// ---------------------------------------------------------------- extensions

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Extensions {
    /// The OS engine can run extensions (macOS 15.4+, Windows).
    supported: bool,
    items: Vec<crate::extensions::ExtInfo>,
}

fn extensions_state(app: &AppHandle) -> Extensions {
    Extensions { supported: crate::platform::ext_supported(), items: crate::extensions::list(app).into_iter().map(|(i, _)| i).collect() }
}

/// Downloads a Chrome Web Store extension, unpacks it and loads it. Also used by the palette.
pub async fn install_store_extension(app: &AppHandle, input: &str) -> Result<crate::extensions::ExtInfo, AppError> {
    let id = crate::extensions::parse_store_id(input).ok_or_else(|| AppError::new("크롬 웹 스토어 주소나 확장 프로그램 ID(32자)를 넣어 주세요.", input))?;
    if !crate::platform::ext_supported() {
        return Err(AppError::new("이 운영체제 버전에서는 확장 프로그램을 쓸 수 없습니다 (macOS 15.4 이상 필요).", ""));
    }
    let a = app.clone();
    let (info, dir) = tauri::async_runtime::spawn_blocking(move || crate::extensions::install_from_store(&a, &id))
        .await
        .map_err(|e| AppError::new("확장 프로그램을 설치하지 못했습니다.", e))??;
    crate::platform::ext_load(app, &info.id, &dir);
    crate::extensions::emit_toolbar(app);
    Ok(info)
}

#[tauri::command]
pub async fn extensions_list(app: AppHandle) -> Result<Extensions, AppError> {
    Ok(extensions_state(&app))
}

#[tauri::command]
pub async fn extension_install(app: AppHandle, input: String) -> Result<Extensions, AppError> {
    let info = install_store_extension(&app, &input).await?;
    toast(&app.state::<Browser>(), format!("확장 프로그램을 설치했습니다: {}", info.name));
    Ok(extensions_state(&app))
}

/// Unpacked extension from a folder the user picks. `None` when the picker was cancelled.
#[tauri::command]
pub async fn extension_install_folder(app: AppHandle) -> Result<Option<Extensions>, AppError> {
    use tauri_plugin_dialog::DialogExt;
    if !crate::platform::ext_supported() {
        return Err(AppError::new("이 운영체제 버전에서는 확장 프로그램을 쓸 수 없습니다 (macOS 15.4 이상 필요).", ""));
    }
    let a = app.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || a.dialog().file().set_title("압축을 푼 확장 프로그램 폴더 (manifest.json이 있는 폴더)").blocking_pick_folder())
        .await
        .map_err(|e| AppError::new("폴더를 고르지 못했습니다.", e))?;
    let Some(path) = picked.and_then(|p| p.into_path().ok()) else { return Ok(None) };
    let a = app.clone();
    let (info, dir) = tauri::async_runtime::spawn_blocking(move || crate::extensions::install_from_folder(&a, &path))
        .await
        .map_err(|e| AppError::new("확장 프로그램을 설치하지 못했습니다.", e))??;
    crate::platform::ext_load(&app, &info.id, &dir);
    crate::extensions::emit_toolbar(&app);
    toast(&app.state::<Browser>(), format!("확장 프로그램을 설치했습니다: {}", info.name));
    Ok(Some(extensions_state(&app)))
}

#[tauri::command]
pub async fn extension_remove(app: AppHandle, id: String) -> Result<Extensions, AppError> {
    crate::platform::ext_unload(&app, &id);
    crate::extensions::remove(&app, &id)?;
    crate::extensions::emit_toolbar(&app);
    Ok(extensions_state(&app))
}

/// Opens the extension's popup or options page in a new tab (Kite has no popup bubble yet).
#[tauri::command]
pub async fn extension_open(app: AppHandle, id: String, page: String) -> Result<(), AppError> {
    let info = crate::extensions::list(&app).into_iter().map(|(i, _)| i).find(|i| i.id == id).ok_or_else(|| AppError::new("설치된 확장 프로그램이 아닙니다.", &id))?;
    let path = match page.as_str() {
        "popup" => info.popup,
        "options" => info.options,
        _ => None,
    }
    .ok_or_else(|| AppError::new("이 확장 프로그램에는 그 페이지가 없습니다.", &page))?;
    let url = crate::platform::ext_page_url(&id, &path).ok_or_else(|| AppError::new("확장 프로그램이 아직 준비되지 않았습니다. 잠시 뒤 다시 시도하세요.", &id))?;
    app.state::<Browser>().new_tab(Some(url), true)?;
    Ok(())
}

// ---------------------------------------------------------------- passwords

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Passwords {
    enabled: bool,
    items: Vec<crate::store::SavedLogin>,
    never: Vec<String>,
}

fn passwords_state(app: &AppHandle) -> Passwords {
    let b = app.state::<Browser>().inner().clone();
    Passwords { enabled: crate::passwords::enabled(&b), items: b.0.store.pw_list(), never: b.0.store.pw_never_list() }
}

#[tauri::command]
pub async fn passwords_list(app: AppHandle) -> Result<Passwords, AppError> {
    Ok(passwords_state(&app))
}

/// The saved password in plain text, for the settings page only (local page, never web pages).
#[tauri::command]
pub async fn password_reveal(app: AppHandle, id: i64) -> Result<String, AppError> {
    let (_, _, secret) = app.state::<Browser>().0.store.pw_get(id).ok_or_else(|| AppError::new("저장된 비밀번호가 없습니다.", id.to_string()))?;
    crate::passwords::open(&secret)
}

#[tauri::command]
pub async fn password_delete(app: AppHandle, id: i64) -> Result<Passwords, AppError> {
    app.state::<Browser>().0.store.pw_delete(id)?;
    Ok(passwords_state(&app))
}

#[tauri::command]
pub async fn password_never_remove(app: AppHandle, origin: String) -> Result<Passwords, AppError> {
    app.state::<Browser>().0.store.pw_set_never(&origin, false)?;
    Ok(passwords_state(&app))
}
