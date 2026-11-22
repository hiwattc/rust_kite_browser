//! Window layout, tabs and their webviews.
//!
//! Every browser window `n` (label `main` for 1, `win-<n>` after) holds several webviews:
//! - `ui` / `ui-<n>`: tab strip + toolbar (local page `index.html`), full width at the top.
//! - `tab-<id>`: one per tab, below the toolbar; only the window's active one is shown. A tab
//!   gets its webview the first time it is shown or automated (restored tabs start without one).
//!   Dragging a tab out of the strip moves its webview to another window (`reparent`).
//! - `palette` / `palette-<n>`: the spotlight overlay, kept hidden and brought to the front when
//!   opened (one open at a time, in the focused window).
//!
//! Rust owns the tab state; each UI only renders its window's `browser-state` event and sends
//! commands. "The active tab" without more context is the focused window's.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::webview::{NewWindowResponse, PageLoadEvent, WebviewBuilder};
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Rect, Webview, WebviewUrl, Window};
use tokio::sync::Notify;

use crate::error::AppError;
use crate::palette;
use crate::store::Store;

pub const UI: &str = "ui";
pub const PALETTE: &str = "palette";
const DEFAULT_CHROME_HEIGHT: f64 = 82.0;
const PALETTE_WIDTH: f64 = 680.0;
const PALETTE_TOP: f64 = 64.0;
/// Shown in the address bar for the new-tab page.
pub const NEWTAB: &str = "kite://newtab";
/// WKWebView's default UA has no `Version/… Safari/…`: some sites (Google among them) then
/// serve a reduced page. Claim the Safari that ships with this macOS (same WebKit as ours): a
/// stale version next to a newer engine looks like a spoofed browser to Google's sign-in, which
/// then asks for extra verification.
#[cfg(target_os = "macos")]
fn safari_ua() -> &'static str {
    static UA: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    UA.get_or_init(|| {
        let version = std::fs::read_to_string("/Applications/Safari.app/Contents/Info.plist")
            .ok()
            .and_then(|p| safari_version(&p))
            .unwrap_or_else(|| "18.5".into());
        format!("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/{version} Safari/605.1.15")
    })
}

/// `CFBundleShortVersionString` from an XML Info.plist (`26.2`).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn safari_version(plist: &str) -> Option<String> {
    let after = &plist[plist.find("<key>CFBundleShortVersionString</key>")?..];
    let start = after.find("<string>")? + "<string>".len();
    let v = &after[start..start + after[start..].find("</string>")?];
    (!v.is_empty() && v.chars().all(|c| c.is_ascii_digit() || c == '.')).then(|| v.to_string())
}
/// Popup windows: `window.close()` and ⌘W/Ctrl+W close them by navigating to this scheme, which
/// Kite catches. macOS: content.js does it (a popup inherits the opener's scripts, and WKWebView
/// ignores `window.close()`); Windows: POPUP_JS (the popup webview takes our own scripts).
const POPUP_CLOSE_SCHEME: &str = "kite-popup-close";
#[cfg(not(target_os = "macos"))]
const POPUP_JS: &str = r#"(function () {
  if (window.top !== window) return;
  var close = function () { location.href = "kite-popup-close:" + Date.now(); };
  try { window.close = close; } catch (e) {}
  window.addEventListener("keydown", function (e) {
    if ((e.metaKey || e.ctrlKey) && !e.shiftKey && !e.altKey && e.code === "KeyW") { e.preventDefault(); close(); }
  }, true);
})();"#;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tab {
    pub id: u64,
    /// What the user sees (`kite://newtab` for the start page).
    pub url: String,
    pub title: String,
    pub loading: bool,
    pub favicon: Option<String>,
    pub zoom: f64,
    pub bookmarked: bool,
    /// Has a webview (restored tabs get one when first shown).
    pub live: bool,
    /// Bumped when a page finishes loading (automation waits on this).
    #[serde(skip)]
    pub loads: u64,
    /// Bumped when a load starts; the load watchdog of an older load stops.
    #[serde(skip)]
    pub load_gen: u64,
    /// Window number the tab is in.
    #[serde(skip)]
    pub win: u32,
    /// macOS ended its page's process while hidden (memory): reload when shown again.
    #[serde(skip)]
    pub purged: bool,
    /// Address Kite asked for and not shown yet (cleared when a page commits).
    #[serde(skip)]
    pub pending: Option<String>,
    /// Showing the "cannot reach" page for `error_url`.
    pub error: bool,
    #[serde(skip)]
    pub error_url: Option<String>,
    /// Showing a site whose certificate the user chose to accept anyway.
    pub bad_cert: bool,
}

/// One button of the bookmark bar: a link, or a folder (`url` = folder name).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BarItem {
    pub kind: String,
    pub title: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserState {
    pub tabs: Vec<Tab>,
    pub active: Option<u64>,
    /// Tab the agent (MCP) works on, if any.
    pub agent_tab: Option<u64>,
    pub mcp: crate::mcp::Status,
    /// The vertical tab list on the left is open.
    pub side: bool,
}

#[derive(Clone)]
struct Floating {
    /// Window following the pointer and the tab in it.
    win: u32,
    id: u64,
    /// Pointer position inside the window's content.
    off: (f64, f64),
    /// Strip under the pointer at the last move (outer None = not checked yet).
    over: Option<Option<u32>>,
}

struct Win {
    active: Option<u64>,
    chrome_height: f64,
    /// Outer position + inner size (logical), kept from move/resize events for the session.
    bounds: Option<[f64; 4]>,
}

#[derive(Default)]
struct State {
    /// All tabs; a window's tabs in this order are its strip.
    tabs: Vec<Tab>,
    wins: BTreeMap<u32, Win>,
    /// Window the user works in (last focused): shortcuts, palette and new tabs go there.
    focused: u32,
    agent_tab: Option<u64>,
    closed: Vec<String>,
    /// Window whose palette is open.
    palette_win: Option<u32>,
    palette_height: f64,
    /// Internal page a tab may open next (set by the app right before navigating there).
    internal_pass: std::collections::HashMap<u64, &'static str>,
}

pub struct Inner {
    pub app: AppHandle,
    pub store: Arc<Store>,
    state: Mutex<State>,
    next_id: AtomicU64,
    next_win: AtomicU32,
    /// The window of a torn-off tab while it is dragged.
    floating: Mutex<Option<Floating>>,
    /// Debounces session saves while a window is moved or resized.
    bounds_gen: AtomicU64,
    /// Woken whenever a tab's load state changes.
    pub changed: Notify,
    content_js: String,
    /// Last picture and scroll position of each tab, to stand in while a tab whose page
    /// process macOS ended reloads (see `revive`).
    shots: Mutex<std::collections::HashMap<u64, Shot>>,
    /// Window → tab being revived under the cover.
    reviving: Mutex<std::collections::HashMap<u32, u64>>,
    /// Vertical tab list: open, width (same for every window; kept in the store).
    side: Mutex<(bool, f64)>,
    /// Windows whose tab list page has started (asked for its state).
    side_ready: Mutex<std::collections::HashSet<u32>>,
}

/// How a tab looked a moment ago.
#[derive(Clone)]
struct Shot {
    url: String,
    /// `data:image/jpeg;base64,…`
    image: String,
    scroll: (f64, f64),
}

/// The vertical tab list on the left of a window's pages.
pub fn side_label(n: u32) -> String {
    if n == 1 { "side".into() } else { format!("side-{n}") }
}

pub const SIDE_DEFAULT: f64 = 260.0;

/// Stand-in picture over the page area while a purged tab reloads.
pub fn cover_label(n: u32) -> String {
    if n == 1 { "cover".into() } else { format!("cover-{n}") }
}

#[derive(Clone)]
pub struct Browser(pub Arc<Inner>);

pub fn tab_label(id: u64) -> String {
    format!("tab-{id}")
}

pub fn tab_id_of(label: &str) -> Option<u64> {
    label.strip_prefix("tab-")?.parse().ok()
}

pub fn win_label(n: u32) -> String {
    if n == 1 { "main".into() } else { format!("win-{n}") }
}

pub fn ui_label(n: u32) -> String {
    if n == 1 { UI.into() } else { format!("{UI}-{n}") }
}

pub fn palette_label(n: u32) -> String {
    if n == 1 { PALETTE.into() } else { format!("{PALETTE}-{n}") }
}

/// Windows whose suggestion list is showing (`Webview` has no `is_visible`).
static OMNI_SHOWN: Mutex<std::collections::BTreeSet<u32>> = Mutex::new(std::collections::BTreeSet::new());

/// The address bar's suggestion list (an overlay webview over the page, like the palette).
pub fn omni_label(n: u32) -> String {
    if n == 1 { "omni".into() } else { format!("omni-{n}") }
}

/// Window number of a window label (`main`, `win-<n>`).
pub fn win_of_label(label: &str) -> Option<u32> {
    if label == "main" { Some(1) } else { label.strip_prefix("win-")?.parse().ok() }
}

/// A toast in the focused window.
pub fn toast(app: &AppHandle, msg: impl Into<String>) {
    let label = app.try_state::<Browser>().map(|b| ui_label(b.focused())).unwrap_or_else(|| UI.into());
    let _ = app.emit_to(label, "toast", msg.into());
}

/// Kite's own pages shown in tabs: `kite://<name>` ↔ `<name>.html` of the app.
pub const INTERNAL: [&str; 6] = ["newtab", "settings", "bookmarks", "meet", "neterror", "home"];

/// Which internal page an app URL is (dev server or bundled assets).
fn internal_page(u: &url::Url) -> Option<&'static str> {
    if !is_app_url(u) {
        return None;
    }
    let name = u.path().strip_prefix('/')?.strip_suffix(".html")?;
    INTERNAL.iter().copied().find(|p| *p == name)
}

#[cfg(test)]
fn is_newtab_url(u: &url::Url) -> bool {
    internal_page(u) == Some("newtab")
}

/// `kite://settings`, `chrome://settings`, `about:settings` (and `edge://`) → `settings`.
pub fn kite_page(input: &str) -> Option<&'static str> {
    let s = input.trim().to_ascii_lowercase();
    let rest = ["kite://", "chrome://", "edge://", "about:"].iter().find_map(|p| s.strip_prefix(p))?;
    let name = rest.trim_end_matches('/').split(['/', '?', '#']).next().unwrap_or("");
    let name = match name {
        "newtab" | "new-tab-page" | "blank" => "newtab",
        "settings" | "preferences" | "prefs" | "extensions" | "extension" | "passwords" | "password-manager" => "settings",
        "bookmarks" | "favorites" => "bookmarks",
        "meet" | "meeting" | "call" => "meet",
        "home" | "portal" => "home",
        _ => return None,
    };
    INTERNAL.iter().copied().find(|p| *p == name)
}

/// Pages of the app itself (the chrome UI, palette): never shown in a content tab.
fn is_app_url(u: &url::Url) -> bool {
    u.scheme() == "tauri" || matches!(u.host_str(), Some("tauri.localhost")) || (cfg!(debug_assertions) && u.host_str() == Some("localhost") && u.port() == Some(1440))
}

/// Schemes the webview loads itself. Anything else (`f5-epi:`, `zoommtg:`, `mailto:` …) belongs
/// to another app: WKWebView/WebView2 would just fail on it, so Kite hands it to the OS instead.
fn is_web_scheme(scheme: &str) -> bool {
    matches!(
        scheme,
        "http" | "https" | "about" | "data" | "blob" | "file" | "javascript" | "view-source" | "ws" | "wss" | "tauri" | "asset" | "ipc" | "webkit-extension" | "chrome-extension"
    )
}

/// One "open in external app?" question at a time: a page retrying in a loop gets no second dialog.
static EXTERNAL_PROMPT: AtomicBool = AtomicBool::new(false);

/// `kite://extensions`, `kite://passwords` (and the `chrome://` names) open the settings page at
/// that section.
pub fn settings_section(input: &str) -> Option<&'static str> {
    let s = input.trim().to_ascii_lowercase();
    if ["://extension", ":extension"].iter().any(|p| s.contains(p)) {
        return Some("extensions");
    }
    ["://password", ":password"].iter().any(|p| s.contains(p)).then_some("passwords")
}

pub fn display_url(u: &url::Url) -> String {
    match internal_page(u) {
        Some(p) => format!("kite://{p}"),
        None => u.to_string(),
    }
}

impl Browser {
    pub fn new(app: AppHandle, store: Arc<Store>) -> Self {
        let side = (store.get("side_tabs").as_deref() == Some("true"), store.get("side_width").and_then(|w| w.parse().ok()).unwrap_or(SIDE_DEFAULT));
        let content_js = include_str!("inject/content.js").replace("__KITE_KEYMAP__", &crate::keymap::json_map());
        Self(Arc::new(Inner {
            app,
            store,
            state: Mutex::new(State { palette_height: 420.0, focused: 1, ..Default::default() }),
            next_id: AtomicU64::new(1),
            next_win: AtomicU32::new(1),
            bounds_gen: AtomicU64::new(0),
            floating: Mutex::new(None),
            changed: Notify::new(),
            content_js,
            shots: Mutex::new(Default::default()),
            reviving: Mutex::new(Default::default()),
            side: Mutex::new(side),
            side_ready: Mutex::new(Default::default()),
        }))
    }

    pub fn content_js(&self) -> &str {
        &self.0.content_js
    }

    pub fn window(&self, n: u32) -> Option<Window> {
        self.0.app.get_window(&win_label(n))
    }

    /// The focused browser window (dialogs attach to it).
    pub fn current_window(&self) -> Option<Window> {
        self.window(self.focused())
    }

    /// The focused window's chrome UI webview (any window's if that one is gone).
    pub fn ui_webview(&self) -> Option<Webview> {
        self.0.app.get_webview(&ui_label(self.focused())).or_else(|| self.wins().into_iter().find_map(|n| self.0.app.get_webview(&ui_label(n))))
    }

    pub fn webview(&self, id: u64) -> Option<Webview> {
        self.0.app.get_webview(&tab_label(id))
    }

    pub fn wins(&self) -> Vec<u32> {
        self.0.state.lock().unwrap().wins.keys().copied().collect()
    }

    pub fn focused(&self) -> u32 {
        self.0.state.lock().unwrap().focused
    }

    /// The window a chrome/palette/tab webview belongs to becomes the one commands act on.
    pub fn touch(&self, wv: &Webview) {
        if let Some(n) = win_of_label(wv.window().label()) {
            let mut s = self.0.state.lock().unwrap();
            if s.wins.contains_key(&n) {
                s.focused = n;
            }
        }
    }

    /// State as window `n` shows it.
    pub fn snapshot(&self, n: u32) -> BrowserState {
        let s = self.0.state.lock().unwrap();
        BrowserState {
            tabs: s.tabs.iter().filter(|t| t.win == n).cloned().collect(),
            active: s.wins.get(&n).and_then(|w| w.active),
            agent_tab: s.agent_tab,
            mcp: self.0.app.try_state::<crate::mcp::McpHub>().map(|h| h.status()).unwrap_or_default(),
            side: self.0.side.lock().unwrap().0,
        }
    }

    pub fn emit(&self) {
        for n in self.wins() {
            let snap = self.snapshot(n);
            if snap.side {
                let _ = self.0.app.emit_to(side_label(n), "browser-state", snap.clone());
            }
            let _ = self.0.app.emit_to(ui_label(n), "browser-state", snap);
        }
        self.0.changed.notify_waiters();
    }

    /// An event for every window's chrome UI.
    pub fn emit_all_ui<S: Serialize + Clone>(&self, event: &str, payload: S) {
        for n in self.wins() {
            let _ = self.0.app.emit_to(ui_label(n), event, payload.clone());
        }
    }

    pub fn tab(&self, id: u64) -> Option<Tab> {
        self.0.state.lock().unwrap().tabs.iter().find(|t| t.id == id).cloned()
    }

    pub fn tabs(&self) -> Vec<Tab> {
        self.0.state.lock().unwrap().tabs.clone()
    }

    /// The focused window's active tab.
    pub fn active(&self) -> Option<u64> {
        let s = self.0.state.lock().unwrap();
        s.wins.get(&s.focused).and_then(|w| w.active).or_else(|| s.wins.values().find_map(|w| w.active))
    }

    /// Tabs of window `n`, in strip order.
    fn win_tabs(&self, n: u32) -> Vec<u64> {
        self.0.state.lock().unwrap().tabs.iter().filter(|t| t.win == n).map(|t| t.id).collect()
    }

    pub fn agent_tab(&self) -> Option<u64> {
        let s = self.0.state.lock().unwrap();
        s.agent_tab.filter(|id| s.tabs.iter().any(|t| t.id == *id))
    }

    pub fn set_agent_tab(&self, id: Option<u64>) {
        self.0.state.lock().unwrap().agent_tab = id;
        self.emit();
    }

    fn update(&self, id: u64, f: impl FnOnce(&mut Tab)) {
        {
            let mut s = self.0.state.lock().unwrap();
            if let Some(t) = s.tabs.iter_mut().find(|t| t.id == id) {
                f(t);
            }
        }
        self.emit();
    }

    // ---------- layout ----------

    /// Window content size in logical pixels.
    fn content_size(&self, n: u32) -> Option<(f64, f64)> {
        let w = self.window(n)?;
        let scale = w.scale_factor().ok()?;
        let size = w.inner_size().ok()?.to_logical::<f64>(scale);
        Some((size.width, size.height))
    }

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect { position: LogicalPosition::new(x, y).into(), size: LogicalSize::new(w.max(1.0), h.max(1.0)).into() }
    }

    pub fn layout_all(&self) {
        for n in self.wins() {
            self.layout(n);
        }
    }

    /// Places window `n`'s webviews.
    pub fn layout(&self, n: u32) {
        let Some((w, h)) = self.content_size(n) else { return };
        let (chrome, active, live, palette_open, palette_h) = {
            let s = self.0.state.lock().unwrap();
            let Some(win) = s.wins.get(&n) else { return };
            let live: Vec<u64> = s.tabs.iter().filter(|t| t.live && t.win == n).map(|t| t.id).collect();
            (win.chrome_height, win.active, live, s.palette_win == Some(n), s.palette_height)
        };
        if let Some(ui) = self.0.app.get_webview(&ui_label(n)) {
            let _ = ui.set_bounds(Self::rect(0.0, 0.0, w, chrome));
        }
        // The vertical tab list takes the left of the page area.
        let sx = self.side_x(w);
        if let Some(side) = self.0.app.get_webview(&side_label(n)) {
            if sx > 0.0 {
                let _ = side.set_bounds(Self::rect(0.0, chrome, sx, h - chrome));
                let _ = side.show();
            } else {
                let _ = side.hide();
            }
        }
        for id in live {
            if let Some(wv) = self.webview(id) {
                if Some(id) == active {
                    let _ = wv.set_bounds(Self::rect(sx, chrome, w - sx, h - chrome));
                    let _ = wv.show();
                } else {
                    let _ = wv.hide();
                }
            }
        }
        if palette_open && let Some(p) = self.0.app.get_webview(&palette_label(n)) {
            let pw = PALETTE_WIDTH.min(w - 24.0);
            let ph = palette_h.min(h - PALETTE_TOP - 16.0);
            let _ = p.set_bounds(Self::rect(((w - pw) / 2.0).round(), PALETTE_TOP, pw, ph));
        }
    }

    /// The tab list page of a window started (its `ui_init`).
    pub fn side_started(&self, label: &str) {
        if let Some(n) = label.strip_prefix("side").map(|r| r.strip_prefix('-').and_then(|x| x.parse().ok()).unwrap_or(1)) {
            self.0.side_ready.lock().unwrap().insert(n);
        }
    }

    /// Width the vertical tab list takes in a window `w` wide (0 when closed).
    fn side_x(&self, w: f64) -> f64 {
        let (open, width) = *self.0.side.lock().unwrap();
        if open { width.clamp(180.0, (w - 320.0).max(180.0)).round() } else { 0.0 }
    }

    pub fn side_open(&self) -> bool {
        self.0.side.lock().unwrap().0
    }

    /// Opens or closes the vertical tab list in every window.
    pub fn toggle_side(&self) -> Result<bool, AppError> {
        let open = {
            let mut s = self.0.side.lock().unwrap();
            s.0 = !s.0;
            s.0
        };
        self.0.store.set("side_tabs", if open { "true" } else { "false" })?;
        if open {
            // A list page that never started (e.g. it failed to load): load it again.
            let ready = self.0.side_ready.lock().unwrap().clone();
            for n in self.wins().into_iter().filter(|n| !ready.contains(n)) {
                if let Some(side) = self.0.app.get_webview(&side_label(n)) {
                    tracing::info!(n, "tab list page not ready; reloading it");
                    let _ = side.reload();
                }
            }
        }
        self.layout_all();
        self.emit();
        if !open {
            self.focus_current();
        }
        Ok(open)
    }

    /// Drag on the list's edge. `save`: the drag ended.
    pub fn resize_side(&self, width: f64, save: bool) {
        let w = width.clamp(180.0, 520.0).round();
        self.0.side.lock().unwrap().1 = w;
        self.layout_all();
        if save {
            let _ = self.0.store.set("side_width", &w.to_string());
        }
    }

    pub fn set_chrome_height(&self, n: u32, h: f64) {
        if let Some(w) = self.0.state.lock().unwrap().wins.get_mut(&n) {
            w.chrome_height = h.clamp(30.0, 300.0);
        }
        self.layout(n);
    }

    // ---------- windows ----------

    /// Opens a browser window (no tabs yet). `pos`/`size` logical; None = OS default / 1280×840.
    pub fn create_window(&self, pos: Option<(f64, f64)>, size: Option<(f64, f64)>) -> Result<u32, AppError> {
        let err = |e: tauri::Error| AppError::new("창을 열지 못했습니다.", e);
        let n = self.0.next_win.fetch_add(1, Ordering::SeqCst);
        let (w, h) = size.unwrap_or((1280.0, 840.0));
        let mut wb = tauri::window::WindowBuilder::new(&self.0.app, win_label(n)).title("Kite").inner_size(w, h).min_inner_size(560.0, 400.0);
        if let Some((x, y)) = pos {
            wb = wb.position(x, y);
        }
        // macOS: tabs in the title bar area, next to the traffic lights.
        #[cfg(target_os = "macos")]
        let wb = wb.title_bar_style(tauri::TitleBarStyle::Overlay).hidden_title(true);
        let window = wb.build().map_err(err)?;
        self.0.state.lock().unwrap().wins.insert(n, Win { active: None, chrome_height: DEFAULT_CHROME_HEIGHT, bounds: None });
        self.note_bounds(n, &window);
        window
            .add_child(
                // accept_first_mouse: on an inactive window a tab drag must drag the tab, not
                // the window (the strip lies in the macOS title bar area)
                WebviewBuilder::new(ui_label(n), WebviewUrl::App("index.html".into())).zoom_hotkeys_enabled(false).browser_extensions_enabled(true).accept_first_mouse(true),
                LogicalPosition::new(0.0, 0.0),
                LogicalSize::new(w, DEFAULT_CHROME_HEIGHT),
            )
            .map_err(err)?;
        let palette = window
            .add_child(
                WebviewBuilder::new(palette_label(n), WebviewUrl::App("palette.html".into())).zoom_hotkeys_enabled(false).focused(false).browser_extensions_enabled(true),
                LogicalPosition::new(0.0, 0.0),
                LogicalSize::new(640.0, 400.0),
            )
            .map_err(err)?;
        let _ = palette.hide();
        let omni = window
            .add_child(
                WebviewBuilder::new(omni_label(n), WebviewUrl::App("omni.html".into())).zoom_hotkeys_enabled(false).focused(false).accept_first_mouse(true).browser_extensions_enabled(true),
                LogicalPosition::new(0.0, 0.0),
                LogicalSize::new(400.0, 200.0),
            )
            .map_err(err)?;
        let _ = omni.hide();
        let cover = window
            .add_child(
                WebviewBuilder::new(cover_label(n), WebviewUrl::App("cover.html".into())).zoom_hotkeys_enabled(false).focused(false).browser_extensions_enabled(true),
                LogicalPosition::new(0.0, 0.0),
                LogicalSize::new(400.0, 300.0),
            )
            .map_err(err)?;
        let _ = cover.hide();
        let side = window
            .add_child(
                WebviewBuilder::new(side_label(n), WebviewUrl::App("side.html".into())).zoom_hotkeys_enabled(false).focused(false).accept_first_mouse(true).browser_extensions_enabled(true),
                LogicalPosition::new(0.0, DEFAULT_CHROME_HEIGHT),
                LogicalSize::new(SIDE_DEFAULT, 400.0),
            )
            .map_err(err)?;
        let _ = side.hide();
        let b = self.clone();
        window.on_window_event(move |e| match e {
            tauri::WindowEvent::Resized(_) | tauri::WindowEvent::ScaleFactorChanged { .. } => {
                b.layout(n);
                if let Some(w) = b.window(n) {
                    b.note_bounds(n, &w);
                }
            }
            tauri::WindowEvent::Moved(_) => {
                if let Some(w) = b.window(n) {
                    b.note_bounds(n, &w);
                }
            }
            // With several webviews in one window none may hold the keyboard after the window
            // is re-activated: give it to the palette or the page, so shortcuts keep working.
            tauri::WindowEvent::Focused(true) => {
                b.0.state.lock().unwrap().focused = n;
                b.focus_current();
            }
            tauri::WindowEvent::CloseRequested { .. } => b.window_closing(n),
            tauri::WindowEvent::Destroyed => b.window_gone(n),
            _ => {}
        });
        Ok(n)
    }

    /// Remembers where window `n` is (saved with the session shortly after it stops moving).
    fn note_bounds(&self, n: u32, w: &Window) {
        let Ok(scale) = w.scale_factor() else { return };
        let (Ok(p), Ok(sz)) = (w.outer_position(), w.inner_size()) else { return };
        if w.is_minimized().unwrap_or(false) {
            return;
        }
        let (p, sz) = (p.to_logical::<f64>(scale), sz.to_logical::<f64>(scale));
        if let Some(win) = self.0.state.lock().unwrap().wins.get_mut(&n) {
            win.bounds = Some([p.x, p.y, sz.width, sz.height]);
        }
        let generation = self.0.bounds_gen.fetch_add(1, Ordering::SeqCst) + 1;
        let b = self.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_millis(600)).await;
            if b.0.bounds_gen.load(Ordering::SeqCst) == generation {
                b.save_session();
            }
        });
    }

    /// A new window a bit below/right of the focused one (⌘N).
    pub fn new_window(&self) -> Result<u32, AppError> {
        let (pos, size) = match self.current_window() {
            Some(w) => {
                let scale = w.scale_factor().unwrap_or(1.0);
                let p = w.outer_position().ok().map(|p| p.to_logical::<f64>(scale)).map(|p| (p.x + 30.0, p.y + 30.0));
                let sz = w.inner_size().ok().map(|s| s.to_logical::<f64>(scale)).map(|s| (s.width, s.height));
                (p, sz)
            }
            None => (None, None),
        };
        let n = self.create_window(pos, size)?;
        self.new_tab_in(n, None, true)?;
        Ok(n)
    }

    /// The user closes window `n`: its tabs go with it (unless it is the last window: the app
    /// quits then and the session keeps them).
    fn window_closing(&self, n: u32) {
        {
            let mut s = self.0.state.lock().unwrap();
            if s.wins.len() <= 1 || !s.wins.contains_key(&n) {
                return;
            }
            s.wins.remove(&n);
            s.tabs.retain(|t| t.win != n);
            if s.agent_tab.is_some_and(|a| !s.tabs.iter().any(|t| t.id == a)) {
                s.agent_tab = None;
            }
            if s.palette_win == Some(n) {
                s.palette_win = None;
            }
            if s.focused == n {
                s.focused = *s.wins.keys().next().unwrap_or(&1);
            }
        }
        self.save_session();
        self.emit();
    }

    fn window_gone(&self, n: u32) {
        let mut s = self.0.state.lock().unwrap();
        if s.wins.len() > 1 && s.wins.remove(&n).is_some() {
            s.tabs.retain(|t| t.win != n);
        }
    }

    /// The browser window whose tab strip is under screen point (`x`, `y`) (logical), other
    /// than `except`.
    fn strip_at(&self, x: f64, y: f64, except: u32) -> Option<u32> {
        self.wins().into_iter().filter(|n| *n != except).find(|n| {
            let Some(w) = self.window(*n) else { return false };
            let scale = w.scale_factor().unwrap_or(1.0);
            let (Ok(p), Ok(sz)) = (w.inner_position(), w.inner_size()) else { return false };
            let (p, sz) = (p.to_logical::<f64>(scale), sz.to_logical::<f64>(scale));
            let chrome = self.0.state.lock().unwrap().wins.get(n).map(|w| w.chrome_height).unwrap_or(DEFAULT_CHROME_HEIGHT);
            // the tab strip (plus a little above it: the title bar on Windows)
            x >= p.x && x <= p.x + sz.width && y >= p.y - 40.0 && y <= p.y + chrome.min(48.0)
        })
    }

    /// A tab is pulled out of its strip, pointer at screen (`x`, `y`): from now on a
    /// see-through window follows the pointer, like Chrome. A tab with siblings moves into a new
    /// window right away; the only tab of a window takes its whole window along.
    /// (`ox`, `oy`): where in that window's content the pointer should sit.
    pub fn drag_start(&self, id: u64, x: f64, y: f64, ox: f64, oy: f64) -> Result<(), AppError> {
        let tab = self.tab(id).ok_or_else(|| AppError::new("탭이 없습니다.", id))?;
        let from = tab.win;
        let n = if self.win_tabs(from).len() <= 1 {
            from
        } else {
            let size = self.content_size(from);
            let n = self.create_window(Some((x - ox, y - oy)), size)?;
            self.move_to_window(id, n)?;
            n
        };
        *self.0.floating.lock().unwrap() = Some(Floating { win: n, id, off: (ox, oy), over: None });
        self.drag_move(x, y);
        Ok(())
    }

    /// Pointer moved while a torn-off tab is dragged: its window follows (main thread).
    pub fn drag_move(&self, x: f64, y: f64) {
        let Some(f) = self.0.floating.lock().unwrap().clone() else { return };
        let Some(w) = self.window(f.win) else { return };
        let scale = w.scale_factor().unwrap_or(1.0);
        // content offset inside the frame (the native title bar on Windows)
        let (dx, dy) = match (w.outer_position(), w.inner_position()) {
            (Ok(o), Ok(i)) => {
                let (o, i) = (o.to_logical::<f64>(scale), i.to_logical::<f64>(scale));
                (i.x - o.x, i.y - o.y)
            }
            _ => (0.0, 0.0),
        };
        let _ = w.set_position(LogicalPosition::new(x - f.off.0 - dx, y - f.off.1 - dy));
        // over another window's strip: fainter (letting go there puts the tab into that window)
        let over = self.strip_at(x, y, f.win);
        if f.over != Some(over) {
            crate::platform::set_window_alpha(&w, if over.is_some() { 0.45 } else { 0.75 });
            if let Some(g) = self.0.floating.lock().unwrap().as_mut() {
                g.over = Some(over);
            }
        }
    }

    /// The dragged tab is let go at (`x`, `y`): into the window whose strip is there, else it
    /// stays in its own window.
    pub fn drag_drop(&self, x: f64, y: f64) -> Result<(), AppError> {
        let Some(f) = self.0.floating.lock().unwrap().take() else { return Ok(()) };
        if let Some(w) = self.window(f.win) {
            crate::platform::set_window_alpha(&w, 1.0);
        }
        match self.strip_at(x, y, f.win) {
            Some(n) => self.move_to_window(f.id, n),
            None => {
                if let Some(w) = self.window(f.win) {
                    let _ = w.set_focus();
                }
                self.save_session();
                Ok(())
            }
        }
    }

    /// Moves a tab (and its webview, page state intact) to window `to`, at the end of its strip.
    fn move_to_window(&self, id: u64, to: u32) -> Result<(), AppError> {
        let window = self.window(to).ok_or_else(|| AppError::new("창이 없습니다.", to))?;
        if let Some(wv) = self.webview(id) {
            wv.reparent(&window).map_err(|e| AppError::new("탭을 옮기지 못했습니다.", e))?;
        }
        let (from, next) = {
            let mut s = self.0.state.lock().unwrap();
            let Some(pos) = s.tabs.iter().position(|t| t.id == id) else { return Ok(()) };
            let mut t = s.tabs.remove(pos);
            let from = t.win;
            t.win = to;
            s.tabs.push(t);
            let rest: Vec<u64> = s.tabs.iter().filter(|t| t.win == from).map(|t| t.id).collect();
            let was_active = s.wins.get(&from).and_then(|w| w.active) == Some(id);
            // the neighbour that took its place in the old strip
            let next = if was_active {
                let before = s.tabs[..pos].iter().filter(|t| t.win == from).count();
                rest.get(before).or_else(|| rest.get(before.wrapping_sub(1))).copied()
            } else {
                None
            };
            if was_active && let Some(w) = s.wins.get_mut(&from) {
                w.active = next;
            }
            (from, next)
        };
        if let Some(n) = next {
            self.ensure_live(n)?;
        }
        self.layout(from);
        if self.win_tabs(from).is_empty()
            && let Some(w) = self.window(from)
        {
            let _ = w.close();
        }
        self.activate(id)?;
        let _ = window.set_focus();
        Ok(())
    }

    // ---------- tabs ----------

    /// Adds a tab (webview made right away when `activate`). `url` None = new-tab page.
    pub fn new_tab(&self, url: Option<String>, activate: bool) -> Result<u64, AppError> {
        self.new_tab_in(self.focused(), url, activate)
    }

    /// Adds a tab to window `win`, after its active tab.
    pub fn new_tab_in(&self, win: u32, url: Option<String>, activate: bool) -> Result<u64, AppError> {
        let id = self.0.next_id.fetch_add(1, Ordering::SeqCst);
        let shown = url.clone().unwrap_or_else(|| NEWTAB.to_string());
        {
            let mut s = self.0.state.lock().unwrap();
            let active = s.wins.get(&win).and_then(|w| w.active);
            let pos = active.and_then(|a| s.tabs.iter().position(|t| t.id == a)).map(|p| p + 1).unwrap_or(s.tabs.len());
            let bookmarked = url.as_deref().is_some_and(|u| self.0.store.is_bookmarked(u));
            s.tabs.insert(
                pos,
                Tab { id, url: shown, title: String::new(), loading: url.is_some(), favicon: None, zoom: 1.0, bookmarked, live: false, loads: 0, load_gen: 0, win, purged: false, pending: None, error: false, error_url: None, bad_cert: false },
            );
        }
        if activate {
            self.activate(id)?;
        } else {
            self.save_session();
            self.emit();
        }
        Ok(id)
    }

    /// Adds a tab that stays without a webview until shown (session restore).
    fn add_restored(&self, url: String, win: u32) -> u64 {
        let id = self.0.next_id.fetch_add(1, Ordering::SeqCst);
        let bookmarked = self.0.store.is_bookmarked(&url);
        let title = self.0.store.history_search(&url, 1).into_iter().find(|v| v.url == url).map(|v| v.title).unwrap_or_default();
        self.0.state.lock().unwrap().tabs.push(Tab { id, url, title, loading: false, favicon: None, zoom: 1.0, bookmarked, live: false, loads: 0, load_gen: 0, win, purged: false, pending: None, error: false, error_url: None, bad_cert: false });
        id
    }

    /// Makes sure the tab has its webview.
    pub fn ensure_live(&self, id: u64) -> Result<Webview, AppError> {
        if let Some(wv) = self.webview(id) {
            return Ok(wv);
        }
        let tab = self.tab(id).ok_or_else(|| AppError::new("탭이 없습니다.", format!("tab {id}")))?;
        let window = self.window(tab.win).ok_or_else(|| AppError::new("창이 없습니다.", format!("window {}", tab.win)))?;
        let target = if let Some(p) = kite_page(&tab.url) {
            self.0.state.lock().unwrap().internal_pass.insert(id, p);
            WebviewUrl::App(format!("{p}.html").into())
        } else {
            WebviewUrl::External(url::Url::parse(&tab.url).map_err(|e| AppError::new("주소가 올바르지 않습니다.", e))?)
        };
        // macOS: the tab's WKWebViewConfiguration carries the extension controller.
        let ext_config = crate::platform::ext_tab_config(&self.0.app, &tab.url);
        let builder = self.content_builder(id, target);
        #[cfg(target_os = "macos")]
        let builder = match ext_config {
            Some(c) => builder.with_webview_configuration(c.0),
            None => builder,
        };
        #[cfg(not(target_os = "macos"))]
        let _ = ext_config;
        let (w, h) = self.content_size(tab.win).unwrap_or((1200.0, 800.0));
        let chrome = self.0.state.lock().unwrap().wins.get(&tab.win).map(|w| w.chrome_height).unwrap_or(DEFAULT_CHROME_HEIGHT);
        let wv = window
            .add_child(builder, LogicalPosition::new(0.0, chrome), LogicalSize::new(w, (h - chrome).max(1.0)))
            .map_err(|e| AppError::new("탭을 만들지 못했습니다.", e))?;
        crate::platform::ext_webview_created(&self.0.app, &tab.url);
        crate::platform::install_cert_exceptions(&wv);
        {
            let mut s = self.0.state.lock().unwrap();
            if let Some(t) = s.tabs.iter_mut().find(|t| t.id == id) {
                t.live = true;
                if tab.url != NEWTAB {
                    t.loading = true;
                }
            }
        }
        // A palette that is open stays above the new webview.
        if self.0.state.lock().unwrap().palette_win == Some(tab.win) {
            self.raise_palette(tab.win);
        }
        // A restored tab (or one made at its address) can fail to load too.
        if tab.url.starts_with("http") {
            self.update(id, |t| t.pending = Some(tab.url.clone()));
            self.watch_failure(id, tab.url.clone());
        }
        Ok(wv)
    }

    fn content_builder(&self, id: u64, url: WebviewUrl) -> WebviewBuilder<tauri::Wry> {
        let b = self.clone();
        let b2 = self.clone();
        let b3 = self.clone();
        let b4 = self.clone();
        let b5 = self.clone();
        let b6 = self.clone();
        let builder = WebviewBuilder::new(tab_label(id), url);
        // macOS: see `safari_ua`. WebView2's default UA (Chrome + Edg) is fine as is.
        #[cfg(target_os = "macos")]
        let builder = builder.user_agent(safari_ua());
        builder
            // Windows: WebView2 extensions (the setting must match on every webview).
            .browser_extensions_enabled(true)
            .initialization_script(self.0.content_js.clone())
            .devtools(true)
            .zoom_hotkeys_enabled(false)
            .focused(true)
            .on_page_load(move |wv, payload| {
                let Some(id) = tab_id_of(wv.label()) else { return };
                let shown = display_url(payload.url());
                tracing::debug!(id, event = ?payload.event(), url = %payload.url(), "page load");
                match payload.event() {
                    PageLoadEvent::Started => {
                        let mut generation = 0;
                        let is_error_page = internal_page(payload.url()) == Some("neterror");
                        b.update(id, |t| {
                            t.loading = true;
                            t.pending = None;
                            // The error page keeps the address that failed in the address bar.
                            t.error = is_error_page;
                            t.bad_cert = payload.url().scheme() == "https" && payload.url().host_str().is_some_and(crate::platform::bad_certificate_allowed);
                            let shown = if is_error_page { t.error_url.clone().unwrap_or(shown) } else { t.error_url = None; shown };
                            t.bookmarked = b.0.store.is_bookmarked(&shown);
                            t.url = shown;
                            t.favicon = None;
                            t.load_gen += 1;
                            generation = t.load_gen;
                        });
                        b.watch_load(id, generation);
                    }
                    PageLoadEvent::Finished => b.finish_load(id, payload.url().clone()),
                }
            })
            .on_document_title_changed(move |wv, title| {
                let Some(id) = tab_id_of(wv.label()) else { return };
                let url = b2.tab(id).map(|t| t.url).unwrap_or_default();
                if url.starts_with("http") {
                    b2.0.store.set_title(&url, &title);
                }
                b2.update(id, |t| t.title = title);
            })
            .on_navigation(move |u| {
                if u.scheme() == POPUP_CLOSE_SCHEME {
                    return false;
                }
                if !is_web_scheme(u.scheme()) {
                    b6.ask_open_external(id, u);
                    return false;
                }
                if !is_app_url(u) {
                    return true;
                }
                // Web pages may not open the app's own pages: the settings and bookmarks pages
                // only when Kite itself navigates there (or reloads them). The start page is fine.
                let allowed = match internal_page(u) {
                    Some("newtab") => true,
                    Some(p) => {
                        let mut s = b5.0.state.lock().unwrap();
                        let pass = s.internal_pass.get(&id) == Some(&p);
                        if pass {
                            s.internal_pass.remove(&id);
                        }
                        let showing = s.tabs.iter().any(|t| t.id == id && t.url == format!("kite://{p}"));
                        pass || showing
                    }
                    None => false,
                };
                if !allowed {
                    tracing::warn!(url = %u, "blocked navigation to an app page");
                }
                allowed
            })
            .on_new_window(move |u, features| b3.new_window_request(u, features))
            .on_download(move |wv, event| {
                // A download is not a failed page load.
                if let Some(id) = tab_id_of(wv.label()) {
                    b4.update(id, |t| t.pending = None);
                }
                b4.download_event(event)
            })
    }

    /// `target=_blank` and plain `window.open(url)` become a tab next to this one. A real popup
    /// (`window.open(url, name, "width=…")`, or a blank one the page fills in, as login and
    /// payment pages do) gets its own small window linked to the page, so `window.opener`,
    /// `postMessage` and `window.close()` keep working (Chrome does the same).
    fn new_window_request(&self, u: url::Url, features: tauri::webview::NewWindowFeatures) -> NewWindowResponse<tauri::Wry> {
        let popup = features.size().is_some() || features.position().is_some() || u.as_str() == "about:blank";
        if popup {
            match self.open_popup(&u, features) {
                Ok(window) => return NewWindowResponse::Create { window },
                Err(e) => tracing::warn!(error = %e.detail, "popup window failed, opening a tab"),
            }
            if u.as_str() == "about:blank" {
                return NewWindowResponse::Deny;
            }
        }
        let b = self.clone();
        let url = u.to_string();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = b.new_tab(Some(url), true) {
                tracing::warn!(error = %e.detail, "new window as tab failed");
            }
        });
        NewWindowResponse::Deny
    }

    fn open_popup(&self, u: &url::Url, features: tauri::webview::NewWindowFeatures) -> Result<tauri::WebviewWindow, AppError> {
        static NEXT_POPUP: AtomicU32 = AtomicU32::new(1);
        let label = format!("popup-{}", NEXT_POPUP.fetch_add(1, Ordering::SeqCst));
        tracing::info!(%label, url = %u, size = ?features.size(), "page opens a popup window");
        let placed = features.position().is_some();
        let me = label.clone();
        let b = self.clone();
        let b2 = self.clone();
        // The engine loads the popup's page itself (keeping a POST body and the opener link).
        let builder = tauri::WebviewWindowBuilder::new(&self.0.app, label, WebviewUrl::External("about:blank".parse().expect("about:blank")))
            .title(u.host_str().unwrap_or("Kite"))
            .inner_size(500.0, 640.0)
            .min_inner_size(200.0, 150.0)
            .window_features(features)
            .browser_extensions_enabled(true)
            .devtools(true)
            .zoom_hotkeys_enabled(false)
            .focused(true)
            .on_document_title_changed(|wv, title| {
                if !title.is_empty() {
                    let _ = wv.set_title(&title);
                }
            })
            .on_navigation(move |u| {
                if u.scheme() == POPUP_CLOSE_SCHEME {
                    // Our window.close() (WKWebView does not close a window by itself).
                    let app = b.0.app.clone();
                    let me = me.clone();
                    let _ = b.0.app.run_on_main_thread(move || {
                        if let Some(w) = app.get_webview_window(&me) {
                            let _ = w.close();
                        }
                    });
                    return false;
                }
                if !is_web_scheme(u.scheme()) {
                    b.ask_open_external(0, u);
                    return false;
                }
                // Popups never open the app's own pages (except the blank start page).
                !is_app_url(u) || internal_page(u) == Some("newtab")
            })
            .on_new_window(move |u, f| b2.new_window_request(u, f));
        #[cfg(target_os = "macos")]
        let builder = builder.user_agent(safari_ua());
        #[cfg(not(target_os = "macos"))]
        let builder = builder.initialization_script(POPUP_JS);
        let builder = if !placed { builder.center() } else { builder };
        let b3 = self.clone();
        let builder = builder.on_download(move |_wv, event| b3.download_event(event));
        builder.build().map_err(|e| AppError::new("팝업 창을 열지 못했습니다.", e))
    }

    fn download_event(&self, event: tauri::webview::DownloadEvent<'_>) -> bool {
        use tauri::webview::DownloadEvent;
        match event {
            DownloadEvent::Requested { url, destination } => {
                let name = destination.file_name().map(|n| n.to_string_lossy().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| {
                    url.path_segments().and_then(|mut s| s.next_back().map(str::to_string)).filter(|s| !s.is_empty()).unwrap_or_else(|| "download".into())
                });
                if let Some(dir) = dirs::download_dir() {
                    *destination = unique_path(&dir, &name);
                }
                toast(&self.0.app, format!("다운로드 시작: {name}"));
            }
            DownloadEvent::Finished { path, success, .. } => {
                let name = path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let msg = if success { format!("다운로드 완료: {name}") } else { format!("다운로드 실패: {name}") };
                toast(&self.0.app, msg);
            }
            _ => {}
        }
        true
    }

    /// A page wants a non-web link (`f5-epi://…`): ask, then let the OS open its app. Browsers
    /// ask too: a page must not start local apps on its own.
    fn ask_open_external(&self, id: u64, u: &url::Url) {
        // Handed to another app: not a failed load.
        self.update(id, |t| t.pending = None);
        use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
        if EXTERNAL_PROMPT.swap(true, Ordering::SeqCst) {
            return;
        }
        let site = self.tab(id).and_then(|t| url::Url::parse(&t.url).ok()).and_then(|t| t.host_str().map(str::to_string)).unwrap_or_else(|| "이 페이지".into());
        let link = u.to_string();
        let shown: String = if link.chars().count() > 80 { link.chars().take(80).chain("…".chars()).collect() } else { link.clone() };
        tracing::info!(scheme = u.scheme(), site = %site, "page asks to open an external app");
        let app = self.0.app.clone();
        let mut dialog = self
            .0
            .app
            .dialog()
            .message(format!("{site}에서 외부 앱을 열려고 합니다.\n\n{shown}"))
            .title(format!("{}: 링크 열기", u.scheme()))
            .kind(MessageDialogKind::Info)
            .buttons(MessageDialogButtons::OkCancelCustom("앱 열기".into(), "취소".into()));
        if let Some(w) = self.tab(id).and_then(|t| self.window(t.win)) {
            dialog = dialog.parent(&w);
        }
        dialog.show(move |ok| {
            EXTERNAL_PROMPT.store(false, Ordering::SeqCst);
            if !ok {
                return;
            }
            use tauri_plugin_opener::OpenerExt;
            if let Err(e) = app.opener().open_url(link, None::<&str>) {
                tracing::warn!(error = %e, "open external link failed");
                toast(&app, "이 링크를 열 앱이 없습니다.");
            }
        });
    }

    /// A load is over (finish event, or the watchdog saw the page complete).
    fn finish_load(&self, id: u64, url: url::Url) {
        let Some(tab) = self.tab(id) else { return };
        if !tab.loading {
            return;
        }
        if self.0.reviving.lock().unwrap().get(&tab.win) == Some(&id) {
            self.revived(id, tab.win);
        } else {
            self.shoot_soon(id);
        }
        crate::session_cookies::touch();
        let shown = display_url(&url);
        if shown != NEWTAB && url.scheme().starts_with("http") {
            let _ = self.0.store.visit(&shown, &tab.title);
        }
        let bookmarked = self.0.store.is_bookmarked(&shown);
        self.update(id, |t| {
            t.loading = false;
            t.loads += 1;
            t.bookmarked = bookmarked;
            // The error page keeps the address that failed.
            t.url = if t.error { t.error_url.clone().unwrap_or(shown) } else { shown };
        });
        self.save_session();
        self.fetch_favicon(id);
    }

    /// WKWebView does not always report the end of a load (redirects, cancelled navigations,
    /// restored tabs). While a load runs, check the page itself: done when the document is
    /// complete and its URL is the one the webview shows (not the previous page during the
    /// provisional phase).
    fn watch_load(&self, id: u64, generation: u64) {
        let b = self.clone();
        tauri::async_runtime::spawn(async move {
            let mut stable = 0;
            for _ in 0..240 {
                tokio::time::sleep(Duration::from_millis(500)).await;
                match b.tab(id) {
                    Some(t) if t.loading && t.load_gen == generation => {}
                    _ => return,
                }
                let Some((complete, same, target)) = b.probe(id).await else { continue };
                if complete && same {
                    stable += 1;
                    if stable >= 2 {
                        tracing::debug!(id, url = %target, "load finished (watchdog)");
                        b.finish_load(id, target);
                        return;
                    }
                } else {
                    stable = 0;
                }
            }
        });
    }

    /// (document complete, page URL == webview URL, webview URL). The URLs differ while a
    /// navigation is pending (the old page is still shown).
    pub async fn probe(&self, id: u64) -> Option<(bool, bool, url::Url)> {
        let wv = self.webview(id)?;
        let target = wv.url().ok()?;
        let (tx, rx) = tokio::sync::oneshot::channel::<String>();
        let tx = std::sync::Mutex::new(Some(tx));
        let js = "(function(){return document.readyState+'\\n'+location.href})()";
        wv.eval_with_callback(js, move |r| {
            if let Some(tx) = tx.lock().unwrap().take() {
                let _ = tx.send(r);
            }
        })
        .ok()?;
        let raw = tokio::time::timeout(Duration::from_secs(3), rx).await.ok()?.ok()?;
        let v: String = serde_json::from_str(&raw).ok()?;
        let (state, href) = v.split_once('\n')?;
        Some((state == "complete", href == target.as_str(), target))
    }

    /// After Kite starts a navigation: if the webview stops loading without ever showing the new
    /// page, it could not be reached; show the error page instead (macOS; see `neterror.rs`).
    fn watch_failure(&self, id: u64, url: String) {
        if !cfg!(target_os = "macos") {
            return;
        }
        let b = self.clone();
        tauri::async_runtime::spawn(async move {
            let mut idle = 0;
            for _ in 0..300 {
                tokio::time::sleep(Duration::from_millis(400)).await;
                // Shown (committed), or another navigation took over.
                if b.tab(id).and_then(|t| t.pending) != Some(url.clone()) {
                    return;
                }
                let Some(wv) = b.webview(id) else { return };
                match crate::platform::is_loading(&wv).await {
                    Some(false) => idle += 1,
                    Some(true) => idle = 0,
                    None => return,
                }
                if idle >= 2 {
                    b.show_error(id, url).await;
                    return;
                }
            }
        });
    }

    async fn show_error(&self, id: u64, url: String) {
        let why = crate::neterror::diagnose(&url).await;
        // Still the same failed navigation after diagnosing?
        if self.tab(id).and_then(|t| t.pending) != Some(url.clone()) {
            return;
        }
        tracing::info!(id, %url, code = why.code, "page could not be reached");
        let (Some(wv), Some(base)) = (self.webview(id), self.app_base()) else { return };
        let Some(page) = crate::neterror::page_url(&base, &url, &why) else { return };
        {
            let mut s = self.0.state.lock().unwrap();
            s.internal_pass.insert(id, "neterror");
            if let Some(t) = s.tabs.iter_mut().find(|t| t.id == id) {
                t.pending = None;
                t.error_url = Some(url);
                t.url = t.error_url.clone().unwrap_or_default();
            }
        }
        let _ = wv.navigate(page);
    }

    /// Marks a tab loading and starts the watchdog for this load.
    fn begin_load(&self, id: u64) {
        let mut generation = 0;
        self.update(id, |t| {
            t.loading = true;
            t.load_gen += 1;
            generation = t.load_gen;
        });
        self.watch_load(id, generation);
    }

    fn fetch_favicon(&self, id: u64) {
        let Some(wv) = self.webview(id) else { return };
        let b = self.clone();
        let js = "(function(){try{return window.__kite?window.__kite.favicon():''}catch(e){return ''}})()";
        let _ = wv.eval_with_callback(js, move |r| {
            let icon: String = serde_json::from_str(&r).unwrap_or_default();
            if icon.starts_with("http") || icon.starts_with("data:image/") {
                b.update(id, |t| t.favicon = Some(icon));
            }
        });
    }

    // ---------- tabs whose page process macOS ended ----------

    /// macOS ended a page process (memory pressure; Safari does the same to old tabs). A tab in
    /// view reloads now; a hidden one waits until it is shown (`revive`), so tabs do not reload
    /// behind the user's back. Kite's own webviews reload at once.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn process_gone(&self, wv: &Webview) {
        let Some(id) = tab_id_of(wv.label()) else {
            let _ = wv.reload();
            return;
        };
        let visible = {
            let mut s = self.0.state.lock().unwrap();
            let Some(t) = s.tabs.iter_mut().find(|t| t.id == id) else { return };
            let win = t.win;
            t.purged = true;
            s.wins.get(&win).and_then(|w| w.active) == Some(id)
        };
        tracing::info!(id, visible, "page process ended by the system");
        if visible {
            self.revive(id);
        }
    }

    /// Reloads a purged tab under its last picture (same address), then puts the scroll
    /// position back: it looks like the page was kept.
    fn revive(&self, id: u64) {
        let Some(tab) = self.tab(id) else { return };
        self.update(id, |t| t.purged = false);
        let Some(wv) = self.webview(id) else { return };
        let shot = self.0.shots.lock().unwrap().get(&id).cloned().filter(|s| s.url == tab.url);
        if let Some(shot) = &shot {
            self.0.reviving.lock().unwrap().insert(tab.win, id);
            self.show_cover(tab.win, &shot.image);
            // Never stuck under the picture.
            let b = self.clone();
            let win = tab.win;
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(Duration::from_secs(6)).await;
                if b.0.reviving.lock().unwrap().get(&win) == Some(&id) {
                    b.end_revive(win);
                }
            });
        }
        tracing::info!(id, cover = shot.is_some(), "reloading a tab whose page process had ended");
        let _ = wv.reload();
    }

    /// The revived page has loaded: its scroll position back, then the picture goes.
    fn revived(&self, id: u64, win: u32) {
        let scroll = self.0.shots.lock().unwrap().get(&id).map(|s| s.scroll).unwrap_or((0.0, 0.0));
        if let Some(wv) = self.webview(id)
            && (scroll.0 > 0.0 || scroll.1 > 0.0)
        {
            let _ = wv.eval(format!("if(!window.scrollX&&!window.scrollY)window.scrollTo({},{})", scroll.0, scroll.1));
        }
        let b = self.clone();
        tauri::async_runtime::spawn(async move {
            // Let the page paint at its old position first.
            tokio::time::sleep(Duration::from_millis(250)).await;
            b.end_revive(win);
        });
    }

    fn end_revive(&self, win: u32) {
        self.0.reviving.lock().unwrap().remove(&win);
        if let Some(c) = self.0.app.get_webview(&cover_label(win)) {
            let _ = c.emit_to(cover_label(win), "cover-hide", ());
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(Duration::from_millis(180)).await;
                let _ = c.hide();
            });
        }
    }

    fn show_cover(&self, win: u32, image: &str) {
        let (Some(c), Some(w)) = (self.0.app.get_webview(&cover_label(win)), self.window(win)) else { return };
        let Some((cw, h)) = self.content_size(win) else { return };
        let chrome = self.0.state.lock().unwrap().wins.get(&win).map(|w| w.chrome_height).unwrap_or(DEFAULT_CHROME_HEIGHT);
        let _ = c.emit_to(cover_label(win), "cover", image);
        let sx = self.side_x(cw);
        let _ = c.set_bounds(Self::rect(sx, chrome, cw - sx, h - chrome));
        let _ = c.reparent(&w);
        #[cfg(windows)]
        crate::platform::bring_to_front(&c);
        let _ = c.show();
    }

    /// Picture and scroll position of a tab being looked at (kept in memory only).
    async fn shoot(&self, id: u64) {
        let Some(tab) = self.tab(id) else { return };
        if tab.loading || tab.purged || !tab.url.starts_with("http") {
            return;
        }
        let Some(wv) = self.webview(id) else { return };
        let Ok(png) = crate::platform::capture_png(&wv).await else { return };
        let scroll = self
            .run_sync(id, "return [window.scrollX||0, window.scrollY||0]")
            .await
            .ok()
            .and_then(|v| Some((v.get(0)?.as_f64()?, v.get(1)?.as_f64()?)))
            .unwrap_or((0.0, 0.0));
        let image = tauri::async_runtime::spawn_blocking(move || crate::platform::encode(&png, None, 1600, true).ok()).await.ok().flatten();
        let Some((jpeg, _, _)) = image else { return };
        use base64::Engine;
        let image = format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(jpeg));
        let live: Vec<u64> = self.0.state.lock().unwrap().tabs.iter().map(|t| t.id).collect();
        let mut shots = self.0.shots.lock().unwrap();
        shots.retain(|k, _| live.contains(k));
        shots.insert(id, Shot { url: tab.url, image, scroll });
    }

    /// A page finished loading: picture it once it has settled.
    fn shoot_soon(&self, id: u64) {
        if !cfg!(target_os = "macos") {
            return;
        }
        let b = self.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            if b.active() == Some(id) {
                b.shoot(id).await;
            }
        });
    }

    /// Every few seconds: the tab in view in each window (so its picture is recent when it is
    /// left). macOS only: there the system may end hidden tabs' processes.
    pub fn start_shots(&self) {
        if !cfg!(target_os = "macos") {
            return;
        }
        let b = self.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(10)).await;
                let active: Vec<u64> = {
                    let s = b.0.state.lock().unwrap();
                    s.wins.values().filter_map(|w| w.active).collect()
                };
                for id in active {
                    b.shoot(id).await;
                }
            }
        });
    }

    pub fn activate(&self, id: u64) -> Result<(), AppError> {
        if self.tab(id).is_none() {
            return Err(AppError::new("탭이 없습니다.", format!("tab {id}")));
        }
        let wv = self.ensure_live(id)?;
        let (win, other_window, palette_here) = {
            let mut s = self.0.state.lock().unwrap();
            let win = s.tabs.iter().find(|t| t.id == id).map(|t| t.win).unwrap_or(1);
            if let Some(w) = s.wins.get_mut(&win) {
                w.active = Some(id);
            }
            let other = s.focused != win;
            s.focused = win;
            (win, other, s.palette_win == Some(win))
        };
        self.layout(win);
        // Leaving a tab that is still being revived: its stand-in goes too.
        let reviving = self.0.reviving.lock().unwrap().get(&win).copied();
        if reviving.is_some_and(|r| r != id) {
            self.end_revive(win);
        }
        if self.tab(id).is_some_and(|t| t.purged) {
            self.revive(id);
        }
        // a tab of another window (palette tab search, agent): bring that window forward
        if other_window && let Some(w) = self.window(win) {
            let _ = w.set_focus();
        }
        if !palette_here {
            let _ = wv.set_focus();
        }
        self.save_session();
        self.emit();
        Ok(())
    }

    pub fn close_tab(&self, id: u64) -> Result<(), AppError> {
        let (win, was_active, next, other_windows) = {
            let mut s = self.0.state.lock().unwrap();
            let Some(pos) = s.tabs.iter().position(|t| t.id == id) else { return Ok(()) };
            let t = s.tabs.remove(pos);
            let win = t.win;
            if t.url != NEWTAB {
                s.closed.push(t.url);
                if s.closed.len() > 30 {
                    s.closed.remove(0);
                }
            }
            if s.agent_tab == Some(id) {
                s.agent_tab = None;
            }
            let rest: Vec<u64> = s.tabs.iter().filter(|t| t.win == win).map(|t| t.id).collect();
            let before = s.tabs[..pos].iter().filter(|t| t.win == win).count();
            let next = rest.get(before).or_else(|| rest.get(before.wrapping_sub(1))).copied();
            let was_active = s.wins.get(&win).and_then(|w| w.active) == Some(id);
            if was_active && let Some(w) = s.wins.get_mut(&win) {
                w.active = None;
            }
            (win, was_active, next, s.wins.len() > 1)
        };
        // A meeting held by this tab ends with it.
        crate::meet::tab_closed(&tab_label(id));
        if let Some(wv) = self.webview(id) {
            let _ = wv.close();
        }
        match next {
            Some(n) if was_active => self.activate(n)?,
            Some(_) => self.layout(win),
            // Last tab of a window: the window closes, unless it is the only one (then it
            // keeps a fresh start page).
            None if other_windows => {
                if let Some(w) = self.window(win) {
                    let _ = w.close();
                }
            }
            None => {
                self.new_tab_in(win, None, true)?;
                self.open_palette("", "")?;
            }
        }
        self.save_session();
        self.emit();
        Ok(())
    }

    pub fn reopen_closed(&self) -> Result<(), AppError> {
        let url = self.0.state.lock().unwrap().closed.pop();
        if let Some(u) = url {
            self.new_tab(Some(u), true)?;
        }
        Ok(())
    }

    pub fn cycle(&self, delta: i64) -> Result<(), AppError> {
        let target = {
            let tabs = self.win_tabs(self.focused());
            if tabs.is_empty() {
                return Ok(());
            }
            let pos = self.active().and_then(|a| tabs.iter().position(|t| *t == a)).unwrap_or(0) as i64;
            tabs[((pos + delta).rem_euclid(tabs.len() as i64)) as usize]
        };
        self.activate(target)
    }

    pub fn select_index(&self, index: usize, last: bool) -> Result<(), AppError> {
        let target = {
            let tabs = self.win_tabs(self.focused());
            if last { tabs.last().copied() } else { tabs.get(index).copied() }
        };
        if let Some(t) = target { self.activate(t) } else { Ok(()) }
    }

    /// Moves a tab to index `to` of its window's strip.
    pub fn move_tab(&self, id: u64, to: usize) {
        {
            let mut s = self.0.state.lock().unwrap();
            let Some(from) = s.tabs.iter().position(|t| t.id == id) else { return };
            let t = s.tabs.remove(from);
            // global index of the window's `to`-th other tab (or right after its last one)
            let mine: Vec<usize> = s.tabs.iter().enumerate().filter(|(_, x)| x.win == t.win).map(|(i, _)| i).collect();
            let at = match mine.get(to) {
                Some(i) => *i,
                None => mine.last().map(|i| i + 1).unwrap_or(s.tabs.len()),
            };
            s.tabs.insert(at, t);
        }
        self.save_session();
        self.emit();
    }

    /// Navigates a tab to `input` (URL or search words).
    pub fn navigate(&self, id: u64, input: &str) -> Result<String, AppError> {
        if let Some(p) = kite_page(input) {
            if let Some(sec) = settings_section(input) {
                let _ = self.0.store.set("settings_section", sec);
            }
            let target = format!("kite://{p}");
            self.navigate_url(id, &target)?;
            return Ok(target);
        }
        let engine = self.0.store.get("search_engine").unwrap_or_else(|| "google".into());
        let target = palette::resolve(input, &engine);
        self.navigate_url(id, &target)?;
        Ok(target)
    }

    pub fn navigate_url(&self, id: u64, target: &str) -> Result<(), AppError> {
        if let Some(p) = kite_page(target) {
            return self.navigate_internal(id, p);
        }
        let u = url::Url::parse(target).map_err(|e| AppError::new("주소가 올바르지 않습니다.", e))?;
        if is_app_url(&u) {
            return Err(AppError::new("열 수 없는 주소입니다.", target));
        }
        let mut had_webview = self.webview(id).is_some();
        let from = self.tab(id).map(|t| t.url).unwrap_or_default();
        if had_webview && crate::platform::needs_new_webview(&from, u.as_str()) {
            if let Some(wv) = self.webview(id) {
                let _ = wv.close();
            }
            for _ in 0..100 {
                if self.webview(id).is_none() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            had_webview = false;
        }
        if !had_webview {
            // Not shown yet: make the webview straight at the new address.
            self.update(id, |t| {
                t.url = u.to_string();
                t.loading = true;
            });
        }
        self.update(id, |t| t.pending = Some(u.to_string()));
        let wv = self.ensure_live(id)?;
        if had_webview {
            self.begin_load(id);
            wv.navigate(u.clone()).map_err(|e| AppError::new("페이지를 열지 못했습니다.", e))?;
        }
        self.watch_failure(id, u.to_string());
        Ok(())
    }

    /// Opens one of Kite's pages in this tab.
    fn navigate_internal(&self, id: u64, page: &'static str) -> Result<(), AppError> {
        let Some(wv) = self.webview(id) else {
            self.update(id, |t| t.url = format!("kite://{page}"));
            self.ensure_live(id)?;
            return Ok(());
        };
        let base = self.app_base().ok_or_else(|| AppError::new("페이지를 열지 못했습니다.", "no app base url"))?;
        let u = base.join(&format!("{page}.html")).map_err(|e| AppError::new("페이지를 열지 못했습니다.", e))?;
        self.0.state.lock().unwrap().internal_pass.insert(id, page);
        self.begin_load(id);
        wv.navigate(u).map_err(|e| AppError::new("페이지를 열지 못했습니다.", e))
    }

    /// Where the app's own pages are served (`tauri://localhost/`, dev server, …).
    fn app_base(&self) -> Option<url::Url> {
        let mut u = self.ui_webview()?.url().ok()?;
        u.set_path("/");
        u.set_query(None);
        u.set_fragment(None);
        Some(u)
    }

    /// Shows a Kite page: its tab if one is open, else the start page tab, else a new tab.
    pub fn open_internal(&self, page: &'static str) -> Result<(), AppError> {
        let url = format!("kite://{page}");
        if let Some(t) = self.tabs().into_iter().find(|t| t.url == url) {
            return self.activate(t.id);
        }
        match self.active().and_then(|a| self.tab(a)).filter(|t| t.url == NEWTAB) {
            Some(t) => {
                self.navigate_internal(t.id, page)?;
                self.activate(t.id)
            }
            None => self.new_tab(Some(url), true).map(|_| ()),
        }
    }

    pub fn eval_simple(&self, id: u64, js: &str) -> Result<(), AppError> {
        let wv = self.ensure_live(id)?;
        wv.eval(js).map_err(|e| AppError::new("페이지 스크립트를 실행하지 못했습니다.", e))
    }

    pub fn back(&self, id: u64) -> Result<(), AppError> {
        self.eval_simple(id, "history.back()")
    }

    pub fn forward(&self, id: u64) -> Result<(), AppError> {
        self.eval_simple(id, "history.forward()")
    }

    pub fn reload(&self, id: u64, hard: bool) -> Result<(), AppError> {
        if let Some(url) = self.tab(id).filter(|t| t.error).and_then(|t| t.error_url) {
            return self.navigate_url(id, &url);
        }
        let Some(wv) = self.webview(id) else {
            let wv = self.ensure_live(id)?;
            let _ = wv;
            return Ok(());
        };
        self.begin_load(id);
        if hard {
            // WKWebView/WebView2 have no public "ignore cache" reload through Tauri: re-navigate.
            let u = wv.url().map_err(|e| AppError::new("새로고침하지 못했습니다.", e))?;
            wv.navigate(u).map_err(|e| AppError::new("새로고침하지 못했습니다.", e))
        } else {
            wv.reload().map_err(|e| AppError::new("새로고침하지 못했습니다.", e))
        }
    }

    pub fn stop(&self, id: u64) -> Result<(), AppError> {
        self.eval_simple(id, "window.stop()")?;
        self.update(id, |t| {
            t.loading = false;
            t.pending = None;
        });
        Ok(())
    }

    pub fn zoom(&self, id: u64, how: &str) -> Result<f64, AppError> {
        const STEPS: [f64; 13] = [0.25, 0.33, 0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];
        let cur = self.tab(id).map(|t| t.zoom).unwrap_or(1.0);
        let next = match how {
            "in" => STEPS.iter().copied().find(|s| *s > cur + 0.001).unwrap_or(3.0),
            "out" => STEPS.iter().rev().copied().find(|s| *s < cur - 0.001).unwrap_or(0.25),
            _ => 1.0,
        };
        let wv = self.ensure_live(id)?;
        wv.set_zoom(next).map_err(|e| AppError::new("확대/축소하지 못했습니다.", e))?;
        self.update(id, |t| t.zoom = next);
        Ok(next)
    }

    /// The webview's current URL (SPA navigation does not fire page-load events).
    pub fn refresh_url(&self, id: u64) {
        if let Some(wv) = self.webview(id)
            && let Ok(u) = wv.url()
        {
            // The error page keeps the address that failed.
            if internal_page(&u) == Some("neterror") && self.tab(id).is_some_and(|t| t.error) {
                return;
            }
            let shown = display_url(&u);
            let bookmarked = self.0.store.is_bookmarked(&shown);
            if self.tab(id).is_some_and(|t| t.url != shown) {
                if u.scheme().starts_with("http") {
                    let title = self.tab(id).map(|t| t.title).unwrap_or_default();
                    let _ = self.0.store.visit(&shown, &title);
                }
                self.update(id, |t| {
                    t.url = shown;
                    t.bookmarked = bookmarked;
                });
                self.save_session();
            }
        }
    }

    pub fn toggle_bookmark(&self, id: u64) -> Result<bool, AppError> {
        let t = self.tab(id).ok_or_else(|| AppError::new("탭이 없습니다.", id))?;
        if !t.url.starts_with("http") {
            return Err(AppError::new("이 페이지는 북마크할 수 없습니다.", t.url));
        }
        let on = self.0.store.bookmark_toggle(&t.url, &t.title)?;
        self.refresh_bookmark_flags();
        Ok(on)
    }

    pub fn refresh_bookmark_flags(&self) {
        {
            let mut s = self.0.state.lock().unwrap();
            for t in s.tabs.iter_mut() {
                t.bookmarked = self.0.store.is_bookmarked(&t.url);
            }
        }
        self.emit();
        self.emit_bookmark_bar();
    }

    /// Items of the bookmark bar, `None` when the bar is hidden.
    pub fn bookmark_bar(&self) -> Option<Vec<BarItem>> {
        if self.0.store.get("bookmark_bar").as_deref() != Some("true") {
            return None;
        }
        let all = self.0.store.bookmarks_all();
        let mut items = Vec::new();
        let mut folders: Vec<String> = Vec::new();
        for b in all {
            if b.folder.is_empty() {
                items.push(BarItem { kind: "link".into(), title: if b.title.is_empty() { palette::short_url(&b.url) } else { b.title }, url: b.url });
            } else {
                let top = b.folder.split('/').next().unwrap_or("").to_string();
                if !folders.contains(&top) {
                    folders.push(top);
                }
            }
        }
        // Empty folders too (kept on their own), after the used ones.
        for f in self.0.store.folders_all() {
            if !f.contains('/') && !folders.contains(&f) {
                folders.push(f);
            }
        }
        // In the order set in the bookmark manager (others keep their first-seen order).
        let order = self.0.store.folder_order();
        let pos = |f: &String| order.iter().position(|o| o == f).unwrap_or(usize::MAX);
        folders.sort_by_key(pos);
        // Folders first, like Chrome's bar after an import.
        let mut out: Vec<BarItem> = folders.into_iter().map(|f| BarItem { kind: "folder".into(), title: f.clone(), url: f }).collect();
        out.extend(items);
        Some(out)
    }

    /// A new color theme: every window's own pages and the Kite pages open in tabs.
    pub fn emit_theme(&self, id: &str) {
        let mut targets = Vec::new();
        for n in self.wins() {
            targets.extend([ui_label(n), palette_label(n), omni_label(n)]);
        }
        let s = self.0.state.lock().unwrap();
        targets.extend(s.tabs.iter().filter(|t| t.live && t.url.starts_with("kite://")).map(|t| tab_label(t.id)));
        drop(s);
        for t in targets {
            let _ = self.0.app.emit_to(t, "theme", id);
        }
    }

    pub fn emit_bookmark_bar(&self) {
        self.emit_all_ui("bookmark-bar", self.bookmark_bar());
        // The tab list's bookmark view reloads its list.
        for n in self.wins() {
            let _ = self.0.app.emit_to(side_label(n), "bookmarks-changed", ());
        }
    }

    // ---------- palette ----------

    /// Brings window `n`'s palette above its tab webviews (they are added later and would cover it).
    /// Shows the address bar suggestions of window `n` at `x,y` (logical, in the window),
    /// `width` wide, one row per item. Keyboard focus stays in the address bar.
    pub fn show_omni(&self, n: u32, x: f64, y: f64, width: f64, items: serde_json::Value, selected: i64) {
        const ROW: f64 = 34.0;
        let (Some(o), Some(w)) = (self.0.app.get_webview(&omni_label(n)), self.window(n)) else { return };
        let count = items.as_array().map(Vec::len).unwrap_or(0);
        if count == 0 {
            let _ = o.hide();
            return;
        }
        let height = count as f64 * ROW + 12.0;
        let _ = o.emit_to(omni_label(n), "omni-items", serde_json::json!({ "items": items, "selected": selected }));
        let _ = o.set_bounds(Self::rect(x.max(0.0), y, width.max(200.0), height));
        if OMNI_SHOWN.lock().unwrap().insert(n) {
            // Above the page (macOS: re-adding the view puts it on top).
            let _ = o.reparent(&w);
            #[cfg(windows)]
            crate::platform::bring_to_front(&o);
            let _ = o.show();
        }
    }

    pub fn hide_omni(&self, n: u32) {
        OMNI_SHOWN.lock().unwrap().remove(&n);
        if let Some(o) = self.0.app.get_webview(&omni_label(n)) {
            let _ = o.hide();
        }
    }

    fn raise_palette(&self, n: u32) {
        let (Some(p), Some(w)) = (self.0.app.get_webview(&palette_label(n)), self.window(n)) else { return };
        // macOS: reparenting re-adds the NSView on top. Windows: also move the HWND to the top.
        let _ = p.reparent(&w);
        #[cfg(windows)]
        crate::platform::bring_to_front(&p);
    }

    /// Opens the palette of the focused window. `mode`: prefix to start with (`>`, `*`, `#`, `@`
    /// or empty); `query`: initial text.
    pub fn open_palette(&self, mode: &str, query: &str) -> Result<(), AppError> {
        let n = self.focused();
        let label = palette_label(n);
        let p = self.0.app.get_webview(&label).ok_or_else(|| AppError::new("팔레트를 열 수 없습니다.", "no palette webview"))?;
        let prev = self.0.state.lock().unwrap().palette_win.replace(n);
        if let Some(o) = prev.filter(|o| *o != n)
            && let Some(op) = self.0.app.get_webview(&palette_label(o))
        {
            let _ = op.hide();
        }
        self.raise_palette(n);
        self.layout(n);
        let _ = p.show();
        let _ = p.set_focus();
        let _ = p.emit_to(label, "palette-open", serde_json::json!({ "mode": mode, "query": query }));
        Ok(())
    }

    pub fn close_palette(&self, refocus: bool) {
        let was = self.0.state.lock().unwrap().palette_win.take();
        if let Some(n) = was
            && let Some(p) = self.0.app.get_webview(&palette_label(n))
        {
            let _ = p.hide();
        }
        if was.is_some() && refocus {
            self.focus_current();
        }
    }

    /// Keyboard focus to the focused window's open palette, else its active tab.
    pub fn focus_current(&self) {
        let n = self.focused();
        if self.0.state.lock().unwrap().palette_win == Some(n) {
            if let Some(p) = self.0.app.get_webview(&palette_label(n)) {
                let _ = p.set_focus();
            }
        } else if let Some(id) = self.active()
            && let Some(wv) = self.webview(id)
        {
            let _ = wv.set_focus();
        }
    }

    pub fn palette_open(&self) -> bool {
        self.0.state.lock().unwrap().palette_win.is_some()
    }

    pub fn resize_palette(&self, h: f64) {
        let n = {
            let mut s = self.0.state.lock().unwrap();
            s.palette_height = h.clamp(60.0, 640.0);
            s.palette_win
        };
        if let Some(n) = n {
            self.layout(n);
        }
    }

    pub fn palette_tabs(&self) -> Vec<palette::TabRef> {
        self.tabs()
            .into_iter()
            .map(|t| {
                let title = if t.url == NEWTAB { "새 탭".to_string() } else if t.title.is_empty() { palette::short_url(&t.url) } else { t.title };
                let url = if t.url == NEWTAB { String::new() } else { t.url };
                palette::TabRef { id: t.id, title, url, favicon: t.favicon }
            })
            .collect()
    }

    // ---------- session ----------

    pub fn save_session(&self) {
        let v = {
            let s = self.0.state.lock().unwrap();
            let windows: Vec<serde_json::Value> = s
                .wins
                .iter()
                .map(|(n, w)| {
                    let tabs: Vec<&Tab> = s.tabs.iter().filter(|t| t.win == *n).collect();
                    let active = w.active.and_then(|a| tabs.iter().position(|t| t.id == a)).unwrap_or(0);
                    serde_json::json!({ "tabs": tabs.iter().map(|t| t.url.clone()).collect::<Vec<_>>(), "active": active, "bounds": w.bounds })
                })
                .collect();
            let focused = s.wins.keys().position(|n| *n == s.focused).unwrap_or(0);
            serde_json::json!({ "windows": windows, "focused": focused })
        };
        let _ = self.0.store.set("session", &v.to_string());
    }

    /// Opens the windows and tabs of the last session (only each window's active tab gets a
    /// webview now). Window 1 (`main`) exists already.
    pub fn restore_session(&self) -> Result<(), AppError> {
        let restore = self.0.store.get("restore_session").as_deref() != Some("false");
        let saved: Option<serde_json::Value> = if restore { self.0.store.get("session").and_then(|s| serde_json::from_str(&s).ok()) } else { None };
        // {"windows":[{"tabs":[…],"active":i}],"focused":i}; before: one window {"tabs","active"}
        let windows: Vec<serde_json::Value> = match &saved {
            Some(v) if v.get("windows").is_some() => v["windows"].as_array().cloned().unwrap_or_default(),
            Some(v) => vec![v.clone()],
            None => Vec::new(),
        };
        let focused = saved.as_ref().and_then(|v| v.get("focused")).and_then(|f| f.as_u64()).unwrap_or(0) as usize;
        let main_pos = self.window(1).and_then(|w| {
            let scale = w.scale_factor().ok()?;
            w.outer_position().ok().map(|p| p.to_logical::<f64>(scale))
        });
        // saved [x, y, w, h], if it still lies on a connected screen
        let bounds_of = |v: &serde_json::Value| -> Option<[f64; 4]> {
            let b: [f64; 4] = serde_json::from_value(v.get("bounds")?.clone()).ok()?;
            let on_screen = self.0.app.available_monitors().ok()?.iter().any(|m| {
                let sc = m.scale_factor();
                let (mp, ms) = (m.position().to_logical::<f64>(sc), m.size().to_logical::<f64>(sc));
                b[0] + 100.0 > mp.x && b[0] < mp.x + ms.width - 100.0 && b[1] >= mp.y - 10.0 && b[1] < mp.y + ms.height - 60.0
            });
            (on_screen && b[2] >= 400.0 && b[3] >= 300.0).then_some(b)
        };
        let mut focus_win = 1;
        let mut placed = 0;
        for (i, wv) in windows.iter().enumerate() {
            let urls: Vec<String> = wv
                .get("tabs")
                .and_then(|t| t.as_array())
                .map(|a| a.iter().filter_map(|u| u.as_str().map(str::to_string)).filter(|u| u.starts_with("http") || kite_page(u).is_some()).collect())
                .unwrap_or_default();
            if urls.is_empty() {
                continue;
            }
            let bounds = bounds_of(wv);
            let n = if placed == 0 {
                if let (Some(b), Some(w)) = (bounds, self.window(1)) {
                    let _ = w.set_size(LogicalSize::new(b[2], b[3]));
                    let _ = w.set_position(LogicalPosition::new(b[0], b[1]));
                }
                1
            } else {
                match bounds {
                    Some(b) => self.create_window(Some((b[0], b[1])), Some((b[2], b[3])))?,
                    None => {
                        let pos = main_pos.map(|p| (p.x + 30.0 * placed as f64, p.y + 30.0 * placed as f64));
                        self.create_window(pos, self.content_size(1))?
                    }
                }
            };
            placed += 1;
            let active = wv.get("active").and_then(|a| a.as_u64()).unwrap_or(0) as usize;
            let ids: Vec<u64> = urls.into_iter().map(|u| self.add_restored(u, n)).collect();
            if let Some(id) = ids.get(active).or(ids.first()) {
                self.activate(*id)?;
            }
            if i == focused {
                focus_win = n;
            }
        }
        if placed == 0 {
            self.new_tab_in(1, None, true)?;
        } else if let Some(id) = self.snapshot(focus_win).active {
            self.activate(id)?;
        }
        Ok(())
    }

    /// Waits until the tab is not loading (or `timeout`). Returns false on timeout.
    pub async fn wait_loaded(&self, id: u64, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let n = self.0.changed.notified();
            tokio::pin!(n);
            n.as_mut().enable();
            match self.tab(id) {
                None => return false,
                Some(t) if !t.loading => return true,
                _ => {}
            }
            if tokio::time::timeout_at(deadline, n).await.is_err() {
                return false;
            }
        }
    }
}

fn unique_path(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let name: String = name.chars().map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c }).collect();
    let p = dir.join(&name);
    if !p.exists() {
        return p;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.clone(), String::new()),
    };
    (1..1000).map(|i| dir.join(format!("{stem} ({i}){ext}"))).find(|p| !p.exists()).unwrap_or(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_safari_version() {
        let p = "<dict>\n\t<key>CFBundleShortVersionString</key>\n\t<string>26.2</string>\n</dict>";
        assert_eq!(safari_version(p).as_deref(), Some("26.2"));
        assert_eq!(safari_version("<dict></dict>"), None);
        assert_eq!(safari_version("<key>CFBundleShortVersionString</key><string>x\"y</string>"), None);
    }

    #[test]
    fn external_schemes_go_to_the_os() {
        for s in ["http", "https", "about", "data", "blob", "file", "tauri"] {
            assert!(is_web_scheme(s), "{s}");
        }
        for s in ["f5-epi", "mailto", "zoommtg", "msteams", "tel"] {
            assert!(!is_web_scheme(s), "{s}");
        }
    }

    #[test]
    fn labels_and_urls() {
        assert_eq!(tab_id_of(&tab_label(42)), Some(42));
        assert_eq!(tab_id_of("ui"), None);
        assert!(is_newtab_url(&url::Url::parse("tauri://localhost/newtab.html").unwrap()));
        assert!(is_newtab_url(&url::Url::parse("http://tauri.localhost/newtab.html").unwrap()));
        assert!(!is_newtab_url(&url::Url::parse("https://evil.com/newtab.html").unwrap()));
        assert!(is_app_url(&url::Url::parse("tauri://localhost/index.html").unwrap()));
        assert!(!is_app_url(&url::Url::parse("https://example.com/").unwrap()));
        assert_eq!(display_url(&url::Url::parse("tauri://localhost/newtab.html").unwrap()), NEWTAB);
        assert_eq!(display_url(&url::Url::parse("http://tauri.localhost/settings.html").unwrap()), "kite://settings");
        assert_eq!(internal_page(&url::Url::parse("https://evil.com/settings.html").unwrap()), None);
        assert_eq!(internal_page(&url::Url::parse("tauri://localhost/index.html").unwrap()), None);
        assert_eq!(kite_page("chrome://settings/"), Some("settings"));
        assert_eq!(kite_page("kite://bookmarks"), Some("bookmarks"));
        assert_eq!(kite_page("edge://favorites"), Some("bookmarks"));
        assert_eq!(kite_page("about:blank"), Some("newtab"));
        assert_eq!(kite_page("chrome://flags"), None);
        assert_eq!(kite_page("kite://extension"), Some("settings"));
        assert_eq!(settings_section("chrome://extensions/"), Some("extensions"));
        assert_eq!(settings_section("kite://settings"), None);
        assert_eq!(settings_section("chrome://password-manager/passwords"), Some("passwords"));
        assert_eq!(kite_page("kite://passwords"), Some("settings"));
        assert_eq!(kite_page("https://settings.com"), None);
    }

    #[test]
    fn unique_download_names() {
        let dir = std::env::temp_dir().join(format!("kite-dl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = unique_path(&dir, "a.txt");
        assert_eq!(a.file_name().unwrap(), "a.txt");
        std::fs::write(&a, "x").unwrap();
        assert_eq!(unique_path(&dir, "a.txt").file_name().unwrap(), "a (1).txt");
        assert_eq!(unique_path(&dir, "b/c?.txt").file_name().unwrap(), "b_c_.txt");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
