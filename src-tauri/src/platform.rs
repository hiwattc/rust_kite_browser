//! Native bits Tauri does not expose: tab screenshots, (Windows) webview z-order and browser
//! extensions.
//!
//! - macOS: `WKWebView takeSnapshotWithConfiguration:` → NSImage → PNG.
//! - Windows: `ICoreWebView2::CapturePreview` (PNG into an in-memory stream).
//! - Extensions: macOS `WKWebExtensionController` (15.4+) attached to every tab's
//!   `WKWebViewConfiguration`; Windows WebView2 `AddBrowserExtension` on the shared profile.

use std::sync::{Arc, Mutex};

use tauri::Webview;
use tokio::sync::oneshot;

type Reply = Arc<Mutex<Option<oneshot::Sender<Result<Vec<u8>, String>>>>>;

fn send(tx: &Reply, r: Result<Vec<u8>, String>) {
    if let Some(tx) = tx.lock().unwrap().take() {
        let _ = tx.send(r);
    }
}

/// The certificate chain (DER, leaf first) a tab's page was loaded with, and the system's verdict
/// on it. `None` where the engine does not expose it (Windows) or the page has none.
pub struct PageCert {
    pub chain: Vec<Vec<u8>>,
    pub trusted: Option<bool>,
    /// Parts of the page came over plain http.
    pub mixed: bool,
}

/// The webview is still loading something (macOS; `None` elsewhere).
pub async fn is_loading(wv: &Webview) -> Option<bool> {
    #[cfg(target_os = "macos")]
    {
        let (tx, rx) = oneshot::channel::<bool>();
        let tx = Arc::new(Mutex::new(Some(tx)));
        wv.with_webview(move |pw| {
            // Safety: Tauri hands us the live WKWebView on the main thread.
            let view: &objc2_web_kit::WKWebView = unsafe { &*(pw.inner() as *const objc2_web_kit::WKWebView) };
            let loading = unsafe { view.isLoading() };
            if let Some(tx) = tx.lock().unwrap().take() {
                let _ = tx.send(loading);
            }
        })
        .ok()?;
        tokio::time::timeout(std::time::Duration::from_secs(2), rx).await.ok()?.ok()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = wv;
        None
    }
}

pub async fn page_certificate(wv: &Webview) -> Option<PageCert> {
    #[cfg(target_os = "macos")]
    {
        let (tx, rx) = oneshot::channel::<Option<PageCert>>();
        let tx = Arc::new(Mutex::new(Some(tx)));
        wv.with_webview(move |pw| {
            use objc2_security::SecTrustResultType;
            use objc2_web_kit::WKWebView;
            // Safety: Tauri hands us the live WKWebView on the main thread.
            let view: &WKWebView = unsafe { &*(pw.inner() as *const WKWebView) };
            let out = unsafe { view.serverTrust() }.map(|trust| {
                let mut chain = Vec::new();
                if let Some(arr) = unsafe { trust.certificate_chain() } {
                    for i in 0..arr.count() {
                        let cert = unsafe { &*(arr.value_at_index(i) as *const objc2_security::SecCertificate) };
                        chain.push(unsafe { cert.data() }.to_vec());
                    }
                }
                // The verdict WebKit already reached (no new evaluation on the main thread).
                let mut r = SecTrustResultType(0);
                let ok = unsafe { trust.trust_result(std::ptr::NonNull::from(&mut r)) } == 0;
                let trusted = ok.then_some(r == SecTrustResultType::Proceed || r == SecTrustResultType::Unspecified);
                PageCert { chain, trusted, mixed: !unsafe { view.hasOnlySecureContent() } }
            });
            if let Some(tx) = tx.lock().unwrap().take() {
                let _ = tx.send(out);
            }
        })
        .ok()?;
        tokio::time::timeout(std::time::Duration::from_secs(3), rx).await.ok()?.ok()?
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = wv;
        None
    }
}

/// PNG of the visible part of the tab.
pub async fn capture_png(wv: &Webview) -> Result<Vec<u8>, String> {
    let (tx, rx) = oneshot::channel();
    let tx: Reply = Arc::new(Mutex::new(Some(tx)));
    let tx2 = tx.clone();
    wv.with_webview(move |pw| capture_native(pw, tx2)).map_err(|e| format!("스크린샷 실패: {e}"))?;
    match tokio::time::timeout(std::time::Duration::from_secs(10), rx).await {
        Ok(Ok(r)) => r,
        Ok(Err(_)) => Err("스크린샷 응답이 없습니다.".into()),
        Err(_) => {
            send(&tx, Err(String::new()));
            Err("스크린샷 시간 초과".into())
        }
    }
}

#[cfg(target_os = "macos")]
fn capture_native(pw: tauri::webview::PlatformWebview, tx: Reply) {
    use block2::RcBlock;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage};
    use objc2_foundation::{NSDictionary, NSError};
    use objc2_web_kit::{WKSnapshotConfiguration, WKWebView};

    let Some(mtm) = MainThreadMarker::new() else {
        send(&tx, Err("메인 스레드가 아닙니다.".into()));
        return;
    };
    // Safety: Tauri hands us the live WKWebView on the main thread.
    let view: &WKWebView = unsafe { &*(pw.inner() as *const WKWebView) };
    let config = unsafe { WKSnapshotConfiguration::new(mtm) };
    unsafe { config.setAfterScreenUpdates(true) };
    let done = tx.clone();
    let block = RcBlock::new(move |img: *mut NSImage, err: *mut NSError| {
        if img.is_null() {
            let why = if err.is_null() { "unknown".to_string() } else { unsafe { (*err).localizedDescription().to_string() } };
            send(&done, Err(format!("스크린샷 실패: {why}")));
            return;
        }
        let img: &NSImage = unsafe { &*img };
        let png = img
            .TIFFRepresentation()
            .and_then(|tiff| NSBitmapImageRep::imageRepWithData(&tiff))
            .and_then(|rep| unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) })
            .map(|d| d.to_vec());
        send(&done, png.ok_or_else(|| "PNG 변환 실패".to_string()));
    });
    unsafe { view.takeSnapshotWithConfiguration_completionHandler(Some(&config), &block) };
}

#[cfg(windows)]
fn capture_native(pw: tauri::webview::PlatformWebview, tx: Reply) {
    use webview2_com::CapturePreviewCompletedHandler;
    use webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG;
    use windows::Win32::System::Com::StructuredStorage::CreateStreamOnHGlobal;
    use windows::Win32::System::Com::{IStream, STREAM_SEEK_SET};

    fn read_all(s: &IStream) -> Result<Vec<u8>, String> {
        unsafe {
            s.Seek(0, STREAM_SEEK_SET, None).map_err(|e| e.to_string())?;
            let mut out = Vec::new();
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let mut n = 0u32;
                let hr = s.Read(buf.as_mut_ptr() as *mut _, buf.len() as u32, Some(&mut n));
                if hr.is_err() {
                    return Err(format!("{hr:?}"));
                }
                if n == 0 {
                    break;
                }
                out.extend_from_slice(&buf[..n as usize]);
            }
            Ok(out)
        }
    }

    let r = (|| -> windows::core::Result<()> {
        unsafe {
            let core = pw.controller().CoreWebView2()?;
            let stream: IStream = CreateStreamOnHGlobal(Default::default(), true)?;
            let s2 = stream.clone();
            let done = tx.clone();
            let handler = CapturePreviewCompletedHandler::create(Box::new(move |res| {
                let out = if res.is_ok() { read_all(&s2) } else { Err(format!("CapturePreview 실패: {res:?}")) };
                send(&done, out);
                Ok(())
            }));
            core.CapturePreview(COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG, &stream, &handler)
        }
    })();
    if let Err(e) = r {
        send(&tx, Err(format!("스크린샷 실패: {e}")));
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn capture_native(_pw: tauri::webview::PlatformWebview, tx: Reply) {
    send(&tx, Err("이 OS에서는 스크린샷을 지원하지 않습니다.".into()));
}

/// Windows: put the webview's host window above its siblings (palette over the tabs).
#[cfg(windows)]
pub fn bring_to_front(wv: &Webview) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{HWND_TOP, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos};
    let _ = wv.with_webview(|pw| unsafe {
        let mut hwnd = HWND::default();
        if pw.controller().ParentWindow(&mut hwnd).is_ok() && !hwnd.is_invalid() {
            let _ = SetWindowPos(hwnd, Some(HWND_TOP), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    });
}

/// Whole-window opacity (0..1): the see-through window that follows a dragged tab.
/// Runs on the main thread (it touches the native window).
pub fn set_window_alpha(w: &tauri::Window, alpha: f64) {
    let win = w.clone();
    let _ = w.run_on_main_thread(move || window_alpha(&win, alpha));
}

fn window_alpha(w: &tauri::Window, alpha: f64) {
    #[cfg(target_os = "macos")]
    if let Ok(ptr) = w.ns_window() {
        // SAFETY: ns_window() is the live NSWindow of this Tauri window; setAlphaValue: takes a CGFloat.
        unsafe {
            let win = &*(ptr as *const objc2::runtime::AnyObject);
            let _: () = objc2::msg_send![win, setAlphaValue: alpha];
        }
    }
    #[cfg(windows)]
    if let Ok(hwnd) = w.hwnd() {
        use windows::Win32::Foundation::COLORREF;
        use windows::Win32::UI::WindowsAndMessaging::{GWL_EXSTYLE, GetWindowLongPtrW, LWA_ALPHA, SetLayeredWindowAttributes, SetWindowLongPtrW, WS_EX_LAYERED};
        // SAFETY: hwnd is this window's live top-level HWND.
        unsafe {
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            if alpha >= 0.999 {
                // fully opaque: drop the layered style again (layered windows redraw slower)
                let _ = SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex & !(WS_EX_LAYERED.0 as isize));
            } else {
                let _ = SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | WS_EX_LAYERED.0 as isize);
                let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), (alpha.clamp(0.0, 1.0) * 255.0) as u8, LWA_ALPHA);
            }
        }
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    let _ = (w, alpha);
}

/// Encodes a screenshot for the agent: optional crop (CSS px rect + viewport width), downscale
/// to `max_width`, JPEG (smaller) or PNG.
pub fn encode(png: &[u8], crop: Option<(f64, f64, f64, f64, f64)>, max_width: u32, jpeg: bool) -> Result<(Vec<u8>, u32, u32), String> {
    let mut img = image::load_from_memory_with_format(png, image::ImageFormat::Png).map_err(|e| format!("이미지 해석 실패: {e}"))?;
    if let Some((x, y, w, h, viewport_w)) = crop {
        // The snapshot is in device pixels, the rect in CSS pixels.
        let (iw, ih) = (img.width() as f64, img.height() as f64);
        let r = if viewport_w > 0.0 { iw / viewport_w } else { 1.0 };
        let x0 = (x * r).clamp(0.0, iw - 1.0);
        let y0 = (y * r).clamp(0.0, ih - 1.0);
        let cw = (w * r).min(iw - x0).max(1.0);
        let ch = (h * r).min(ih - y0).max(1.0);
        img = img.crop_imm(x0 as u32, y0 as u32, cw as u32, ch as u32);
    }
    if max_width > 0 && img.width() > max_width {
        let h = (img.height() as f64 * max_width as f64 / img.width() as f64).round().max(1.0) as u32;
        img = img.resize(max_width, h, image::imageops::FilterType::Triangle);
    }
    let (w, h) = (img.width(), img.height());
    let mut out = std::io::Cursor::new(Vec::new());
    if jpeg {
        let rgb = image::DynamicImage::ImageRgb8(img.to_rgb8());
        let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80);
        rgb.write_with_encoder(enc).map_err(|e| e.to_string())?;
    } else {
        img.write_to(&mut out, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    }
    Ok((out.into_inner(), w, h))
}

// ---------------------------------------------------------------- browser extensions

/// The `WKWebViewConfiguration` a tab webview is created with (macOS): carries the extension
/// controller. Only touched on the main thread; it just travels inside the builder.
pub struct TabConfig(#[cfg(target_os = "macos")] pub objc2::rc::Retained<objc2_web_kit::WKWebViewConfiguration>);
#[cfg(target_os = "macos")]
unsafe impl Send for TabConfig {}

/// URL of a page inside an extension (popup, options), if the engine has it loaded.
pub fn ext_page_url(id: &str, path: &str) -> Option<String> {
    let path = path.trim_start_matches('/');
    #[cfg(target_os = "macos")]
    return Some(format!("webkit-extension://{id}/{path}"));
    #[cfg(windows)]
    return ext_win::IDS.lock().unwrap().get(id).map(|wv2| format!("chrome-extension://{wv2}/{path}"));
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (id, path);
        None
    }
}

/// Whether this OS version can run extensions.
pub fn ext_supported() -> bool {
    #[cfg(target_os = "macos")]
    return objc2::available!(macos = 15.4);
    #[cfg(windows)]
    return true;
    #[cfg(not(any(target_os = "macos", windows)))]
    false
}

/// Loads (or reloads) an installed extension into the engine. Errors end up in a toast.
pub fn ext_load(app: &tauri::AppHandle, id: &str, dir: &std::path::Path) {
    if !ext_supported() {
        return;
    }
    #[cfg(target_os = "macos")]
    {
        let (a, id, dir) = (app.clone(), id.to_string(), dir.to_path_buf());
        let _ = app.run_on_main_thread(move || ext_mac::load(a, id, dir));
    }
    #[cfg(windows)]
    ext_win::load(app, id, dir);
    #[cfg(not(any(target_os = "macos", windows)))]
    let _ = (app, id, dir);
}

pub fn ext_unload(app: &tauri::AppHandle, id: &str) {
    if !ext_supported() {
        return;
    }
    #[cfg(target_os = "macos")]
    {
        let id = id.to_string();
        let _ = app.run_on_main_thread(move || ext_mac::unload(&id));
    }
    #[cfg(windows)]
    ext_win::unload(app, id);
    #[cfg(not(any(target_os = "macos", windows)))]
    let _ = (app, id);
}

/// Configuration for a new tab webview (macOS): extension pages get their extension's own
/// configuration, everything else a fresh one wired to the extension controller.
pub fn ext_tab_config(app: &tauri::AppHandle, url: &str) -> Option<TabConfig> {
    #[cfg(target_os = "macos")]
    {
        if !ext_supported() {
            return None;
        }
        let url = url.to_string();
        let make = move |mtm| ext_mac::tab_config(mtm, &url).map(TabConfig);
        if let Some(mtm) = objc2::MainThreadMarker::new() {
            return make(mtm);
        }
        let (tx, rx) = std::sync::mpsc::channel();
        app.run_on_main_thread(move || {
            let _ = tx.send(make(objc2::MainThreadMarker::new().expect("main thread")));
        })
        .ok()?;
        rx.recv_timeout(std::time::Duration::from_secs(5)).ok().flatten()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, url);
        None
    }
}

/// Extensions loaded in the engine right now (macOS).
#[cfg(target_os = "macos")]
static EXT_LOADED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// macOS: extension pages load only in a webview made with that extension's configuration (and
/// web pages should not run in one), so moving between the two needs a new webview.
/// Never on the main thread: the old webview must be gone before its label is reused.
pub fn needs_new_webview(from: &str, to: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        let ext = |u: &str| u.starts_with("webkit-extension://");
        let host = |u: &str| u.split('/').nth(2).map(str::to_string);
        objc2::MainThreadMarker::new().is_none() && (ext(from) || ext(to)) && !(ext(from) && ext(to) && host(from) == host(to))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (from, to);
        false
    }
}

/// A tab webview was just created (macOS). WebKit gives an extension's blocking rules
/// (declarativeNetRequest) to the webviews that exist when the extension loads, but not to ones
/// made later (content scripts are fine). So reload the extensions that have such rules, a
/// moment after the last new tab (the compiled rules are cached, so this is quick).
/// Extension pages themselves are skipped: reloading would cut them off.
pub fn ext_webview_created(app: &tauri::AppHandle, url: &str) {
    #[cfg(target_os = "macos")]
    {
        use std::sync::atomic::{AtomicU64, Ordering};
        static GEN: AtomicU64 = AtomicU64::new(0);
        if EXT_LOADED.load(Ordering::SeqCst) == 0 || url.starts_with("webkit-extension://") {
            return;
        }
        let generation = GEN.fetch_add(1, Ordering::SeqCst) + 1;
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            if GEN.load(Ordering::SeqCst) == generation {
                let _ = app.run_on_main_thread(ext_mac::reload_rule_extensions);
            }
        });
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, url);
}

fn ext_toast(app: &tauri::AppHandle, msg: String) {
    crate::browser::toast(app, msg);
}

#[cfg(target_os = "macos")]
mod ext_mac {
    use std::cell::OnceCell;
    use std::path::PathBuf;

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_foundation::{NSError, NSString, NSURL};
    use objc2_web_kit::{
        WKWebExtension, WKWebExtensionContext, WKWebExtensionContextPermissionStatus, WKWebExtensionController, WKWebExtensionControllerConfiguration, WKWebViewConfiguration,
    };

    thread_local! {
        static CONTROLLER: OnceCell<Retained<WKWebExtensionController>> = const { OnceCell::new() };
    }

    pub fn reload_rule_extensions() {
        let mtm = MainThreadMarker::new().expect("main thread");
        let c = controller(mtm);
        unsafe {
            for ctx in c.extensionContexts().iter().filter(|x| x.hasContentModificationRules()) {
                let _ = c.unloadExtensionContext_error(&ctx);
                if let Err(e) = c.loadExtensionContext_error(&ctx) {
                    tracing::warn!(id = %ctx.uniqueIdentifier(), error = %e.localizedDescription(), "extension reload failed");
                }
            }
        }
    }

    /// One controller for the app, with persistent extension storage.
    fn controller(mtm: MainThreadMarker) -> Retained<WKWebExtensionController> {
        CONTROLLER.with(|c| {
            c.get_or_init(|| unsafe {
                let config = WKWebExtensionControllerConfiguration::defaultConfiguration(mtm);
                WKWebExtensionController::initWithConfiguration(WKWebExtensionController::alloc(mtm), &config)
            })
            .clone()
        })
    }

    fn context(mtm: MainThreadMarker, id: &str) -> Option<Retained<WKWebExtensionContext>> {
        unsafe { controller(mtm).extensionContexts().iter().find(|c| c.uniqueIdentifier().to_string() == id) }
    }

    pub fn tab_config(mtm: MainThreadMarker, url: &str) -> Option<Retained<WKWebViewConfiguration>> {
        let ext_id = url.strip_prefix("webkit-extension://").and_then(|r| r.split('/').next());
        if let Some(cfg) = ext_id.and_then(|id| context(mtm, id)).and_then(|c| unsafe { c.webViewConfiguration() }) {
            return Some(cfg);
        }
        let cfg = unsafe { WKWebViewConfiguration::new(mtm) };
        unsafe { cfg.setWebExtensionController(Some(&controller(mtm))) };
        Some(cfg)
    }

    pub fn unload(id: &str) {
        let mtm = MainThreadMarker::new().expect("main thread");
        if let Some(Err(e)) = context(mtm, id).map(|ctx| unsafe { controller(mtm).unloadExtensionContext_error(&ctx) }) {
            tracing::warn!(id, error = %e.localizedDescription(), "unload extension failed");
        }
        super::EXT_LOADED.store(unsafe { controller(mtm).extensionContexts().count() }, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn load(app: tauri::AppHandle, id: String, dir: PathBuf) {
        let mtm = MainThreadMarker::new().expect("main thread");
        unload(&id);
        let url = NSURL::fileURLWithPath_isDirectory(&NSString::from_str(&dir.to_string_lossy()), true);
        let block = RcBlock::new(move |ext: *mut WKWebExtension, err: *mut NSError| {
            let Some(ext) = (unsafe { ext.as_ref() }) else {
                let why = unsafe { err.as_ref() }.map(|e| e.localizedDescription().to_string()).unwrap_or_default();
                tracing::warn!(id = %id, error = %why, "extension rejected");
                super::ext_toast(&app, format!("확장 프로그램을 불러오지 못했습니다: {why}"));
                return;
            };
            unsafe {
                for e in ext.errors().iter() {
                    tracing::info!(id = %id, warning = %e.localizedDescription(), "extension manifest warning");
                }
                let ctx = WKWebExtensionContext::contextForExtension(ext);
                ctx.setUniqueIdentifier(&NSString::from_str(&id));
                if let Some(base) = NSURL::URLWithString(&NSString::from_str(&format!("webkit-extension://{id}/"))) {
                    ctx.setBaseURL(&base);
                }
                ctx.setInspectable(true);
                // The user installed it on purpose: grant what the manifest asks for, like Chrome.
                for p in ext.requestedPermissions().iter() {
                    ctx.setPermissionStatus_forPermission(WKWebExtensionContextPermissionStatus::GrantedExplicitly, &p);
                }
                for m in ext.allRequestedMatchPatterns().iter() {
                    ctx.setPermissionStatus_forMatchPattern(WKWebExtensionContextPermissionStatus::GrantedExplicitly, &m);
                }
                match controller(mtm).loadExtensionContext_error(&ctx) {
                    Ok(()) => {
                        super::EXT_LOADED.store(controller(mtm).extensionContexts().count(), std::sync::atomic::Ordering::SeqCst);
                        tracing::info!(id = %id, "extension loaded");
                    }
                    Err(e) => {
                        let why = e.localizedDescription().to_string();
                        tracing::warn!(id = %id, error = %why, "extension load failed");
                        super::ext_toast(&app, format!("확장 프로그램을 켜지 못했습니다: {why}"));
                    }
                }
            }
        });
        unsafe { WKWebExtension::extensionWithResourceBaseURL_completionHandler(&url, &block, mtm) };
    }
}

#[cfg(windows)]
mod ext_win {
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::{LazyLock, Mutex};

    use tauri::Manager;
    use webview2_com::Microsoft::Web::WebView2::Win32::{ICoreWebView2_13, ICoreWebView2BrowserExtension, ICoreWebView2Profile7};
    use webview2_com::{BrowserExtensionRemoveCompletedHandler, ProfileAddBrowserExtensionCompletedHandler, ProfileGetBrowserExtensionsCompletedHandler, take_pwstr};
    use windows::core::{HSTRING, Interface, PWSTR};

    /// Kite's extension ID → the ID WebView2 gave it (used in `chrome-extension://` URLs).
    pub static IDS: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(Default::default);

    fn profile(pw: &tauri::webview::PlatformWebview) -> windows::core::Result<ICoreWebView2Profile7> {
        unsafe { pw.controller().CoreWebView2()?.cast::<ICoreWebView2_13>()?.Profile()?.cast::<ICoreWebView2Profile7>() }
    }

    fn ext_id(e: &ICoreWebView2BrowserExtension) -> Option<String> {
        let mut p = PWSTR::null();
        unsafe { e.Id(&mut p) }.ok()?;
        Some(take_pwstr(p))
    }

    /// Runs `f` on the chrome UI webview (all webviews share one WebView2 profile).
    fn with_profile(app: &tauri::AppHandle, f: impl FnOnce(ICoreWebView2Profile7) -> windows::core::Result<()> + Send + 'static) {
        let Some(ui) = app.try_state::<crate::browser::Browser>().and_then(|b| b.ui_webview()) else { return };
        let _ = ui.with_webview(move |pw| match profile(&pw) {
            Ok(p) => {
                if let Err(e) = f(p) {
                    tracing::warn!(error = %e, "webview2 extension call failed");
                }
            }
            Err(e) => tracing::warn!(error = %e, "webview2 profile unavailable (WebView2 runtime too old?)"),
        });
    }

    pub fn load(app: &tauri::AppHandle, id: &str, dir: &Path) {
        let (a, id, path) = (app.clone(), id.to_string(), HSTRING::from(dir));
        with_profile(app, move |p| unsafe {
            let handler = ProfileAddBrowserExtensionCompletedHandler::create(Box::new(move |r, ext| {
                match (r, ext.as_ref().and_then(ext_id)) {
                    (Ok(()), Some(wv2)) => {
                        tracing::info!(id = %id, wv2 = %wv2, "extension loaded");
                        IDS.lock().unwrap().insert(id, wv2);
                    }
                    (r, _) => {
                        let why = r.err().map(|e| e.message()).unwrap_or_default();
                        tracing::warn!(id = %id, error = %why, "extension load failed");
                        super::ext_toast(&a, format!("확장 프로그램을 불러오지 못했습니다: {why}"));
                    }
                }
                Ok(())
            }));
            p.AddBrowserExtension(&path, &handler)
        });
    }

    pub fn unload(app: &tauri::AppHandle, id: &str) {
        let Some(wv2) = IDS.lock().unwrap().remove(id) else { return };
        with_profile(app, move |p| unsafe {
            let handler = ProfileGetBrowserExtensionsCompletedHandler::create(Box::new(move |_, list| {
                let Some(list) = list else { return Ok(()) };
                let mut n = 0;
                list.Count(&mut n)?;
                for i in 0..n {
                    let e = list.GetValueAtIndex(i)?;
                    if ext_id(&e).as_deref() == Some(wv2.as_str()) {
                        e.Remove(&BrowserExtensionRemoveCompletedHandler::create(Box::new(|_| Ok(()))))?;
                    }
                }
                Ok(())
            }));
            p.GetBrowserExtensions(&handler)
        });
    }
}

// ---------------------------------------------------------------- certificate exceptions

/// Hosts whose certificate the user chose to accept anyway ("계속 진행", like Chrome's
/// "proceed (unsafe)"). Kept for this run only.
static CERT_EXCEPTIONS: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn allow_bad_certificate(host: &str) {
    let mut list = CERT_EXCEPTIONS.lock().unwrap();
    if !list.iter().any(|h| h == host) {
        list.push(host.to_string());
    }
}

pub fn bad_certificate_allowed(host: &str) -> bool {
    CERT_EXCEPTIONS.lock().unwrap().iter().any(|h| h == host)
}

/// wry's WKNavigationDelegate does not answer authentication challenges, so WebKit rejects any
/// certificate it does not trust. Kite adds `webView:didReceiveAuthenticationChallenge:
/// completionHandler:` to that class: server-trust challenges for a host in the exception list
/// are accepted with the server's own trust; everything else gets WebKit's default handling
/// (the same as before). Called after each tab webview is made; adding twice is a no-op.
/// WebKit reads which delegate methods exist when the delegate is set, so the webview's
/// delegate is set again afterwards.
pub fn install_cert_exceptions(wv: &Webview) {
    #[cfg(target_os = "macos")]
    let _ = wv.with_webview(|pw| {
        // Safety: the live WKWebView on the main thread; wry keeps its delegate alive.
        unsafe { mac_cert::hook(pw.inner() as *mut objc2::runtime::AnyObject) };
        // The download delegate class exists once a webview with a download handler does.
        mac_download::install();
    });
    #[cfg(not(target_os = "macos"))]
    let _ = wv;
}

#[cfg(target_os = "macos")]
mod mac_cert {
    use objc2::ffi::{class_addMethod, object_getClass};
    use objc2::runtime::{AnyObject, Imp, Sel};
    use objc2::{msg_send, sel};
    use objc2_foundation::NSString;

    type Handler = block2::Block<dyn Fn(isize, *mut AnyObject)>;
    const USE_CREDENTIAL: isize = 0;
    const DEFAULT_HANDLING: isize = 1;

    unsafe extern "C-unwind" fn challenge(_this: *mut AnyObject, _cmd: Sel, _webview: *mut AnyObject, challenge: *mut AnyObject, handler: *mut Handler) {
        // Safety: WebKit calls this on the main thread with live objects and a completion block.
        unsafe {
            let handler = &*handler;
            let space: *mut AnyObject = msg_send![challenge, protectionSpace];
            if !space.is_null() {
                let method: *mut NSString = msg_send![space, authenticationMethod];
                let host: *mut NSString = msg_send![space, host];
                if !method.is_null() && !host.is_null() && (*method).to_string() == "NSURLAuthenticationMethodServerTrust" && super::bad_certificate_allowed(&(*host).to_string()) {
                    let trust: *mut std::ffi::c_void = msg_send![space, serverTrust];
                    if !trust.is_null() {
                        let cred: *mut AnyObject = msg_send![objc2::class!(NSURLCredential), credentialForTrust: trust];
                        handler.call((USE_CREDENTIAL, cred));
                        return;
                    }
                }
            }
            handler.call((DEFAULT_HANDLING, std::ptr::null_mut()));
        }
    }

    /// Adds the method to the class of this webview's navigation delegate (wry's), then sets
    /// `navigationDelegate = nil; navigationDelegate = it` so WebKit sees the added method.
    pub unsafe fn hook(view: *mut AnyObject) {
        unsafe {
            let d: *mut AnyObject = msg_send![view, navigationDelegate];
            if d.is_null() {
                return;
            }
            let cls = object_getClass(d as *const _);
            if cls.is_null() {
                return;
            }
            let imp: Imp = std::mem::transmute::<unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject, *mut Handler), Imp>(challenge);
            // Returns NO when it is already there (a later tab): fine.
            class_addMethod(cls as *mut _, sel!(webView:didReceiveAuthenticationChallenge:completionHandler:), imp, c"v@:@@@?".as_ptr());
            let _: () = msg_send![view, setNavigationDelegate: std::ptr::null_mut::<AnyObject>()];
            let _: () = msg_send![view, setNavigationDelegate: d];
        }
    }
}

// ---------------------------------------------------------------- download progress

/// The engine's own download object (macOS `WKDownload`, retained) and the size the server
/// announced (`-1` unknown). Only touched on the main thread; released when dropped.
pub struct DownloadHandle {
    #[cfg(target_os = "macos")]
    ptr: usize,
    pub expected: i64,
}

#[cfg(target_os = "macos")]
impl Drop for DownloadHandle {
    fn drop(&mut self) {
        // Safety: we hold one retain on this object (taken in `mac_download::decide`).
        unsafe { objc2::ffi::objc_release(self.ptr as *mut _) };
    }
}

/// The download whose destination Tauri is being asked for right now (set just before, on the
/// same main-thread call), if the engine exposes one.
pub fn take_download_handle() -> Option<DownloadHandle> {
    #[cfg(target_os = "macos")]
    return mac_download::CURRENT.with(|c| c.take()).map(|(ptr, expected)| DownloadHandle { ptr, expected });
    #[cfg(not(target_os = "macos"))]
    None
}

/// Bytes received and expected (`-1` unknown). Main thread.
pub fn download_progress(h: &DownloadHandle) -> (i64, i64) {
    #[cfg(target_os = "macos")]
    {
        use objc2::msg_send;
        use objc2::runtime::AnyObject;
        // Safety: a live, retained WKDownload; NSProgress counts are plain getters.
        unsafe {
            let dl = h.ptr as *mut AnyObject;
            let p: *mut AnyObject = msg_send![dl, progress];
            if p.is_null() {
                return (0, h.expected);
            }
            let got: i64 = msg_send![p, completedUnitCount];
            let total: i64 = msg_send![p, totalUnitCount];
            (got, if total > 0 { total } else { h.expected })
        }
    }
    #[cfg(not(target_os = "macos"))]
    (0, h.expected)
}

/// Stops a download (WebKit then reports nothing: the caller ends it). Main thread.
pub fn download_cancel(h: &DownloadHandle) {
    #[cfg(target_os = "macos")]
    {
        use objc2::msg_send;
        use objc2::runtime::AnyObject;
        // Safety: a live WKDownload; the completion handler may be nil.
        unsafe {
            let _: () = msg_send![h.ptr as *mut AnyObject, cancel: std::ptr::null_mut::<AnyObject>()];
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = h;
}

/// wry's download delegate asks Tauri for the destination inside
/// `download:decideDestinationUsingResponse:suggestedFilename:completionHandler:` but passes
/// neither the WKDownload nor the response on. Kite wraps that method once: it notes the
/// download (retained) and the response's expected length, then runs wry's own code, during
/// which Kite's download handler takes them (`take_download_handle`).
#[cfg(target_os = "macos")]
mod mac_download {
    use std::cell::Cell;
    use std::sync::OnceLock;

    use objc2::ffi::{class_getInstanceMethod, class_getName, method_setImplementation, objc_copyClassList, objc_retain};
    use objc2::msg_send;
    use objc2::runtime::{AnyObject, Imp, Sel};

    thread_local! {
        pub static CURRENT: Cell<Option<(usize, i64)>> = const { Cell::new(None) };
    }

    type Decide = unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject, *mut AnyObject, *mut AnyObject);
    static ORIGINAL: OnceLock<usize> = OnceLock::new();

    unsafe extern "C-unwind" fn decide(this: *mut AnyObject, cmd: Sel, download: *mut AnyObject, response: *mut AnyObject, name: *mut AnyObject, handler: *mut AnyObject) {
        // Safety: called by WebKit on the main thread with live objects.
        unsafe {
            let expected: i64 = if response.is_null() { -1 } else { msg_send![response, expectedContentLength] };
            if !download.is_null() {
                objc_retain(download as *mut _);
                CURRENT.with(|c| c.set(Some((download as usize, expected))));
            }
            if let Some(orig) = ORIGINAL.get() {
                let f: Decide = std::mem::transmute::<usize, Decide>(*orig);
                f(this, cmd, download, response, name, handler);
            }
            // Not taken (no handler ran): give the retain back.
            if let Some((p, _)) = CURRENT.with(|c| c.take()) {
                objc2::ffi::objc_release(p as *mut _);
            }
        }
    }

    /// Wraps the method on wry's download delegate class (found by name), once.
    pub fn install() {
        if ORIGINAL.get().is_some() {
            return;
        }
        // Safety: Objective-C runtime calls; `decide` has the selector's signature.
        unsafe {
            let mut count: std::ffi::c_uint = 0;
            let list = objc_copyClassList(&mut count);
            if list.is_null() {
                return;
            }
            let classes = std::slice::from_raw_parts(list, count as usize);
            let cls = classes.iter().copied().find(|c| std::ffi::CStr::from_ptr(class_getName(*c)).to_string_lossy().contains("WryDownloadDelegate"));
            libc_free(list as *mut _);
            let Some(cls) = cls else { return };
            let sel = objc2::sel!(download:decideDestinationUsingResponse:suggestedFilename:completionHandler:);
            let m = class_getInstanceMethod(cls, sel);
            if m.is_null() {
                return;
            }
            let ours: Imp = std::mem::transmute::<Decide, Imp>(decide);
            if let Some(old) = method_setImplementation(m as *mut _, ours) {
                let _ = ORIGINAL.set(old as usize);
            }
        }
    }

    unsafe extern "C" {
        #[link_name = "free"]
        fn libc_free(p: *mut std::ffi::c_void);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([10, 20, 30, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn encode_scales_crops_and_converts() {
        let src = png(2000, 1000);
        let (b, w, h) = encode(&src, None, 1000, true).unwrap();
        assert_eq!((w, h), (1000, 500));
        assert_eq!(&b[..2], &[0xFF, 0xD8]); // JPEG
        let (b, w, h) = encode(&src, Some((10.0, 10.0, 100.0, 50.0, 1000.0)), 0, false).unwrap();
        assert_eq!((w, h), (200, 100));
        assert_eq!(&b[1..4], b"PNG");
        // crop beyond the edge is clamped
        let (_, w, h) = encode(&src, Some((990.0, 490.0, 100.0, 100.0, 1000.0)), 0, false).unwrap();
        assert_eq!((w, h), (20, 20));
    }
}
