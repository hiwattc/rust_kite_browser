//! Browser extensions in the Chrome format, installed into `<app data>/extensions/<id>/`
//! (one unpacked extension per folder).
//!
//! - From the Chrome Web Store: the CRX is downloaded from Google's update server, its header
//!   stripped and the ZIP inside unpacked. The folder name is the store ID.
//! - From a folder (an unpacked extension): copied in; the ID is derived from the folder path
//!   like Chrome does for unpacked extensions.
//!
//! The OS engine runs them (see `platform.rs`): WebKit's `WKWebExtensionController` on macOS,
//! WebView2 browser extensions on Windows. This module only handles files and manifests.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Manager};

use crate::error::AppError;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    /// Path of the toolbar popup page inside the extension, if any.
    pub popup: Option<String>,
    pub options: Option<String>,
    /// `data:` URL of the icon.
    pub icon: Option<String>,
    /// `store` or `local`.
    pub source: String,
    pub manifest_version: u64,
}

/// What the toolbar shows for an installed extension.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolbarExt {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    pub popup: bool,
}

/// Extensions for the toolbar (none where the engine has no extension support).
pub fn toolbar(app: &AppHandle) -> Vec<ToolbarExt> {
    if !crate::platform::ext_supported() {
        return Vec::new();
    }
    list(app).into_iter().map(|(i, _)| ToolbarExt { id: i.id, name: i.name, icon: i.icon, popup: i.popup.is_some() }).collect()
}

/// Tells every window's toolbar that the installed extensions changed.
pub fn emit_toolbar(app: &AppHandle) {
    if let Some(b) = app.try_state::<crate::browser::Browser>() {
        b.emit_all_ui("extensions", toolbar(app));
    }
}

pub fn dir(app: &AppHandle) -> Result<PathBuf, AppError> {
    let d = app.path().app_data_dir().map_err(|e| AppError::new("데이터 폴더를 찾지 못했습니다.", e))?.join("extensions");
    fs::create_dir_all(&d).map_err(|e| AppError::new("확장 프로그램 폴더를 만들지 못했습니다.", e))?;
    Ok(d)
}

/// Installed extensions (folders with a readable manifest), by name.
pub fn list(app: &AppHandle) -> Vec<(ExtInfo, PathBuf)> {
    let Ok(d) = dir(app) else { return vec![] };
    let mut v: Vec<(ExtInfo, PathBuf)> = fs::read_dir(d)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
        .filter_map(|p| read_info(&p).ok().map(|i| (i, p)))
        .collect();
    v.sort_by_key(|(i, _)| i.name.to_lowercase());
    v
}

/// A Chrome Web Store ID (32 letters a–p) in a store URL or on its own.
pub fn parse_store_id(input: &str) -> Option<String> {
    let is_id = |s: &str| s.len() == 32 && s.bytes().all(|b| (b'a'..=b'p').contains(&b));
    let s = input.trim();
    if is_id(s) {
        return Some(s.to_string());
    }
    let path = url::Url::parse(s).ok().map(|u| u.path().to_string()).unwrap_or_else(|| s.to_string());
    path.split('/').find(|seg| is_id(seg)).map(str::to_string)
}

/// Chrome's ID for an unpacked extension: SHA-256 of the path, first 16 bytes, hex digits → a–p.
pub fn local_id(path: &Path) -> String {
    let d = ring::digest::digest(&ring::digest::SHA256, path.to_string_lossy().as_bytes());
    d.as_ref()[..16].iter().flat_map(|b| [b >> 4, b & 0xf]).map(|n| (b'a' + n) as char).collect()
}

/// Downloads the extension from the Chrome Web Store and unpacks it. Blocking (network).
pub fn install_from_store(app: &AppHandle, id: &str) -> Result<(ExtInfo, PathBuf), AppError> {
    let crx = download_crx(id)?;
    let zip = crx_zip(&crx).ok_or_else(|| AppError::new("확장 프로그램 파일 형식이 아닙니다.", format!("{} bytes", crx.len())))?;
    let base = dir(app)?;
    let tmp = base.join(format!(".tmp-{id}"));
    let _ = fs::remove_dir_all(&tmp);
    unzip(zip, &tmp)?;
    finish(&base, &tmp, id, "store")
}

fn download_crx(id: &str) -> Result<Vec<u8>, AppError> {
    let url = format!(
        "https://clients2.google.com/service/update2/crx?response=redirect&prodversion=138.0.0.0&acceptformat=crx2,crx3&x=id%3D{id}%26installsource%3Dondemand%26uc"
    );
    let mut resp = ureq::get(&url).call().map_err(|e| AppError::new("웹 스토어에서 받지 못했습니다. 주소나 ID를 확인하세요.", e))?;
    resp.body_mut().with_config().limit(200 * 1024 * 1024).read_to_vec().map_err(|e| AppError::new("웹 스토어에서 받지 못했습니다.", e))
}

/// Copies an unpacked extension folder in.
pub fn install_from_folder(app: &AppHandle, src: &Path) -> Result<(ExtInfo, PathBuf), AppError> {
    if !src.join("manifest.json").is_file() {
        return Err(AppError::new("manifest.json이 있는 폴더(압축을 푼 확장 프로그램)를 고르세요.", src.display().to_string()));
    }
    let id = local_id(src);
    let base = dir(app)?;
    let tmp = base.join(format!(".tmp-{id}"));
    let _ = fs::remove_dir_all(&tmp);
    copy_dir(src, &tmp).map_err(|e| AppError::new("폴더를 복사하지 못했습니다.", e))?;
    finish(&base, &tmp, &id, "local")
}

pub fn remove(app: &AppHandle, id: &str) -> Result<(), AppError> {
    if parse_store_id(id).as_deref() != Some(id) {
        return Err(AppError::new("확장 프로그램 ID가 올바르지 않습니다.", id));
    }
    let p = dir(app)?.join(id);
    if p.exists() {
        fs::remove_dir_all(&p).map_err(|e| AppError::new("확장 프로그램을 지우지 못했습니다.", e))?;
    }
    Ok(())
}

/// Moves the unpacked folder into place (replacing an older version) and reads it back.
fn finish(base: &Path, tmp: &Path, id: &str, source: &str) -> Result<(ExtInfo, PathBuf), AppError> {
    // Chromium refuses unpacked extensions with `_`-prefixed entries other than `_locales`
    // (the store adds `_metadata` with signatures).
    let _ = fs::remove_dir_all(tmp.join("_metadata"));
    if let Err(e) = read_info(tmp) {
        let _ = fs::remove_dir_all(tmp);
        return Err(e);
    }
    fs::write(tmp.join(".kite-source"), source).map_err(|e| AppError::new("확장 프로그램을 설치하지 못했습니다.", e))?;
    let dest = base.join(id);
    let _ = fs::remove_dir_all(&dest);
    fs::rename(tmp, &dest).map_err(|e| AppError::new("확장 프로그램을 설치하지 못했습니다.", e))?;
    Ok((read_info(&dest)?, dest))
}

/// The ZIP inside a CRX file (CRX2 or CRX3), or the input itself if it is already a ZIP.
fn crx_zip(b: &[u8]) -> Option<&[u8]> {
    let u32_at = |i: usize| b.get(i..i + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap()) as usize);
    if b.starts_with(b"PK\x03\x04") {
        return Some(b);
    }
    if !b.starts_with(b"Cr24") {
        return None;
    }
    let start = match u32_at(4)? {
        2 => 16 + u32_at(8)? + u32_at(12)?,
        3 => 12 + u32_at(8)?,
        _ => return None,
    };
    b.get(start..).filter(|z| z.starts_with(b"PK"))
}

fn unzip(zip: &[u8], dest: &Path) -> Result<(), AppError> {
    let bad = |e: &dyn std::fmt::Display| AppError::new("확장 프로그램 압축을 풀지 못했습니다.", e.to_string());
    let mut a = zip::ZipArchive::new(std::io::Cursor::new(zip)).map_err(|e| bad(&e))?;
    for i in 0..a.len() {
        let mut f = a.by_index(i).map_err(|e| bad(&e))?;
        // `enclosed_name` drops absolute paths and `..`: nothing lands outside `dest`.
        let Some(rel) = f.enclosed_name() else { continue };
        let out = dest.join(rel);
        if f.is_dir() {
            fs::create_dir_all(&out).map_err(|e| bad(&e))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(|e| bad(&e))?;
        }
        let mut buf = Vec::with_capacity(f.size() as usize);
        f.read_to_end(&mut buf).map_err(|e| bad(&e))?;
        fs::write(&out, buf).map_err(|e| bad(&e))?;
    }
    Ok(())
}

fn copy_dir(src: &Path, dest: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dest)?;
    for e in fs::read_dir(src)? {
        let e = e?;
        let name = e.file_name();
        if name.to_string_lossy().starts_with(".git") || name == "node_modules" {
            continue;
        }
        let ty = e.file_type()?;
        if ty.is_dir() {
            copy_dir(&e.path(), &dest.join(&name))?;
        } else if ty.is_file() {
            fs::copy(e.path(), dest.join(&name))?;
        }
    }
    Ok(())
}

fn read_info(dir: &Path) -> Result<ExtInfo, AppError> {
    let bad = |e: &dyn std::fmt::Display| AppError::new("manifest.json을 읽지 못했습니다.", e.to_string());
    let text = fs::read_to_string(dir.join("manifest.json")).map_err(|e| bad(&e))?;
    // Some manifests start with a BOM.
    let m: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| bad(&e))?;
    let s = |v: &Value| v.as_str().map(str::to_string);
    let msg = |v: Option<String>| v.map(|t| localize(dir, &m, &t)).unwrap_or_default();
    let id = dir.file_name().map(|n| n.to_string_lossy().trim_start_matches(".tmp-").to_string()).unwrap_or_default();
    let popup = m.pointer("/action/default_popup").or_else(|| m.pointer("/browser_action/default_popup")).and_then(s).filter(|p| !p.is_empty());
    let options = m.pointer("/options_ui/page").or_else(|| m.get("options_page")).and_then(s);
    let icon = best_icon(&m).and_then(|p| fs::read(dir.join(p.trim_start_matches('/'))).ok().map(|bytes| (p, bytes))).map(|(p, bytes)| {
        use base64::Engine;
        let mime = if p.ends_with(".svg") { "image/svg+xml" } else if p.ends_with(".jpg") || p.ends_with(".jpeg") { "image/jpeg" } else { "image/png" };
        format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))
    });
    Ok(ExtInfo {
        id,
        name: msg(m.get("name").and_then(s)),
        version: m.get("version").and_then(s).unwrap_or_default(),
        description: msg(m.get("description").and_then(s)),
        popup,
        options,
        icon,
        source: fs::read_to_string(dir.join(".kite-source")).unwrap_or_else(|_| "local".into()),
        manifest_version: m.get("manifest_version").and_then(Value::as_u64).unwrap_or(2),
    })
}

/// `__MSG_name__` → the message from `_locales/<ko|default_locale|en>/messages.json`.
fn localize(dir: &Path, m: &Value, text: &str) -> String {
    let Some(key) = text.strip_prefix("__MSG_").and_then(|t| t.strip_suffix("__")) else { return text.to_string() };
    let default = m.get("default_locale").and_then(Value::as_str).unwrap_or("en");
    for loc in ["ko", default, "en"] {
        let Ok(raw) = fs::read_to_string(dir.join("_locales").join(loc).join("messages.json")) else { continue };
        let Ok(msgs) = serde_json::from_str::<Value>(raw.trim_start_matches('\u{feff}')) else { continue };
        if let Some(t) = msgs.as_object().and_then(|o| o.iter().find(|(k, _)| k.eq_ignore_ascii_case(key))).and_then(|(_, v)| v.get("message")).and_then(Value::as_str) {
            return t.to_string();
        }
    }
    text.to_string()
}

/// The icon closest to 48px (manifest `icons`, else the action's default icon).
fn best_icon(m: &Value) -> Option<String> {
    let icons = m.get("icons").or_else(|| m.pointer("/action/default_icon")).or_else(|| m.pointer("/browser_action/default_icon"))?;
    if let Some(p) = icons.as_str() {
        return Some(p.to_string());
    }
    icons
        .as_object()?
        .iter()
        .filter_map(|(k, v)| Some((k.parse::<i64>().ok()?, v.as_str()?)))
        .min_by_key(|(size, _)| (size - 48).abs())
        .map(|(_, p)| p.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_ids_from_urls() {
        let id = "cjpalhdlnbpafiamejdnhcphjbkeiagm";
        assert_eq!(parse_store_id(id).as_deref(), Some(id));
        assert_eq!(parse_store_id(&format!("https://chromewebstore.google.com/detail/ublock-origin/{id}")).as_deref(), Some(id));
        assert_eq!(parse_store_id(&format!("https://chrome.google.com/webstore/detail/{id}?hl=ko")).as_deref(), Some(id));
        assert_eq!(parse_store_id("https://example.com/abc"), None);
        assert_eq!(parse_store_id("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"), None);
    }

    #[test]
    fn local_ids_look_like_chrome_ids() {
        let id = local_id(Path::new("/tmp/my-ext"));
        assert_eq!(parse_store_id(&id).as_deref(), Some(id.as_str()));
        assert_eq!(id, local_id(Path::new("/tmp/my-ext")));
    }

    #[test]
    fn crx_headers() {
        let zip = b"PK\x03\x04rest";
        let mut crx3 = b"Cr24".to_vec();
        crx3.extend(3u32.to_le_bytes());
        crx3.extend(5u32.to_le_bytes());
        crx3.extend([0u8; 5]);
        crx3.extend(zip);
        assert_eq!(crx_zip(&crx3), Some(&zip[..]));
        let mut crx2 = b"Cr24".to_vec();
        crx2.extend(2u32.to_le_bytes());
        crx2.extend(2u32.to_le_bytes());
        crx2.extend(3u32.to_le_bytes());
        crx2.extend([0u8; 5]);
        crx2.extend(zip);
        assert_eq!(crx_zip(&crx2), Some(&zip[..]));
        assert_eq!(crx_zip(zip), Some(&zip[..]));
        assert_eq!(crx_zip(b"nope"), None);
    }

    /// `KITE_TEST_CRX=/path/to/x.crx cargo test --lib real_crx -- --ignored`
    #[test]
    #[ignore]
    fn real_crx() {
        let crx = fs::read(std::env::var("KITE_TEST_CRX").unwrap()).unwrap();
        let d = std::env::temp_dir().join(format!("kite-crx-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        unzip(crx_zip(&crx).unwrap(), &d).unwrap();
        let i = read_info(&d).unwrap();
        println!("{} {} mv{} popup={:?} options={:?} icon={}", i.name, i.version, i.manifest_version, i.popup, i.options, i.icon.is_some());
        let _ = fs::remove_dir_all(&d);
    }

    /// Network: `cargo test --lib store_download -- --ignored`
    #[test]
    #[ignore]
    fn store_download() {
        let crx = download_crx("eimadpbcbfnmbkopoojfekhnkhdbieeh").unwrap(); // Dark Reader
        let d = std::env::temp_dir().join(format!("kite-dl-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        unzip(crx_zip(&crx).unwrap(), &d).unwrap();
        let i = read_info(&d).unwrap();
        println!("{} {} mv{}", i.name, i.version, i.manifest_version);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn manifest_names_are_localized() {
        let d = std::env::temp_dir().join(format!("kite-ext-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("_locales/en")).unwrap();
        fs::write(d.join("manifest.json"), r#"{"name":"__MSG_appName__","version":"1.2","manifest_version":3,"default_locale":"en","action":{"default_popup":"popup.html"},"options_ui":{"page":"opt.html"}}"#).unwrap();
        fs::write(d.join("_locales/en/messages.json"), r#"{"appname":{"message":"My Ext"}}"#).unwrap();
        let i = read_info(&d).unwrap();
        assert_eq!((i.name.as_str(), i.version.as_str(), i.popup.as_deref(), i.options.as_deref()), ("My Ext", "1.2", Some("popup.html"), Some("opt.html")));
        let _ = fs::remove_dir_all(&d);
    }
}
