// Every app command is listed so the capability files decide who may call it: the chrome UI
// and palette get theirs, Kite's own pages in tabs (settings, bookmarks) theirs, and web pages
// in tabs only `content_event`.
const COMMANDS: &[&str] = &[
    "ui_init",
    "shortcut",
    "content_event",
    "tab_new",
    "tab_close",
    "tab_activate",
    "tab_move",
    "navigate",
    "set_chrome_height",
    "palette_open",
    "palette_close",
    "palette_resize",
    "palette_search",
    "palette_run",
    "toggle_bookmark",
    "mcp_info",
    "settings_get",
    "settings_set",
    "mcp_renew_token",
    "copy_text",
    "clear_data",
    "bookmarks_all",
    "bookmark_save",
    "bookmark_delete",
    "bookmarks_import",
];

fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)))
        .expect("tauri build");
}
