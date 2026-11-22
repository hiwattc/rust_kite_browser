//! Downloads: where files go, progress for the toolbar ring, and the history behind
//! `kite://downloads`.
//!
//! Tauri reports only "requested" (pick the destination) and "finished". Progress comes from:
//! - macOS: `platform::mac_download` wraps wry's `download:decideDestinationUsingResponse:…` and
//!   hands over the `WKDownload` (its `progress` gives bytes so far and the total, and it can be
//!   cancelled) just before Tauri asks for the destination;
//! - elsewhere: the file's size on disk (total unknown).
//!
//! Finished downloads are kept in the store (`downloads`, newest first, at most `KEEP`).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State, Webview};

use crate::browser::{Browser, tab_id_of, ui_label};
use crate::error::AppError;

const KEY: &str = "downloads";
const DIR_KEY: &str = "download_dir";
const KEEP: usize = 300;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum DlState {
    Progress,
    Done,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Download {
    pub id: u64,
    pub url: String,
    pub name: String,
    pub path: String,
    /// Bytes expected (`-1` unknown) and received so far.
    pub total: i64,
    pub received: i64,
    pub state: DlState,
    /// Unix ms.
    pub started: i64,
    pub ended: Option<i64>,
    /// The file is no longer where it was saved.
    #[serde(default)]
    pub missing: bool,
    /// Can be cancelled (macOS keeps the engine's download object).
    #[serde(skip_deserializing, default)]
    pub cancelable: bool,
}

#[derive(Default)]
struct Inner {
    active: Vec<(Download, Option<crate::platform::DownloadHandle>)>,
    history: Vec<Download>,
    loaded: bool,
    next: u64,
    canceling: Vec<u64>,
    polling: bool,
}

#[derive(Clone, Default)]
pub struct Downloads(Arc<Mutex<Inner>>);

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Everything to show: active first, then history (newest first).
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub items: Vec<Download>,
    /// For the toolbar ring: 0..1, or -1 when some active download has no known size.
    pub progress: f64,
    pub active: usize,
    pub dir: String,
}

impl Downloads {
    fn load(&self, app: &AppHandle) {
        let mut s = self.0.lock().unwrap();
        if s.loaded {
            return;
        }
        s.loaded = true;
        let b = app.state::<Browser>();
        s.history = b.0.store.get(KEY).and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default();
        s.next = s.history.iter().map(|d| d.id).max().unwrap_or(0) + 1;
    }

    fn save(&self, app: &AppHandle) {
        let json = {
            let s = self.0.lock().unwrap();
            serde_json::to_string(&s.history).unwrap_or_else(|_| "[]".into())
        };
        let _ = app.state::<Browser>().0.store.set(KEY, &json);
    }

    pub fn snapshot(&self, app: &AppHandle) -> Snapshot {
        self.load(app);
        let s = self.0.lock().unwrap();
        let mut items: Vec<Download> = s.active.iter().map(|(d, _)| d.clone()).collect();
        items.extend(s.history.iter().cloned());
        let unknown = s.active.iter().any(|(d, _)| d.total <= 0);
        let (got, want) = s.active.iter().fold((0i64, 0i64), |(g, w), (d, _)| (g + d.received.max(0), w + d.total.max(0)));
        let progress = if s.active.is_empty() {
            1.0
        } else if unknown || want == 0 {
            -1.0
        } else {
            (got as f64 / want as f64).clamp(0.0, 1.0)
        };
        Snapshot { items, progress, active: s.active.len(), dir: dir(app).display().to_string() }
    }

    /// Toolbars (every window) and open `kite://downloads` pages.
    pub fn emit(&self, app: &AppHandle) {
        let snap = self.snapshot(app);
        let b = app.state::<Browser>();
        for n in b.wins() {
            let _ = app.emit_to(ui_label(n), "downloads", snap.clone());
        }
        for t in b.tabs().into_iter().filter(|t| t.url == "kite://downloads") {
            let _ = app.emit_to(format!("tab-{}", t.id), "downloads", snap.clone());
        }
    }

    /// Tauri asks where to save `url`: in the download folder, never over an existing file.
    pub fn requested(&self, app: &AppHandle, url: &str, name: &str) -> PathBuf {
        self.load(app);
        let path = unique_path(&dir(app), name);
        let handle = crate::platform::take_download_handle();
        let total = handle.as_ref().map(|h| h.expected).unwrap_or(-1);
        {
            let mut s = self.0.lock().unwrap();
            let id = s.next;
            s.next += 1;
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| name.to_string());
            let d = Download {
                id,
                url: url.to_string(),
                name,
                path: path.display().to_string(),
                total,
                received: 0,
                state: DlState::Progress,
                started: now_ms(),
                ended: None,
                missing: false,
                cancelable: handle.is_some(),
            };
            s.active.push((d, handle));
        }
        self.emit(app);
        self.poll(app);
        path
    }

    /// The engine says the download of `url` ended.
    pub fn finished(&self, app: &AppHandle, url: &str, success: bool) -> Option<Download> {
        let i = self.0.lock().unwrap().active.iter().position(|(d, _)| d.url == url)?;
        self.end(app, i, success)
    }

    fn finish_id(&self, app: &AppHandle, id: u64) {
        let i = self.0.lock().unwrap().active.iter().position(|(d, _)| d.id == id);
        if let Some(i) = i {
            self.end(app, i, false);
        }
    }

    fn end(&self, app: &AppHandle, i: usize, success: bool) -> Option<Download> {
        let done = {
            let mut s = self.0.lock().unwrap();
            if i >= s.active.len() {
                return None;
            }
            // (On the main thread: the engine's download object is released here.)
            let (mut d, _handle) = s.active.remove(i);
            let canceled = s.canceling.contains(&d.id);
            s.canceling.retain(|x| *x != d.id);
            d.state = if success {
                DlState::Done
            } else if canceled {
                DlState::Canceled
            } else {
                DlState::Failed
            };
            d.ended = Some(now_ms());
            d.cancelable = false;
            if let Ok(m) = std::fs::metadata(&d.path) {
                d.received = m.len() as i64;
                if success {
                    d.total = d.received;
                }
            }
            if !success {
                // A partial file is of no use.
                let _ = std::fs::remove_file(&d.path);
            }
            s.history.insert(0, d.clone());
            s.history.truncate(KEEP);
            d
        };
        self.save(app);
        self.emit(app);
        Some(done)
    }

    /// While anything downloads: read progress a few times a second and tell the toolbar.
    fn poll(&self, app: &AppHandle) {
        {
            let mut s = self.0.lock().unwrap();
            if s.polling {
                return;
            }
            s.polling = true;
        }
        let me = self.clone();
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(400)).await;
                if me.0.lock().unwrap().active.is_empty() {
                    me.0.lock().unwrap().polling = false;
                    break;
                }
                // The engine's download objects are read on the main thread.
                let (tx, rx) = tokio::sync::oneshot::channel::<Vec<(u64, (i64, i64))>>();
                let me2 = me.clone();
                let _ = app.run_on_main_thread(move || {
                    let s = me2.0.lock().unwrap();
                    let r = s.active.iter().filter_map(|(d, h)| h.as_ref().map(|h| (d.id, crate::platform::download_progress(h)))).collect();
                    let _ = tx.send(r);
                });
                let native = rx.await.unwrap_or_default();
                {
                    let mut s = me.0.lock().unwrap();
                    for (d, _) in s.active.iter_mut() {
                        let (got, total) = native.iter().find(|(id, _)| *id == d.id).map(|(_, p)| *p).unwrap_or_else(|| (on_disk(&d.path), -1));
                        d.received = got.max(d.received);
                        if total > 0 {
                            d.total = total;
                        }
                    }
                }
                me.emit(&app);
            }
        });
    }

    fn cancel(&self, app: &AppHandle, id: u64) -> Result<(), AppError> {
        {
            let mut s = self.0.lock().unwrap();
            if !s.active.iter().any(|(d, h)| d.id == id && h.is_some()) {
                return Err(AppError::new("이 다운로드는 취소할 수 없습니다.", id));
            }
            s.canceling.push(id);
        }
        let me = self.clone();
        let app2 = app.clone();
        app.run_on_main_thread(move || {
            {
                let s = me.0.lock().unwrap();
                let Some((_, Some(h))) = s.active.iter().find(|(d, _)| d.id == id) else {
                    return;
                };
                crate::platform::download_cancel(h);
            }
            // WebKit does not report a download it was told to cancel: end it here.
            me.finish_id(&app2, id);
        })
        .map_err(|e| AppError::new("취소하지 못했습니다.", e))
    }

    fn find(&self, id: u64) -> Option<Download> {
        let s = self.0.lock().unwrap();
        s.active.iter().map(|(d, _)| d).chain(s.history.iter()).find(|d| d.id == id).cloned()
    }
}

/// Bytes in the file so far (WebView2 may write next to it as `.crdownload` first).
fn on_disk(path: &str) -> i64 {
    [path.to_string(), format!("{path}.crdownload")].iter().filter_map(|p| std::fs::metadata(p).ok()).map(|m| m.len() as i64).max().unwrap_or(0)
}

/// Where downloads go: the folder chosen in settings, else the OS download folder.
pub fn dir(app: &AppHandle) -> PathBuf {
    app.state::<Browser>().0.store.get(DIR_KEY).map(PathBuf::from).filter(|p| p.is_dir()).or_else(dirs::download_dir).or_else(dirs::home_dir).unwrap_or_else(std::env::temp_dir)
}

pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let name: String = name.chars().map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c }).collect();
    let name = if name.trim().is_empty() { "download".to_string() } else { name };
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

// ---------------------------------------------------------------- commands

#[tauri::command]
pub async fn downloads_list(app: AppHandle, d: State<'_, Downloads>) -> Result<Snapshot, AppError> {
    let mut snap = d.snapshot(&app);
    for x in snap.items.iter_mut().filter(|x| x.state == DlState::Done) {
        x.missing = !Path::new(&x.path).exists();
    }
    Ok(snap)
}

/// `what`: `open` (the file), `show` (in its folder), `cancel`, `remove` (from the list),
/// `retry` (download again in a new tab).
#[tauri::command]
pub async fn download_action(app: AppHandle, d: State<'_, Downloads>, id: u64, what: String) -> Result<(), AppError> {
    use tauri_plugin_opener::OpenerExt;
    let item = d.find(id).ok_or_else(|| AppError::new("다운로드 기록을 찾지 못했습니다.", id))?;
    match what.as_str() {
        "open" => app.opener().open_path(&item.path, None::<&str>).map_err(|e| AppError::new("파일을 열지 못했습니다. 옮겨졌거나 지워졌을 수 있습니다.", e))?,
        "show" => app.opener().reveal_item_in_dir(&item.path).map_err(|e| AppError::new("폴더를 열지 못했습니다. 파일이 옮겨졌거나 지워졌을 수 있습니다.", e))?,
        "cancel" => d.cancel(&app, id)?,
        "remove" => {
            {
                let mut s = d.0.lock().unwrap();
                s.history.retain(|x| x.id != id);
            }
            d.save(&app);
            d.emit(&app);
        }
        "retry" => {
            let b = app.state::<Browser>().inner().clone();
            b.new_tab(Some(item.url.clone()), false)?;
        }
        _ => return Err(AppError::new("알 수 없는 동작입니다.", what)),
    }
    Ok(())
}

/// Clears the finished part of the list (files stay).
#[tauri::command]
pub async fn downloads_clear(app: AppHandle, d: State<'_, Downloads>) -> Result<(), AppError> {
    d.0.lock().unwrap().history.clear();
    d.save(&app);
    d.emit(&app);
    Ok(())
}

/// Opens the download folder.
#[tauri::command]
pub async fn downloads_open_dir(app: AppHandle) -> Result<(), AppError> {
    use tauri_plugin_opener::OpenerExt;
    let p = dir(&app);
    app.opener().open_path(p.display().to_string(), None::<&str>).map_err(|e| AppError::new("다운로드 폴더를 열지 못했습니다.", e))
}

/// Settings: pick another download folder (`reset`: back to the OS one). Returns the folder.
#[tauri::command]
pub async fn downloads_pick_dir(app: AppHandle, reset: bool) -> Result<String, AppError> {
    use tauri_plugin_dialog::DialogExt;
    let b = app.state::<Browser>().inner().clone();
    if reset {
        b.0.store.set(DIR_KEY, "")?;
    } else {
        let a = app.clone();
        let picked = tauri::async_runtime::spawn_blocking(move || a.dialog().file().set_title("다운로드 폴더 선택").set_directory(dir(&a)).blocking_pick_folder())
            .await
            .map_err(|e| AppError::new("폴더를 고르지 못했습니다.", e))?;
        if let Some(p) = picked.and_then(|p| p.into_path().ok()) {
            b.0.store.set(DIR_KEY, &p.display().to_string())?;
        }
    }
    let d = app.state::<Downloads>();
    d.emit(&app);
    Ok(dir(&app).display().to_string())
}

/// The toolbar button: the downloads page in this window.
#[tauri::command]
pub async fn downloads_page(app: AppHandle, webview: Webview) -> Result<(), AppError> {
    let b = app.state::<Browser>().inner().clone();
    b.touch(&webview);
    let _ = tab_id_of(webview.label());
    b.open_internal("downloads")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_download_names() {
        let dir = std::env::temp_dir().join(format!("kite-dl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = unique_path(&dir, "a.txt");
        assert_eq!(a.file_name().unwrap(), "a.txt");
        std::fs::write(&a, "x").unwrap();
        assert_eq!(unique_path(&dir, "a.txt").file_name().unwrap(), "a (1).txt");
        assert_eq!(unique_path(&dir, "b/c?.txt").file_name().unwrap(), "b_c_.txt");
        assert_eq!(unique_path(&dir, " ").file_name().unwrap(), "download");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn history_roundtrip() {
        let d = Download {
            id: 3,
            url: "https://a.example/f.zip".into(),
            name: "f.zip".into(),
            path: "/tmp/f.zip".into(),
            total: 10,
            received: 10,
            state: DlState::Done,
            started: 1,
            ended: Some(2),
            missing: false,
            cancelable: true,
        };
        let j = serde_json::to_string(&vec![d]).unwrap();
        assert!(j.contains("\"state\":\"done\""));
        let back: Vec<Download> = serde_json::from_str(&j).unwrap();
        assert_eq!(back[0].state, DlState::Done);
        assert!(!back[0].cancelable);
    }
}
