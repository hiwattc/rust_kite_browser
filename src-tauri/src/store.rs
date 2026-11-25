//! Local data in one SQLite file (`kite.db` in the app data folder): bookmarks, history and
//! small settings (key/value).

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::error::AppError;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Bookmark {
    pub id: i64,
    pub url: String,
    pub title: String,
    /// Folder path, `/`-joined (`북마크바/개발`), empty at the top.
    pub folder: String,
    pub created: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Visit {
    pub url: String,
    pub title: String,
    pub visit_count: i64,
    pub last_visit: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedLogin {
    pub id: i64,
    pub origin: String,
    pub username: String,
    pub last_used: i64,
}

pub struct Store {
    db: Mutex<Connection>,
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn db_err(e: impl std::fmt::Display) -> AppError {
    AppError::new("저장소 작업에 실패했습니다.", e)
}

/// `LIKE` pattern for one search word (wildcards in the word are literal).
fn like(word: &str) -> String {
    let mut s = String::from("%");
    for c in word.chars() {
        if matches!(c, '%' | '_' | '\\') {
            s.push('\\');
        }
        s.push(c);
    }
    s.push('%');
    s
}

/// `/a//b/ ` → `a/b`.
pub fn normalize_folder(f: &str) -> String {
    f.split('/').map(str::trim).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("/")
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, AppError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(db_err)?;
        }
        Self::with(Connection::open(path).map_err(db_err)?)
    }

    /// Saved site icon (data URL) per origin, taken while the site was open in a tab. Kite never
    /// fetches icons for bookmarks or history on its own: that would contact every site in them.
    pub fn favicon_save(&self, origin: &str, data: &str) -> Result<(), AppError> {
        self.db
            .lock()
            .unwrap()
            .execute("INSERT INTO favicons(origin, data, saved) VALUES(?1, ?2, ?3) ON CONFLICT(origin) DO UPDATE SET data=?2, saved=?3", rusqlite::params![origin, data, now_ms()])
            .map(|_| ())
            .map_err(db_err)
    }

    /// When the icon of `origin` was saved (unix ms), if it was.
    pub fn favicon_saved_at(&self, origin: &str) -> Option<i64> {
        self.db.lock().unwrap().query_row("SELECT saved FROM favicons WHERE origin=?1", [origin], |r| r.get(0)).ok()
    }

    pub fn favicons(&self, origins: &[String]) -> std::collections::HashMap<String, String> {
        let db = self.db.lock().unwrap();
        let Ok(mut st) = db.prepare_cached("SELECT data FROM favicons WHERE origin=?1") else { return Default::default() };
        origins.iter().filter_map(|o| st.query_row([o], |r| r.get::<_, String>(0)).ok().map(|d| (o.clone(), d))).collect()
    }

    pub fn favicons_clear(&self) -> Result<(), AppError> {
        self.db.lock().unwrap().execute("DELETE FROM favicons", []).map(|_| ()).map_err(db_err)
    }

    #[cfg(test)]
    pub fn memory() -> Result<Self, AppError> {
        Self::with(Connection::open_in_memory().map_err(db_err)?)
    }

    fn with(c: Connection) -> Result<Self, AppError> {
        c.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS bookmarks(
               id INTEGER PRIMARY KEY, url TEXT NOT NULL UNIQUE, title TEXT NOT NULL DEFAULT '',
               folder TEXT NOT NULL DEFAULT '', created INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS history(
               url TEXT PRIMARY KEY, title TEXT NOT NULL DEFAULT '',
               visit_count INTEGER NOT NULL DEFAULT 0, last_visit INTEGER NOT NULL);
             CREATE INDEX IF NOT EXISTS history_last ON history(last_visit);
             CREATE TABLE IF NOT EXISTS kv(key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS passwords(
               id INTEGER PRIMARY KEY, origin TEXT NOT NULL, username TEXT NOT NULL, secret BLOB NOT NULL,
               created INTEGER NOT NULL, last_used INTEGER NOT NULL, UNIQUE(origin, username));
             CREATE TABLE IF NOT EXISTS password_never(origin TEXT PRIMARY KEY);
             CREATE TABLE IF NOT EXISTS bookmark_folders(path TEXT PRIMARY KEY);
             CREATE TABLE IF NOT EXISTS favicons(origin TEXT PRIMARY KEY, data TEXT NOT NULL, saved INTEGER NOT NULL);",
        )
        .map_err(db_err)?;
        let s = Self { db: Mutex::new(c) };
        s.migrate()?;
        Ok(s)
    }

    /// One-time fixes of older data.
    fn migrate(&self) -> Result<(), AppError> {
        // 0.1.0 imported Chrome's bookmark bar as a folder ("북마크바/…"); it is the bar itself.
        if self.get("migrated_bar_root").is_none() {
            let db = self.db.lock().unwrap();
            for root in ["북마크바", "북마크 바", "Bookmarks bar", "Bookmarks Bar", "즐겨찾기 모음", "Favorites bar"] {
                db.execute(
                    "UPDATE OR IGNORE bookmarks SET folder = CASE WHEN folder = ?1 THEN '' ELSE substr(folder, length(?1) + 2) END
                     WHERE folder = ?1 OR folder LIKE ?1 || '/%'",
                    [root],
                )
                .map_err(db_err)?;
            }
            drop(db);
            self.set("migrated_bar_root", "1")?;
        }
        // Folders are kept on their own (so an empty one stays); every folder a bookmark uses
        // is one (also for data from before the table).
        let used: Vec<String> = {
            let db = self.db.lock().unwrap();
            let mut st = db.prepare("SELECT DISTINCT folder FROM bookmarks WHERE folder <> ''").map_err(db_err)?;
            st.query_map([], |r| r.get(0)).map_err(db_err)?.filter_map(Result::ok).collect()
        };
        for f in used {
            self.folder_keep(&f)?;
        }
        Ok(())
    }

    /// Records `folder` and the folders above it.
    fn folder_keep(&self, folder: &str) -> Result<(), AppError> {
        let f = normalize_folder(folder);
        if f.is_empty() {
            return Ok(());
        }
        let db = self.db.lock().unwrap();
        let parts: Vec<&str> = f.split('/').collect();
        for i in 1..=parts.len() {
            db.execute("INSERT OR IGNORE INTO bookmark_folders(path) VALUES(?1)", [parts[..i].join("/")]).map_err(db_err)?;
        }
        Ok(())
    }

    // ---- passwords (the secret is encrypted by `passwords.rs`; never plaintext here) ----

    pub fn pw_list(&self) -> Vec<SavedLogin> {
        let db = self.db.lock().unwrap();
        let Ok(mut st) = db.prepare("SELECT id, origin, username, last_used FROM passwords ORDER BY origin, username") else { return vec![] };
        st.query_map([], |r| Ok(SavedLogin { id: r.get(0)?, origin: r.get(1)?, username: r.get(2)?, last_used: r.get(3)? }))
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    /// Logins for one origin, most recently used first: (id, username, secret).
    pub fn pw_for_origin(&self, origin: &str) -> Vec<(i64, String, Vec<u8>)> {
        let db = self.db.lock().unwrap();
        let Ok(mut st) = db.prepare("SELECT id, username, secret FROM passwords WHERE origin=?1 ORDER BY last_used DESC") else { return vec![] };
        st.query_map([origin], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map(|rows| rows.flatten().collect()).unwrap_or_default()
    }

    pub fn pw_get(&self, id: i64) -> Option<(String, String, Vec<u8>)> {
        self.db
            .lock()
            .unwrap()
            .query_row("SELECT origin, username, secret FROM passwords WHERE id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()
            .ok()
            .flatten()
    }

    pub fn pw_save(&self, origin: &str, username: &str, secret: &[u8]) -> Result<(), AppError> {
        let t = now_ms();
        self.db
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO passwords(origin, username, secret, created, last_used) VALUES(?1, ?2, ?3, ?4, ?4)
                 ON CONFLICT(origin, username) DO UPDATE SET secret=excluded.secret, last_used=excluded.last_used",
                params![origin, username, secret, t],
            )
            .map_err(db_err)?;
        Ok(())
    }

    pub fn pw_touch(&self, id: i64) {
        let _ = self.db.lock().unwrap().execute("UPDATE passwords SET last_used=?2 WHERE id=?1", params![id, now_ms()]);
    }

    pub fn pw_delete(&self, id: i64) -> Result<(), AppError> {
        self.db.lock().unwrap().execute("DELETE FROM passwords WHERE id=?1", [id]).map_err(db_err)?;
        Ok(())
    }

    pub fn pw_never(&self, origin: &str) -> bool {
        self.db.lock().unwrap().query_row("SELECT 1 FROM password_never WHERE origin=?1", [origin], |_| Ok(())).optional().ok().flatten().is_some()
    }

    pub fn pw_set_never(&self, origin: &str, never: bool) -> Result<(), AppError> {
        let db = self.db.lock().unwrap();
        if never {
            db.execute("INSERT OR IGNORE INTO password_never(origin) VALUES(?1)", [origin]).map_err(db_err)?;
        } else {
            db.execute("DELETE FROM password_never WHERE origin=?1", [origin]).map_err(db_err)?;
        }
        Ok(())
    }

    pub fn pw_never_list(&self) -> Vec<String> {
        let db = self.db.lock().unwrap();
        let Ok(mut st) = db.prepare("SELECT origin FROM password_never ORDER BY origin") else { return vec![] };
        st.query_map([], |r| r.get(0)).map(|rows| rows.flatten().collect()).unwrap_or_default()
    }

    // ---- settings ----

    pub fn get(&self, key: &str) -> Option<String> {
        self.db
            .lock()
            .unwrap()
            .query_row("SELECT value FROM kv WHERE key=?1", [key], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    pub fn set(&self, key: &str, value: &str) -> Result<(), AppError> {
        self.db
            .lock()
            .unwrap()
            .execute("INSERT INTO kv(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [key, value])
            .map_err(db_err)?;
        Ok(())
    }

    // ---- bookmarks ----

    pub fn is_bookmarked(&self, url: &str) -> bool {
        self.db
            .lock()
            .unwrap()
            .query_row("SELECT 1 FROM bookmarks WHERE url=?1", [url], |_| Ok(()))
            .optional()
            .ok()
            .flatten()
            .is_some()
    }

    pub fn bookmark_add(&self, url: &str, title: &str, folder: &str) -> Result<bool, AppError> {
        let folder = normalize_folder(folder);
        let n = self
            .db
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO bookmarks(url,title,folder,created) VALUES(?1,?2,?3,?4) ON CONFLICT(url) DO NOTHING",
                params![url, title, folder, now_ms()],
            )
            .map_err(db_err)?;
        if n > 0 {
            self.folder_keep(&folder)?;
        }
        Ok(n > 0)
    }

    pub fn bookmark_remove(&self, url: &str) -> Result<bool, AppError> {
        Ok(self.db.lock().unwrap().execute("DELETE FROM bookmarks WHERE url=?1", [url]).map_err(db_err)? > 0)
    }

    /// Every bookmark in the order they were added (bar order).
    pub fn bookmarks_all(&self) -> Vec<Bookmark> {
        let db = self.db.lock().unwrap();
        let Ok(mut st) = db.prepare("SELECT id,url,title,folder,created FROM bookmarks ORDER BY created, id") else { return Vec::new() };
        st.query_map([], |r| Ok(Bookmark { id: r.get(0)?, url: r.get(1)?, title: r.get(2)?, folder: r.get(3)?, created: r.get(4)? }))
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }

    /// Edits a bookmark (the URL may change, but not to one that is already bookmarked).
    pub fn bookmark_update(&self, id: i64, url: &str, title: &str, folder: &str) -> Result<(), AppError> {
        let url = url.trim();
        if url::Url::parse(url).is_err() {
            return Err(AppError::new("주소가 올바르지 않습니다.", url));
        }
        let n = self
            .db
            .lock()
            .unwrap()
            .execute("UPDATE bookmarks SET url=?2, title=?3, folder=?4 WHERE id=?1", params![id, url, title.trim(), normalize_folder(folder)])
            .map_err(|e| {
                if e.to_string().contains("UNIQUE") { AppError::new("같은 주소의 북마크가 이미 있습니다.", e) } else { db_err(e) }
            })?;
        if n == 0 {
            return Err(AppError::new("북마크를 찾지 못했습니다.", id));
        }
        self.folder_keep(folder)
    }

    pub fn bookmark_delete(&self, id: i64) -> Result<(), AppError> {
        self.db.lock().unwrap().execute("DELETE FROM bookmarks WHERE id=?1", [id]).map_err(db_err)?;
        Ok(())
    }

    /// Folder order the user set by dragging: full paths, siblings sort by their position here
    /// (folders not listed come after, alphabetically).
    pub fn folder_order(&self) -> Vec<String> {
        self.get("folder_order").and_then(|v| serde_json::from_str(&v).ok()).unwrap_or_default()
    }

    pub fn set_folder_order(&self, order: &[String]) -> Result<(), AppError> {
        let mut seen = std::collections::HashSet::new();
        let list: Vec<String> = order.iter().map(|f| normalize_folder(f)).filter(|f| !f.is_empty() && seen.insert(f.clone())).collect();
        self.set("folder_order", &serde_json::to_string(&list).unwrap_or_else(|_| "[]".into()))
    }

    /// Every folder (kept ones too, empty or not), as paths.
    pub fn folders_all(&self) -> Vec<String> {
        let db = self.db.lock().unwrap();
        let Ok(mut st) = db.prepare("SELECT path FROM bookmark_folders ORDER BY path") else { return Vec::new() };
        st.query_map([], |r| r.get(0)).map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
    }

    /// Makes an (empty) folder; returns its path.
    pub fn folder_create(&self, path: &str) -> Result<String, AppError> {
        let p = normalize_folder(path);
        if p.is_empty() {
            return Err(AppError::new("폴더 이름을 입력하세요.", "empty folder"));
        }
        self.folder_keep(&p)?;
        Ok(p)
    }

    /// Deletes a folder, its subfolders and every bookmark in them; returns how many bookmarks.
    pub fn folder_delete(&self, path: &str) -> Result<usize, AppError> {
        let p = normalize_folder(path);
        if p.is_empty() {
            return Err(AppError::new("지울 폴더가 없습니다.", "empty folder"));
        }
        let n = {
            let db = self.db.lock().unwrap();
            let under = "?1 OR substr(X, 1, length(?1) + 1) = ?1 || '/'";
            let n = db.execute(&format!("DELETE FROM bookmarks WHERE folder = {}", under.replace('X', "folder")), [&p]).map_err(db_err)?;
            db.execute(&format!("DELETE FROM bookmark_folders WHERE path = {}", under.replace('X', "path")), [&p]).map_err(db_err)?;
            n
        };
        let order: Vec<String> = self.folder_order().into_iter().filter(|f| f != &p && !f.starts_with(&format!("{p}/"))).collect();
        self.set_folder_order(&order)?;
        Ok(n)
    }

    /// Moves folder `from` with everything in it under `parent` (`""` = top level); returns its
    /// new path. Into a folder of the same name: the two merge.
    pub fn folder_move(&self, from: &str, parent: &str) -> Result<String, AppError> {
        let from = normalize_folder(from);
        let parent = normalize_folder(parent);
        if from.is_empty() {
            return Err(AppError::new("옮길 폴더가 없습니다.", "empty folder"));
        }
        if parent == from || parent.starts_with(&format!("{from}/")) {
            return Err(AppError::new("폴더를 자기 안으로 옮길 수 없습니다.", format!("{from} -> {parent}")));
        }
        let name = from.rsplit('/').next().unwrap_or(&from);
        let to = if parent.is_empty() { name.to_string() } else { format!("{parent}/{name}") };
        self.folder_relocate(&from, &to)?;
        Ok(to)
    }

    /// Renames a folder (same place); returns its new path. To an existing name: they merge.
    pub fn folder_rename(&self, path: &str, name: &str) -> Result<String, AppError> {
        let from = normalize_folder(path);
        let name = name.trim();
        if from.is_empty() || name.is_empty() {
            return Err(AppError::new("폴더 이름을 입력하세요.", "empty name"));
        }
        if name.contains('/') {
            return Err(AppError::new("폴더 이름에는 / 를 쓸 수 없습니다.", name));
        }
        let parent = from.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
        let to = if parent.is_empty() { name.to_string() } else { format!("{parent}/{name}") };
        self.folder_relocate(&from, &to)?;
        Ok(to)
    }

    /// `from` (and below) becomes `to` in bookmarks, kept folders and the saved order.
    fn folder_relocate(&self, from: &str, to: &str) -> Result<(), AppError> {
        if to == from {
            return Ok(());
        }
        {
            // substr/length count characters in SQLite, so Korean names are fine.
            let db = self.db.lock().unwrap();
            db.execute(
                "UPDATE bookmarks SET folder = ?2 || substr(folder, length(?1) + 1)
                 WHERE folder = ?1 OR substr(folder, 1, length(?1) + 1) = ?1 || '/'",
                params![from, to],
            )
            .map_err(db_err)?;
            // Merging into an existing folder: its rows are there already.
            db.execute(
                "UPDATE OR IGNORE bookmark_folders SET path = ?2 || substr(path, length(?1) + 1)
                 WHERE path = ?1 OR substr(path, 1, length(?1) + 1) = ?1 || '/'",
                params![from, to],
            )
            .map_err(db_err)?;
            db.execute("DELETE FROM bookmark_folders WHERE path = ?1 OR substr(path, 1, length(?1) + 1) = ?1 || '/'", [from]).map_err(db_err)?;
        }
        self.folder_keep(to)?;
        let moved = |f: &str| if f == from { Some(to.to_string()) } else { f.strip_prefix(&format!("{from}/")).map(|rest| format!("{to}/{rest}")) };
        let order: Vec<String> = self.folder_order().into_iter().map(|f| moved(&f).unwrap_or(f)).collect();
        self.set_folder_order(&order)
    }

    /// Adds or removes; returns whether the page is bookmarked afterwards.
    pub fn bookmark_toggle(&self, url: &str, title: &str) -> Result<bool, AppError> {
        if self.bookmark_remove(url)? {
            Ok(false)
        } else {
            self.bookmark_add(url, title, "")?;
            Ok(true)
        }
    }

    /// Bookmarks whose title, url or folder contain every word of `query` (all when empty).
    pub fn bookmarks_search(&self, query: &str, limit: usize) -> Vec<Bookmark> {
        let words: Vec<String> = query.split_whitespace().map(like).collect();
        let mut sql = String::from("SELECT id,url,title,folder,created FROM bookmarks");
        for (i, _) in words.iter().enumerate() {
            sql.push_str(if i == 0 { " WHERE " } else { " AND " });
            sql.push_str(&format!("(title||' '||url||' '||folder) LIKE ?{} ESCAPE '\\'", i + 1));
        }
        sql.push_str(&format!(" ORDER BY created DESC LIMIT {limit}"));
        let db = self.db.lock().unwrap();
        let Ok(mut st) = db.prepare(&sql) else { return Vec::new() };
        st.query_map(rusqlite::params_from_iter(words.iter()), |r| {
            Ok(Bookmark { id: r.get(0)?, url: r.get(1)?, title: r.get(2)?, folder: r.get(3)?, created: r.get(4)? })
        })
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
    }

    /// Imports Chrome's `Bookmarks` JSON file; returns how many were new.
    pub fn import_chrome_bookmarks(&self, json: &str) -> Result<usize, AppError> {
        let v: serde_json::Value = serde_json::from_str(json).map_err(|e| AppError::new("크롬 북마크 파일을 읽지 못했습니다.", e))?;
        let mut out = Vec::new();
        fn walk(node: &serde_json::Value, path: &str, out: &mut Vec<(String, String, String)>) {
            match node.get("type").and_then(|t| t.as_str()) {
                Some("url") => {
                    let url = node.get("url").and_then(|u| u.as_str()).unwrap_or_default();
                    if url.starts_with("http") {
                        let title = node.get("name").and_then(|u| u.as_str()).unwrap_or_default();
                        out.push((url.to_string(), title.to_string(), path.to_string()));
                    }
                }
                Some("folder") => {
                    let name = node.get("name").and_then(|u| u.as_str()).unwrap_or_default();
                    let p = if path.is_empty() { name.to_string() } else { format!("{path}/{name}") };
                    for c in node.get("children").and_then(|c| c.as_array()).into_iter().flatten() {
                        walk(c, &p, out);
                    }
                }
                _ => {}
            }
        }
        // Chrome's bookmark bar becomes Kite's bar (top level); the other roots become folders.
        if let Some(roots) = v.get("roots").and_then(|r| r.as_object()) {
            for (key, root) in roots {
                let base = match key.as_str() {
                    "bookmark_bar" => "",
                    "other" => "기타 북마크",
                    "synced" => "모바일 북마크",
                    _ => continue,
                };
                for c in root.get("children").and_then(|c| c.as_array()).into_iter().flatten() {
                    walk(c, base, &mut out);
                }
            }
        }
        let mut n = 0;
        for (url, title, folder) in out {
            if self.bookmark_add(&url, &title, &folder)? {
                n += 1;
            }
        }
        Ok(n)
    }

    // ---- history ----

    pub fn visit(&self, url: &str, title: &str) -> Result<(), AppError> {
        self.db
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO history(url,title,visit_count,last_visit) VALUES(?1,?2,1,?3)
                 ON CONFLICT(url) DO UPDATE SET visit_count=visit_count+1, last_visit=excluded.last_visit,
                 title=CASE WHEN excluded.title<>'' THEN excluded.title ELSE history.title END",
                params![url, title, now_ms()],
            )
            .map_err(db_err)?;
        Ok(())
    }

    pub fn set_title(&self, url: &str, title: &str) {
        let _ = self.db.lock().unwrap().execute("UPDATE history SET title=?2 WHERE url=?1", [url, title]);
    }

    /// Visits matching every word, most used and most recent first.
    pub fn history_search(&self, query: &str, limit: usize) -> Vec<Visit> {
        let words: Vec<String> = query.split_whitespace().map(like).collect();
        let mut sql = String::from("SELECT url,title,visit_count,last_visit FROM history");
        for (i, _) in words.iter().enumerate() {
            sql.push_str(if i == 0 { " WHERE " } else { " AND " });
            sql.push_str(&format!("(title||' '||url) LIKE ?{} ESCAPE '\\'", i + 1));
        }
        // Frecency-ish: recent visits first when browsing, frequent ones first when searching.
        sql.push_str(if words.is_empty() { " ORDER BY last_visit DESC" } else { " ORDER BY visit_count DESC, last_visit DESC" });
        sql.push_str(&format!(" LIMIT {limit}"));
        let db = self.db.lock().unwrap();
        let Ok(mut st) = db.prepare(&sql) else { return Vec::new() };
        st.query_map(rusqlite::params_from_iter(words.iter()), |r| {
            Ok(Visit { url: r.get(0)?, title: r.get(1)?, visit_count: r.get(2)?, last_visit: r.get(3)? })
        })
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
    }

    /// Visits whose address (without `http(s)://` and `www.`) starts with `prefix`, most used first.
    pub fn history_prefix(&self, prefix: &str, limit: usize) -> Vec<Visit> {
        let p: String = prefix.chars().flat_map(|c| if matches!(c, '%' | '_' | '\\') { vec!['\\', c] } else { vec![c] }).collect();
        let sql = format!(
            "SELECT url,title,visit_count,last_visit FROM history
             WHERE url LIKE 'https://'||?1||'%' ESCAPE '\\' OR url LIKE 'http://'||?1||'%' ESCAPE '\\'
                OR url LIKE 'https://www.'||?1||'%' ESCAPE '\\' OR url LIKE 'http://www.'||?1||'%' ESCAPE '\\'
             ORDER BY visit_count DESC, last_visit DESC LIMIT {limit}"
        );
        let db = self.db.lock().unwrap();
        let Ok(mut st) = db.prepare(&sql) else { return Vec::new() };
        st.query_map([p], |r| Ok(Visit { url: r.get(0)?, title: r.get(1)?, visit_count: r.get(2)?, last_visit: r.get(3)? }))
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }

    pub fn history_clear(&self) -> Result<(), AppError> {
        self.db.lock().unwrap().execute("DELETE FROM history", []).map_err(db_err)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn favicons_only_what_was_saved() {
        let s = Store::memory().unwrap();
        assert!(s.favicons(&["https://a.example".into()]).is_empty());
        s.favicon_save("https://a.example", "data:image/png;base64,AAAA").unwrap();
        let got = s.favicons(&["https://a.example".into(), "https://never.example".into()]);
        assert_eq!(got.len(), 1);
        assert!(s.favicon_saved_at("https://a.example").is_some());
        s.favicons_clear().unwrap();
        assert!(s.favicons(&["https://a.example".into()]).is_empty());
    }

    #[test]
    fn bookmarks_toggle_and_search() {
        let s = Store::memory().unwrap();
        assert!(s.bookmark_toggle("https://docs.rs/tauri", "Tauri 문서").unwrap());
        assert!(s.is_bookmarked("https://docs.rs/tauri"));
        s.bookmark_add("https://github.com", "GitHub", "개발").unwrap();
        assert_eq!(s.bookmarks_search("", 10).len(), 2);
        assert_eq!(s.bookmarks_search("tauri 문서", 10).len(), 1);
        assert_eq!(s.bookmarks_search("개발", 10)[0].url, "https://github.com");
        assert_eq!(s.bookmarks_search("100%", 10).len(), 0);
        assert!(!s.bookmark_toggle("https://docs.rs/tauri", "").unwrap());
        assert!(!s.is_bookmarked("https://docs.rs/tauri"));
    }

    #[test]
    fn history_counts_and_keeps_titles() {
        let s = Store::memory().unwrap();
        s.visit("https://a.com", "A 사이트").unwrap();
        s.visit("https://a.com", "").unwrap();
        s.visit("https://b.com", "B").unwrap();
        let h = s.history_search("a.com", 10);
        assert_eq!(h[0].visit_count, 2);
        assert_eq!(h[0].title, "A 사이트");
        s.set_title("https://b.com", "Bee");
        assert_eq!(s.history_search("bee", 10).len(), 1);
        s.history_clear().unwrap();
        assert!(s.history_search("", 10).is_empty());
    }

    #[test]
    fn chrome_import() {
        let s = Store::memory().unwrap();
        let json = r#"{"roots":{"bookmark_bar":{"type":"folder","name":"북마크바","children":[
            {"type":"url","name":"Rust","url":"https://rust-lang.org"},
            {"type":"folder","name":"dev","children":[{"type":"url","name":"crates","url":"https://crates.io"},
              {"type":"url","name":"js","url":"javascript:alert(1)"}]}]},
            "other":{"type":"folder","name":"기타","children":[]}}}"#;
        assert_eq!(s.import_chrome_bookmarks(json).unwrap(), 2);
        assert_eq!(s.import_chrome_bookmarks(json).unwrap(), 0);
        assert_eq!(s.bookmarks_search("crates", 5)[0].folder, "dev");
        assert_eq!(s.bookmarks_search("rust", 5)[0].folder, "");
    }

    #[test]
    fn bookmark_edit_and_delete() {
        let s = Store::memory().unwrap();
        s.bookmark_add("https://a.com", "A", " /일/ 이 /").unwrap();
        s.bookmark_add("https://b.com", "B", "").unwrap();
        let all = s.bookmarks_all();
        assert_eq!(all.iter().map(|b| b.url.as_str()).collect::<Vec<_>>(), ["https://a.com", "https://b.com"]);
        assert_eq!(all[0].folder, "일/이");
        s.bookmark_update(all[0].id, "https://a2.com", " A2 ", "x").unwrap();
        assert!(s.is_bookmarked("https://a2.com"));
        assert_eq!(s.bookmark_update(all[0].id, "https://b.com", "", "").unwrap_err().user_message, "같은 주소의 북마크가 이미 있습니다.");
        assert!(s.bookmark_update(all[0].id, "not a url", "", "").is_err());
        s.bookmark_delete(all[1].id).unwrap();
        assert_eq!(s.bookmarks_all().len(), 1);
    }

    #[test]
    fn folders_move_with_their_contents() {
        let s = Store::memory().unwrap();
        s.bookmark_add("https://a.com/", "a", "개발/러스트").unwrap();
        s.bookmark_add("https://b.com/", "b", "개발/러스트/웹뷰").unwrap();
        s.bookmark_add("https://c.com/", "c", "개발/러스트2").unwrap();
        s.bookmark_add("https://d.com/", "d", "업무").unwrap();
        s.set_folder_order(&["개발".into(), "개발/러스트/웹뷰".into(), "업무".into()]).unwrap();
        assert_eq!(s.folder_move("개발/러스트", "업무").unwrap(), "업무/러스트");
        let f = |u: &str| s.bookmarks_all().into_iter().find(|b| b.url == u).unwrap().folder;
        assert_eq!((f("https://a.com/"), f("https://b.com/"), f("https://c.com/")), ("업무/러스트".into(), "업무/러스트/웹뷰".into(), "개발/러스트2".into()));
        assert_eq!(s.folder_order(), vec!["개발", "업무/러스트/웹뷰", "업무"]);
        // To the top level, and never into itself.
        assert_eq!(s.folder_move("업무/러스트/웹뷰", "").unwrap(), "웹뷰");
        assert!(s.folder_move("업무", "업무/러스트").is_err());
        assert!(s.folder_move("업무", "업무").is_err());
    }

    #[test]
    fn empty_folders_are_kept() {
        let s = Store::memory().unwrap();
        s.bookmark_add("https://a.com/", "a", "개발/러스트").unwrap();
        assert_eq!(s.folders_all(), vec!["개발", "개발/러스트"]);
        // The last bookmark goes: the folder stays.
        let id = s.bookmarks_all()[0].id;
        s.bookmark_delete(id).unwrap();
        assert_eq!(s.folders_all(), vec!["개발", "개발/러스트"]);
        assert_eq!(s.folder_create(" 업무 / 보고서 ").unwrap(), "업무/보고서");
        assert_eq!(s.folder_rename("개발/러스트", "Rust").unwrap(), "개발/Rust");
        assert!(s.folder_rename("개발/Rust", "a/b").is_err());
        assert_eq!(s.folder_move("업무/보고서", "개발").unwrap(), "개발/보고서");
        assert_eq!(s.folders_all(), vec!["개발", "개발/Rust", "개발/보고서", "업무"]);
        // Deleting takes subfolders and their bookmarks along.
        s.bookmark_add("https://b.com/", "b", "개발/Rust").unwrap();
        s.bookmark_add("https://c.com/", "c", "업무").unwrap();
        assert_eq!(s.folder_delete("개발").unwrap(), 1);
        assert_eq!(s.folders_all(), vec!["업무"]);
        assert_eq!(s.bookmarks_all().len(), 1);
    }

    #[test]
    fn old_bar_folder_is_moved_to_the_top() {
        let s = Store::memory().unwrap();
        s.bookmark_add("https://a.com", "A", "북마크바").unwrap();
        s.bookmark_add("https://b.com", "B", "북마크바/개발").unwrap();
        s.bookmark_add("https://c.com", "C", "북마크바놀이").unwrap();
        s.set("migrated_bar_root", "").unwrap();
        s.db.lock().unwrap().execute("DELETE FROM kv WHERE key='migrated_bar_root'", []).unwrap();
        s.migrate().unwrap();
        let f: Vec<String> = s.bookmarks_all().into_iter().map(|b| b.folder).collect();
        assert_eq!(f, ["", "개발", "북마크바놀이"]);
    }

    #[test]
    fn kv() {
        let s = Store::memory().unwrap();
        assert_eq!(s.get("x"), None);
        s.set("x", "1").unwrap();
        s.set("x", "2").unwrap();
        assert_eq!(s.get("x").as_deref(), Some("2"));
    }
}
