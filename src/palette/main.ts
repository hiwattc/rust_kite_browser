// Spotlight palette: its own webview, shown above the tabs. Search runs in Rust
// (`palette_search`); this renders groups, handles keys and resizes the webview to fit.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { combo } from "../shared/keys";
import { icons } from "../shared/icons";
import type { Group, Init, Item } from "../shared/types";
import "./palette.css";

const isMac = navigator.platform.toLowerCase().includes("mac");
const mod = isMac ? "⌘" : "Ctrl+";

document.body.innerHTML = `
  <div class="box">
    <div class="head">
      <span class="head-icon">${icons.search}</span>
      <input id="q" spellcheck="false" autocomplete="off" placeholder="탭, 북마크, 방문 기록, 명령 검색 또는 주소 입력" />
      <span class="mode" id="mode" hidden></span>
    </div>
    <div class="list" id="list"></div>
    <div class="foot">
      <span><kbd>↑</kbd><kbd>↓</kbd> 이동</span>
      <span><kbd>Enter</kbd> 열기</span>
      <span><kbd>${mod}Enter</kbd> 새 탭</span>
      <span class="sep"></span>
      <span class="prefix"><b>&gt;</b> 명령 <b>*</b> 북마크 <b>#</b> 기록 <b>@</b> 탭</span>
    </div>
  </div>
`;

const q = document.getElementById("q") as HTMLInputElement;
const list = document.getElementById("list") as HTMLElement;
const modeEl = document.getElementById("mode") as HTMLElement;
let items: Item[] = [];
let sel = 0;
let seq = 0;
let openedAt = 0;
let keymap: Record<string, string> = {};

const KIND_ICON: Record<Item["kind"], string> = {
  tab: icons.tab,
  bookmark: icons.bookmark,
  history: icons.history,
  command: icons.command,
  url: icons.arrow,
  search: icons.search,
};
const MODE_NAME: Record<string, string> = { ">": "명령", "*": "북마크", "#": "방문 기록", "@": "열린 탭" };

function esc(s: string) {
  return s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);
}

/** Wraps the typed words in <mark>. */
function highlight(text: string, words: string[]) {
  if (!words.length) return esc(text);
  const lower = text.toLowerCase();
  const marks = new Array(text.length).fill(false);
  for (const w of words) {
    let i = lower.indexOf(w);
    while (w && i >= 0) {
      for (let k = i; k < i + w.length; k++) marks[k] = true;
      i = lower.indexOf(w, i + w.length);
    }
  }
  let out = "";
  let open = false;
  for (let i = 0; i < text.length; i++) {
    if (marks[i] && !open) (out += "<mark>"), (open = true);
    if (!marks[i] && open) (out += "</mark>"), (open = false);
    out += esc(text[i]);
  }
  return open ? out + "</mark>" : out;
}

function words() {
  return q.value.replace(/^[>*#@]/, "").trim().toLowerCase().split(/\s+/).filter(Boolean);
}

function render(groups: Group[]) {
  items = groups.flatMap((g) => g.items);
  sel = Math.min(sel, Math.max(0, items.length - 1));
  const w = words();
  let i = 0;
  let html = "";
  for (const g of groups) {
    if (g.title) html += `<div class="group">${esc(g.title)}</div>`;
    for (const it of g.items) {
      const icon = it.icon
        ? `<img src="${esc(it.icon)}" alt="" onerror="this.replaceWith(document.createRange().createContextualFragment(window.__fallbackIcon))" />`
        : KIND_ICON[it.kind];
      const right = it.hint ? `<kbd>${esc(it.hint)}</kbd>` : it.kind === "tab" ? `<span class="badge">전환</span>` : "";
      html += `<div class="item${i === sel ? " sel" : ""}" data-i="${i}">
        <span class="ic">${icon}</span>
        <span class="txt"><span class="t">${highlight(it.title || it.subtitle, w)}</span>${it.subtitle && it.title ? `<span class="s">${highlight(it.subtitle, w)}</span>` : ""}</span>
        ${right}
      </div>`;
      i++;
    }
  }
  if (!items.length) html = `<div class="empty">${q.value.trim() ? "결과가 없습니다" : "최근 탭·기록이 여기에 표시됩니다"}</div>`;
  list.innerHTML = html;
  resize();
}
(window as unknown as { __fallbackIcon: string }).__fallbackIcon = icons.globe;

function resize() {
  const h = Math.ceil((document.querySelector(".box") as HTMLElement).getBoundingClientRect().height);
  invoke("palette_resize", { height: h });
}

function select(i: number) {
  if (!items.length) return;
  sel = (i + items.length) % items.length;
  list.querySelectorAll(".item").forEach((el) => el.classList.toggle("sel", Number((el as HTMLElement).dataset.i) === sel));
  list.querySelector(".item.sel")?.scrollIntoView({ block: "nearest" });
}

async function search() {
  const my = ++seq;
  const m = q.value.charAt(0);
  modeEl.hidden = !MODE_NAME[m];
  if (MODE_NAME[m]) modeEl.textContent = MODE_NAME[m];
  const groups = await invoke<Group[]>("palette_search", { query: q.value });
  if (my !== seq) return;
  sel = 0;
  render(groups);
}

function run(it: Item | undefined, newTab: boolean) {
  if (!it) return;
  invoke("palette_run", { kind: it.kind, id: it.id, newTab }).catch((e) => console.error(e));
}

function close(refocus: boolean) {
  invoke("palette_close", { refocus });
}

q.addEventListener("input", () => search());
q.addEventListener("keydown", (e) => {
  if (e.isComposing) return; // Korean IME: let the syllable finish first
  if (e.key === "ArrowDown" || (e.ctrlKey && e.key === "n")) {
    e.preventDefault();
    select(sel + 1);
  } else if (e.key === "ArrowUp" || (e.ctrlKey && e.key === "p" && !e.shiftKey)) {
    e.preventDefault();
    select(sel - 1);
  } else if (e.key === "Enter") {
    e.preventDefault();
    run(items[sel], isMac ? e.metaKey : e.ctrlKey);
  } else if (e.key === "Escape") {
    e.preventDefault();
    if (q.value && !openedWithQuery) {
      q.value = "";
      search();
    } else {
      close(true);
    }
  } else if (e.key === "Tab") {
    e.preventDefault();
    select(sel + (e.shiftKey ? -1 : 1));
  }
});
list.addEventListener("mousemove", (e) => {
  const el = (e.target as HTMLElement).closest<HTMLElement>(".item");
  if (el && Number(el.dataset.i) !== sel) select(Number(el.dataset.i));
});
list.addEventListener("mousedown", (e) => {
  const el = (e.target as HTMLElement).closest<HTMLElement>(".item");
  if (!el) return;
  e.preventDefault();
  run(items[Number(el.dataset.i)], e.button === 1 || (isMac ? e.metaKey : e.ctrlKey));
});

// App shortcuts still work here (Ctrl+Shift+P closes, ⌘T opens a tab, …).
window.addEventListener(
  "keydown",
  (e) => {
    const action = keymap[combo(e)];
    if (!action) return;
    e.preventDefault();
    e.stopPropagation();
    invoke("shortcut", { action });
  },
  true,
);

// Clicking anywhere else (the page, the toolbar) closes the palette.
window.addEventListener("blur", () => {
  if (Date.now() - openedAt > 250) close(false);
});

let openedWithQuery = false;
async function main() {
  await listen<{ mode: string; query: string }>("palette-open", (e) => {
    openedAt = Date.now();
    openedWithQuery = !!e.payload.query;
    q.value = (e.payload.mode || "") + (e.payload.query || "");
    q.focus();
    if (e.payload.query) q.select();
    search();
  });
  const init = await invoke<Init>("ui_init");
  keymap = init.keymap;
  document.documentElement.dataset.platform = init.platform;
}
main();
