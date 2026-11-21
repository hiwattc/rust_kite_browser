//! Kite: a small browser on Tauri v2 (OS webviews: WebView2 on Windows, WKWebView on macOS)
//! with a spotlight palette and an MCP server for agents (Claude Code).

mod automation;
mod browser;
mod commands;
mod error;
mod hangul;
mod keymap;
mod mcp;
mod pages;
mod palette;
mod platform;
mod store;

use std::sync::Arc;

use tauri::webview::WebviewBuilder;
use tauri::{LogicalPosition, LogicalSize, Manager, WebviewUrl, WindowEvent};

use crate::browser::{Browser, PALETTE, UI};
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
    let browser = Browser::new(handle.clone(), store.clone());
    app.manage(browser.clone());
    app.state::<McpHub>().set_app(handle.clone());

    let wb = tauri::window::WindowBuilder::new(app, "main").title("Kite").inner_size(1280.0, 840.0).min_inner_size(560.0, 400.0);
    // macOS: tabs in the title bar area, next to the traffic lights.
    #[cfg(target_os = "macos")]
    let wb = wb.title_bar_style(tauri::TitleBarStyle::Overlay).hidden_title(true);
    let window = wb.build()?;
    let size = window.inner_size()?.to_logical::<f64>(window.scale_factor()?);

    window.add_child(
        WebviewBuilder::new(UI, WebviewUrl::App("index.html".into())).zoom_hotkeys_enabled(false),
        LogicalPosition::new(0.0, 0.0),
        LogicalSize::new(size.width, 82.0),
    )?;
    let palette = window.add_child(
        WebviewBuilder::new(PALETTE, WebviewUrl::App("palette.html".into())).zoom_hotkeys_enabled(false).focused(false),
        LogicalPosition::new(0.0, 0.0),
        LogicalSize::new(640.0, 400.0),
    )?;
    palette.hide()?;

    let b = browser.clone();
    window.on_window_event(move |e| {
        match e {
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => b.layout(),
            // With several webviews in one window none may hold the keyboard after the window
            // is re-activated: give it to the palette or the page, so shortcuts keep working.
            WindowEvent::Focused(true) => b.focus_current(),
            _ => {}
        }
    });

    browser.restore_session()?;
    browser.layout();

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
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "kite_lib=info".into()))
        .init();
    let builder = tauri::Builder::default().manage(McpHub::default());
    #[cfg(target_os = "macos")]
    let builder = builder.menu(menu);
    builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(setup)
        .invoke_handler(tauri::generate_handler![
            commands::ui_init,
            commands::shortcut,
            commands::content_event,
            commands::tab_new,
            commands::tab_close,
            commands::tab_activate,
            commands::tab_move,
            commands::navigate,
            commands::set_chrome_height,
            commands::palette_open,
            commands::palette_close,
            commands::palette_resize,
            commands::palette_search,
            commands::palette_run,
            commands::toggle_bookmark,
            commands::mcp_info,
            pages::settings_get,
            pages::settings_set,
            pages::mcp_renew_token,
            pages::copy_text,
            pages::clear_data,
            pages::bookmarks_all,
            pages::bookmark_save,
            pages::bookmark_delete,
            pages::bookmarks_import,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Kite");
}
