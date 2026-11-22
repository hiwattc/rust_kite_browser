//! Address bar suggestions (Chrome's omnibox): an inline completion of the typed text
//! (`you` → `youtube.com`) and a short list of matches from history, bookmarks and a built-in
//! list of popular sites, so the completion works on a fresh profile too.

use std::collections::HashMap;

use serde::Serialize;

use crate::palette::{as_url, engine_url, score, short_url};
use crate::store::Store;

/// Popular sites, most likely first (a Korean user's view). Only used for completion and
/// suggestions; history and bookmarks always win over this list.
const POPULAR: &[&str] = &[
    "naver.com", "google.com", "youtube.com", "daum.net", "coupang.com", "namu.wiki", "kakao.com", "github.com",
    "gmail.com", "instagram.com", "chatgpt.com", "claude.ai", "x.com", "facebook.com", "netflix.com", "tistory.com",
    "wikipedia.org", "11st.co.kr", "gmarket.co.kr", "auction.co.kr", "ssg.com", "musinsa.com", "danawa.com", "yes24.com",
    "kyobobook.co.kr", "aladin.co.kr", "melon.com", "tving.com", "wavve.com", "coupangplay.com", "chzzk.naver.com", "sooplive.co.kr",
    "twitch.tv", "dcinside.com", "fmkorea.com", "ppomppu.co.kr", "clien.net", "ruliweb.com", "inven.co.kr", "nate.com",
    "papago.naver.com", "map.naver.com", "blog.naver.com", "cafe.naver.com", "mail.naver.com", "news.naver.com", "drive.google.com",
    "docs.google.com", "mail.google.com", "maps.google.com", "translate.google.com", "gemini.google.com", "perplexity.ai", "notion.so",
    "figma.com", "slack.com", "zoom.us", "linkedin.com", "reddit.com", "stackoverflow.com", "velog.io", "npmjs.com", "crates.io",
    "docs.rs", "developer.mozilla.org", "apple.com", "microsoft.com", "office.com", "outlook.com", "amazon.com", "aliexpress.com",
    "ebay.com", "temu.com", "airbnb.com", "booking.com", "yanolja.com", "yeogi.com", "baemin.com", "toss.im", "kbstar.com",
    "shinhan.com", "wooribank.com", "hanabank.com", "kakaobank.com", "hometax.go.kr", "gov.kr", "korail.com", "letskorail.com",
    "weather.go.kr", "threads.net", "tiktok.com", "pinterest.com", "spotify.com", "bing.com", "duckduckgo.com",
];

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    /// `url` | `history` | `bookmark` | `popular` | `search`
    pub kind: String,
    /// What opens (a full URL).
    pub url: String,
    pub title: String,
    /// Shown in the address bar while the row is selected.
    pub fill: String,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Suggest {
    /// The whole text to show for an inline completion: starts with what was typed
    /// (ignoring case); the UI selects the added part.
    pub inline: Option<String>,
    pub items: Vec<Suggestion>,
}

/// `https://www.YouTube.com/x` → `youtube.com/x` (what completion matches against).
fn bare(url: &str) -> String {
    let s = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let s = s.strip_prefix("www.").unwrap_or(s);
    let s = s.strip_suffix('/').unwrap_or(s);
    s.to_lowercase()
}

fn host_of(bare: &str) -> &str {
    bare.split(['/', '?', '#']).next().unwrap_or(bare)
}

fn engine_name(engine: &str) -> &'static str {
    match engine {
        "naver" => "네이버",
        "duckduckgo" => "DuckDuckGo",
        "bing" => "Bing",
        _ => "Google",
    }
}

/// Typed text that could start a web address (no spaces, ASCII, not a scheme-less search).
fn completable(input: &str) -> bool {
    !input.is_empty() && !input.contains(char::is_whitespace) && input.is_ascii()
}

pub fn suggest(input: &str, store: &Store, engine: &str) -> Suggest {
    let typed = input.trim_start();
    let q = typed.trim();
    if q.is_empty() {
        return Suggest::default();
    }
    let lower = q.to_lowercase();
    // `http://yo` / `www.yo` complete like `yo`.
    let key = bare(&lower);
    let key = if lower.ends_with('/') && !key.ends_with('/') { format!("{key}/") } else { key };

    // ---- inline completion ----
    // Candidates by host (or the full address once a path is typed), scored: visited sites by
    // how often and how recently, then bookmarks, then the popular list in its order.
    let mut inline: Option<String> = None;
    if completable(q) {
        let with_path = key.contains('/');
        let mut best: HashMap<String, i64> = HashMap::new();
        let mut add = |cand: String, s: i64| {
            if cand.len() > key.len() && cand.starts_with(&key) {
                let e = best.entry(cand).or_insert(0);
                *e = (*e).max(0) + s;
            }
        };
        let now = crate::store::now_ms();
        for v in store.history_prefix(&key, 200) {
            let b = bare(&v.url);
            let days = ((now - v.last_visit) / 86_400_000).max(0);
            let s = 10_000 + v.visit_count.min(500) * 20 - days.min(365);
            add(if with_path { b } else { host_of(&b).to_string() }, s);
        }
        for bm in store.bookmarks_all() {
            let b = bare(&bm.url);
            add(if with_path { b } else { host_of(&b).to_string() }, 5_000);
        }
        if !with_path {
            for (i, p) in POPULAR.iter().enumerate() {
                add(p.to_string(), 1_000 - i as i64);
            }
        }
        // Prefer the shortest of equal scores (youtube.com over youtube.com.br).
        let pick = best.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.len().cmp(&a.0.len())));
        if let Some((cand, _)) = pick {
            // Keep what was typed (`www.`, `https://`, its case) and add the rest.
            let rest = &cand[key.len()..];
            inline = Some(format!("{q}{rest}"));
        }
    }

    // ---- list ----
    let mut items: Vec<Suggestion> = Vec::new();
    let search = Suggestion {
        kind: "search".into(),
        url: format!("{}{}", engine_url(engine), url::form_urlencoded::byte_serialize(q.as_bytes()).collect::<String>()),
        title: format!("{q} - {} 검색", engine_name(engine)),
        fill: q.to_string(),
    };
    // First row: what Enter does.
    if let Some(full) = &inline {
        let url = as_url(full).unwrap_or_else(|| format!("https://{full}"));
        items.push(Suggestion { kind: "url".into(), title: short_url(&url), url, fill: full.clone() });
    } else if let Some(url) = crate::browser::kite_page(q).map(|p| format!("kite://{p}")).or_else(|| as_url(q)) {
        items.push(Suggestion { kind: "url".into(), title: short_url(&url), url, fill: q.to_string() });
    } else {
        items.push(search.clone());
    }

    // History and bookmarks matching every word (also the text retyped in the other keyboard
    // layout: ㅛㅐㅕ → you), best first.
    let mut queries = vec![q.to_string()];
    if let Some(alt) = crate::hangul::to_qwerty(q).filter(|a| a != q) {
        queries.push(alt);
    }
    let mut scored: Vec<(u32, Suggestion)> = Vec::new();
    for qq in &queries {
        let words: Vec<String> = qq.split_whitespace().map(str::to_lowercase).collect();
        let k = bare(&qq.to_lowercase());
        let prefix_bonus = |u: &str| if bare(u).starts_with(&k) { 200 } else { 0 };
        for h in store.history_search(qq, 30) {
            let s = score(&words, &h.title, &h.url).max(1) + h.visit_count.min(50) as u32 * 2 + prefix_bonus(&h.url);
            let title = if h.title.is_empty() { short_url(&h.url) } else { h.title };
            scored.push((s, Suggestion { kind: "history".into(), fill: short_url(&h.url), url: h.url, title }));
        }
        for b in store.bookmarks_search(qq, 20) {
            let s = score(&words, &b.title, &b.url).max(1) + 30 + prefix_bonus(&b.url);
            scored.push((s, Suggestion { kind: "bookmark".into(), fill: short_url(&b.url), url: b.url, title: b.title }));
        }
        if completable(qq) {
            for (i, p) in POPULAR.iter().enumerate().filter(|(_, p)| p.starts_with(&k)).take(3) {
                scored.push((60 - i.min(50) as u32, Suggestion { kind: "popular".into(), url: format!("https://{p}"), title: p.to_string(), fill: p.to_string() }));
            }
        }
    }
    scored.sort_by_key(|x| std::cmp::Reverse(x.0));
    for (_, s) in scored {
        if items.len() >= 6 {
            break;
        }
        if !items.iter().any(|x| bare(&x.url) == bare(&s.url)) {
            items.push(s);
        }
    }
    if items[0].kind != "search" {
        items.push(search);
    }
    Suggest { inline, items }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completes_popular_sites_on_a_fresh_profile() {
        let s = Store::memory().unwrap();
        let r = suggest("you", &s, "google");
        assert_eq!(r.inline.as_deref(), Some("youtube.com"));
        assert_eq!(r.items[0].url, "https://youtube.com/");
        assert_eq!(r.items.last().unwrap().kind, "search");
        assert_eq!(suggest("nav", &s, "google").inline.as_deref(), Some("naver.com"));
        assert_eq!(suggest("YOU", &s, "google").inline.as_deref(), Some("YOUtube.com"));
        assert_eq!(suggest("www.you", &s, "google").inline.as_deref(), Some("www.youtube.com"));
        // Not an address: no completion, search first.
        assert_eq!(suggest("you tube", &s, "google").inline, None);
        assert_eq!(suggest("유튜브", &s, "google").items[0].kind, "search");
    }

    #[test]
    fn history_beats_the_popular_list() {
        let s = Store::memory().unwrap();
        s.visit("https://www.yoursite.dev/docs/a", "Your site").unwrap();
        s.visit("https://www.yoursite.dev/docs/a", "Your site").unwrap();
        assert_eq!(suggest("you", &s, "google").inline.as_deref(), Some("yoursite.dev"));
        // With a path typed, complete the full address from history.
        assert_eq!(suggest("yoursite.dev/d", &s, "google").inline.as_deref(), Some("yoursite.dev/docs/a"));
        let r = suggest("your", &s, "google");
        assert!(r.items.iter().any(|i| i.kind == "history" && i.url == "https://www.yoursite.dev/docs/a"));
    }

    #[test]
    fn wrong_keyboard_layout_still_suggests() {
        let s = Store::memory().unwrap();
        let r = suggest("ㅛㅐㅕ", &s, "google");
        assert!(r.items.iter().any(|i| i.url == "https://youtube.com"), "{r:?}");
    }
}
