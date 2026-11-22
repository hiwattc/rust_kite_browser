//! Saved site logins: offer to save after a login form is submitted, fill them in next time.
//!
//! - Passwords are encrypted (AES-256-GCM) before they reach `kite.db`; the key is random and
//!   lives in the OS credential store (macOS Keychain / Windows Credential Manager), so the
//!   database file alone reveals nothing.
//! - The site is always the origin the webview shows (read back from the webview, never taken
//!   from the page). A page that claims another origin is ignored; filling happens only when the
//!   page in the tab still has the saved origin (checked again inside the page).
//! - Logins are saved only after the user says so in a native dialog (one dialog at a time).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use base64::Engine;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};
use serde::Deserialize;
use tauri::{AppHandle, Manager};

use crate::browser::Browser;
use crate::commands::toast;
use crate::error::AppError;

const KEYCHAIN_SERVICE: &str = "com.kite.browser";
const KEYCHAIN_ACCOUNT: &str = "password-vault-key";

static KEY: Mutex<Option<[u8; 32]>> = Mutex::new(None);
static ASKING: AtomicBool = AtomicBool::new(false);

pub fn enabled(b: &Browser) -> bool {
    b.0.store.get("passwords_enabled").as_deref() != Some("false")
}

/// The vault key from the OS credential store, made on first use.
fn key() -> Result<[u8; 32], AppError> {
    let mut cached = KEY.lock().unwrap();
    if let Some(k) = *cached {
        return Ok(k);
    }
    let fail = |e: keyring::Error| AppError::new("운영체제 키체인(자격 증명 저장소)에 접근하지 못했습니다.", e);
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT).map_err(fail)?;
    let k = match entry.get_password() {
        Ok(b64) => base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .ok()
            .and_then(|v| <[u8; 32]>::try_from(v).ok())
            .ok_or_else(|| AppError::new("저장된 비밀번호 키가 손상되었습니다.", "bad vault key"))?,
        Err(keyring::Error::NoEntry) => {
            let mut k = [0u8; 32];
            SystemRandom::new().fill(&mut k).map_err(|_| AppError::new("난수를 만들지 못했습니다.", "rng"))?;
            entry.set_password(&base64::engine::general_purpose::STANDARD.encode(k)).map_err(fail)?;
            k
        }
        Err(e) => return Err(fail(e)),
    };
    *cached = Some(k);
    Ok(k)
}

/// nonce ‖ ciphertext ‖ tag
fn seal_with(key: &[u8; 32], plain: &str) -> Result<Vec<u8>, AppError> {
    let bad = |_| AppError::new("비밀번호를 암호화하지 못했습니다.", "seal");
    let k = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).map_err(bad)?);
    let mut nonce = [0u8; NONCE_LEN];
    SystemRandom::new().fill(&mut nonce).map_err(bad)?;
    let mut buf = plain.as_bytes().to_vec();
    k.seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::empty(), &mut buf).map_err(bad)?;
    Ok([nonce.as_slice(), &buf].concat())
}

fn open_with(key: &[u8; 32], secret: &[u8]) -> Result<String, AppError> {
    let bad = |_| AppError::new("저장된 비밀번호를 풀지 못했습니다.", "open");
    if secret.len() < NONCE_LEN {
        return Err(bad(ring::error::Unspecified));
    }
    let k = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).map_err(bad)?);
    let (nonce, body) = secret.split_at(NONCE_LEN);
    let mut buf = body.to_vec();
    let plain = k.open_in_place(Nonce::try_assume_unique_for_key(nonce).map_err(bad)?, Aad::empty(), &mut buf).map_err(bad)?;
    String::from_utf8(plain.to_vec()).map_err(|_| bad(ring::error::Unspecified))
}

pub fn seal(plain: &str) -> Result<Vec<u8>, AppError> {
    seal_with(&key()?, plain)
}

pub fn open(secret: &[u8]) -> Result<String, AppError> {
    open_with(&key()?, secret)
}

/// The origin the tab's webview shows (http/https only).
fn tab_origin(b: &Browser, tab: u64) -> Option<String> {
    let u = b.webview(tab)?.url().ok()?;
    matches!(u.scheme(), "http" | "https").then(|| u.origin().ascii_serialization())
}

/// What the page reports: `form:<origin>` (a password field appeared) or a submitted login.
pub async fn on_event(app: &AppHandle, tab: u64, action: &str) {
    let b = app.state::<Browser>().inner().clone();
    if !enabled(&b) {
        return;
    }
    let result = match action.strip_prefix("form:") {
        Some(claimed) => fill(&b, tab, claimed).await,
        None => submitted(app, &b, tab, action),
    };
    if let Err(e) = result {
        tracing::warn!(error = %e.detail, "password manager");
    }
}

/// Fills the most recently used login for this origin into the page.
async fn fill(b: &Browser, tab: u64, claimed: &str) -> Result<(), AppError> {
    let Some(origin) = tab_origin(b, tab) else { return Ok(()) };
    if origin != claimed {
        return Ok(());
    }
    let Some((id, user, secret)) = b.0.store.pw_for_origin(&origin).into_iter().next() else { return Ok(()) };
    let pass = open(&secret)?;
    let js = |s: &str| serde_json::to_string(s).unwrap_or_default();
    let script = format!("return window.__kite.fillLogin({}, {}, {});", js(&origin), js(&user), js(&pass));
    if b.run_sync(tab, &script).await.map_err(|e| AppError::new("", e))? == serde_json::Value::Bool(true) {
        b.0.store.pw_touch(id);
    }
    Ok(())
}

#[derive(Deserialize)]
struct Submitted {
    o: String,
    #[serde(default)]
    u: String,
    p: String,
}

/// A login form was sent: offer to save (or update) it.
fn submitted(app: &AppHandle, b: &Browser, tab: u64, action: &str) -> Result<(), AppError> {
    let Ok(s) = serde_json::from_str::<Submitted>(action) else { return Ok(()) };
    let (user, pass) = (s.u.trim().to_string(), s.p);
    if pass.is_empty() || pass.len() > 1024 || user.len() > 256 {
        return Ok(());
    }
    let Some(origin) = tab_origin(b, tab) else { return Ok(()) };
    if origin != s.o || b.0.store.pw_never(&origin) {
        tracing::debug!(origin, claimed = %s.o, "login not offered for saving");
        return Ok(());
    }
    let existing = b.0.store.pw_for_origin(&origin).into_iter().find(|(_, u, _)| *u == user);
    if let Some((id, _, _)) = existing.as_ref().filter(|(_, _, secret)| open(secret).ok().as_deref() == Some(pass.as_str())) {
        b.0.store.pw_touch(*id);
        return Ok(());
    }
    tracing::debug!(origin, update = existing.is_some(), "offer to save login");
    ask(app, origin, user, pass, existing.is_some());
    Ok(())
}

fn ask(app: &AppHandle, origin: String, user: String, pass: String, update: bool) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogResult};
    if ASKING.swap(true, Ordering::SeqCst) {
        return;
    }
    let host = url::Url::parse(&origin).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_else(|| origin.clone());
    let shown_user = if user.is_empty() { "(아이디 없음)".to_string() } else { user.clone() };
    let (title, buttons) = if update {
        ("비밀번호 업데이트", MessageDialogButtons::OkCancelCustom("업데이트".into(), "취소".into()))
    } else {
        ("비밀번호 저장", MessageDialogButtons::YesNoCancelCustom("저장".into(), "이 사이트는 안 함".into(), "나중에".into()))
    };
    let message = if update {
        format!("{host}에 저장된 비밀번호를 방금 입력한 것으로 바꿀까요?\n\n아이디: {shown_user}")
    } else {
        format!("{host}의 비밀번호를 Kite에 저장할까요?\n\n아이디: {shown_user}\n다음에 이 사이트에 들어오면 자동으로 입력합니다.")
    };
    let a = app.clone();
    let mut dialog = app.dialog().message(message).title(title).buttons(buttons);
    // A sheet on Kite's window (without a parent, macOS shows a detached system alert).
    if let Some(w) = a.state::<Browser>().current_window() {
        dialog = dialog.parent(&w);
    }
    dialog.show_with_result(move |r| {
        ASKING.store(false, Ordering::SeqCst);
        let b = a.state::<Browser>().inner().clone();
        let label = match r {
            MessageDialogResult::Custom(l) => l,
            MessageDialogResult::Yes | MessageDialogResult::Ok => if update { "업데이트" } else { "저장" }.to_string(),
            _ => String::new(),
        };
        let done = match label.as_str() {
            "저장" | "업데이트" => seal(&pass).and_then(|sec| b.0.store.pw_save(&origin, &user, &sec)).map(|_| "비밀번호를 저장했습니다."),
            "이 사이트는 안 함" => b.0.store.pw_set_never(&origin, true).map(|_| "이 사이트의 비밀번호는 저장하지 않습니다."),
            _ => return,
        };
        match done {
            Ok(msg) => toast(&b, msg),
            Err(e) => toast(&b, e.user_message),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_and_open() {
        let k = [7u8; 32];
        let s = seal_with(&k, "p@ss 비밀").unwrap();
        assert_ne!(&s[NONCE_LEN..], "p@ss 비밀".as_bytes());
        assert_eq!(open_with(&k, &s).unwrap(), "p@ss 비밀");
        // Fresh nonce every time.
        assert_ne!(seal_with(&k, "x").unwrap(), seal_with(&k, "x").unwrap());
        // Wrong key or tampered data fail.
        assert!(open_with(&[8u8; 32], &s).is_err());
        let mut t = s.clone();
        *t.last_mut().unwrap() ^= 1;
        assert!(open_with(&k, &t).is_err());
        assert!(open_with(&k, b"short").is_err());
    }
}
