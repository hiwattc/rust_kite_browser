// Color themes: tokens per theme in `styles/base.css` (`:root[data-theme="…"]`). Every Kite page
// calls `initTheme()`; Rust keeps the choice (`theme` setting) and tells open pages on change.
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";

export interface Theme {
  id: string;
  name: string;
  /** strip, toolbar, accent, text (for the preview in Settings) */
  colors: [string, string, string, string];
  dark: boolean;
}

export const THEMES: Theme[] = [
  { id: "default", name: "기본", colors: ["#dfe3ea", "#f6f7f9", "#6d4aff", "#1c2230"], dark: false },
  { id: "ocean", name: "오션", colors: ["#d6e4f0", "#f3f8fc", "#1f6fd1", "#13263a"], dark: false },
  { id: "forest", name: "포레스트", colors: ["#d9e6dc", "#f4f8f4", "#1f8a4c", "#182a1f"], dark: false },
  { id: "sunset", name: "선셋", colors: ["#f3dfd3", "#fdf6f1", "#e0662f", "#33201a"], dark: false },
  { id: "sakura", name: "벚꽃", colors: ["#f3dde5", "#fdf5f8", "#d6457f", "#331d27"], dark: false },
  { id: "sand", name: "샌드", colors: ["#e8e0d2", "#faf7f1", "#a87a2a", "#2e281f"], dark: false },
  { id: "midnight", name: "미드나잇", colors: ["#0d1424", "#162036", "#5b9dff", "#e3eaf7"], dark: true },
  { id: "nordic", name: "노르딕", colors: ["#2a303c", "#343b48", "#88c0d0", "#e5e9f0"], dark: true },
  { id: "grape", name: "그레이프", colors: ["#1c1626", "#282036", "#c18cff", "#ece6f5"], dark: true },
  { id: "mocha", name: "모카", colors: ["#1f1a17", "#2b2420", "#e0a46b", "#efe6de"], dark: true },
  { id: "graphite", name: "그래파이트", colors: ["#111214", "#1c1d20", "#2dd4bf", "#ececec"], dark: true },
];

const KEY = "kite.theme";

export function applyTheme(id: string) {
  const known = THEMES.some((t) => t.id === id) ? id : "default";
  if (known === "default") delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = known;
  try {
    localStorage.setItem(KEY, known);
  } catch {
    /* only speeds up the next start */
  }
}

/** Last theme at once (no flash of the default), then the saved one, then live changes. */
export function initTheme() {
  try {
    const last = localStorage.getItem(KEY);
    if (last) applyTheme(last);
  } catch {
    /* fine */
  }
  invoke<string>("theme_get").then(applyTheme, () => {});
  getCurrentWebview()
    .listen<string>("theme", (e) => applyTheme(e.payload))
    .catch(() => {});
}
