//! Native bits Tauri does not expose: tab screenshots and (Windows) webview z-order.
//!
//! - macOS: `WKWebView takeSnapshotWithConfiguration:` → NSImage → PNG.
//! - Windows: `ICoreWebView2::CapturePreview` (PNG into an in-memory stream).

use std::sync::{Arc, Mutex};

use tauri::Webview;
use tokio::sync::oneshot;

type Reply = Arc<Mutex<Option<oneshot::Sender<Result<Vec<u8>, String>>>>>;

fn send(tx: &Reply, r: Result<Vec<u8>, String>) {
    if let Some(tx) = tx.lock().unwrap().take() {
        let _ = tx.send(r);
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
