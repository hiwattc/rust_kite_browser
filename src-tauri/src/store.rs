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
             CREATE TABLE IF NOT EXISTS kv(key TEXT PRIMARY KEY, value TEXT NOT NULL);",
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
        Ok(())
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
        Ok(())
    }

    pub fn bookmark_delete(&self, id: i64) -> Result<(), AppError> {
        self.db.lock().unwrap().execute("DELETE FROM bookmarks WHERE id=?1", [id]).map_err(db_err)?;
        Ok(())
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

    pub fn history_clear(&self) -> Result<(), AppError> {
        self.db.lock().unwrap().execute("DELETE FROM history", []).map_err(db_err)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
