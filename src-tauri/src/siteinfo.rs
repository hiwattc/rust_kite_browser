//! Site information (the lock in the address bar), like Chrome's page info: whether the
//! connection is secure and the site's certificate chain.
//!
//! macOS: the chain and verdict come from the page itself (`WKWebView.serverTrust`, the trust
//! WebKit evaluated for this page). Elsewhere (and if that is not available) Kite opens its own
//! TLS connection to the site to read the chain and checks it against the public roots.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Webview, WebviewUrl};

use crate::browser::Browser;
use crate::error::AppError;

pub const LABEL: &str = "siteinfo";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CertInfo {
    pub subject: String,
    pub subject_org: Option<String>,
    pub issuer: String,
    pub issuer_org: Option<String>,
    pub not_before: String,
    pub not_after: String,
    pub days_left: i64,
    pub serial: String,
    pub sha256: String,
    pub sans: Vec<String>,
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SiteInfo {
    pub url: String,
    pub host: String,
    /// `secure` | `insecure` (http) | `internal` (Kite pages) | `local` (file, data…)
    pub kind: String,
    /// The system / Kite trusts the chain.
    pub trusted: Option<bool>,
    pub trust_note: Option<String>,
    /// `page` (the certificate the page loaded with) | `separate` (Kite's own connection)
    pub source: String,
    /// Some resources of the page came over plain http.
    pub mixed: bool,
    /// Leaf first.
    pub chain: Vec<CertInfo>,
    pub error: Option<String>,
}

// ---------------------------------------------------------------- certificates

fn name_part(n: &x509_parser::x509::X509Name, oid: &x509_parser::der_parser::oid::Oid) -> Option<String> {
    n.iter_by_oid(oid).next().and_then(|a| a.as_str().ok()).map(str::to_string)
}

pub fn parse_cert(der: &[u8]) -> Option<CertInfo> {
    use x509_parser::extensions::GeneralName;
    use x509_parser::oid_registry::{OID_X509_COMMON_NAME, OID_X509_ORGANIZATION_NAME};
    let (_, c) = x509_parser::parse_x509_certificate(der).ok()?;
    let subject = name_part(c.subject(), &OID_X509_COMMON_NAME).or_else(|| name_part(c.subject(), &OID_X509_ORGANIZATION_NAME)).unwrap_or_else(|| c.subject().to_string());
    let issuer = name_part(c.issuer(), &OID_X509_COMMON_NAME).or_else(|| name_part(c.issuer(), &OID_X509_ORGANIZATION_NAME)).unwrap_or_else(|| c.issuer().to_string());
    let day = |t: &x509_parser::time::ASN1Time| {
        let d = t.to_datetime();
        format!("{:04}-{:02}-{:02}", d.year(), u8::from(d.month()), d.day())
    };
    let now = x509_parser::time::ASN1Time::now();
    let days_left = (c.validity().not_after.timestamp() - now.timestamp()) / 86_400;
    let sans = c
        .subject_alternative_name()
        .ok()
        .flatten()
        .map(|ext| ext.value.general_names.iter().filter_map(|g| if let GeneralName::DNSName(d) = g { Some(d.to_string()) } else { None }).collect())
        .unwrap_or_default();
    let spki = c.public_key();
    let key = match spki.parsed() {
        Ok(x509_parser::public_key::PublicKey::RSA(r)) => format!("RSA {}비트", r.key_size()),
        Ok(x509_parser::public_key::PublicKey::EC(e)) => format!("ECDSA {}비트", e.key_size()),
        _ => spki.algorithm.algorithm.to_id_string(),
    };
    let sha = ring::digest::digest(&ring::digest::SHA256, der);
    Some(CertInfo {
        subject,
        subject_org: name_part(c.subject(), &OID_X509_ORGANIZATION_NAME),
        issuer,
        issuer_org: name_part(c.issuer(), &OID_X509_ORGANIZATION_NAME),
        not_before: day(&c.validity().not_before),
        not_after: day(&c.validity().not_after),
        days_left,
        serial: c.raw_serial().iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":"),
        sha256: sha.as_ref().iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":"),
        sans,
        key,
    })
}

/// DER certificates, leaf first.
type Chain = Vec<Vec<u8>>;
/// What a separate connection found: the chain, trusted by the public roots, why not.
pub type Fetched = (Chain, bool, Option<String>);

/// Kite's own TLS connection: the chain the server presents, and whether it chains to a public
/// root (Mozilla's set). Nothing is sent after the handshake.
/// For the error page's diagnosis.
pub fn fetch_chain_pub(host: &str, port: u16) -> Result<Fetched, String> {
    fetch_chain(host, port)
}

fn fetch_chain(host: &str, port: u16) -> Result<Fetched, String> {
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};

    #[derive(Debug)]
    struct Capture {
        inner: Arc<rustls::client::WebPkiServerVerifier>,
        provider: Arc<rustls::crypto::CryptoProvider>,
        got: Mutex<Option<(Chain, Result<(), String>)>>,
    }
    impl ServerCertVerifier for Capture {
        fn verify_server_cert(&self, end: &CertificateDer<'_>, inter: &[CertificateDer<'_>], name: &ServerName<'_>, ocsp: &[u8], now: UnixTime) -> Result<ServerCertVerified, rustls::Error> {
            let chain = std::iter::once(end).chain(inter.iter()).map(|c| c.as_ref().to_vec()).collect();
            let verdict = self.inner.verify_server_cert(end, inter, name, ocsp, now).map(|_| ()).map_err(|e| e.to_string());
            *self.got.lock().unwrap() = Some((chain, verdict));
            // Only looking: accept, the connection is closed right after.
            Ok(ServerCertVerified::assertion())
        }
        fn verify_tls12_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &rustls::DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls12_signature(m, c, d, &self.provider.signature_verification_algorithms)
        }
        fn verify_tls13_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &rustls::DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls13_signature(m, c, d, &self.provider.signature_verification_algorithms)
        }
        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            self.provider.signature_verification_algorithms.supported_schemes()
        }
    }

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let roots = Arc::new(rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() });
    let inner = rustls::client::WebPkiServerVerifier::builder_with_provider(roots, provider.clone()).build().map_err(|e| e.to_string())?;
    let capture = Arc::new(Capture { inner, provider: provider.clone(), got: Mutex::new(None) });
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(capture.clone())
        .with_no_client_auth();
    let name = ServerName::try_from(host.to_string()).map_err(|e| e.to_string())?;
    let addr = (host, port).to_socket_addrs().map_err(|e| e.to_string())?.next().ok_or("주소를 찾지 못했습니다")?;
    let mut sock = TcpStream::connect_timeout(&addr, Duration::from_secs(5)).map_err(|e| e.to_string())?;
    let _ = sock.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = sock.set_write_timeout(Some(Duration::from_secs(5)));
    let mut conn = rustls::ClientConnection::new(Arc::new(config), name).map_err(|e| e.to_string())?;
    while conn.is_handshaking() {
        conn.complete_io(&mut sock).map_err(|e| e.to_string())?;
    }
    conn.send_close_notify();
    let _ = conn.complete_io(&mut sock);
    let _ = sock.flush();
    let mut sink = [0u8; 1];
    let _ = sock.read(&mut sink);
    let (chain, verdict) = capture.got.lock().unwrap().take().ok_or("인증서를 받지 못했습니다")?;
    Ok((chain, verdict.is_ok(), verdict.err()))
}

/// What the lock shows for the active tab of window `n`.
pub async fn info(b: &Browser) -> SiteInfo {
    let Some(tab) = b.active().and_then(|id| b.tab(id)) else { return SiteInfo { kind: "local".into(), ..Default::default() } };
    let mut out = SiteInfo { url: tab.url.clone(), ..Default::default() };
    if tab.url.starts_with("kite://") {
        out.kind = "internal".into();
        out.host = tab.url.clone();
        return out;
    }
    let Ok(u) = url::Url::parse(&tab.url) else {
        out.kind = "local".into();
        return out;
    };
    out.host = u.host_str().unwrap_or("").to_string();
    match u.scheme() {
        "https" => {}
        "http" => {
            out.kind = "insecure".into();
            return out;
        }
        _ => {
            out.kind = "local".into();
            return out;
        }
    }
    out.kind = "secure".into();
    // The page's own certificate first (macOS).
    if let Some(wv) = b.webview(tab.id)
        && let Some(pc) = crate::platform::page_certificate(&wv).await
    {
        out.source = "page".into();
        out.trusted = pc.trusted;
        out.mixed = pc.mixed;
        out.chain = pc.chain.iter().filter_map(|d| parse_cert(d)).collect();
        if !out.chain.is_empty() {
            return out;
        }
    }
    let host = out.host.clone();
    let port = u.port().unwrap_or(443);
    match tauri::async_runtime::spawn_blocking(move || fetch_chain(&host, port)).await {
        Ok(Ok((chain, trusted, note))) => {
            out.source = "separate".into();
            out.trusted = Some(trusted);
            out.trust_note = note;
            out.chain = chain.iter().filter_map(|d| parse_cert(d)).collect();
        }
        Ok(Err(e)) => out.error = Some(format!("인증서를 가져오지 못했습니다: {e}")),
        Err(e) => out.error = Some(e.to_string()),
    }
    out
}

// ---------------------------------------------------------------- the bubble window

/// Lock clicked: the site info bubble under it (`x`, `y`: the icon's bottom left in the toolbar).
/// Clicking it again (or elsewhere) closes it.
#[tauri::command]
pub async fn site_info_open(app: AppHandle, webview: Webview, x: f64, y: f64) -> Result<(), AppError> {
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.close();
        return Ok(());
    }
    app.state::<Browser>().touch(&webview);
    let win = webview.window();
    let scale = win.scale_factor().unwrap_or(1.0);
    let origin = win.inner_position().map(|p| p.to_logical::<f64>(scale)).unwrap_or(tauri::LogicalPosition::new(0.0, 0.0));
    let w = tauri::WebviewWindowBuilder::new(&app, LABEL, WebviewUrl::App("siteinfo.html".into()))
        .title("사이트 정보")
        .decorations(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .shadow(true)
        .inner_size(380.0, 160.0)
        .position(origin.x + x - 8.0, origin.y + y + 6.0)
        .visible(false)
        .focused(true)
        .zoom_hotkeys_enabled(false)
        .browser_extensions_enabled(true)
        .build()
        .map_err(|e| AppError::new("사이트 정보를 열지 못했습니다.", e))?;
    let w2 = w.clone();
    let opened = std::time::Instant::now();
    w.on_window_event(move |e| {
        if let tauri::WindowEvent::Focused(false) = e
            && opened.elapsed() > Duration::from_millis(400)
        {
            let _ = w2.close();
        }
    });
    Ok(())
}

/// The bubble's page asks what to show.
#[tauri::command]
pub async fn site_info(b: tauri::State<'_, Browser>) -> Result<SiteInfo, AppError> {
    Ok(info(&b).await)
}

/// The bubble's page fits the window to its content, then it shows.
#[tauri::command]
pub fn site_info_size(app: AppHandle, height: f64) -> Result<(), AppError> {
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.set_size(tauri::LogicalSize::new(380.0, height.clamp(80.0, 720.0)));
        let _ = w.show();
        let _ = w.set_focus();
    }
    Ok(())
}

/// Closes the bubble (Esc, a link that opens something).
#[tauri::command]
pub fn site_info_close(app: AppHandle) -> Result<(), AppError> {
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.emit_to(LABEL, "closing", ());
        let _ = w.close();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real network: Kite's own connection reads and parses a public site's chain.
    #[test]
    #[ignore]
    fn reads_a_public_chain() {
        let (chain, trusted, _) = fetch_chain("example.com", 443).unwrap();
        let leaf = parse_cert(&chain[0]).unwrap();
        println!("{leaf:?} trusted={trusted} chain={}", chain.len());
        assert!(trusted && leaf.days_left > 0 && leaf.sha256.len() == 95);
    }
}
