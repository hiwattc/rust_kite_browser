//! Commands for the chrome UI and the palette (local pages), plus `content_event`, the only
//! command web pages in tabs may call (see `capabilities/content.json`).

use serde::Serialize;
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, Manager, State, Webview};

use crate::browser::{Browser, NEWTAB, tab_id_of, win_of_label};
use crate::error::AppError;
use crate::mcp::McpHub;
use crate::palette;

pub fn toast(b: &Browser, msg: impl Into<String>) {
    crate::browser::toast(&b.0.app, msg);
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
        "open_downloads" => b.open_internal("downloads")?,
        "open_meet" => b.open_internal("meet")?,
        "open_home" => b.open_internal("home")?,
        "open_passwords" => {
            b.0.store.set("settings_section", "passwords")?;
            b.open_internal("settings")?;
        }
        "open_extensions" => {
            b.0.store.set("settings_section", "extensions")?;
            b.open_internal("settings")?;
        }
        "install_extension" => {
            let url = b.tab(active(&b)?).map(|t| t.url).unwrap_or_default();
            if crate::extensions::parse_store_id(&url).is_none() || !url.contains("chrome") {
                toast(&b, "크롬 웹 스토어(chromewebstore.google.com)의 확장 프로그램 페이지에서 실행하세요.");
                return Ok(());
            }
            toast(&b, "확장 프로그램을 받는 중…");
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let b = app.state::<Browser>().inner().clone();
                match crate::pages::install_store_extension(&app, &url).await {
                    Ok(info) => toast(&b, format!("확장 프로그램을 설치했습니다: {}", info.name)),
                    Err(e) => toast(&b, e.user_message),
                }
            });
        }
        "toggle_side_tabs" => {
            b.toggle_side()?;
        }
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
        "new_window" => {
            b.new_window()?;
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
            let win = b.tab(keep).map(|t| t.win);
            for t in b.tabs() {
                if t.id != keep && Some(t.win) == win {
                    b.close_tab(t.id)?;
                }
            }
        }
        "focus_address" => {
            if let Some(ui) = b.ui_webview() {
                let _ = ui.set_focus();
                let _ = ui.emit_to(ui.label(), "focus-address", ());
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
    extensions: Vec<crate::extensions::ToolbarExt>,
}

#[tauri::command]
pub fn ui_init(b: State<'_, Browser>, webview: Webview) -> Init {
    if webview.label().starts_with("side") {
        b.side_started(webview.label());
    }
    Init {
        state: b.snapshot(win_of_label(webview.window().label()).unwrap_or(1)),
        bookmark_bar: b.bookmark_bar(),
        keymap: serde_json::from_str(&crate::keymap::json_map()).unwrap_or(json!({})),
        platform: if cfg!(target_os = "macos") { "mac" } else if cfg!(windows) { "windows" } else { "linux" },
        extensions: crate::extensions::toolbar(webview.app_handle()),
    }
}

/// The open extension popup (window label, extension id) and when the last one closed by
/// losing focus: a click on the same toolbar icon then closes it instead of reopening (Chrome).
static EXT_POPUP: std::sync::Mutex<Option<(String, String)>> = std::sync::Mutex::new(None);
static EXT_POPUP_BLUR: std::sync::Mutex<Option<(String, std::time::Instant)>> = std::sync::Mutex::new(None);
static EXT_POPUP_N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

/// Toolbar icon clicked: the extension's popup in a small window under the icon (`right`,
/// `bottom`: the icon's corner in the toolbar, logical px), sized to its page. No popup: its
/// options page in a tab.
#[tauri::command]
pub async fn ext_popup(app: AppHandle, webview: Webview, id: String, right: f64, bottom: f64) -> Result<(), AppError> {
    let info = crate::extensions::list(&app).into_iter().map(|(i, _)| i).find(|i| i.id == id).ok_or_else(|| AppError::new("설치된 확장 프로그램이 아닙니다.", &id))?;
    let open_now = EXT_POPUP.lock().unwrap().take();
    if let Some((label, _)) = &open_now
        && let Some(w) = app.get_webview_window(label)
    {
        let _ = w.close();
    }
    let just_closed = EXT_POPUP_BLUR.lock().unwrap().take().is_some_and(|(i, at)| i == id && at.elapsed() < std::time::Duration::from_millis(400));
    if just_closed || open_now.is_some_and(|(_, i)| i == id) {
        return Ok(());
    }
    let Some(popup) = info.popup.clone() else {
        if let Some(o) = &info.options
            && let Some(url) = crate::platform::ext_page_url(&id, o)
        {
            app.state::<Browser>().new_tab(Some(url), true)?;
            return Ok(());
        }
        return Err(AppError::new(format!("{}에는 팝업이 없습니다.", info.name), &id));
    };
    let url = crate::platform::ext_page_url(&id, &popup).ok_or_else(|| AppError::new("확장 프로그램이 아직 준비되지 않았습니다. 잠시 뒤 다시 시도하세요.", &id))?;
    let parsed: url::Url = url.parse().map_err(|e| AppError::new("확장 프로그램 주소가 올바르지 않습니다.", e))?;

    // Screen position of the icon's bottom-right corner (logical).
    let win = webview.window();
    let scale = win.scale_factor().unwrap_or(1.0);
    let origin = win.inner_position().map(|p| p.to_logical::<f64>(scale)).unwrap_or(tauri::LogicalPosition::new(0.0, 0.0));
    let (ax, ay) = (origin.x + right, origin.y + bottom + 6.0);
    let (w0, h0) = (320.0, 240.0);

    let label = format!("ext-popup-{}", EXT_POPUP_N.fetch_add(1, std::sync::atomic::Ordering::SeqCst));
    tracing::info!(%label, id = %id, %url, "extension popup");
    let fit_label = label.clone();
    // Closing on blur only once it is on screen (a hidden window can report a blur first).
    let shown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let shown_fit = shown.clone();
    let builder = tauri::WebviewWindowBuilder::new(&app, &label, tauri::WebviewUrl::External(parsed))
        .title(&info.name)
        .decorations(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .shadow(true)
        .inner_size(w0, h0)
        .position((ax - w0).max(origin.x), ay)
        .visible(false)
        .focused(true)
        .browser_extensions_enabled(true)
        .zoom_hotkeys_enabled(false)
        .on_page_load(move |w, p| {
            if matches!(p.event(), tauri::webview::PageLoadEvent::Finished) {
                ext_popup_fit(&w, ax, ay, Some(shown_fit.clone()));
                // Popups often fill in after loading (storage, fetch): measure once more.
                let (w2, l) = (w.clone(), fit_label.clone());
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(450));
                    if w2.app_handle().get_webview_window(&l).is_some() {
                        ext_popup_fit(&w2, ax, ay, None);
                    }
                });
            }
        });
    // macOS: the page runs only in a webview made with that extension's configuration.
    #[cfg(target_os = "macos")]
    let builder = match crate::platform::ext_tab_config(&app, &url) {
        Some(c) => builder.with_webview_configuration(c.0),
        None => builder,
    };
    let w = builder.build().map_err(|e| AppError::new("확장 프로그램 팝업을 열지 못했습니다.", e))?;
    *EXT_POPUP.lock().unwrap() = Some((label.clone(), id.clone()));
    let (w2, id2, l2) = (w.clone(), id.clone(), label.clone());
    let shown_blur = shown.clone();
    let opened = std::time::Instant::now();
    w.on_window_event(move |e| {
        if let tauri::WindowEvent::Focused(false) = e
            && shown_blur.load(std::sync::atomic::Ordering::SeqCst)
        {
            // The click that opened it may also activate Kite's window a moment later (Kite was
            // not the active app): keep the popup then.
            if opened.elapsed() < std::time::Duration::from_millis(700) {
                tracing::debug!("extension popup lost focus right after opening; taking it back");
                let _ = w2.set_focus();
                return;
            }
            tracing::debug!("extension popup lost focus; closing");
            *EXT_POPUP_BLUR.lock().unwrap() = Some((id2.clone(), std::time::Instant::now()));
            let mut open = EXT_POPUP.lock().unwrap();
            if open.as_ref().is_some_and(|(l, _)| *l == l2) {
                *open = None;
            }
            let _ = w2.close();
        }
    });
    // Shown when measured; if the page never reports, show it anyway.
    let w3 = w.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(1500));
        if !shown.load(std::sync::atomic::Ordering::SeqCst) && w3.app_handle().get_webview_window(w3.label()).is_some() {
            tracing::warn!(label = %w3.label(), "extension popup did not report its size; showing it as is");
            let _ = w3.show();
            let _ = w3.set_focus();
            shown.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    });
    Ok(())
}

/// Sizes the popup to its page (Chrome: at most 800×600), right edge under the icon.
/// `show`: first measurement, then show the window and mark it shown.
fn ext_popup_fit(w: &tauri::WebviewWindow, ax: f64, ay: f64, show: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>) {
    const JS: &str = r#"(function(){var h=document.documentElement,b=document.body;if(!b)return "[0,0]";var old=h.style.width;h.style.width="max-content";var w=Math.ceil(h.getBoundingClientRect().width);w=Math.max(80,Math.min(800,w));h.style.width=w+"px";var cs=getComputedStyle(b);var ht=Math.ceil(b.getBoundingClientRect().bottom+parseFloat(cs.marginBottom||"0")+window.scrollY);h.style.width=old;return JSON.stringify([w,ht]);})()"#;
    let win = w.clone();
    let _ = w.eval_with_callback(JS, move |raw| {
        let size: Option<(f64, f64)> = serde_json::from_str::<String>(&raw).ok().and_then(|s| serde_json::from_str::<(f64, f64)>(&s).ok());
        let (pw, ph) = match size {
            Some((pw, ph)) if pw > 0.0 && ph > 0.0 => (pw, ph.clamp(40.0, 600.0)),
            _ => (320.0, 240.0),
        };
        tracing::debug!(label = %win.label(), pw, ph, "extension popup size");
        let _ = win.set_size(tauri::LogicalSize::new(pw, ph));
        let _ = win.set_position(tauri::LogicalPosition::new(ax - pw, ay));
        if let Some(shown) = &show {
            let _ = win.show();
            let _ = win.set_focus();
            shown.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    });
}

#[tauri::command]
pub async fn shortcut(app: AppHandle, webview: Webview, action: String) -> Result<(), AppError> {
    app.state::<Browser>().touch(&webview);
    run_action(&app, &action).await
}

/// From web pages in tabs. Only fixed kinds; the payload is never trusted as data.
#[tauri::command]
pub async fn content_event(app: AppHandle, webview: Webview, kind: String, action: String) -> Result<(), AppError> {
    let Some(id) = tab_id_of(webview.label()) else { return Ok(()) };
    let b = app.state::<Browser>().inner().clone();
    // Never log what a password event carries.
    let shown = if kind == "password" { action.split(':').next().unwrap_or("").chars().take(4).collect::<String>() + "…" } else { action.clone() };
    tracing::debug!(id, kind, action = %shown, active = ?b.active(), "content event");
    match kind.as_str() {
        "shortcut" if crate::keymap::bindings().iter().any(|k| k.action == action) => {
            // A key pressed in a background tab cannot happen; ignore scripted ones from them.
            b.touch(&webview);
            if b.active() == Some(id) {
                run_action(&app, &action).await?;
            }
        }
        "url" => b.refresh_url(id),
        "password" => crate::passwords::on_event(&app, id, &action).await,
        _ => {}
    }
    Ok(())
}

#[tauri::command]
pub async fn tab_new(b: State<'_, Browser>, webview: Webview, url: Option<String>) -> Result<u64, AppError> {
    b.touch(&webview);
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

/// Bookmarks for the tab list's bookmark view, with the folder order of the bookmark manager.
#[derive(Serialize)]
pub struct SideBookmarks {
    bookmarks: Vec<crate::store::Bookmark>,
    order: Vec<String>,
}

#[tauri::command]
pub async fn side_bookmarks(b: State<'_, Browser>) -> Result<SideBookmarks, AppError> {
    Ok(SideBookmarks { bookmarks: b.0.store.bookmarks_all(), order: b.0.store.folder_order() })
}

/// Right click on the toolbar, tab strip or bookmark bar: a native menu to show or hide the
/// bookmark bar and the tab list (Chrome has the same on its bar). Picks come back through
/// `on_menu_event` in `lib.rs` (ids `ctx:<action>`).
#[tauri::command]
pub async fn ui_menu(app: AppHandle, webview: Webview, x: f64, y: f64) -> Result<(), AppError> {
    use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
    let b = app.state::<Browser>().inner().clone();
    b.touch(&webview);
    let err = |e: tauri::Error| AppError::new("메뉴를 열지 못했습니다.", e);
    let bar = b.0.store.get("bookmark_bar").as_deref() == Some("true");
    let menu = Menu::with_items(
        &app,
        &[
            &CheckMenuItem::with_id(&app, "ctx:toggle_bookmark_bar", "북마크 바 표시", true, bar, Some("CmdOrCtrl+Shift+B")).map_err(err)?,
            &CheckMenuItem::with_id(&app, "ctx:toggle_side_tabs", "세로 탭 목록 표시", true, b.side_open(), None::<&str>).map_err(err)?,
            &PredefinedMenuItem::separator(&app).map_err(err)?,
            &MenuItem::with_id(&app, "ctx:open_bookmarks", "북마크 관리자", true, None::<&str>).map_err(err)?,
            &MenuItem::with_id(&app, "ctx:open_settings", "설정", true, None::<&str>).map_err(err)?,
        ],
    )
    .map_err(err)?;
    let window = webview.window();
    app.run_on_main_thread(move || {
        let _ = window.popup_menu_at(&menu, tauri::LogicalPosition::new(x, y));
    })
    .map_err(err)?;
    Ok(())
}

/// Edge of the vertical tab list dragged. Sync: in call order.
#[tauri::command]
pub fn side_resize(b: State<'_, Browser>, width: f64, save: bool) -> Result<(), AppError> {
    b.resize_side(width, save);
    Ok(())
}

#[tauri::command]
pub async fn tab_move(b: State<'_, Browser>, id: u64, to: usize) -> Result<(), AppError> {
    b.move_tab(id, to);
    Ok(())
}

/// A tab pulled out of the strip: its window follows the pointer from now on. Screen points
/// (`x`, `y`) and the pointer's place in the window content (`ox`, `oy`) are CSS px = logical.
#[tauri::command]
pub async fn tab_drag_start(b: State<'_, Browser>, id: u64, x: f64, y: f64, ox: f64, oy: f64) -> Result<(), AppError> {
    b.close_palette(false);
    b.drag_start(id, x, y, ox, oy)
}

/// Sync on purpose: runs on the main thread in call order (moves the window every frame).
#[tauri::command]
pub fn tab_drag_move(b: State<'_, Browser>, x: f64, y: f64) {
    b.drag_move(x, y);
}

#[tauri::command]
pub async fn tab_drag_drop(b: State<'_, Browser>, x: f64, y: f64) -> Result<(), AppError> {
    b.drag_drop(x, y)
}

/// Address bar Enter: URL or search words.
#[tauri::command]
pub async fn navigate(b: State<'_, Browser>, webview: Webview, id: Option<u64>, input: String) -> Result<(), AppError> {
    b.touch(&webview);
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
pub fn set_chrome_height(b: State<'_, Browser>, webview: Webview, height: f64) -> Result<(), AppError> {
    if let Some(n) = win_of_label(webview.window().label()) {
        b.set_chrome_height(n, height);
    }
    Ok(())
}

/// Address bar suggestions for what is typed so far.
#[tauri::command]
pub async fn omni_suggest(b: State<'_, Browser>, input: String) -> Result<crate::omnibox::Suggest, AppError> {
    let engine = b.0.store.get("search_engine").unwrap_or_else(|| "google".into());
    Ok(crate::omnibox::suggest(&input, &b.0.store, &engine))
}

/// Sync: show/hide must keep their order.
#[tauri::command]
pub fn omni_show(b: State<'_, Browser>, webview: Webview, x: f64, y: f64, width: f64, items: serde_json::Value, selected: i64) -> Result<(), AppError> {
    if let Some(n) = win_of_label(webview.window().label()) {
        b.show_omni(n, x, y, width, items, selected);
    }
    Ok(())
}

#[tauri::command]
pub fn omni_hide(b: State<'_, Browser>, webview: Webview) -> Result<(), AppError> {
    if let Some(n) = win_of_label(webview.window().label()) {
        b.hide_omni(n);
    }
    Ok(())
}

/// A suggestion clicked in the list: open it in the window's tab (`new_tab`: ⌘/Ctrl+click).
#[tauri::command]
pub async fn omni_pick(app: AppHandle, webview: Webview, url: String, new_tab: bool) -> Result<(), AppError> {
    let b = app.state::<Browser>().inner().clone();
    let Some(n) = win_of_label(webview.window().label()) else { return Ok(()) };
    b.hide_omni(n);
    let _ = app.emit_to(crate::browser::ui_label(n), "omni-picked", ());
    b.touch(&webview);
    if new_tab {
        b.new_tab(Some(url), true)?;
        return Ok(());
    }
    let id = active(&b)?;
    b.navigate(id, &url)?;
    if let Some(wv) = b.webview(id) {
        let _ = wv.set_focus();
    }
    Ok(())
}

#[tauri::command]
pub async fn palette_open(b: State<'_, Browser>, webview: Webview, mode: String, query: String) -> Result<(), AppError> {
    b.touch(&webview);
    b.open_palette(&mode, &query)
}

#[tauri::command]
pub async fn palette_close(app: AppHandle, b: State<'_, Browser>, webview: Webview, refocus: bool) -> Result<(), AppError> {
    if webview.label() == crate::launcher::LABEL {
        crate::launcher::hide(&app, true);
        return Ok(());
    }
    b.close_palette(refocus);
    Ok(())
}

#[tauri::command]
/// Sync on purpose: runs in call order (async commands may overtake each other).
pub fn palette_resize(app: AppHandle, b: State<'_, Browser>, webview: Webview, height: f64) -> Result<(), AppError> {
    if webview.label() == crate::launcher::LABEL {
        crate::launcher::resize(&app, height);
        return Ok(());
    }
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
pub async fn palette_run(app: AppHandle, webview: Webview, kind: String, id: String, new_tab: bool) -> Result<(), AppError> {
    let b = app.state::<Browser>().inner().clone();
    b.touch(&webview);
    // From the global launcher: Kite comes forward to show what was picked.
    if webview.label() == crate::launcher::LABEL {
        crate::launcher::picked(&app);
    }
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
pub async fn toggle_bookmark(b: State<'_, Browser>, webview: Webview, id: Option<u64>) -> Result<bool, AppError> {
    b.touch(&webview);
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
