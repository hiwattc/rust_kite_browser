//! Keyboard shortcuts: one table for the chrome UI, the palette and every content tab.
//!
//! Combos use `KeyboardEvent.code` (physical key), so they work with a Korean IME on
//! (`e.key` would be `ㅔ` for P) and on any keyboard layout. Format:
//! `Ctrl+Alt+Shift+Meta+<code>`, modifiers in that order, only those held.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Binding {
    pub combo: String,
    pub action: &'static str,
}

/// Human readable label of the first binding of each action, for the palette.
pub fn label_for(action: &str) -> Option<String> {
    let b = bindings().into_iter().find(|b| b.action == action)?;
    Some(pretty(&b.combo))
}

fn pretty(combo: &str) -> String {
    let mac = cfg!(target_os = "macos");
    combo
        .split('+')
        .map(|p| match p {
            "Meta" if mac => "⌘".to_string(),
            "Ctrl" if mac => "⌃".to_string(),
            "Alt" if mac => "⌥".to_string(),
            "Shift" if mac => "⇧".to_string(),
            "Meta" => "Win".to_string(),
            "BracketLeft" => "[".to_string(),
            "BracketRight" => "]".to_string(),
            "Equal" => "=".to_string(),
            "Minus" => "-".to_string(),
            "Comma" => ",".to_string(),
            "ArrowLeft" => "←".to_string(),
            "ArrowRight" => "→".to_string(),
            p => p.strip_prefix("Key").or_else(|| p.strip_prefix("Digit")).unwrap_or(p).to_string(),
        })
        .collect::<Vec<_>>()
        .join(if mac { "" } else { "+" })
}

pub fn bindings() -> Vec<Binding> {
    let mac = cfg!(target_os = "macos");
    // "Mod" = ⌘ on macOS, Ctrl elsewhere.
    let m = if mac { "Meta" } else { "Ctrl" };
    let mut v: Vec<(String, &'static str)> = vec![
        ("Ctrl+Shift+KeyP".to_string(), "palette"),
        (format!("{m}+KeyT"), "new_tab"),
        (format!("{m}+KeyW"), "close_tab"),
        (format!("{m}+Shift+KeyT"), "reopen_tab"),
        (format!("{m}+KeyL"), "focus_address"),
        (format!("{m}+KeyR"), "reload"),
        (format!("{m}+Shift+KeyR"), "hard_reload"),
        (format!("{m}+KeyD"), "bookmark"),
        (format!("{m}+Shift+KeyA"), "search_tabs"),
        (format!("{m}+Equal"), "zoom_in"),
        (format!("{m}+Shift+Equal"), "zoom_in"),
        (format!("{m}+NumpadAdd"), "zoom_in"),
        (format!("{m}+Minus"), "zoom_out"),
        (format!("{m}+NumpadSubtract"), "zoom_out"),
        (format!("{m}+Digit0"), "zoom_reset"),
        ("Ctrl+Tab".into(), "next_tab"),
        ("Ctrl+Shift+Tab".into(), "prev_tab"),
        ("F12".into(), "devtools"),
    ];
    if mac {
        v.extend([
            ("Meta+Shift+KeyP".into(), "palette"),
            ("Meta+BracketLeft".into(), "back"),
            ("Meta+BracketRight".into(), "forward"),
            ("Meta+Alt+ArrowRight".into(), "next_tab"),
            ("Meta+Alt+ArrowLeft".into(), "prev_tab"),
            ("Meta+Shift+BracketRight".into(), "next_tab"),
            ("Meta+Shift+BracketLeft".into(), "prev_tab"),
            ("Meta+Alt+KeyI".into(), "devtools"),
            ("Meta+KeyY".into(), "search_history"),
            ("Meta+Alt+KeyB".into(), "open_bookmarks"),
            ("Meta+Shift+KeyB".into(), "toggle_bookmark_bar"),
            ("Meta+Comma".into(), "open_settings"),
        ]);
    } else {
        v.extend([
            ("Ctrl+F4".into(), "close_tab"),
            ("Alt+KeyD".into(), "focus_address"),
            ("F6".into(), "focus_address"),
            ("F5".into(), "reload"),
            ("Ctrl+F5".into(), "hard_reload"),
            ("Alt+ArrowLeft".into(), "back"),
            ("Alt+ArrowRight".into(), "forward"),
            ("Ctrl+PageDown".into(), "next_tab"),
            ("Ctrl+PageUp".into(), "prev_tab"),
            ("Ctrl+Shift+KeyI".into(), "devtools"),
            ("Ctrl+KeyH".into(), "search_history"),
            ("Ctrl+Shift+KeyO".into(), "open_bookmarks"),
            ("Ctrl+Shift+KeyB".into(), "toggle_bookmark_bar"),
            ("Ctrl+Comma".into(), "open_settings"),
        ]);
    }
    for (i, a) in ["tab_1", "tab_2", "tab_3", "tab_4", "tab_5", "tab_6", "tab_7", "tab_8", "tab_last"].iter().enumerate() {
        v.push((format!("{m}+Digit{}", i + 1), a));
    }
    v.into_iter().map(|(combo, action)| Binding { combo: normalize(&combo), action }).collect()
}

/// Puts modifiers in the fixed order the JS side uses (`Ctrl+Alt+Shift+Meta+code`).
fn normalize(combo: &str) -> String {
    const ORDER: [&str; 4] = ["Ctrl", "Alt", "Shift", "Meta"];
    let mut parts: Vec<&str> = combo.split('+').collect();
    let key = parts.pop().unwrap_or_default();
    parts.sort_by_key(|m| ORDER.iter().position(|o| o == m).unwrap_or(9));
    parts.push(key);
    parts.join("+")
}

/// Name of an action for people (settings page shortcut list).
pub fn action_title(action: &str) -> String {
    let fixed = match action {
        "palette" => "스포트라이트 (탭·북마크·기록·명령 검색)",
        "focus_address" => "주소창으로 이동",
        "next_tab" => "다음 탭",
        "prev_tab" => "이전 탭",
        "tab_last" => "마지막 탭",
        "search_tabs" => "열린 탭 검색",
        "search_history" => "방문 기록 검색",
        "search_bookmarks" => "북마크 검색",
        "stop" => "로딩 중지",
        a if a.starts_with("tab_") => return format!("{}번째 탭", &a[4..]),
        a => return crate::palette::COMMANDS.iter().find(|c| c.id == a).map(|c| c.title.to_string()).unwrap_or_else(|| a.to_string()),
    };
    fixed.to_string()
}

/// Every action with all its key labels, in table order.
pub fn describe() -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for b in bindings() {
        let label = pretty(&b.combo);
        match out.iter_mut().find(|(a, _)| a == b.action) {
            Some((_, keys)) => keys.push(label),
            None => out.push((b.action.to_string(), vec![label])),
        }
    }
    out.into_iter().map(|(a, k)| (action_title(&a), k)).collect()
}

/// `{"Meta+KeyT":"new_tab", ...}` for the injected scripts.
pub fn json_map() -> String {
    let map: serde_json::Map<String, serde_json::Value> =
        bindings().into_iter().map(|b| (b.combo, serde_json::Value::String(b.action.into()))).collect();
    serde_json::Value::Object(map).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_is_ctrl_shift_p_everywhere_and_combos_are_unique() {
        let b = bindings();
        assert!(b.iter().any(|b| b.combo == "Ctrl+Shift+KeyP" && b.action == "palette"));
        let mut seen = std::collections::HashSet::new();
        for x in &b {
            assert!(seen.insert(x.combo.clone()), "duplicate combo {}", x.combo);
            // modifier order is fixed so the JS side can build the same string
            let mods: Vec<&str> = x.combo.split('+').collect();
            let order = ["Ctrl", "Alt", "Shift", "Meta"];
            let idx: Vec<usize> = mods[..mods.len() - 1].iter().map(|m| order.iter().position(|o| o == m).unwrap()).collect();
            assert!(idx.windows(2).all(|w| w[0] < w[1]), "bad order {}", x.combo);
        }
        assert!(json_map().contains("\"Ctrl+Shift+KeyP\":\"palette\""));
    }

    #[test]
    fn labels() {
        let l = label_for("new_tab").unwrap();
        if cfg!(target_os = "macos") {
            assert_eq!(l, "⌘T");
        } else {
            assert_eq!(l, "Ctrl+T");
        }
    }
}
