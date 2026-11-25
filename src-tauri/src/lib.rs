//! Kite: a small browser on Tauri v2 (OS webviews: WebView2 on Windows, WKWebView on macOS)
//! with a spotlight palette and an MCP server for agents (Claude Code).

mod automation;
mod browser;
mod commands;
mod error;
mod extensions;
mod hangul;
mod keymap;
mod launcher;
mod mcp;
mod neterror;
mod meet;
mod omnibox;
mod pages;
mod palette;
mod passwords;
mod diag;
mod downloads;
mod platform;
mod portal;
mod session_cookies;
mod siteinfo;
mod store;

use std::sync::Arc;

use tauri::Manager;

use crate::browser::Browser;
use crate::mcp::McpHub;
use crate::store::Store;

#[cfg(target_os = "macos")]
fn menu(app: &tauri::AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{Menu, PredefinedMenuItem, Submenu};
    // Only standard items: Kite's own shortcuts are handled by the key table (keymap.rs),
    // so they work the same in every webview and are not run twice.
    let app_menu = Submenu::with_items(
        app,
        "Kite",
        true,
        &[
            &PredefinedMenuItem::about(app, Some("Kite 정보"), None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::services(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::show_all(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::quit(app, None)?,
        ],
    )?;
    let edit = Submenu::with_items(
        app,
        "편집",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;
    let window = Submenu::with_items(
        app,
        "윈도우",
        true,
        &[&PredefinedMenuItem::minimize(app, None)?, &PredefinedMenuItem::maximize(app, None)?, &PredefinedMenuItem::fullscreen(app, None)?],
    )?;
    Menu::with_items(app, &[&app_menu, &edit, &window])
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle().clone();
    let data = app.path().app_data_dir()?;
    let store = Arc::new(Store::open(&data.join("kite.db"))?);
    diag::watchdog(handle.clone());
    let browser = Browser::new(handle.clone(), store.clone());
    app.manage(browser.clone());
    app.state::<McpHub>().set_app(handle.clone());

    diag::mark("setup: create window 1");
    browser.create_window(None, None)?; // window 1 = `main`
    tracing::info!("window 1 created");

    for (ext, dir) in extensions::list(&handle) {
        platform::ext_load(&handle, &ext.id, &dir);
    }
    // Session cookies first (logins kept only in them, e.g. Naver), then the tabs. Off the main
    // thread: setting cookies waits for the engine, which runs on it.
    let b = browser.clone();
    std::thread::spawn(move || {
        session_cookies::restore(&b);
        tracing::info!("session cookies step done");
        let b2 = b.clone();
        let _ = b.0.app.run_on_main_thread(move || {
            diag::mark("startup: restore tabs");
            if let Err(e) = b2.restore_session() {
                tracing::warn!(error = %e.detail, "session restore failed");
            }
            diag::mark("startup: layout");
            b2.layout_all();
            b2.start_shots();
            diag::mark("startup: launcher shortcut");
            if let Err(e) = launcher::apply(&b2.0.app) {
                commands::toast(&b2, e.user_message);
            }
            session_cookies::start(b2);
            diag::mark("startup: done");
            tracing::info!("startup finished on the main thread");
        });
    });

    if store.get("mcp_enabled").as_deref() != Some("false") {
        let h = handle.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = commands::mcp_start(&h).await {
                commands::toast(&h.state::<Browser>(), e.user_message);
            }
        });
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    diag::init_logging();
    let builder = tauri::Builder::default().manage(McpHub::default()).manage(downloads::Downloads::default());
    // macOS may end a hidden tab's page process to free memory. Tauri's default reloads it at
    // once; Kite waits until the tab is shown and puts its last picture up meanwhile.
    #[cfg(target_os = "macos")]
    let builder = builder.on_web_content_process_terminate(|wv| {
        if let Some(b) = wv.try_state::<Browser>() {
            b.process_gone(wv);
        } else {
            let _ = wv.reload();
        }
    });
    #[cfg(target_os = "macos")]
    let builder = builder.menu(menu);
    builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        // Rust에서만 씀. 기본값은 모든 웹뷰에 target=_blank 링크를 가로채는 스크립트를 넣어
        // 탭 안의 링크가 막힘(opener.open_url not allowed) → 끔.
        .plugin(tauri_plugin_opener::Builder::new().open_js_links_on_click(false).build())
        // The toolbar's right-click menu (`commands::ui_menu`).
        .on_menu_event(|app, e| {
            if let Some(action) = e.id().as_ref().strip_prefix("ctx:") {
                let (app, action) = (app.clone(), action.to_string());
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = commands::run_action(&app, &action).await {
                        commands::toast(&app.state::<Browser>(), e.user_message);
                    }
                });
            }
        })
        .setup(setup)
        .invoke_handler(tauri::generate_handler![
            commands::ui_init,
            commands::shortcut,
            commands::content_event,
            commands::tab_new,
            commands::bookmark_open,
            commands::pane_color,
            commands::warm_get,
            commands::tab_close,
            commands::tab_activate,
            commands::tab_move,
            commands::side_resize,
            commands::side_bookmarks,
            commands::ui_menu,
            siteinfo::site_info_open,
            siteinfo::site_info,
            siteinfo::site_info_size,
            siteinfo::site_info_close,
            neterror::neterror_retry,
            neterror::neterror_search,
            neterror::neterror_proceed,
            downloads::downloads_list,
            downloads::download_action,
            downloads::downloads_clear,
            downloads::downloads_open_dir,
            downloads::downloads_pick_dir,
            downloads::downloads_page,
            portal::portal_get,
            portal::favicons,
            portal::portal_set,
            portal::portal_seed,
            portal::portal_icon,
            portal::portal_export,
            portal::portal_import,
            commands::tab_drag_start,
            commands::tab_drag_move,
            commands::tab_drag_drop,
            commands::navigate,
            commands::omni_suggest,
            commands::omni_show,
            commands::omni_hide,
            commands::omni_pick,
            commands::set_chrome_height,
            commands::palette_open,
            commands::palette_close,
            commands::palette_resize,
            commands::palette_search,
            commands::palette_run,
            commands::toggle_bookmark,
            commands::mcp_info,
            commands::ext_popup,
            meet::meet_host,
            meet::meet_join,
            meet::meet_send,
            meet::meet_admit,
            meet::meet_leave,
            pages::settings_get,
            pages::settings_set,
            pages::theme_get,
            pages::mcp_renew_token,
            pages::copy_text,
            pages::clear_data,
            pages::bookmarks_all,
            pages::bookmark_save,
            pages::bookmark_delete,
            pages::bookmarks_import,
            pages::bookmark_folder_move,
            pages::bookmark_folder_order,
            pages::bookmark_folder_order_set,
            pages::bookmark_folders,
            pages::bookmark_folder_create,
            pages::bookmark_folder_rename,
            pages::bookmark_folder_delete,
            pages::extensions_list,
            pages::extension_install,
            pages::extension_install_folder,
            pages::extension_remove,
            pages::extension_open,
            pages::passwords_list,
            pages::password_reveal,
            pages::password_delete,
            pages::password_never_remove,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Kite");
}
