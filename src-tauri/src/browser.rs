//! Window layout, tabs and their webviews.
//!
//! One window `main` holds several webviews:
//! - `ui`: tab strip + toolbar (local page `index.html`), full width at the top.
//! - `tab-<id>`: one per tab, below the toolbar; only the active one is shown. A tab gets its
//!   webview the first time it is shown or automated (restored tabs start without one).
//! - `palette`: the spotlight overlay, kept hidden and brought to the front when opened.
//!
//! Rust owns the tab state; the UI only renders the `browser-state` event and sends commands.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
}

#[derive(Default)]
struct State {
    tabs: Vec<Tab>,
    active: Option<u64>,
    agent_tab: Option<u64>,
    closed: Vec<String>,
    chrome_height: f64,
    palette_open: bool,
    palette_height: f64,
    /// Internal page a tab may open next (set by the app right before navigating there).
    internal_pass: std::collections::HashMap<u64, &'static str>,
}

pub struct Inner {
    pub app: AppHandle,
    pub store: Arc<Store>,
    state: Mutex<State>,
    next_id: AtomicU64,
    /// Woken whenever a tab's load state changes.
    pub changed: Notify,
    content_js: String,
}

#[derive(Clone)]
pub struct Browser(pub Arc<Inner>);

pub fn tab_label(id: u64) -> String {
    format!("tab-{id}")
}

pub fn tab_id_of(label: &str) -> Option<u64> {
    label.strip_prefix("tab-")?.parse().ok()
}

/// Kite's own pages shown in tabs: `kite://<name>` ↔ `<name>.html` of the app.
pub const INTERNAL: [&str; 3] = ["newtab", "settings", "bookmarks"];

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
        "settings" | "preferences" | "prefs" => "settings",
        "bookmarks" | "favorites" => "bookmarks",
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
    matches!(scheme, "http" | "https" | "about" | "data" | "blob" | "file" | "javascript" | "view-source" | "ws" | "wss" | "tauri" | "asset" | "ipc")
}

/// One "open in external app?" question at a time: a page retrying in a loop gets no second dialog.
static EXTERNAL_PROMPT: AtomicBool = AtomicBool::new(false);

pub fn display_url(u: &url::Url) -> String {
    match internal_page(u) {
        Some(p) => format!("kite://{p}"),
        None => u.to_string(),
    }
}

impl Browser {
    pub fn new(app: AppHandle, store: Arc<Store>) -> Self {
        let content_js = include_str!("inject/content.js").replace("__KITE_KEYMAP__", &crate::keymap::json_map());
        Self(Arc::new(Inner {
            app,
            store,
            state: Mutex::new(State { chrome_height: DEFAULT_CHROME_HEIGHT, palette_height: 420.0, ..Default::default() }),
            next_id: AtomicU64::new(1),
            changed: Notify::new(),
            content_js,
        }))
    }

    pub fn content_js(&self) -> &str {
        &self.0.content_js
    }

    fn window(&self) -> Option<Window> {
        self.0.app.get_window("main")
    }

    pub fn webview(&self, id: u64) -> Option<Webview> {
        self.0.app.get_webview(&tab_label(id))
    }

    pub fn snapshot(&self) -> BrowserState {
        let s = self.0.state.lock().unwrap();
        BrowserState {
            tabs: s.tabs.clone(),
            active: s.active,
            agent_tab: s.agent_tab,
            mcp: self.0.app.try_state::<crate::mcp::McpHub>().map(|h| h.status()).unwrap_or_default(),
        }
    }

    pub fn emit(&self) {
        let _ = self.0.app.emit_to(UI, "browser-state", self.snapshot());
        self.0.changed.notify_waiters();
    }

    pub fn tab(&self, id: u64) -> Option<Tab> {
        self.0.state.lock().unwrap().tabs.iter().find(|t| t.id == id).cloned()
    }

    pub fn tabs(&self) -> Vec<Tab> {
        self.0.state.lock().unwrap().tabs.clone()
    }

    pub fn active(&self) -> Option<u64> {
        self.0.state.lock().unwrap().active
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
    fn content_size(&self) -> Option<(f64, f64)> {
        let w = self.window()?;
        let scale = w.scale_factor().ok()?;
        let size = w.inner_size().ok()?.to_logical::<f64>(scale);
        Some((size.width, size.height))
    }

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect { position: LogicalPosition::new(x, y).into(), size: LogicalSize::new(w.max(1.0), h.max(1.0)).into() }
    }

    pub fn layout(&self) {
        let Some((w, h)) = self.content_size() else { return };
        let (chrome, active, live, palette_open, palette_h) = {
            let s = self.0.state.lock().unwrap();
            let live: Vec<u64> = s.tabs.iter().filter(|t| t.live).map(|t| t.id).collect();
            (s.chrome_height, s.active, live, s.palette_open, s.palette_height)
        };
        if let Some(ui) = self.0.app.get_webview(UI) {
            let _ = ui.set_bounds(Self::rect(0.0, 0.0, w, chrome));
        }
        for id in live {
            if let Some(wv) = self.webview(id) {
                if Some(id) == active {
                    let _ = wv.set_bounds(Self::rect(0.0, chrome, w, h - chrome));
                    let _ = wv.show();
                } else {
                    let _ = wv.hide();
                }
            }
        }
        if palette_open && let Some(p) = self.0.app.get_webview(PALETTE) {
            let pw = PALETTE_WIDTH.min(w - 24.0);
            let ph = palette_h.min(h - PALETTE_TOP - 16.0);
            let _ = p.set_bounds(Self::rect(((w - pw) / 2.0).round(), PALETTE_TOP, pw, ph));
        }
    }

    pub fn set_chrome_height(&self, h: f64) {
        self.0.state.lock().unwrap().chrome_height = h.clamp(30.0, 300.0);
        self.layout();
    }

    // ---------- tabs ----------

    /// Adds a tab (webview made right away when `activate`). `url` None = new-tab page.
    pub fn new_tab(&self, url: Option<String>, activate: bool) -> Result<u64, AppError> {
        let id = self.0.next_id.fetch_add(1, Ordering::SeqCst);
        let shown = url.clone().unwrap_or_else(|| NEWTAB.to_string());
        {
            let mut s = self.0.state.lock().unwrap();
            let pos = s.active.and_then(|a| s.tabs.iter().position(|t| t.id == a)).map(|p| p + 1).unwrap_or(s.tabs.len());
            let bookmarked = url.as_deref().is_some_and(|u| self.0.store.is_bookmarked(u));
            s.tabs.insert(
                pos,
                Tab { id, url: shown, title: String::new(), loading: url.is_some(), favicon: None, zoom: 1.0, bookmarked, live: false, loads: 0, load_gen: 0 },
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
    fn add_restored(&self, url: String) -> u64 {
        let id = self.0.next_id.fetch_add(1, Ordering::SeqCst);
        let bookmarked = self.0.store.is_bookmarked(&url);
        let title = self.0.store.history_search(&url, 1).into_iter().find(|v| v.url == url).map(|v| v.title).unwrap_or_default();
        self.0.state.lock().unwrap().tabs.push(Tab { id, url, title, loading: false, favicon: None, zoom: 1.0, bookmarked, live: false, loads: 0, load_gen: 0 });
        id
    }

    /// Makes sure the tab has its webview.
    pub fn ensure_live(&self, id: u64) -> Result<Webview, AppError> {
        if let Some(wv) = self.webview(id) {
            return Ok(wv);
        }
        let tab = self.tab(id).ok_or_else(|| AppError::new("탭이 없습니다.", format!("tab {id}")))?;
        let window = self.window().ok_or_else(|| AppError::new("창이 없습니다.", "no main window"))?;
        let target = if let Some(p) = kite_page(&tab.url) {
            self.0.state.lock().unwrap().internal_pass.insert(id, p);
            WebviewUrl::App(format!("{p}.html").into())
        } else {
            WebviewUrl::External(url::Url::parse(&tab.url).map_err(|e| AppError::new("주소가 올바르지 않습니다.", e))?)
        };
        let builder = self.content_builder(id, target);
        let (w, h) = self.content_size().unwrap_or((1200.0, 800.0));
        let chrome = self.0.state.lock().unwrap().chrome_height;
        let wv = window
            .add_child(builder, LogicalPosition::new(0.0, chrome), LogicalSize::new(w, (h - chrome).max(1.0)))
            .map_err(|e| AppError::new("탭을 만들지 못했습니다.", e))?;
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
        if self.0.state.lock().unwrap().palette_open {
            self.raise_palette();
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
        // WKWebView's default UA has no `Version/… Safari/…`: some sites (Google among them)
        // then serve a reduced page. WebView2's default UA (Chrome + Edg) is fine as is.
        #[cfg(target_os = "macos")]
        let builder = builder.user_agent(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Safari/605.1.15",
        );
        builder
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
                        b.update(id, |t| {
                            t.loading = true;
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
            .on_new_window(move |u, _features| {
                // window.open('') (a blank popup the page then points somewhere): the popup is
                // denied anyway (the page gets null), so don't leave an empty tab behind.
                if u.as_str() == "about:blank" {
                    return NewWindowResponse::Deny;
                }
                // target=_blank / window.open: a new tab next to this one.
                let b = b3.clone();
                let url = u.to_string();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = b.new_tab(Some(url), true) {
                        tracing::warn!(error = %e.detail, "new window as tab failed");
                    }
                });
                NewWindowResponse::Deny
            })
            .on_download(move |_wv, event| {
                use tauri::webview::DownloadEvent;
                match event {
                    DownloadEvent::Requested { url, destination } => {
                        let name = destination.file_name().map(|n| n.to_string_lossy().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| {
                            url.path_segments().and_then(|mut s| s.next_back().map(str::to_string)).filter(|s| !s.is_empty()).unwrap_or_else(|| "download".into())
                        });
                        if let Some(dir) = dirs::download_dir() {
                            *destination = unique_path(&dir, &name);
                        }
                        let _ = b4.0.app.emit_to(UI, "toast", format!("다운로드 시작: {name}"));
                    }
                    DownloadEvent::Finished { path, success, .. } => {
                        let name = path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                        let msg = if success { format!("다운로드 완료: {name}") } else { format!("다운로드 실패: {name}") };
                        let _ = b4.0.app.emit_to(UI, "toast", msg);
                    }
                    _ => {}
                }
                true
            })
    }

    /// A page wants a non-web link (`f5-epi://…`): ask, then let the OS open its app. Browsers
    /// ask too: a page must not start local apps on its own.
    fn ask_open_external(&self, id: u64, u: &url::Url) {
        use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
        if EXTERNAL_PROMPT.swap(true, Ordering::SeqCst) {
            return;
        }
        let site = self.tab(id).and_then(|t| url::Url::parse(&t.url).ok()).and_then(|t| t.host_str().map(str::to_string)).unwrap_or_else(|| "이 페이지".into());
        let link = u.to_string();
        let shown: String = if link.chars().count() > 80 { link.chars().take(80).chain("…".chars()).collect() } else { link.clone() };
        tracing::info!(scheme = u.scheme(), site = %site, "page asks to open an external app");
        let app = self.0.app.clone();
        self.0
            .app
            .dialog()
            .message(format!("{site}에서 외부 앱을 열려고 합니다.\n\n{shown}"))
            .title(format!("{}: 링크 열기", u.scheme()))
            .kind(MessageDialogKind::Info)
            .buttons(MessageDialogButtons::OkCancelCustom("앱 열기".into(), "취소".into()))
            .show(move |ok| {
                EXTERNAL_PROMPT.store(false, Ordering::SeqCst);
                if !ok {
                    return;
                }
                use tauri_plugin_opener::OpenerExt;
                if let Err(e) = app.opener().open_url(link, None::<&str>) {
                    tracing::warn!(error = %e, "open external link failed");
                    let _ = app.emit_to(UI, "toast", "이 링크를 열 앱이 없습니다.");
                }
            });
    }

    /// A load is over (finish event, or the watchdog saw the page complete).
    fn finish_load(&self, id: u64, url: url::Url) {
        let Some(tab) = self.tab(id) else { return };
        if !tab.loading {
            return;
        }
        let shown = display_url(&url);
        if shown != NEWTAB && url.scheme().starts_with("http") {
            let _ = self.0.store.visit(&shown, &tab.title);
        }
        let bookmarked = self.0.store.is_bookmarked(&shown);
        self.update(id, |t| {
            t.loading = false;
            t.loads += 1;
            t.bookmarked = bookmarked;
            t.url = shown;
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

    pub fn activate(&self, id: u64) -> Result<(), AppError> {
        if self.tab(id).is_none() {
            return Err(AppError::new("탭이 없습니다.", format!("tab {id}")));
        }
        let wv = self.ensure_live(id)?;
        self.0.state.lock().unwrap().active = Some(id);
        self.layout();
        if !self.0.state.lock().unwrap().palette_open {
            let _ = wv.set_focus();
        }
        self.save_session();
        self.emit();
        Ok(())
    }

    pub fn close_tab(&self, id: u64) -> Result<(), AppError> {
        let next = {
            let mut s = self.0.state.lock().unwrap();
            let Some(pos) = s.tabs.iter().position(|t| t.id == id) else { return Ok(()) };
            let t = s.tabs.remove(pos);
            if t.url != NEWTAB {
                s.closed.push(t.url);
                if s.closed.len() > 30 {
                    s.closed.remove(0);
                }
            }
            if s.agent_tab == Some(id) {
                s.agent_tab = None;
            }
            if s.active == Some(id) {
                s.active = None;
                s.tabs.get(pos).or_else(|| s.tabs.get(pos.wrapping_sub(1))).map(|t| t.id)
            } else {
                s.active
            }
        };
        if let Some(wv) = self.webview(id) {
            let _ = wv.close();
        }
        match next {
            Some(n) => self.activate(n)?,
            // Last tab closed: keep the window with a fresh start page.
            None => {
                self.new_tab(None, true)?;
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
            let s = self.0.state.lock().unwrap();
            if s.tabs.is_empty() {
                return Ok(());
            }
            let pos = s.active.and_then(|a| s.tabs.iter().position(|t| t.id == a)).unwrap_or(0) as i64;
            let n = s.tabs.len() as i64;
            s.tabs[((pos + delta).rem_euclid(n)) as usize].id
        };
        self.activate(target)
    }

    pub fn select_index(&self, index: usize, last: bool) -> Result<(), AppError> {
        let target = {
            let s = self.0.state.lock().unwrap();
            if last { s.tabs.last().map(|t| t.id) } else { s.tabs.get(index).map(|t| t.id) }
        };
        if let Some(t) = target { self.activate(t) } else { Ok(()) }
    }

    pub fn move_tab(&self, id: u64, to: usize) {
        {
            let mut s = self.0.state.lock().unwrap();
            let Some(from) = s.tabs.iter().position(|t| t.id == id) else { return };
            let t = s.tabs.remove(from);
            let to = to.min(s.tabs.len());
            s.tabs.insert(to, t);
        }
        self.save_session();
        self.emit();
    }

    /// Navigates a tab to `input` (URL or search words).
    pub fn navigate(&self, id: u64, input: &str) -> Result<String, AppError> {
        if let Some(p) = kite_page(input) {
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
        let had_webview = self.webview(id).is_some();
        if !had_webview {
            // Not shown yet: make the webview straight at the new address.
            self.update(id, |t| {
                t.url = u.to_string();
                t.loading = true;
            });
        }
        let wv = self.ensure_live(id)?;
        if had_webview {
            self.begin_load(id);
            wv.navigate(u).map_err(|e| AppError::new("페이지를 열지 못했습니다.", e))?;
        }
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
        let mut u = self.0.app.get_webview(UI)?.url().ok()?;
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
        self.update(id, |t| t.loading = false);
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
        // Folders first, like Chrome's bar after an import.
        let mut out: Vec<BarItem> = folders.into_iter().map(|f| BarItem { kind: "folder".into(), title: f.clone(), url: f }).collect();
        out.extend(items);
        Some(out)
    }

    pub fn emit_bookmark_bar(&self) {
        let _ = self.0.app.emit_to(UI, "bookmark-bar", self.bookmark_bar());
    }

    // ---------- palette ----------

    /// Brings the palette webview above the tab webviews (they are added later and would cover it).
    fn raise_palette(&self) {
        let (Some(p), Some(w)) = (self.0.app.get_webview(PALETTE), self.window()) else { return };
        // macOS: reparenting re-adds the NSView on top. Windows: also move the HWND to the top.
        let _ = p.reparent(&w);
        #[cfg(windows)]
        crate::platform::bring_to_front(&p);
    }

    /// `mode`: prefix to start with (`>`, `*`, `#`, `@` or empty); `query`: initial text.
    pub fn open_palette(&self, mode: &str, query: &str) -> Result<(), AppError> {
        let p = self.0.app.get_webview(PALETTE).ok_or_else(|| AppError::new("팔레트를 열 수 없습니다.", "no palette webview"))?;
        self.0.state.lock().unwrap().palette_open = true;
        self.raise_palette();
        self.layout();
        let _ = p.show();
        let _ = p.set_focus();
        let _ = p.emit_to(PALETTE, "palette-open", serde_json::json!({ "mode": mode, "query": query }));
        Ok(())
    }

    pub fn close_palette(&self, refocus: bool) {
        let was = std::mem::replace(&mut self.0.state.lock().unwrap().palette_open, false);
        if let Some(p) = self.0.app.get_webview(PALETTE) {
            let _ = p.hide();
        }
        if was && refocus {
            self.focus_current();
        }
    }

    /// Keyboard focus to the open palette, else the active tab.
    pub fn focus_current(&self) {
        if self.palette_open() {
            if let Some(p) = self.0.app.get_webview(PALETTE) {
                let _ = p.set_focus();
            }
        } else if let Some(id) = self.active()
            && let Some(wv) = self.webview(id)
        {
            let _ = wv.set_focus();
        }
    }

    pub fn palette_open(&self) -> bool {
        self.0.state.lock().unwrap().palette_open
    }

    pub fn resize_palette(&self, h: f64) {
        self.0.state.lock().unwrap().palette_height = h.clamp(60.0, 640.0);
        self.layout();
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
        let (urls, active) = {
            let s = self.0.state.lock().unwrap();
            let urls: Vec<String> = s.tabs.iter().map(|t| t.url.clone()).collect();
            let active = s.active.and_then(|a| s.tabs.iter().position(|t| t.id == a)).unwrap_or(0);
            (urls, active)
        };
        let v = serde_json::json!({ "tabs": urls, "active": active });
        let _ = self.0.store.set("session", &v.to_string());
    }

    /// Opens the tabs of the last session (only the active one gets a webview now).
    pub fn restore_session(&self) -> Result<(), AppError> {
        let restore = self.0.store.get("restore_session").as_deref() != Some("false");
        let saved: Option<serde_json::Value> = if restore { self.0.store.get("session").and_then(|s| serde_json::from_str(&s).ok()) } else { None };
        let urls: Vec<String> = saved
            .as_ref()
            .and_then(|v| v.get("tabs"))
            .and_then(|t| t.as_array())
            .map(|a| a.iter().filter_map(|u| u.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        let active = saved.as_ref().and_then(|v| v.get("active")).and_then(|a| a.as_u64()).unwrap_or(0) as usize;
        let ids: Vec<u64> = urls.into_iter().filter(|u| u.starts_with("http") || kite_page(u).is_some()).map(|u| self.add_restored(u)).collect();
        match ids.get(active).or(ids.first()) {
            Some(id) => self.activate(*id),
            None => self.new_tab(None, true).map(|_| ()),
        }
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
