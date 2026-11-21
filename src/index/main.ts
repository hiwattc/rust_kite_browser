// Browser chrome: tab strip + toolbar. Rust owns the state; this renders `browser-state`
// and sends commands.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { combo } from "../shared/keys";
import { icons } from "../shared/icons";
import { NEWTAB, type BarItem, type BrowserState, type Init, type Tab } from "../shared/types";
import "./chrome.css";

const $ = <T extends HTMLElement>(sel: string) => document.querySelector(sel) as T;

document.body.innerHTML = `
  <div class="strip" data-tauri-drag-region>
    <div class="traffic" data-tauri-drag-region></div>
    <div class="tabs" id="tabs"></div>
    <button class="strip-btn" id="new-tab" title="새 탭">${icons.plus}</button>
    <div class="strip-fill" data-tauri-drag-region></div>
  </div>
  <div class="toolbar">
    <button class="tb" id="back" title="뒤로">${icons.back}</button>
    <button class="tb" id="forward" title="앞으로">${icons.forward}</button>
    <button class="tb" id="reload" title="새로고침">${icons.reload}</button>
    <form class="omni" id="omni" autocomplete="off">
      <span class="omni-icon" id="omni-icon">${icons.search}</span>
      <input id="address" spellcheck="false" placeholder="주소 입력 또는 검색 · Ctrl+Shift+P 스포트라이트" />
      <span class="zoom" id="zoom" hidden></span>
      <button type="button" class="omni-btn" id="star" title="북마크">${icons.star}</button>
    </form>
    <div class="toast" id="toast" hidden></div>
    <div class="agent" id="agent" hidden title="AI 에이전트가 이 브라우저를 조작하고 있습니다">${icons.bot}<span id="agent-text">에이전트</span></div>
    <button class="mcp" id="mcp" title="MCP 서버 (클릭: Claude Code 연결 명령 복사)"><span class="dot"></span>MCP</button>
    <button class="tb" id="menu" title="메뉴: 설정·북마크·명령 (Ctrl+Shift+P → >)">${icons.menu}</button>
  </div>
  <div class="bbar" id="bbar" hidden></div>
`;

let state: BrowserState = { tabs: [], active: null, agentTab: null, mcp: { running: false, port: 0, url: "" } };
let keymap: Record<string, string> = {};
let editing = false;

const tabsEl = $("#tabs");
const address = $<HTMLInputElement>("#address");

function activeTab(): Tab | undefined {
  return state.tabs.find((t) => t.id === state.active);
}

/** Readable URL for the address bar: %EC%84%9C… → 서…, spaces stay %20 (so Enter re-opens it). */
function shownUrl(t?: Tab) {
  if (!t || t.url === NEWTAB) return "";
  try {
    return decodeURI(t.url).replace(/ /g, "%20");
  } catch {
    return t.url;
  }
}

function host(u: string) {
  try {
    return new URL(u).host;
  } catch {
    return u;
  }
}

function tabEl(t: Tab): HTMLElement {
  const el = document.createElement("div");
  el.className = "tab";
  el.dataset.id = String(t.id);
  el.draggable = true;
  const title = t.title || (t.url === NEWTAB ? "새 탭" : host(t.url));
  el.title = `${title}\n${shownUrl(t)}`;
  const fav = t.loading
    ? `<span class="spinner"></span>`
    : t.favicon
      ? `<img class="fav" src="${t.favicon.replace(/"/g, "&quot;")}" alt="" />`
      : `<span class="fav-ph">${icons.globe}</span>`;
  el.innerHTML = `${fav}<span class="title"></span>${t.id === state.agentTab ? `<span class="tab-bot" title="에이전트 대상 탭">${icons.bot}</span>` : ""}<button class="tab-close" title="탭 닫기">${icons.close}</button>`;
  (el.querySelector(".title") as HTMLElement).textContent = title;
  const img = el.querySelector("img");
  if (img) img.onerror = () => img.replaceWith(Object.assign(document.createElement("span"), { className: "fav-ph", innerHTML: icons.globe }));
  el.classList.toggle("active", t.id === state.active);
  el.classList.toggle("agent-tab", t.id === state.agentTab);
  return el;
}

function render() {
  tabsEl.replaceChildren(...state.tabs.map(tabEl));
  const t = activeTab();
  if (!editing) address.value = shownUrl(t);
  const secure = t?.url.startsWith("https://");
  $("#omni-icon").innerHTML = secure ? icons.lock : icons.search;
  $("#star").classList.toggle("on", !!t?.bookmarked);
  $("#star").toggleAttribute("disabled", !t || !t.url.startsWith("http"));
  $("#reload").innerHTML = t?.loading ? icons.stop : icons.reload;
  $("#reload").title = t?.loading ? "중지" : "새로고침";
  const zoom = $("#zoom");
  zoom.hidden = !t || Math.abs(t.zoom - 1) < 0.001;
  if (t) zoom.textContent = `${Math.round(t.zoom * 100)}%`;
  $("#mcp").classList.toggle("on", state.mcp.running);
  $("#mcp").title = state.mcp.running ? `MCP 서버 실행 중: ${state.mcp.url}\n클릭: Claude Code 연결 명령 복사` : "MCP 서버 꺼짐 (클릭: 연결 명령 복사)";
  document.title = t?.title ? `${t.title} - Kite` : "Kite";
}

// ---- tab strip ----
tabsEl.addEventListener("mousedown", (e) => {
  const el = (e.target as HTMLElement).closest<HTMLElement>(".tab");
  if (!el) return;
  const id = Number(el.dataset.id);
  if (e.button === 1) {
    e.preventDefault();
    invoke("tab_close", { id });
  } else if (e.button === 0 && !(e.target as HTMLElement).closest(".tab-close")) {
    invoke("tab_activate", { id });
  }
});
tabsEl.addEventListener("click", (e) => {
  const close = (e.target as HTMLElement).closest(".tab-close");
  const el = (e.target as HTMLElement).closest<HTMLElement>(".tab");
  if (close && el) invoke("tab_close", { id: Number(el.dataset.id) });
});
let dragId: number | null = null;
tabsEl.addEventListener("dragstart", (e) => {
  const el = (e.target as HTMLElement).closest<HTMLElement>(".tab");
  dragId = el ? Number(el.dataset.id) : null;
  e.dataTransfer?.setData("text/plain", String(dragId));
});
tabsEl.addEventListener("dragover", (e) => e.preventDefault());
tabsEl.addEventListener("drop", (e) => {
  e.preventDefault();
  if (dragId == null) return;
  const els = [...tabsEl.querySelectorAll<HTMLElement>(".tab")];
  let to = els.findIndex((x) => {
    const r = x.getBoundingClientRect();
    return e.clientX < r.left + r.width / 2;
  });
  if (to < 0) to = els.length;
  const from = state.tabs.findIndex((t) => t.id === dragId);
  if (from >= 0 && from < to) to -= 1;
  invoke("tab_move", { id: dragId, to });
  dragId = null;
});
$("#new-tab").onclick = () => invoke("shortcut", { action: "new_tab" });
$(".strip-fill").addEventListener("dblclick", () => {
  if (keymap && !isMac) invoke("shortcut", { action: "new_tab" });
});

// ---- toolbar ----
$("#back").onclick = () => invoke("shortcut", { action: "back" });
$("#forward").onclick = () => invoke("shortcut", { action: "forward" });
$("#reload").onclick = () => invoke("shortcut", { action: activeTab()?.loading ? "stop" : "reload" });
$("#star").onclick = () => invoke("shortcut", { action: "bookmark" });
$("#menu").onclick = () => invoke("palette_open", { mode: ">", query: "" });
$("#mcp").onclick = () => invoke("shortcut", { action: "mcp_copy" });

address.addEventListener("focus", () => {
  editing = true;
  setTimeout(() => address.select(), 0);
});
address.addEventListener("blur", () => {
  editing = false;
  address.value = shownUrl(activeTab());
});
address.addEventListener("keydown", (e) => {
  if (e.key === "Escape") {
    address.value = shownUrl(activeTab());
    address.blur();
    invoke("tab_activate", { id: state.active });
  }
});
$<HTMLFormElement>("#omni").addEventListener("submit", (e) => {
  e.preventDefault();
  const input = address.value.trim();
  if (!input) return;
  editing = false;
  address.blur();
  invoke("navigate", { id: state.active, input }).catch((err) => toast(String(err)));
});

// ---- shortcuts (same table as the content tabs) ----
const isMac = navigator.platform.toLowerCase().includes("mac");
window.addEventListener(
  "keydown",
  (e) => {
    const action = keymap[combo(e)];
    if (!action) return;
    // Let the address field keep its own editing keys.
    if (document.activeElement === address && ["zoom_in", "zoom_out", "zoom_reset"].includes(action)) return;
    e.preventDefault();
    e.stopPropagation();
    invoke("shortcut", { action }).catch((err) => toast(String(err)));
  },
  true,
);

// ---- bookmark bar ----
const bbar = $("#bbar");
const escAttr = (s: string) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);
function favicon(u: string) {
  try {
    const x = new URL(u);
    return /^https?:$/.test(x.protocol) ? `${x.origin}/favicon.ico` : "";
  } catch {
    return "";
  }
}
function renderBar(items: BarItem[] | null) {
  bbar.hidden = !items;
  if (items) {
    const html = items
      .map((it, i) => {
        const icon =
          it.kind === "folder"
            ? `<span class="bb-ic">${icons.folder}</span>`
            : favicon(it.url)
              ? `<img class="bb-ic" src="${escAttr(favicon(it.url))}" alt="" />`
              : `<span class="bb-ic">${icons.globe}</span>`;
        return `<button class="bb" data-i="${i}" title="${escAttr(it.kind === "folder" ? `폴더: ${it.title}` : `${it.title}\n${it.url}`)}">${icon}<span>${escAttr(it.title)}</span></button>`;
      })
      .join("");
    bbar.innerHTML = `<div class="bb-items">${html || `<span class="bb-empty">북마크 바가 비어 있습니다. ☆ 또는 ${isMac ? "⌘" : "Ctrl+"}D로 추가하세요.</span>`}</div><button class="bb bb-more" id="bb-more" title="나머지 북마크 (스포트라이트에서 검색)" hidden>»</button><button class="bb bb-all" id="bb-all">${icons.bookmark}<span>모든 북마크</span></button>`;
    bbar.querySelectorAll<HTMLImageElement>("img.bb-ic").forEach((img) => {
      img.onerror = () => img.replaceWith(Object.assign(document.createElement("span"), { className: "bb-ic", innerHTML: icons.globe }));
    });
  }
  barItems = items ?? [];
  syncHeight();
  markOverflow();
}
/** Items that do not fit are hidden whole, and » opens the rest in the spotlight. */
function markOverflow() {
  const box = bbar.querySelector<HTMLElement>(".bb-items");
  const more = document.getElementById("bb-more");
  if (!box || !more) return;
  const right = box.getBoundingClientRect().right;
  let hidden = 0;
  box.querySelectorAll<HTMLElement>(".bb").forEach((b) => {
    b.style.visibility = "";
    if (b.getBoundingClientRect().right > right + 0.5) {
      b.style.visibility = "hidden";
      hidden++;
    }
  });
  more.hidden = hidden === 0;
}
window.addEventListener("resize", markOverflow);
let barItems: BarItem[] = [];
bbar.addEventListener("click", (e) => openBarItem(e as MouseEvent, false));
bbar.addEventListener("auxclick", (e) => {
  if ((e as MouseEvent).button === 1) openBarItem(e as MouseEvent, true);
});
function openBarItem(e: MouseEvent, middle: boolean) {
  const btn = (e.target as HTMLElement).closest<HTMLElement>(".bb");
  if (!btn) return;
  if (btn.id === "bb-more") {
    invoke("palette_open", { mode: "*", query: "" });
    return;
  }
  if (btn.id === "bb-all") {
    invoke("shortcut", { action: "open_bookmarks" });
    return;
  }
  const it = barItems[Number(btn.dataset.i)];
  if (!it) return;
  if (it.kind === "folder") {
    invoke("palette_open", { mode: "*", query: `/${it.url}` });
  } else if (middle || (isMac ? e.metaKey : e.ctrlKey)) {
    invoke("tab_new", { url: it.url });
  } else {
    invoke("navigate", { id: state.active, input: it.url }).catch((err) => toast(String(err)));
  }
}
function syncHeight() {
  const h = Math.ceil(document.body.getBoundingClientRect().height);
  invoke("set_chrome_height", { height: h });
}

// ---- toast + agent activity ----
let toastTimer = 0;
function toast(msg: string) {
  const el = $("#toast");
  el.textContent = msg;
  el.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => (el.hidden = true), 3200);
}
let agentTimer = 0;
function agentActivity(tool: string) {
  const el = $("#agent");
  $("#agent-text").textContent = tool.replace(/^browser_/, "");
  el.hidden = false;
  el.classList.remove("pulse");
  void el.offsetWidth;
  el.classList.add("pulse");
  clearTimeout(agentTimer);
  agentTimer = window.setTimeout(() => (el.hidden = true), 4000);
}

async function main() {
  await listen<BrowserState>("browser-state", (e) => {
    state = e.payload;
    render();
  });
  await listen<string>("toast", (e) => toast(e.payload));
  await listen("focus-address", () => {
    address.focus();
    address.select();
  });
  await listen<{ tabId: number; tool: string }>("agent-activity", (e) => agentActivity(e.payload.tool));
  await listen<BarItem[] | null>("bookmark-bar", (e) => renderBar(e.payload));
  const init = await invoke<Init>("ui_init");
  keymap = init.keymap;
  document.documentElement.dataset.platform = init.platform;
  state = init.state;
  render();
  renderBar(init.bookmarkBar);
}

main();
