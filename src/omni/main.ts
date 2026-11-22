// Address bar suggestions: an overlay webview under the address bar. The address bar (index)
// keeps the keyboard and asks Rust to show this list; a click here opens the row.
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { icons } from "../shared/icons";
import "./omni.css";
import { initTheme } from "../shared/theme";

initTheme();

type Suggestion = { kind: string; url: string; title: string; fill: string };

const wv = getCurrentWebview();
const list = document.createElement("div");
list.className = "list";
document.body.append(list);

const kindIcon: Record<string, string> = {
  url: icons.globe,
  popular: icons.globe,
  history: icons.history,
  bookmark: icons.star,
  search: icons.search,
};

const esc = (s: string) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);

/** URL without scheme/www, for display. */
function short(u: string) {
  try {
    const d = decodeURI(u);
    return d.replace(/^https?:\/\/(www\.)?/, "").replace(/\/$/, "");
  } catch {
    return u;
  }
}

let items: Suggestion[] = [];

wv.listen<{ items: Suggestion[]; selected: number }>("omni-items", (e) => {
  items = e.payload.items;
  list.innerHTML = items
    .map((it, i) => {
      const url = it.kind === "search" ? "" : `<span class="u">${esc(short(it.url))}</span>`;
      const title = it.kind === "search" || it.kind === "url" || it.kind === "popular" ? it.title : it.title || short(it.url);
      return `<div class="row${i === e.payload.selected ? " sel" : ""}" data-i="${i}">
        <span class="ic">${kindIcon[it.kind] ?? icons.globe}</span>
        <span class="t">${esc(title)}</span>${it.kind === "history" || it.kind === "bookmark" ? url : ""}
      </div>`;
    })
    .join("");
});

// mousedown, not click: the address bar loses focus on press and hides the list soon after.
list.addEventListener("mousedown", (e) => {
  const row = (e.target as HTMLElement).closest<HTMLElement>(".row");
  if (!row || e.button > 1) return;
  e.preventDefault();
  const it = items[Number(row.dataset.i)];
  if (it) invoke("omni_pick", { url: it.url, newTab: e.button === 1 || e.metaKey || e.ctrlKey });
});
