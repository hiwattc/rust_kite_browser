// Browser chrome: tab strip + toolbar. Rust owns the state; this renders `browser-state`
// and sends commands.
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
// Only events sent to this webview (a plain `listen` also gets those for other windows).
const wv = getCurrentWebview();
const listen: typeof wv.listen = (...a) => wv.listen(...a);
import { combo } from "../shared/keys";
import { icons } from "../shared/icons";
import { NEWTAB, type BarItem, type BrowserState, type Init, type Tab, type ToolbarExt } from "../shared/types";
import "./chrome.css";
import { initTheme } from "../shared/theme";

initTheme();

const $ = <T extends HTMLElement>(sel: string) => document.querySelector(sel) as T;
const warnIcon = `<svg class="icon" viewBox="0 0 24 24"><path d="M12 3.5l9.5 16.5h-19z"/><path d="M12 10v4.5M12 17.3v.2"/></svg>`;
const infoIcon = `<svg class="icon" viewBox="0 0 24 24"><circle cx="12" cy="12" r="8.5"/><path d="M12 11v5.5M12 7.8v.2"/></svg>`;

document.body.innerHTML = `
  <div class="strip" data-tauri-drag-region>
    <div class="traffic" data-tauri-drag-region></div>
    <div class="tabs" id="tabs"></div>
    <button class="strip-btn" id="new-tab" title="새 탭">${icons.plus}</button>
    <div class="strip-fill" data-tauri-drag-region></div>
  </div>
  <div class="toolbar">
    <button class="tb" id="side" title="세로 탭 목록 (탭이 많을 때 · 검색)">${icons.sidebar}</button>
    <button class="tb" id="back" title="뒤로">${icons.back}</button>
    <button class="tb" id="forward" title="앞으로">${icons.forward}</button>
    <button class="tb" id="reload" title="새로고침">${icons.reload}</button>
    <form class="omni" id="omni" autocomplete="off">
      <span class="omni-icon" id="omni-icon">${icons.search}</span>
      <input id="address" spellcheck="false" placeholder="주소 입력 또는 검색 · Ctrl+Shift+P 스포트라이트" />
      <span class="zoom" id="zoom" hidden></span>
      <button type="button" class="omni-btn" id="star" title="북마크">${icons.star}</button>
    </form>
    <div class="exts" id="exts" hidden></div>
    <div class="toast" id="toast" hidden></div>
    <div class="agent" id="agent" hidden title="AI 에이전트가 이 브라우저를 조작하고 있습니다">${icons.bot}<span id="agent-text">에이전트</span></div>
    <button class="mcp" id="mcp" title="MCP 서버 (클릭: Claude Code 연결 명령 복사)"><span class="dot"></span>MCP</button>
    <button class="tb" id="home" title="홈 (개인 포털)">${icons.home}</button>
    <button class="tb" id="settings" title="설정 (${navigator.platform.toLowerCase().includes("mac") ? "⌘," : "Ctrl+,"})">${icons.settings}</button>
    <button class="tb" id="menu" title="메뉴: 설정·북마크·명령 (Ctrl+Shift+P → >)">${icons.menu}</button>
  </div>
  <div class="bbar" id="bbar" hidden></div>
`;

let state: BrowserState = { tabs: [], active: null, agentTab: null, mcp: { running: false, port: 0, url: "" }, side: false };
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
  // while a tab slides in this strip, endDrag() renders the latest state (once torn off, the
  // strip may update: the tab left it)
  if (drag?.el && !drag.torn) return;
  tabsEl.replaceChildren(...state.tabs.map(tabEl));
  const t = activeTab();
  if (!editing) address.value = shownUrl(t);
  const secure = t?.url.startsWith("https://") && !t.error && !t.badCert;
  // The icon before the address: lock (https), warning (http), info (Kite pages); a click opens
  // the site info bubble (siteinfo.rs).
  const insecure = !!t && (t.url.startsWith("http://") || !!t.error || !!t.badCert);
  const internal = !!t && t.url.startsWith("kite://") && t.url !== NEWTAB;
  const icon = $("#omni-icon");
  icon.innerHTML = secure ? icons.lock : insecure ? warnIcon : internal ? infoIcon : icons.search;
  icon.classList.toggle("clickable", secure || insecure || internal);
  icon.classList.toggle("insecure", insecure);
  icon.title = secure ? "사이트 정보 · 연결이 안전합니다" : t?.error ? "연결할 수 없음" : t?.badCert ? "사이트 정보 · 인증서가 올바르지 않음 (계속 진행함)" : insecure ? "사이트 정보 · 안전하지 않음" : internal ? "Kite 내부 페이지" : "";
  $("#star").classList.toggle("on", !!t?.bookmarked);
  $("#star").toggleAttribute("disabled", !t || !t.url.startsWith("http"));
  $("#reload").innerHTML = t?.loading ? icons.stop : icons.reload;
  $("#reload").title = t?.loading ? "중지" : "새로고침";
  const zoom = $("#zoom");
  zoom.hidden = !t || Math.abs(t.zoom - 1) < 0.001;
  if (t) zoom.textContent = `${Math.round(t.zoom * 100)}%`;
  $("#settings").classList.toggle("on", !!t?.url.startsWith("kite://settings"));
  $("#side").classList.toggle("on", !!state.side);
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
// Drag to reorder (pointer events — HTML5 drag-and-drop is unreliable in the webview, and the
// strip re-renders on every state change). The dragged tab follows the pointer, the others slide
// out of its way, and on release Rust gets the final index. Renders wait until the drag ends.
// Pulled well away from the strip (below it, or out of the window) the tab "tears off", like
// Chrome: it moves into a new see-through window that follows the pointer (the only tab of a
// window takes its whole window along at once). Let go over another Kite window's strip, it goes
// into that window; anywhere else it stays in its own window. Rust moves the window
// (`tab_drag_*`); the pointer stays captured here, so moves keep coming from this strip.
type Drag = {
  id: number; x0: number; y0: number; el?: HTMLElement; others: HTMLElement[]; from: number; to: number; slot: number; mids: number[]; mid0: number;
  /** pointer's offset inside the dragged tab at the start */ grab: number; firstLeft: number;
  torn?: Promise<unknown>; sx: number; sy: number; frame: number;
};
const TEAR = 36; // px beyond the strip before a tab tears off
let drag: Drag | null = null;

tabsEl.addEventListener("pointerdown", (e) => {
  const el = (e.target as HTMLElement).closest<HTMLElement>(".tab");
  if (!el || e.button !== 0 || (e.target as HTMLElement).closest(".tab-close")) return;
  drag = { id: Number(el.dataset.id), x0: e.clientX, y0: e.clientY, others: [], from: -1, to: -1, slot: 0, mids: [], mid0: 0, grab: e.clientX - el.getBoundingClientRect().left, firstLeft: 0, sx: 0, sy: 0, frame: 0 };
  try {
    tabsEl.setPointerCapture(e.pointerId); // keep getting moves when the pointer leaves the strip
  } catch {
    /* synthetic events */
  }
});

function startDrag(d: Drag) {
  // the strip may have re-rendered since pointerdown (activation), so look the tab up again
  const els = [...tabsEl.querySelectorAll<HTMLElement>(".tab")];
  const from = els.findIndex((x) => Number(x.dataset.id) === d.id);
  if (from < 0) return false;
  const rects = els.map((x) => x.getBoundingClientRect());
  d.el = els[from];
  d.from = d.to = from;
  d.slot = els.length > 1 ? rects[1].left - rects[0].left : rects[0].width;
  d.mids = rects.map((r) => r.left + r.width / 2);
  d.mid0 = d.mids[from];
  d.firstLeft = rects[0].left;
  d.others = els;
  tabsEl.classList.add("dragging");
  d.el.classList.add("lifted");
  return true;
}

tabsEl.addEventListener("pointermove", (e) => {
  const d = drag;
  if (!d) return;
  const dx = e.clientX - d.x0;
  const dy = e.clientY - d.y0;
  if (!d.el) {
    if (Math.max(Math.abs(dx), Math.abs(dy)) < 5 || !startDrag(d)) return;
  }
  // screen point (CSS px of the screen = logical points, what Rust positions windows with)
  d.sx = e.screenX;
  d.sy = e.screenY;
  if (d.torn) {
    // one window move per frame
    if (!d.frame) {
      d.frame = requestAnimationFrame(() => {
        d.frame = 0;
        if (drag === d) invoke("tab_drag_move", { x: d.sx, y: d.sy });
      });
    }
    return;
  }
  const strip = tabsEl.getBoundingClientRect();
  const alone = d.others.length === 1;
  const out = alone || e.clientY > strip.bottom + TEAR || e.clientY < -TEAR || e.clientX < -TEAR || e.clientX > window.innerWidth + TEAR;
  if (out) {
    // tear off: the tab leaves this strip (the others close the gap) and its window follows
    d.el!.style.transform = "";
    if (!alone) {
      d.el!.style.visibility = "hidden";
      d.others.forEach((x, i) => {
        if (i !== d.from) x.style.transform = i > d.from ? `translateX(${-d.slot}px)` : "";
      });
    }
    // where the pointer sits in the moving window: its own grab point when the whole window
    // goes; on the (single, first) tab of the new window otherwise
    const [ox, oy] = alone ? [d.x0, d.y0] : [d.firstLeft + Math.min(d.grab, 200), d.y0];
    d.torn = invoke("tab_drag_start", { id: d.id, x: d.sx, y: d.sy, ox, oy }).catch(() => undefined);
    return;
  }
  const n = d.others.length;
  const first = d.mids[0] - d.mid0, last = d.mids[n - 1] - d.mid0;
  // can go one pixel past the end tabs' centres so it can take the first/last slot
  const off = Math.max(first - 1, Math.min(last + 1, dx));
  d.el!.style.transform = `translateX(${off}px)`;
  const mid = d.mid0 + off;
  let to = 0;
  d.mids.forEach((m, i) => {
    if (i !== d.from && m < mid) to++;
  });
  if (to === d.to) return;
  d.to = to;
  d.others.forEach((x, i) => {
    if (i === d.from) return;
    const shift = d.from < to && i > d.from && i <= to ? -d.slot : d.from > to && i >= to && i < d.from ? d.slot : 0;
    x.style.transform = shift ? `translateX(${shift}px)` : "";
  });
});

function endDrag(e: PointerEvent) {
  const d = drag;
  drag = null;
  if (!d?.el) return;
  tabsEl.classList.remove("dragging");
  if (d.frame) cancelAnimationFrame(d.frame);
  if (d.torn) {
    const [x, y] = e.type === "pointerup" ? [e.screenX, e.screenY] : [d.sx, d.sy];
    d.torn.then(() => invoke("tab_drag_drop", { x, y })).finally(render);
    return;
  } else if (d.to !== d.from && d.to >= 0) {
    // apply locally so there is no snap-back flash before Rust's state arrives
    const [t] = state.tabs.splice(d.from, 1);
    state.tabs.splice(d.to, 0, t);
    invoke("tab_move", { id: d.id, to: d.to });
  }
  render();
}
tabsEl.addEventListener("pointerup", endDrag);
tabsEl.addEventListener("pointercancel", endDrag);

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
// Right click (bookmark bar, toolbar, tab strip): show/hide the bookmark bar and the tab list.
// Text fields keep their own menu (copy, paste).
document.addEventListener("contextmenu", (e) => {
  const t = e.target as HTMLElement;
  if (t.closest("input, textarea, [contenteditable]")) return;
  e.preventDefault();
  invoke("ui_menu", { x: e.clientX, y: e.clientY }).catch((err) => toast(String(err)));
});
$("#omni-icon").addEventListener("mousedown", (e) => {
  const icon = e.currentTarget as HTMLElement;
  if (!icon.classList.contains("clickable")) return;
  e.preventDefault();
  const r = icon.getBoundingClientRect();
  invoke("site_info_open", { x: Math.round(r.left), y: Math.round(r.bottom) }).catch((err) => toast(String(err)));
});
$("#side").onclick = () => invoke("shortcut", { action: "toggle_side_tabs" }).catch((err) => toast(String(err)));
$("#home").onclick = () => invoke("shortcut", { action: "open_home" }).catch((err) => toast(String(err)));
$("#settings").onclick = () => invoke("shortcut", { action: "open_settings" }).catch((err) => toast(String(err)));
$("#mcp").onclick = () => invoke("shortcut", { action: "mcp_copy" });

// ---- address bar: inline completion + suggestion list (Chrome's omnibox) ----
// Rust (`omni_suggest`) finds matches in history, bookmarks and popular sites. The typed text is
// completed in place with the added part selected (you → you|tube.com), so typing on replaces
// it and Enter takes it; the list is a separate overlay webview (`omni`) under the bar.
type Suggestion = { kind: string; url: string; title: string; fill: string };
let sugs: Suggestion[] = [];
let sel = 0;
let typed = "";
let seq = 0;
let listShown = false;
let blurTimer = 0;

function hideList() {
  sugs = [];
  sel = 0;
  if (listShown) {
    listShown = false;
    invoke("omni_hide");
  }
}
function showList() {
  if (!sugs.length || !editing) return hideList();
  const r = $("#omni").getBoundingClientRect();
  listShown = true;
  invoke("omni_show", { x: Math.round(r.left), y: Math.round(r.bottom + 4), width: Math.round(r.width), items: sugs, selected: sel });
}
async function suggest(allowInline: boolean) {
  const my = ++seq;
  const q = typed;
  if (!q.trim()) return hideList();
  const r = await invoke<{ inline?: string | null; items: Suggestion[] }>("omni_suggest", { input: q }).catch(() => null);
  if (!r || my !== seq || !editing || address.value !== q) return;
  const caretAtEnd = address.selectionStart === q.length && address.selectionEnd === q.length;
  let items = r.items;
  if (allowInline && caretAtEnd && r.inline && r.inline.length > q.length && r.inline.toLowerCase().startsWith(q.toLowerCase())) {
    address.value = r.inline;
    address.setSelectionRange(q.length, r.inline.length);
  } else if (r.inline && items[0]?.kind === "url" && items[0].fill !== q && items[items.length - 1]?.kind === "search") {
    // Completion not shown (deleting, typing Korean…): Enter searches what was typed, so that
    // row comes first.
    items = [items[items.length - 1], ...items.slice(0, -1)];
  }
  sugs = items;
  sel = 0;
  showList();
}

address.addEventListener("focus", () => {
  clearTimeout(blurTimer);
  editing = true;
  setTimeout(() => address.select(), 0);
});
address.addEventListener("blur", () => {
  editing = false;
  address.value = shownUrl(activeTab());
  // Later: a press in the list blurs the bar first, and the list handles that press itself.
  blurTimer = window.setTimeout(hideList, 150);
});
address.addEventListener("input", (e) => {
  const ie = e as InputEvent;
  typed = address.value;
  const deleting = (ie.inputType || "").startsWith("delete");
  const composing = ie.isComposing || ie.inputType === "insertCompositionText";
  suggest(!deleting && !composing);
});
address.addEventListener("keydown", (e) => {
  if (e.isComposing) return;
  if (e.key === "Escape") {
    if (listShown) {
      // First Esc: drop the completion and the list; second: back to the page's address.
      address.value = typed;
      hideList();
      return;
    }
    address.value = shownUrl(activeTab());
    address.blur();
    invoke("tab_activate", { id: state.active });
    return;
  }
  if (listShown && (e.key === "ArrowDown" || e.key === "ArrowUp")) {
    e.preventDefault();
    const n = sugs.length;
    sel = (sel + (e.key === "ArrowDown" ? 1 : n - 1)) % n;
    address.value = sel === 0 && sugs[0].kind === "search" ? typed : sugs[sel].fill;
    showList();
  }
});
$<HTMLFormElement>("#omni").addEventListener("submit", (e) => {
  e.preventDefault();
  const picked = sel > 0 ? sugs[sel] : undefined;
  const input = picked ? picked.url : address.value.trim();
  if (!input) return;
  hideList();
  editing = false;
  address.blur();
  invoke("navigate", { id: state.active, input }).catch((err) => toast(String(err)));
});
window.addEventListener("resize", () => {
  if (listShown) showList();
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

// ---- extension buttons (like Chrome's pinned extensions): click = the popup under the icon ----
function renderExts(list: ToolbarExt[]) {
  const box = $("#exts");
  box.hidden = !list.length;
  box.innerHTML = list
    .map(
      (x) =>
        `<button class="ext" data-id="${escAttr(x.id)}" title="${escAttr(x.name)}">${x.icon ? `<img src="${escAttr(x.icon)}" alt="" draggable="false" />` : icons.puzzle}</button>`,
    )
    .join("");
}
$("#exts").addEventListener("click", (e) => {
  const btn = (e.target as HTMLElement).closest<HTMLElement>(".ext");
  if (!btn) return;
  const r = btn.getBoundingClientRect();
  invoke("ext_popup", { id: btn.dataset.id, right: Math.round(r.right), bottom: Math.round(r.bottom) }).catch((err) => toast(String(err)));
});

async function main() {
  await listen<BrowserState>("browser-state", (e) => {
    state = e.payload;
    render();
  });
  await listen<string>("toast", (e) => toast(e.payload));
  await listen("omni-picked", () => {
    hideList();
    address.blur();
  });
  await listen("focus-address", () => {
    address.focus();
    address.select();
  });
  await listen<{ tabId: number; tool: string }>("agent-activity", (e) => agentActivity(e.payload.tool));
  await listen<BarItem[] | null>("bookmark-bar", (e) => renderBar(e.payload));
  await listen<ToolbarExt[]>("extensions", (e) => renderExts(e.payload));
  const init = await invoke<Init>("ui_init");
  keymap = init.keymap;
  document.documentElement.dataset.platform = init.platform;
  state = init.state;
  render();
  renderBar(init.bookmarkBar);
  renderExts(init.extensions ?? []);
}

main();
