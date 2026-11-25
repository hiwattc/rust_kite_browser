//! `kite://home`: a personal start page (like iGoogle was): workspaces of app-style site icons
//! and a to-do list. The page owns the layout; Kite keeps its JSON in the store (not in the
//! page's local storage, which can be cleared), finds sites' icons, and saves/loads workspace
//! files.

use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, State};
use ureq::ResponseExt;

use crate::browser::Browser;
use crate::error::AppError;

const KEY: &str = "portal";
/// Icons are kept inline (data URLs); this is plenty for a few hundred of them.
const MAX_JSON: usize = 8 * 1024 * 1024;
const MAX_ICON: u64 = 600 * 1024;

#[tauri::command]
pub async fn portal_get(b: State<'_, Browser>) -> Result<Option<String>, AppError> {
    Ok(b.0.store.get(KEY))
}

#[tauri::command]
pub async fn portal_set(b: State<'_, Browser>, json: String) -> Result<(), AppError> {
    check(&json)?;
    b.0.store.set(KEY, &json)
}

fn check(json: &str) -> Result<(), AppError> {
    if json.len() > MAX_JSON {
        return Err(AppError::new("홈 데이터가 너무 큽니다(아이콘 이미지를 줄여 주세요).", json.len()));
    }
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| AppError::new("홈 데이터 형식이 올바르지 않습니다.", e))?;
    if !v.get("workspaces").is_some_and(|w| w.is_array()) {
        return Err(AppError::new("Kite 홈 워크스페이스 파일이 아닙니다.", "no workspaces"));
    }
    Ok(())
}

#[derive(Serialize)]
pub struct Site {
    title: String,
    url: String,
}

/// First run: the sites visited most (one per host) to start the icon grid with.
#[tauri::command]
pub async fn portal_seed(b: State<'_, Browser>) -> Result<Vec<Site>, AppError> {
    let mut visits = b.0.store.history_search("", 400);
    visits.sort_by_key(|v| std::cmp::Reverse(v.visit_count));
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for v in visits {
        let Ok(u) = url::Url::parse(&v.url) else { continue };
        let Some(host) = u.host_str() else { continue };
        if !u.scheme().starts_with("http") || host == "localhost" || host.parse::<std::net::IpAddr>().is_ok() || u.path().starts_with("/search") {
            continue;
        }
        let key = host.trim_start_matches("www.").to_string();
        if !seen.insert(key.clone()) {
            continue;
        }
        let title = short_title(&v.title).unwrap_or_else(|| host_name(&key));
        out.push(Site { title, url: format!("{}://{}/", u.scheme(), host) });
        if out.len() >= 10 {
            break;
        }
    }
    Ok(out)
}

/// "YouTube", "GitHub · Build software" → the name part when it is short enough for a label.
fn short_title(title: &str) -> Option<String> {
    let cut = [" - ", " | ", " : ", " · ", " — ", " – "].iter().filter_map(|sep| title.find(sep)).min().unwrap_or(title.len());
    let first = title[..cut].trim();
    if first.is_empty() {
        return None;
    }
    (first.chars().count() <= 14).then(|| first.to_string())
}

/// "news.ycombinator.com" → "Ycombinator", "naver.com" → "Naver".
fn host_name(host: &str) -> String {
    let parts: Vec<&str> = host.split('.').collect();
    let name = if parts.len() >= 3 && matches!(parts[parts.len() - 2], "co" | "or" | "go" | "ac" | "ne" | "com") { parts[parts.len() - 3] } else if parts.len() >= 2 { parts[parts.len() - 2] } else { host };
    let mut c = name.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

// ---------------------------------------------------------------- site icons

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(6)))
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Safari/605.1.15")
        .build()
        .into()
}

/// `<link rel="…icon…" href sizes>` of a page, best first (apple-touch-icon, then the largest).
pub fn icon_links(html: &str, base: &url::Url) -> Vec<url::Url> {
    let lower = html.to_ascii_lowercase();
    let mut found: Vec<(i64, url::Url)> = Vec::new();
    let mut at = 0;
    while let Some(i) = lower[at..].find("<link") {
        let start = at + i;
        let end = lower[start..].find('>').map(|e| start + e).unwrap_or(lower.len());
        let tag = &html[start..end];
        let tag_l = &lower[start..end];
        at = end;
        let attr = |name: &str| -> Option<String> {
            let p = tag_l.find(&format!("{name}="))? + name.len() + 1;
            let rest = &tag[p..];
            let (q, body) = match rest.chars().next()? {
                '"' => ('"', &rest[1..]),
                '\'' => ('\'', &rest[1..]),
                _ => (' ', rest),
            };
            Some(body[..body.find(q).unwrap_or(body.len())].trim().to_string())
        };
        let Some(rel) = attr("rel").map(|r| r.to_ascii_lowercase()) else { continue };
        if !rel.contains("icon") || rel.contains("mask") {
            continue;
        }
        let Some(href) = attr("href").and_then(|h| base.join(&h).ok()) else { continue };
        let size = attr("sizes").and_then(|s| s.split(['x', 'X']).next().and_then(|n| n.trim().parse::<i64>().ok())).unwrap_or(if href.path().ends_with(".svg") { 256 } else { 32 });
        let score = size + if rel.contains("apple-touch") { 1000 } else { 0 };
        found.push((score, href));
    }
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    found.into_iter().map(|(_, u)| u).collect()
}

fn fetch_icon(page: &str) -> Option<String> {
    use base64::Engine;
    let base = url::Url::parse(page).ok()?;
    let agent = agent();
    let mut candidates = Vec::new();
    if let Ok(mut resp) = agent.get(base.as_str()).call()
        && let Ok(html) = resp.body_mut().with_config().limit(2 * 1024 * 1024).read_to_string()
    {
        // Redirected (http → https, www): icons are relative to where the page ended up.
        let at = resp.get_uri().to_string().parse::<url::Url>().unwrap_or(base.clone());
        candidates = icon_links(&html, &at);
    }
    candidates.push(base.join("/apple-touch-icon.png").ok()?);
    candidates.push(base.join("/favicon.ico").ok()?);
    for c in candidates.into_iter().take(5) {
        let Ok(mut resp) = agent.get(c.as_str()).call() else { continue };
        let mime = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").split(';').next().unwrap_or("").trim().to_string();
        let Ok(bytes) = resp.body_mut().with_config().limit(MAX_ICON).read_to_vec() else { continue };
        let mime = if mime.starts_with("image/") {
            mime
        } else if c.path().ends_with(".svg") {
            "image/svg+xml".into()
        } else if c.path().ends_with(".png") {
            "image/png".into()
        } else if c.path().ends_with(".ico") {
            "image/x-icon".into()
        } else {
            continue;
        };
        if bytes.len() < 64 {
            continue;
        }
        return Some(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)));
    }
    None
}

/// One image (an icon the open page names) as a data URL, at most `max` bytes.
pub fn fetch_image_data_url(url: &str, max: u64) -> Option<String> {
    use base64::Engine;
    let u = url::Url::parse(url).ok()?;
    if !u.scheme().starts_with("http") {
        return None;
    }
    let mut resp = agent().get(u.as_str()).call().ok()?;
    let mime = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").split(';').next().unwrap_or("").trim().to_string();
    let bytes = resp.body_mut().with_config().limit(max).read_to_vec().ok()?;
    let mime = if mime.starts_with("image/") {
        mime
    } else if u.path().ends_with(".ico") {
        "image/x-icon".into()
    } else if u.path().ends_with(".png") {
        "image/png".into()
    } else if u.path().ends_with(".svg") {
        "image/svg+xml".into()
    } else {
        return None;
    };
    (bytes.len() >= 32).then(|| format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

/// Saved icons of these sites (`origin` → data URL). Only what was kept while each site was open;
/// never contacts a site.
#[tauri::command]
pub async fn favicons(b: State<'_, Browser>, origins: Vec<String>) -> Result<std::collections::HashMap<String, String>, AppError> {
    let origins: Vec<String> = origins.into_iter().take(2000).collect();
    Ok(b.0.store.favicons(&origins))
}

/// The site's own icon (largest it offers) as a data URL, or `None`.
#[tauri::command]
pub async fn portal_icon(url: String) -> Result<Option<String>, AppError> {
    let u = url::Url::parse(&url).map_err(|e| AppError::new("주소가 올바르지 않습니다.", e))?;
    if !u.scheme().starts_with("http") {
        return Ok(None);
    }
    Ok(tauri::async_runtime::spawn_blocking(move || fetch_icon(u.as_str())).await.ok().flatten())
}

// ---------------------------------------------------------------- files

/// Saves workspaces to a file the user picks. `false` when cancelled.
#[tauri::command]
pub async fn portal_export(app: AppHandle, json: String, name: String) -> Result<bool, AppError> {
    use tauri_plugin_dialog::DialogExt;
    check(&json)?;
    let file = format!("{}.kite-home.json", name.chars().filter(|c| !"/\\:*?\"<>|".contains(*c)).collect::<String>().trim());
    let a = app.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || a.dialog().file().set_title("Kite 홈 내보내기").set_file_name(&file).add_filter("Kite 홈", &["json"]).blocking_save_file())
        .await
        .map_err(|e| AppError::new("파일을 고르지 못했습니다.", e))?;
    let Some(path) = picked.and_then(|p| p.into_path().ok()) else { return Ok(false) };
    std::fs::write(&path, json).map_err(|e| AppError::new("파일을 저장하지 못했습니다.", e))?;
    Ok(true)
}

/// Reads a workspace file the user picks (`None` when cancelled).
#[tauri::command]
pub async fn portal_import(app: AppHandle) -> Result<Option<String>, AppError> {
    use tauri_plugin_dialog::DialogExt;
    let a = app.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || a.dialog().file().set_title("Kite 홈 불러오기").add_filter("Kite 홈", &["json"]).blocking_pick_file())
        .await
        .map_err(|e| AppError::new("파일을 고르지 못했습니다.", e))?;
    let Some(path) = picked.and_then(|p| p.into_path().ok()) else { return Ok(None) };
    let json = std::fs::read_to_string(&path).map_err(|e| AppError::new("파일을 읽지 못했습니다.", e))?;
    check(&json)?;
    Ok(Some(json))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_best_icon() {
        let base = url::Url::parse("https://site.example/a/").unwrap();
        let html = r#"<link rel="icon" href="/f16.png" sizes="16x16"><LINK REL='apple-touch-icon' HREF='touch.png'>
            <link rel="icon" type="image/png" sizes="192x192" href="https://cdn.example/i192.png"><link rel="mask-icon" href="/m.svg">"#;
        let got: Vec<String> = icon_links(html, &base).into_iter().map(|u| u.to_string()).collect();
        assert_eq!(got, vec!["https://site.example/a/touch.png", "https://cdn.example/i192.png", "https://site.example/f16.png"]);
    }

    #[test]
    fn names_sites() {
        assert_eq!(short_title("GitHub · Build and ship software").as_deref(), Some("GitHub"));
        assert_eq!(short_title("Hacker News").as_deref(), Some("Hacker News"));
        assert_eq!(short_title("Internet Assigned Numbers Authority"), None);
        assert_eq!(host_name("news.ycombinator.com"), "Ycombinator");
        assert_eq!(host_name("www.bbc.co.uk"), "Bbc");
        assert_eq!(host_name("naver.com"), "Naver");
    }

    #[test]
    fn checks_workspace_files() {
        assert!(check(r#"{"version":1,"workspaces":[]}"#).is_ok());
        assert!(check(r#"{"apps":[]}"#).is_err());
        assert!(check("not json").is_err());
    }
}
