//! When a page cannot be reached. WKWebView shows nothing when a navigation fails before
//! anything arrives (and wry does not report it), so Kite watches its own navigations: if the
//! webview stops loading without ever showing the new page, the load failed. Kite then finds
//! out why (no internet, unknown name, refused, timed out, TLS) with its own connection and
//! shows `neterror.html` (a small 3D scene and what to do) while the address bar keeps the
//! address that failed. Reload or "다시 시도" go to that address again.
//!
//! Windows: WebView2 shows its own error page, so nothing is needed there.

use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Manager, Webview};

use crate::browser::{Browser, tab_id_of};
use crate::error::AppError;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Diagnosis {
    /// `offline` | `dns` | `refused` | `timeout` | `tls` | `cert` | `unknown`
    pub code: &'static str,
    pub detail: String,
}

async fn online() -> bool {
    for addr in ["1.1.1.1:443", "8.8.8.8:443"] {
        if let Ok(Ok(_)) = tokio::time::timeout(Duration::from_secs(3), tokio::net::TcpStream::connect(addr)).await {
            return true;
        }
    }
    false
}

/// Why `url` could not be loaded, found with Kite's own connection.
pub async fn diagnose(url: &str) -> Diagnosis {
    let d = |code, detail: String| Diagnosis { code, detail };
    let Ok(u) = url::Url::parse(url) else { return d("unknown", "주소 형식이 올바르지 않습니다.".into()) };
    let Some(host) = u.host_str().map(str::to_string) else { return d("unknown", String::new()) };
    let port = u.port_or_known_default().unwrap_or(443);
    let addrs = match tokio::time::timeout(Duration::from_secs(5), tokio::net::lookup_host((host.as_str(), port))).await {
        Ok(Ok(a)) => a.collect::<Vec<_>>(),
        Ok(Err(e)) => {
            return if online().await { d("dns", e.to_string()) } else { d("offline", "인터넷 연결을 찾지 못했습니다.".into()) };
        }
        Err(_) => return if online().await { d("dns", "이름 조회 시간 초과".into()) } else { d("offline", String::new()) },
    };
    let Some(addr) = addrs.first().copied() else { return d("dns", String::new()) };
    match tokio::time::timeout(Duration::from_secs(8), tokio::net::TcpStream::connect(addr)).await {
        Err(_) => return d("timeout", format!("{addr}에 8초 안에 연결되지 않았습니다.")),
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::ConnectionRefused => return d("refused", format!("{addr}가 연결을 거부했습니다.")),
        Ok(Err(e)) => return if online().await { d("timeout", e.to_string()) } else { d("offline", e.to_string()) },
        Ok(Ok(_)) => {}
    }
    if u.scheme() == "https" {
        let h = host.clone();
        match tauri::async_runtime::spawn_blocking(move || crate::siteinfo::fetch_chain_pub(&h, port)).await {
            Ok(Err(e)) => return d("tls", e),
            Ok(Ok((_, false, note))) => return d("cert", note.unwrap_or_default()),
            _ => {}
        }
    }
    d("unknown", "서버에는 연결되지만 페이지를 받지 못했습니다.".into())
}

/// Address of the error page for `url`.
pub fn page_url(base: &url::Url, url: &str, why: &Diagnosis) -> Option<url::Url> {
    let mut u = base.join("neterror.html").ok()?;
    u.query_pairs_mut().append_pair("u", url).append_pair("r", why.code).append_pair("d", &why.detail);
    Some(u)
}

/// "다시 시도" on the error page: the address that failed, again.
#[tauri::command]
pub async fn neterror_retry(app: AppHandle, webview: Webview) -> Result<(), AppError> {
    let b = app.state::<Browser>().inner().clone();
    let id = tab_id_of(webview.label()).ok_or_else(|| AppError::new("탭을 찾지 못했습니다.", webview.label()))?;
    let url = b.tab(id).and_then(|t| t.error_url).ok_or_else(|| AppError::new("다시 열 주소가 없습니다.", id))?;
    b.navigate_url(id, &url)
}

/// "이동 (안전하지 않음)" on a certificate error: accept this host's certificate for this run
/// and load the address again. Only the address the tab failed on (never one from the page).
#[tauri::command]
pub async fn neterror_proceed(app: AppHandle, webview: Webview) -> Result<(), AppError> {
    let b = app.state::<Browser>().inner().clone();
    let id = tab_id_of(webview.label()).ok_or_else(|| AppError::new("탭을 찾지 못했습니다.", webview.label()))?;
    let url = b.tab(id).and_then(|t| t.error_url).ok_or_else(|| AppError::new("다시 열 주소가 없습니다.", id))?;
    let host = url::Url::parse(&url).ok().filter(|u| u.scheme() == "https").and_then(|u| u.host_str().map(str::to_string)).ok_or_else(|| AppError::new("HTTPS 주소가 아닙니다.", &url))?;
    tracing::info!(%host, "certificate accepted by the user for this run");
    crate::platform::allow_bad_certificate(&host);
    b.navigate_url(id, &url)
}

/// "검색하기": the failed address's name in the search engine.
#[tauri::command]
pub async fn neterror_search(app: AppHandle, webview: Webview) -> Result<(), AppError> {
    let b = app.state::<Browser>().inner().clone();
    let id = tab_id_of(webview.label()).ok_or_else(|| AppError::new("탭을 찾지 못했습니다.", webview.label()))?;
    let url = b.tab(id).and_then(|t| t.error_url).unwrap_or_default();
    let q = url::Url::parse(&url).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or(url);
    b.navigate(id, &q).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn names_that_do_not_exist() {
        let r = diagnose("https://this-host-does-not-exist-kite-test.invalid/").await;
        assert!(r.code == "dns" || r.code == "offline", "{r:?}");
    }

    #[tokio::test]
    async fn refused_port() {
        // Nothing listens on this local port.
        let r = diagnose("http://127.0.0.1:9/").await;
        assert_eq!(r.code, "refused", "{r:?}");
    }

    #[test]
    fn page_url_carries_the_address() {
        let base = url::Url::parse("tauri://localhost/").unwrap();
        let u = page_url(&base, "https://a.example/x?y=1", &Diagnosis { code: "dns", detail: "no".into() }).unwrap();
        assert_eq!(u.path(), "/neterror.html");
        assert!(u.query_pairs().any(|(k, v)| k == "u" && v == "https://a.example/x?y=1"));
    }
}
