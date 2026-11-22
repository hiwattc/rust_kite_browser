//! Spotlight palette (Ctrl+Shift+P): one search over open tabs, bookmarks, history and
//! commands, plus "go to URL" / "search the web" for whatever was typed.
//!
//! Prefixes narrow the search: `>` commands, `*` bookmarks, `#` history, `@` tabs.
//! Matching is "every space-separated word appears" (case-insensitive), scored so that title
//! prefix and word-start hits come first.

use serde::{Deserialize, Serialize};

use crate::keymap;
use crate::store::Store;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    /// `tab` | `bookmark` | `history` | `command` | `url` | `search`
    pub kind: String,
    /// Tab id, command id or URL.
    pub id: String,
    pub title: String,
    pub subtitle: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Shortcut label for commands.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    pub title: String,
    pub items: Vec<Item>,
}

/// What the palette needs to know about an open tab.
pub struct TabRef {
    pub id: u64,
    pub title: String,
    pub url: String,
    pub favicon: Option<String>,
}

pub struct Command {
    pub id: &'static str,
    pub title: &'static str,
    pub keywords: &'static str,
}

pub const COMMANDS: &[Command] = &[
    Command { id: "open_settings", title: "설정", keywords: "settings preferences 환경설정 옵션 kite://settings" },
    Command { id: "open_home", title: "홈 (개인 포털)", keywords: "home portal start igoogle workspace 홈 포털 시작 워크스페이스 할 일 todo kite://home" },
    Command { id: "open_meet", title: "영상회의 (1:1, 화면 공유)", keywords: "meet video call conference webrtc p2p 화상 회의 통화 화면 공유 kite://meet" },
    Command { id: "open_bookmarks", title: "북마크 관리자", keywords: "bookmarks manager 즐겨찾기 편집 kite://bookmarks" },
    Command { id: "toggle_side_tabs", title: "세로 탭 목록 표시/숨기기", keywords: "vertical tabs side panel list 세로 탭 사이드 목록 패널" },
    Command { id: "toggle_bookmark_bar", title: "북마크 바 표시/숨기기", keywords: "bookmark bar toolbar 즐겨찾기 바" },
    Command { id: "new_tab", title: "새 탭", keywords: "new tab" },
    Command { id: "new_window", title: "새 창", keywords: "new window" },
    Command { id: "close_tab", title: "탭 닫기", keywords: "close tab" },
    Command { id: "reopen_tab", title: "닫은 탭 다시 열기", keywords: "reopen closed tab undo" },
    Command { id: "duplicate_tab", title: "탭 복제", keywords: "duplicate tab" },
    Command { id: "close_other_tabs", title: "다른 탭 모두 닫기", keywords: "close other tabs" },
    Command { id: "reload", title: "새로고침", keywords: "reload refresh" },
    Command { id: "hard_reload", title: "강력 새로고침 (캐시 무시)", keywords: "hard reload" },
    Command { id: "back", title: "뒤로", keywords: "back" },
    Command { id: "forward", title: "앞으로", keywords: "forward" },
    Command { id: "bookmark", title: "현재 페이지 북마크 추가/삭제", keywords: "bookmark star favorite 즐겨찾기" },
    Command { id: "copy_url", title: "현재 주소 복사", keywords: "copy url link" },
    Command { id: "zoom_in", title: "확대", keywords: "zoom in" },
    Command { id: "zoom_out", title: "축소", keywords: "zoom out" },
    Command { id: "zoom_reset", title: "확대/축소 초기화", keywords: "zoom reset 100%" },
    Command { id: "devtools", title: "개발자 도구", keywords: "devtools inspector console" },
    Command { id: "import_chrome", title: "크롬 북마크 가져오기", keywords: "import chrome bookmarks 즐겨찾기" },
    Command { id: "clear_history", title: "방문 기록 모두 지우기", keywords: "clear history" },
    Command { id: "install_extension", title: "확장 프로그램 설치 (지금 보는 크롬 웹 스토어 페이지)", keywords: "extension install chrome web store 확장 설치 add" },
    Command { id: "open_extensions", title: "확장 프로그램 관리", keywords: "extensions manage 확장 프로그램 chrome://extensions" },
    Command { id: "open_passwords", title: "비밀번호 관리자", keywords: "passwords password manager 암호 로그인 저장 kite://passwords" },
    Command { id: "mcp_copy", title: "Claude Code 연결 명령 복사 (MCP)", keywords: "mcp claude agent automation 자동화" },
    Command { id: "mcp_toggle", title: "MCP 자동화 서버 켜기/끄기", keywords: "mcp server on off agent" },
];

pub const ENGINES: &[(&str, &str)] = &[
    ("google", "https://www.google.com/search?q="),
    ("naver", "https://search.naver.com/search.naver?query="),
    ("duckduckgo", "https://duckduckgo.com/?q="),
    ("bing", "https://www.bing.com/search?q="),
];

pub fn engine_url(engine: &str) -> &'static str {
    ENGINES.iter().find(|(n, _)| *n == engine).map(|(_, u)| *u).unwrap_or(ENGINES[0].1)
}

fn encode(q: &str) -> String {
    url::form_urlencoded::byte_serialize(q.as_bytes()).collect()
}

/// What typed text means: a URL to open, or `None` (search it).
pub fn as_url(input: &str) -> Option<String> {
    let s = input.trim();
    if s.is_empty() || s.contains(char::is_whitespace) {
        return None;
    }
    let lower = s.to_ascii_lowercase();
    for scheme in ["http://", "https://", "file://", "about:", "data:", "view-source:", "chrome-extension://", "webkit-extension://"] {
        if lower.starts_with(scheme) {
            return url::Url::parse(s).ok().map(|u| u.to_string());
        }
    }
    // host[:port][/path]: localhost, IPs, or something with a dot and a plausible TLD.
    let host_port = s.split(['/', '?', '#']).next().unwrap_or("");
    let host = host_port.rsplit_once(':').filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty()).map(|(h, _)| h).unwrap_or(host_port);
    let is_local = host.eq_ignore_ascii_case("localhost") || host.parse::<std::net::Ipv4Addr>().is_ok() || host.starts_with('[');
    let looks_domain = host.contains('.')
        && !host.starts_with('.')
        && !host.ends_with('.')
        && host.rsplit('.').next().is_some_and(|tld| tld.len() >= 2 && tld.chars().all(|c| c.is_alphabetic()))
        && host.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '.');
    if !(is_local || looks_domain) {
        return None;
    }
    let scheme = if is_local { "http://" } else { "https://" };
    url::Url::parse(&format!("{scheme}{s}")).ok().map(|u| u.to_string())
}

/// URL to open for what was typed in the address bar or palette.
pub fn resolve(input: &str, engine: &str) -> String {
    if let Some(p) = crate::browser::kite_page(input) {
        return format!("kite://{p}");
    }
    as_url(input).unwrap_or_else(|| format!("{}{}", engine_url(engine), encode(input.trim())))
}

/// 0 when not every word matches; higher is better.
pub fn score(words: &[String], title: &str, rest: &str) -> u32 {
    if words.is_empty() {
        return 1;
    }
    let t = title.to_lowercase();
    let r = rest.to_lowercase();
    let mut total = 0;
    for w in words {
        let s = if t.starts_with(w.as_str()) {
            30
        } else if t.split(|c: char| !c.is_alphanumeric()).any(|p| p.starts_with(w.as_str())) {
            20
        } else if t.contains(w.as_str()) {
            12
        } else if r.split(|c: char| !c.is_alphanumeric()).any(|p| p.starts_with(w.as_str())) {
            8
        } else if r.contains(w.as_str()) {
            4
        } else {
            return 0;
        };
        total += s;
    }
    total
}

/// Short display form of a URL: host + path, no scheme, no trailing slash.
/// Readable: `%EB%B6%80…` decoded (invalid UTF-8 kept as is).
pub fn short_url(u: &str) -> String {
    let s = u.strip_prefix("https://").or_else(|| u.strip_prefix("http://")).unwrap_or(u);
    let s = s.strip_prefix("www.").unwrap_or(s);
    let s = s.strip_suffix('/').unwrap_or(s);
    match percent_encoding::percent_decode_str(s).decode_utf8() {
        Ok(d) => d.into_owned(),
        Err(_) => s.to_string(),
    }
}

/// Searches `query`; Hangul typed on the wrong input source (`햣` for `git`) is also searched
/// as QWERTY and its hits are added after the direct ones.
pub fn search(query: &str, tabs: &[TabRef], store: &Store, engine: &str) -> Vec<Group> {
    let mut groups = search_one(query, tabs, store, engine, true);
    let plain = query.trim_start_matches(['>', '*', '#', '@']).trim();
    let prefix = &query.trim_start()[..query.trim_start().len() - query.trim_start().trim_start_matches(['>', '*', '#', '@']).len()];
    // English typed while meaning Korean (qntks → 부산): offer to search the Korean word too.
    let korean = crate::hangul::from_qwerty(plain).filter(|k| k.chars().all(|c| c == ' ' || ('\u{AC00}'..='\u{D7A3}').contains(&c)));
    if let Some(k) = &korean
        && prefix.is_empty()
        && let Some(g) = groups.first_mut().filter(|g| g.title.is_empty())
    {
        g.items.push(Item {
            kind: "search".into(),
            id: format!("{}{}", engine_url(engine), encode(k)),
            title: k.clone(),
            subtitle: format!("한/영 전환 · {plain} → {k}"),
            icon: None,
            hint: None,
        });
    }
    let alt = crate::hangul::to_qwerty(query).filter(|a| a.trim() != query.trim()).or_else(|| korean.map(|k| format!("{prefix}{k}")));
    if let Some(alt) = alt {
        for g in search_one(&alt, tabs, store, engine, false) {
            if g.title.is_empty() {
                continue;
            }
            match groups.iter_mut().find(|x| x.title == g.title) {
                Some(x) => {
                    for it in g.items {
                        if !x.items.iter().any(|y| y.kind == it.kind && y.id == it.id) {
                            x.items.push(it);
                        }
                    }
                }
                None => groups.push(g),
            }
        }
    }
    groups
}

fn search_one(query: &str, tabs: &[TabRef], store: &Store, engine: &str, typed: bool) -> Vec<Group> {
    let q = query.trim_start();
    let (mode, rest) = match q.chars().next() {
        Some(c @ ('>' | '*' | '#' | '@')) => (Some(c), q[c.len_utf8()..].trim()),
        _ => (None, q.trim()),
    };
    let words: Vec<String> = rest.split_whitespace().map(str::to_lowercase).collect();
    let only = |c: char| mode.is_none() || mode == Some(c);
    let per = if mode.is_some() { 50 } else { 6 };
    let mut groups = Vec::new();

    // Typed text first: go there / search for it.
    if typed && mode.is_none() && !rest.is_empty() {
        let mut items = Vec::new();
        if let Some(p) = crate::browser::kite_page(rest) {
            let name = match p {
                "settings" => "설정",
                "bookmarks" => "북마크 관리자",
                "meet" => "영상회의",
                "home" => "홈 (개인 포털)",
                _ => "새 탭",
            };
            items.push(Item { kind: "url".into(), id: format!("kite://{p}"), title: name.into(), subtitle: format!("kite://{p}"), icon: None, hint: None });
        } else if let Some(u) = as_url(rest) {
            items.push(Item { kind: "url".into(), id: u.clone(), title: short_url(&u), subtitle: "이동".into(), icon: None, hint: None });
        }
        let name = match engine {
            "naver" => "네이버",
            "duckduckgo" => "DuckDuckGo",
            "bing" => "Bing",
            _ => "Google",
        };
        items.push(Item {
            kind: "search".into(),
            id: format!("{}{}", engine_url(engine), encode(rest)),
            title: rest.to_string(),
            subtitle: format!("{name} 검색"),
            icon: None,
            hint: None,
        });
        groups.push(Group { title: String::new(), items });
    }

    if only('@') {
        let mut v: Vec<(u32, Item)> = tabs
            .iter()
            .filter_map(|t| {
                let s = score(&words, &t.title, &t.url);
                (s > 0).then(|| {
                    (s, Item { kind: "tab".into(), id: t.id.to_string(), title: t.title.clone(), subtitle: short_url(&t.url), icon: t.favicon.clone(), hint: None })
                })
            })
            .collect();
        v.sort_by_key(|x| std::cmp::Reverse(x.0));
        push(&mut groups, "열린 탭", v, per);
    }

    if only('*') {
        // `*/개발` = bookmarks in the folder 개발 (and its subfolders), from the bookmark bar.
        let folder = (mode == Some('*')).then(|| rest.strip_prefix('/')).flatten().map(crate::store::normalize_folder);
        let words: Vec<String> = if folder.is_some() { Vec::new() } else { words.clone() };
        let mut v: Vec<(u32, Item)> = store
            .bookmarks_all()
            .into_iter()
            .filter(|b| folder.as_ref().is_none_or(|f| b.folder == *f || b.folder.starts_with(&format!("{f}/"))))
            .filter_map(|b| {
                let s = score(&words, &b.title, &format!("{} {}", b.url, b.folder));
                let sub = if b.folder.is_empty() { short_url(&b.url) } else { format!("{} · {}", b.folder, short_url(&b.url)) };
                (s > 0).then(|| (s, Item { kind: "bookmark".into(), id: b.url, title: b.title, subtitle: sub, icon: None, hint: None }))
            })
            .collect();
        if !words.is_empty() {
            v.sort_by_key(|x| std::cmp::Reverse(x.0));
        }
        let title = folder.map(|f| format!("북마크 · {f}")).unwrap_or_else(|| "북마크".into());
        push(&mut groups, &title, v, per);
    }

    if only('#') {
        let v: Vec<(u32, Item)> = store
            .history_search(rest, if mode.is_some() { 50 } else { 30 })
            .into_iter()
            .map(|h| {
                let s = score(&words, &h.title, &h.url).max(1) + (h.visit_count.min(20) as u32);
                let title = if h.title.is_empty() { short_url(&h.url) } else { h.title };
                (s, Item { kind: "history".into(), subtitle: short_url(&h.url), id: h.url, title, icon: None, hint: None })
            })
            .collect();
        // Already ordered by the query (frequency / recency): keep that order when browsing.
        let mut v = v;
        if !words.is_empty() {
            v.sort_by_key(|x| std::cmp::Reverse(x.0));
        }
        push(&mut groups, "방문 기록", v, per);
    }

    if only('>') && (mode == Some('>') || !words.is_empty()) {
        let mut v: Vec<(u32, Item)> = COMMANDS
            .iter()
            .filter_map(|c| {
                let s = score(&words, c.title, c.keywords);
                (s > 0).then(|| {
                    (s, Item { kind: "command".into(), id: c.id.into(), title: c.title.into(), subtitle: String::new(), icon: None, hint: keymap::label_for(c.id) })
                })
            })
            .collect();
        if !words.is_empty() {
            v.sort_by_key(|x| std::cmp::Reverse(x.0));
        }
        push(&mut groups, "명령", v, if mode.is_some() { 50 } else { 4 });
    }
    groups
}

fn push(groups: &mut Vec<Group>, title: &str, v: Vec<(u32, Item)>, max: usize) {
    if !v.is_empty() {
        groups.push(Group { title: title.into(), items: v.into_iter().take(max).map(|(_, i)| i).collect() });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_vs_searches() {
        assert_eq!(as_url("naver.com").as_deref(), Some("https://naver.com/"));
        assert_eq!(as_url("localhost:3000/a").as_deref(), Some("http://localhost:3000/a"));
        assert_eq!(as_url("127.0.0.1:8080").as_deref(), Some("http://127.0.0.1:8080/"));
        assert_eq!(as_url("https://x.dev/a?b=1").as_deref(), Some("https://x.dev/a?b=1"));
        assert_eq!(as_url("docs.rs/tauri/latest").as_deref(), Some("https://docs.rs/tauri/latest"));
        assert_eq!(as_url("rust tauri"), None);
        assert_eq!(as_url("hello"), None);
        assert_eq!(short_url("https://ko.wikipedia.org/wiki/%EB%B6%80%EC%82%B0"), "ko.wikipedia.org/wiki/부산");
        assert_eq!(short_url("https://www.a.com/%FF/"), "a.com/%FF");
        assert_eq!(as_url("1.5"), None);
        assert_eq!(as_url("파이썬"), None);
        assert_eq!(resolve("러스트 브라우저", "google"), "https://www.google.com/search?q=%EB%9F%AC%EC%8A%A4%ED%8A%B8+%EB%B8%8C%EB%9D%BC%EC%9A%B0%EC%A0%80");
        assert_eq!(resolve("chrome://settings", "google"), "kite://settings");
        assert!(resolve("a&b", "naver").starts_with("https://search.naver.com/search.naver?query=a%26b"));
    }

    #[test]
    fn scoring_prefers_title_prefix() {
        let w = vec!["git".to_string()];
        assert!(score(&w, "GitHub", "") > score(&w, "My GitHub", ""));
        assert!(score(&w, "My GitHub", "") > score(&w, "Digit", ""));
        assert!(score(&w, "Digit", "") > score(&w, "Other", "https://git.example"));
        assert_eq!(score(&w, "Other", "x"), 0);
        let two = vec!["깃".to_string(), "허브".to_string()];
        assert!(score(&two, "깃 허브", "") > 0);
        assert_eq!(score(&two, "깃", ""), 0);
    }

    #[test]
    fn search_groups_and_prefixes() {
        let s = Store::memory().unwrap();
        s.bookmark_add("https://github.com", "GitHub", "dev").unwrap();
        s.visit("https://gitlab.com", "GitLab").unwrap();
        let tabs = vec![TabRef { id: 7, title: "Git docs".into(), url: "https://git-scm.com".into(), favicon: None }];
        let g = search("git", &tabs, &s, "google");
        let titles: Vec<&str> = g.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(titles, ["", "열린 탭", "북마크", "방문 기록"]);
        assert_eq!(g[0].items[0].kind, "search");
        assert_eq!(g[1].items[0].id, "7");
        s.bookmark_add("https://x.com/a", "에이", "개발/웹").unwrap();
        let f = search("*/개발", &tabs, &s, "google");
        assert_eq!(f[0].title, "북마크 · 개발");
        assert_eq!(f[0].items.len(), 1); // only 개발/웹 (github is in "dev")
        assert_eq!(f[0].items[0].subtitle, "개발/웹 · x.com/a");
        let only = search("*git", &tabs, &s, "google");
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].title, "북마크");
        let cmds = search(">", &tabs, &s, "google");
        assert_eq!(cmds[0].items.len(), COMMANDS.len());
        assert!(search("북마크", &tabs, &s, "google").iter().any(|g| g.title == "명령"));
        // empty query: no "search" row, commands hidden
        let empty = search("", &tabs, &s, "google");
        assert!(empty.iter().all(|g| g.title != "명령" && !g.title.is_empty()));
        // wrong input source: 햣 = git
        let k = search("햣", &tabs, &s, "google");
        assert!(k.iter().any(|g| g.title == "북마크" && g.items[0].id == "https://github.com"));
        assert_eq!(k[0].items[0].title, "햣"); // still offers to search what was typed
        // and the other way: qntks = 부산
        s.visit("https://ko.wikipedia.org/wiki/%EB%B6%80%EC%82%B0", "부산 - 위키백과").unwrap();
        let e = search("qntks", &tabs, &s, "google");
        assert_eq!(e[0].items[1].title, "부산");
        assert!(e.iter().any(|g| g.title == "방문 기록" && g.items[0].title == "부산 - 위키백과"));
        assert!(search("github", &tabs, &s, "google")[0].items.len() == 1); // no bogus Korean
    }
}
