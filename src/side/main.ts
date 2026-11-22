// Left panel of a window's pages (toolbar button / "세로 탭 목록"): the window's open tabs, or the
// bookmarks, as a vertical list with a search filter. Rust owns the tabs; this draws
// `browser-state` like the strip and asks for changes with the same commands.
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { icons } from "../shared/icons";
import { initTheme } from "../shared/theme";
import { NEWTAB, type BrowserState, type Init, type Tab } from "../shared/types";
import "./side.css";

initTheme();
const wv = getCurrentWebview();
const isMac = navigator.platform.toLowerCase().includes("mac");

type Mode = "tabs" | "bookmarks";
interface Bookmark {
  id: number;
  url: string;
  title: string;
  folder: string;
}
/** One row the keyboard can select (tab or bookmark). */
type Row = { kind: "tab"; tab: Tab } | { kind: "bm"; bm: Bookmark };

const MODE_KEY = "kite.side.mode";
let mode: Mode = (() => {
  try {
    return localStorage.getItem(MODE_KEY) === "bookmarks" ? "bookmarks" : "tabs";
  } catch {
    return "tabs";
  }
})();
let state: BrowserState = { tabs: [], active: null, agentTab: null, mcp: { running: false, port: 0, url: "" }, side: true };
let bookmarks: Bookmark[] = [];
let folderOrder: string[] = [];
let bookmarksLoaded = false;
let query = "";
let sel = -1;
let rows: Row[] = [];
let lastActive: number | null = null;

document.body.innerHTML = `
  <div class="head">
    <div class="search">${icons.search}<input id="q" type="search" spellcheck="false" /></div>
    <button class="new" id="new" title="새 탭">${icons.plus}</button>
  </div>
  <div class="list" id="list"></div>
  <div class="foot">
    <div class="seg" role="tablist">
      <button data-mode="tabs">${icons.tab}<span>열린 탭</span><i id="n-tabs"></i></button>
      <button data-mode="bookmarks">${icons.star}<span>북마크</span><i id="n-bm"></i></button>
    </div>
  </div>
  <div class="grip" id="grip" title="끌어서 폭 조절 · 더블클릭: 기본 폭"></div>`;

const list = document.getElementById("list")!;
const q = document.getElementById("q") as HTMLInputElement;

const esc = (s: string) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);
const host = (u: string) => {
  if (u === NEWTAB) return "새 탭";
  try {
    const x = new URL(u);
    return x.protocol.startsWith("http") ? x.host.replace(/^www\./, "") : u;
  } catch {
    return u;
  }
};
const titleOf = (t: Tab) => (t.url === NEWTAB ? "새 탭" : t.title || host(t.url));
const favicon = (u: string) => {
  try {
    const x = new URL(u);
    return /^https?:$/.test(x.protocol) ? `${x.origin}/favicon.ico` : "";
  } catch {
    return "";
  }
};

function words() {
  return query.toLowerCase().split(/\s+/).filter(Boolean);
}
const matches = (hay: string) => words().every((w) => hay.toLowerCase().includes(w));
function mark(text: string) {
  for (const w of words()) {
    const i = text.toLowerCase().indexOf(w);
    if (i >= 0) return `${esc(text.slice(0, i))}<mark>${esc(text.slice(i, i + w.length))}</mark>${esc(text.slice(i + w.length))}`;
  }
  return esc(text);
}

// ---------------------------------------------------------------- rendering

function render() {
  document.querySelectorAll<HTMLElement>(".seg button").forEach((b) => b.classList.toggle("on", b.dataset.mode === mode));
  q.placeholder = mode === "tabs" ? "탭 검색 (제목·주소)" : "북마크 검색 (제목·주소·폴더)";
  document.getElementById("n-tabs")!.textContent = String(state.tabs.length);
  document.getElementById("n-bm")!.textContent = bookmarksLoaded ? String(bookmarks.length) : "";
  if (mode === "tabs") renderTabs();
  else renderBookmarks();
}

function renderTabs() {
  const tabs = query ? state.tabs.filter((t) => matches(`${titleOf(t)} ${t.url}`)) : state.tabs;
  rows = tabs.map((tab) => ({ kind: "tab", tab }));
  if (sel >= rows.length) sel = rows.length - 1;
  list.innerHTML =
    tabs
      .map((t, i) => {
        const icon = t.loading ? `<span class="spin"></span>` : t.favicon ? `<img src="${esc(t.favicon)}" alt="" draggable="false" />` : icons.globe;
        const cls = ["item", t.id === state.active ? "on" : "", i === sel ? "sel" : ""].join(" ");
        return `<div class="${cls}" data-i="${i}" data-id="${t.id}" title="${esc(titleOf(t))}\n${esc(t.url)}">
          <span class="ic">${icon}</span>
          <span class="txt"><b>${mark(titleOf(t))}</b><small>${mark(host(t.url))}</small></span>
          ${t.id === state.agentTab ? `<span class="agent" title="AI 에이전트가 쓰는 탭">${icons.bot}</span>` : ""}
          <button class="x" title="탭 닫기">${icons.close}</button>
        </div>`;
      })
      .join("") || `<div class="empty">${query ? "일치하는 탭이 없습니다" : "탭이 없습니다"}</div>`;
  list.classList.toggle("filtered", !!query);
  if (state.active !== lastActive) {
    lastActive = state.active;
    list.querySelector(".item.on")?.scrollIntoView({ block: "nearest" });
  }
}

/** Bookmarks grouped by folder: bookmark bar first, then the bookmark manager's folder order. */
function renderBookmarks() {
  if (!bookmarksLoaded) {
    list.innerHTML = `<div class="empty"><span class="spin"></span></div>`;
    return;
  }
  const shown = query ? bookmarks.filter((b) => matches(`${b.title} ${b.url} ${b.folder}`)) : bookmarks;
  const groups = new Map<string, Bookmark[]>();
  for (const b of shown) {
    if (!groups.has(b.folder)) groups.set(b.folder, []);
    groups.get(b.folder)!.push(b);
  }
  const rank = (f: string) => {
    if (f === "") return -1;
    const i = folderOrder.indexOf(f);
    return i < 0 ? Number.MAX_SAFE_INTEGER : i;
  };
  const keys = [...groups.keys()].sort((a, b) => rank(a) - rank(b) || a.localeCompare(b, "ko"));
  rows = [];
  let html = "";
  for (const k of keys) {
    const parts = k ? k.split("/") : [];
    const label = k ? parts.map((p, i) => (i === parts.length - 1 ? `<b>${mark(p)}</b>` : `<span>${esc(p)}</span><i>›</i>`)).join("") : "<b>북마크 바</b>";
    html += `<div class="cat">${k ? icons.folder : icons.star}<span class="path">${label}</span><em>${groups.get(k)!.length}</em></div>`;
    for (const bm of groups.get(k)!) {
      const i = rows.length;
      rows.push({ kind: "bm", bm });
      const fav = favicon(bm.url);
      html += `<div class="item bm${i === sel ? " sel" : ""}" data-i="${i}" title="${esc(bm.title || bm.url)}\n${esc(bm.url)}">
          <span class="ic">${fav ? `<img src="${esc(fav)}" alt="" draggable="false" onerror="this.remove()" />` : ""}${icons.globe}</span>
          <span class="txt"><b>${mark(bm.title || host(bm.url))}</b><small>${mark(host(bm.url))}</small></span>
        </div>`;
    }
  }
  if (sel >= rows.length) sel = rows.length - 1;
  list.innerHTML = html || `<div class="empty">${query ? "일치하는 북마크가 없습니다" : "북마크가 없습니다. 페이지에서 ☆를 누르면 여기에 모입니다."}</div>`;
}

async function loadBookmarks() {
  try {
    const r = await invoke<{ bookmarks: Bookmark[]; order: string[] }>("side_bookmarks");
    bookmarks = r.bookmarks;
    folderOrder = r.order;
    bookmarksLoaded = true;
  } catch {
    /* keep what we had */
  }
  render();
}

function setMode(m: Mode) {
  if (m === mode) return;
  mode = m;
  try {
    localStorage.setItem(MODE_KEY, m);
  } catch {
    /* fine */
  }
  q.value = "";
  query = "";
  sel = -1;
  list.scrollTop = 0;
  if (m === "bookmarks") loadBookmarks();
  render();
  q.focus();
}
document.querySelectorAll<HTMLElement>(".seg button").forEach((b) => (b.onclick = () => setMode(b.dataset.mode as Mode)));

// ---------------------------------------------------------------- opening

function openRow(r: Row | undefined, newTab: boolean) {
  if (!r) return;
  if (r.kind === "tab") invoke("tab_activate", { id: r.tab.id });
  else if (newTab || state.active === null) invoke("tab_new", { url: r.bm.url });
  else invoke("navigate", { id: state.active, input: r.bm.url });
}

list.addEventListener("mousedown", (e) => {
  const item = (e.target as HTMLElement).closest<HTMLElement>(".item");
  if (!item || e.button !== 1) return;
  e.preventDefault();
  const r = rows[Number(item.dataset.i)];
  if (r?.kind === "tab") invoke("tab_close", { id: r.tab.id });
  else openRow(r, true);
});
list.addEventListener("click", (e) => {
  if (dragged) return;
  const item = (e.target as HTMLElement).closest<HTMLElement>(".item");
  if (!item) return;
  const r = rows[Number(item.dataset.i)];
  if (r?.kind === "tab" && (e.target as HTMLElement).closest(".x")) invoke("tab_close", { id: r.tab.id });
  else openRow(r, isMac ? e.metaKey : e.ctrlKey);
});
document.getElementById("new")!.onclick = () => invoke("tab_new", {});

// ---------------------------------------------------------------- search

q.addEventListener("input", () => {
  query = q.value.trim();
  sel = query ? 0 : -1;
  render();
});
q.addEventListener("keydown", (e) => {
  if (e.key === "ArrowDown" || e.key === "ArrowUp") {
    e.preventDefault();
    if (!rows.length) return;
    sel = (Math.max(sel, e.key === "ArrowDown" ? -1 : 0) + (e.key === "ArrowDown" ? 1 : rows.length - 1)) % rows.length;
    render();
    list.querySelector(".item.sel")?.scrollIntoView({ block: "nearest" });
  } else if (e.key === "Enter") {
    openRow(rows[Math.max(sel, 0)], isMac ? e.metaKey : e.ctrlKey);
  } else if (e.key === "Escape") {
    q.value = "";
    query = "";
    sel = -1;
    render();
  }
});

// ---------------------------------------------------------------- drag tabs to reorder
// (open tabs only, and not while filtering: the order would be partial)

let dragged = false;
list.addEventListener("pointerdown", (e) => {
  const item = (e.target as HTMLElement).closest<HTMLElement>(".item");
  if (mode !== "tabs" || !item || e.button !== 0 || query || (e.target as HTMLElement).closest(".x")) return;
  const id = Number(item.dataset.id);
  const startY = e.clientY;
  let moving = false;
  let to = -1;
  dragged = false;
  const items = () => [...list.querySelectorAll<HTMLElement>(".item")];
  const move = (m: PointerEvent) => {
    if (!moving) {
      if (Math.abs(m.clientY - startY) < 5) return;
      moving = true;
      item.classList.add("lifted");
      document.body.classList.add("dragging");
    }
    const all = items();
    to = all.findIndex((el) => m.clientY < el.getBoundingClientRect().top + el.offsetHeight / 2);
    if (to < 0) to = all.length;
    all.forEach((el, i) => el.classList.toggle("drop-before", i === to && el !== item));
    list.classList.toggle("drop-end", to === all.length);
    const r = list.getBoundingClientRect();
    if (m.clientY < r.top + 24) list.scrollTop -= 10;
    else if (m.clientY > r.bottom - 24) list.scrollTop += 10;
  };
  const up = () => {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", up);
    if (!moving) return;
    dragged = true;
    setTimeout(() => (dragged = false), 0);
    item.classList.remove("lifted");
    document.body.classList.remove("dragging");
    items().forEach((el) => el.classList.remove("drop-before"));
    list.classList.remove("drop-end");
    const from = state.tabs.findIndex((t) => t.id === id);
    const index = Math.max(0, Math.min(state.tabs.length - 1, to > from ? to - 1 : to));
    if (to >= 0 && index !== from) invoke("tab_move", { id, to: index });
  };
  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", up);
});

// ---------------------------------------------------------------- width: drag the right edge

const grip = document.getElementById("grip")!;
grip.addEventListener("pointerdown", (e) => {
  e.preventDefault();
  grip.setPointerCapture(e.pointerId);
  const sx = e.screenX;
  const w0 = window.innerWidth;
  document.body.classList.add("resizing");
  let last = w0;
  grip.onpointermove = (m) => {
    last = w0 + m.screenX - sx;
    invoke("side_resize", { width: last, save: false });
  };
  grip.onpointerup = grip.onpointercancel = () => {
    grip.onpointermove = grip.onpointerup = grip.onpointercancel = null;
    document.body.classList.remove("resizing");
    invoke("side_resize", { width: last, save: true });
  };
});
grip.ondblclick = () => invoke("side_resize", { width: 260, save: true });

async function main() {
  await wv.listen<BrowserState>("browser-state", (e) => {
    state = e.payload;
    if (mode === "tabs") render();
    else document.getElementById("n-tabs")!.textContent = String(state.tabs.length);
  });
  await wv.listen("bookmarks-changed", () => {
    if (bookmarksLoaded || mode === "bookmarks") loadBookmarks();
  });
  const init = await invoke<Init>("ui_init");
  document.documentElement.dataset.platform = init.platform;
  state = init.state;
  render();
  loadBookmarks();
}
main();
