// kite://home — a personal start page (like iGoogle was). Each workspace is a free grid
// (12 columns × row units) of sections: app icons (rounded squares like a phone's home screen)
// or a to-do list. In edit mode sections are added, renamed, deleted, moved (drag the header)
// and resized (drag the corner); icons are reordered or moved between sections by dragging.
// The data is kept by Kite (`src-tauri/src/portal.rs`) as one JSON document.
import { call, esc, flash } from "../shared/page";
import { icons } from "../shared/icons";
import { initTheme } from "../shared/theme";
import "./home.css";

initTheme();
document.title = "홈";

interface App {
  id: string;
  title: string;
  url: string;
  /** Picture (data URL). `""` = a letter on a color; null/missing = the site's icon, not fetched yet. */
  icon?: string | null;
  /** The picture is a file the user picked (not the site's icon). */
  uploaded?: boolean;
  letter?: string;
  color?: number;
}
interface Todo {
  id: string;
  text: string;
  done: boolean;
  at: number;
}
interface Section {
  id: string;
  kind: "apps" | "todo";
  title: string;
  /** Columns (1–12) and rows (row units) it spans. */
  w: number;
  h: number;
  apps?: App[];
  todos?: Todo[];
}
interface Workspace {
  id: string;
  name: string;
  sections: Section[];
}
interface Portal {
  version: 2;
  active: string;
  workspaces: Workspace[];
}

const COLS = 12;
const ROW = 56; // px per row unit (grid-auto-rows)
const GAP = 18;
const MAX_H = 24;

const COLORS = [
  ["#6d4aff", "#a78bfa"],
  ["#0ea5e9", "#67e8f9"],
  ["#10b981", "#6ee7b7"],
  ["#f59e0b", "#fcd34d"],
  ["#ef4444", "#fca5a5"],
  ["#ec4899", "#f9a8d4"],
  ["#334155", "#94a3b8"],
  ["#0f766e", "#2dd4bf"],
];
const DEFAULT_SITES = [
  { title: "네이버", url: "https://www.naver.com/" },
  { title: "YouTube", url: "https://www.youtube.com/" },
  { title: "Google", url: "https://www.google.com/" },
  { title: "Gmail", url: "https://mail.google.com/" },
  { title: "GitHub", url: "https://github.com/" },
  { title: "나무위키", url: "https://namu.wiki/" },
];

const uid = () => Math.random().toString(36).slice(2, 10);
const host = (u: string) => {
  try {
    return new URL(u).host.replace(/^www\./, "");
  } catch {
    return u;
  }
};
const colorOf = (s: string) => [...s].reduce((a, c) => a + c.charCodeAt(0), 0) % COLORS.length;
const app = (title: string, url: string): App => ({ id: uid(), title, url, icon: null, letter: (title.trim()[0] ?? "?").toUpperCase(), color: colorOf(host(url)) });
const clamp = (v: number, lo: number, hi: number) => Math.max(lo, Math.min(hi, Math.round(v)));
const appsSection = (apps: App[] = [], title = "자주 가는 곳", w = 8, h = 6): Section => ({ id: uid(), kind: "apps", title, w, h, apps });
const todoSection = (todos: Todo[] = [], title = "할 일", w = 4, h = 6): Section => ({ id: uid(), kind: "todo", title, w, h, todos });

let data: Portal = { version: 2, active: "", workspaces: [] };
let editing = false;
const todoFilter = new Map<string, "all" | "open" | "done">();

const ws = () => data.workspaces.find((w) => w.id === data.active) ?? data.workspaces[0];
const sec = (id: string | undefined) => ws().sections.find((s) => s.id === id);
const allApps = () => data.workspaces.flatMap((w) => w.sections.flatMap((s) => s.apps ?? []));

/** Any workspace (older files had `apps` + `todos` directly) → sections. */
function normalize(raw: Partial<Workspace> & { apps?: App[]; todos?: Todo[] }): Workspace {
  let sections = Array.isArray(raw.sections) ? raw.sections : [appsSection(raw.apps ?? []), todoSection(raw.todos ?? [])];
  sections = sections
    .filter((s) => s && (s.kind === "apps" || s.kind === "todo"))
    .map((s) => ({
      ...s,
      id: s.id || uid(),
      title: s.title ?? (s.kind === "apps" ? "앱" : "할 일"),
      w: clamp(s.w ?? 6, 1, COLS),
      h: clamp(s.h ?? 6, 2, MAX_H),
      ...(s.kind === "apps" ? { apps: (s.apps ?? []).filter((a) => a?.url).map((a) => ({ ...a, id: a.id || uid() })) } : { todos: (s.todos ?? []).map((t) => ({ ...t, id: t.id || uid() })) }),
    }));
  return { id: raw.id || uid(), name: raw.name || "워크스페이스", sections };
}

// ---------------------------------------------------------------- saving

let saveTimer = 0;
function save() {
  clearTimeout(saveTimer);
  saveTimer = window.setTimeout(() => call("portal_set", { json: JSON.stringify(data) }), 300);
}

async function load() {
  const raw = await call<string | null>("portal_get");
  if (raw) {
    try {
      const d = JSON.parse(raw);
      data = { version: 2, active: d.active, workspaces: (d.workspaces ?? []).map(normalize) };
    } catch {
      /* fall through to a fresh start */
    }
  }
  if (!data.workspaces.length) {
    // First visit: the sites visited most, topped up with a few common ones.
    const seed = (await call<{ title: string; url: string }[]>("portal_seed")) ?? [];
    const seen = new Set(seed.map((s) => host(s.url)));
    const sites = [...seed, ...DEFAULT_SITES.filter((s) => !seen.has(host(s.url)))].slice(0, 12);
    const first: Workspace = {
      id: uid(),
      name: "홈",
      sections: [appsSection(sites.map((s) => app(s.title, s.url))), todoSection([{ id: uid(), text: "Kite 홈 꾸미기 (오른쪽 위 ‘편집’)", done: false, at: Date.now() }])],
    };
    data = { version: 2, active: first.id, workspaces: [first] };
    save();
  }
  if (!data.workspaces.some((w) => w.id === data.active)) data.active = data.workspaces[0].id;
  render();
  fetchMissingIcons();
}

/** Site icons for apps that have none yet (first run, imported files), one at a time. */
async function fetchMissingIcons() {
  for (const a of allApps()) {
    if (a.icon != null) continue;
    const icon = await call<string | null>("portal_icon", { url: a.url });
    a.icon = icon || "";
    save();
    renderBoard();
  }
}

// ---------------------------------------------------------------- page

document.body.innerHTML = `
  <header class="top">
    <nav class="spaces" id="spaces"></nav>
    <div class="tools">
      <button class="pill" id="edit">${icons.edit}<span>편집</span></button>
      <div class="menu-wrap"><button class="round" id="more" title="워크스페이스 메뉴">${icons.menu}</button>
        <div class="menu" id="menu" hidden>
          <button data-m="add-apps">앱 섹션 추가</button>
          <button data-m="add-todo">할 일 섹션 추가</button>
          <hr />
          <button data-m="rename">워크스페이스 이름 바꾸기</button>
          <button data-m="delete" class="danger">이 워크스페이스 삭제</button>
          <hr />
          <button data-m="export">이 워크스페이스 내보내기…</button>
          <button data-m="export-all">모두 내보내기…</button>
          <button data-m="import">불러오기…</button>
        </div>
      </div>
    </div>
  </header>
  <section class="hero">
    <div class="clock" id="clock"></div>
    <div class="date" id="date"></div>
    <form class="search" id="search"><span>${icons.search}</span><input type="text" id="q" placeholder="링크 아이콘 검색  ( / )" autocomplete="off" spellcheck="false" /><small class="hits" id="hits"></small></form>
  </section>
  <p class="edit-hint" id="edit-hint" hidden>섹션 제목을 끌어 옮기고, 오른쪽 아래 모서리를 끌어 크기를 바꾸세요. 아이콘은 끌어서 순서를 바꾸거나 다른 섹션으로 옮길 수 있어요.</p>
  <main class="board" id="board"></main>
  <dialog id="app-dlg"><form method="dialog" id="app-form">
    <div class="dlg-head"><span class="tile-preview" id="preview"></span><div><h3 id="app-dlg-title">앱 편집</h3><small id="app-host"></small></div></div>
    <label>이름<input type="text" name="title" maxlength="24" required /></label>
    <label>주소<input type="text" name="url" required spellcheck="false" /></label>
    <fieldset class="icon-choice">
      <legend>아이콘</legend>
      <div class="choices">
        <label><input type="radio" name="kind" value="site" /> 사이트 아이콘 <button type="button" class="link" id="refetch">다시 받기</button></label>
        <label><input type="radio" name="kind" value="letter" /> 글자 <input type="text" name="letter" maxlength="2" class="letter" /></label>
        <label><input type="radio" name="kind" value="image" /> 이미지 <button type="button" class="link" id="pick">파일 고르기…</button><input type="file" id="file" accept="image/*" hidden /></label>
      </div>
      <div class="swatches" id="swatches"></div>
    </fieldset>
    <div class="actions"><button type="button" class="btn danger" id="app-delete">삭제</button><span></span><button class="btn" value="cancel" formnovalidate>취소</button><button class="btn primary" value="ok">저장</button></div>
  </form></dialog>
  <dialog id="ask"><form method="dialog" id="ask-form">
    <h3 id="ask-title"></h3><p id="ask-msg"></p>
    <input type="text" id="ask-input" maxlength="24" />
    <div class="actions"><span></span><button class="btn" value="cancel" formnovalidate>취소</button><button class="btn primary" value="ok" id="ask-ok">확인</button></div>
  </form></dialog>`;

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

function render() {
  renderSpaces();
  renderBoard();
  document.body.classList.toggle("editing", editing);
  $("edit").classList.toggle("on", editing);
  $("edit").querySelector("span")!.textContent = editing ? "완료" : "편집";
  $("edit-hint").hidden = !editing;
}

function renderSpaces() {
  $("spaces").innerHTML =
    data.workspaces.map((w) => `<button class="space${w.id === data.active ? " on" : ""}" data-id="${w.id}">${esc(w.name)}</button>`).join("") +
    `<button class="space add" id="add-space" title="워크스페이스 추가">${icons.plus}</button>`;
}

function tileIcon(a: App) {
  if (a.icon) return `<img src="${esc(a.icon)}" alt="" draggable="false" />`;
  const [c1, c2] = COLORS[(a.color ?? 0) % COLORS.length];
  return `<span class="letter" style="background:linear-gradient(135deg,${c1},${c2})">${esc(a.letter || a.title[0] || "?")}</span>`;
}

function tilesHtml(s: Section) {
  return (
    (s.apps ?? [])
      .map(
        (a) => `<a class="tile" href="${esc(a.url)}" target="_blank" rel="noopener" data-id="${a.id}" title="${esc(a.title)}\n${esc(a.url)}" draggable="false">
        <span class="icon">${tileIcon(a)}</span><span class="label">${esc(a.title)}</span>
        <button class="del" title="삭제">${icons.close}</button></a>`,
      )
      .join("") + `<button class="tile add"><span class="icon">${icons.plus}</span><span class="label">추가</span></button>`
  );
}

function todosHtml(s: Section) {
  const f = todoFilter.get(s.id) ?? "all";
  const list = s.todos ?? [];
  const shown = list.filter((t) => (f === "all" ? true : f === "open" ? !t.done : t.done));
  return (
    shown
      .map(
        (t) => `<li class="${t.done ? "done" : ""}" data-id="${t.id}">
        <button class="check" title="${t.done ? "다시 할 일로" : "완료"}">${t.done ? "✓" : ""}</button>
        <span class="text" title="두 번 눌러 고치기">${esc(t.text)}</span>
        <button class="x" title="삭제">${icons.close}</button></li>`,
      )
      .join("") || `<li class="empty">${f === "done" ? "완료한 일이 없어요" : "할 일이 없어요. 여유로운 하루예요 ☕"}</li>`
  );
}

function countText(s: Section) {
  if (s.kind === "apps") return `${s.apps?.length ?? 0}개`;
  const open = (s.todos ?? []).filter((t) => !t.done).length;
  return s.todos?.length ? `${open}개 남음` : "";
}

function sectionHtml(s: Section) {
  const f = todoFilter.get(s.id) ?? "all";
  const body =
    s.kind === "apps"
      ? `<div class="grid">${tilesHtml(s)}</div>`
      : `<form class="todo-add"><input type="text" placeholder="할 일을 적고 Enter" autocomplete="off" /><button>${icons.plus}</button></form>
         <div class="filters">
           ${(["all", "open", "done"] as const).map((k) => `<button data-f="${k}" class="${k === f ? "on" : ""}">${{ all: "전체", open: "할 일", done: "완료" }[k]}</button>`).join("")}
           <button class="clear"${s.todos?.some((t) => t.done) ? "" : " hidden"}>완료 항목 지우기</button>
         </div>
         <ul class="todos">${todosHtml(s)}</ul>`;
  return `<section class="card sec sec-${s.kind}" data-sec="${s.id}" style="grid-column:span ${s.w};grid-row:span ${s.h}">
    <h2 class="sec-head"><span class="grip" title="끌어서 옮기기">⠿</span><span class="sec-title">${esc(s.title)}</span><small class="count">${countText(s)}</small>
      <span class="sec-tools"><button class="sec-btn rename" title="이름 바꾸기">${icons.edit}</button><button class="sec-btn remove" title="섹션 삭제">${icons.trash}</button></span></h2>
    <div class="sec-body">${body}</div>
    <span class="resize" title="끌어서 크기 바꾸기"></span><span class="size-badge"></span>
  </section>`;
}

function renderBoard() {
  const w = ws();
  $("board").innerHTML =
    w.sections.map(sectionHtml).join("") +
    `<div class="card add-sec"><b>섹션 추가</b><div><button class="btn" data-add="apps">${icons.layers} 앱 아이콘</button><button class="btn" data-add="todo">${icons.edit} 할 일</button></div></div>` +
    (w.sections.length ? "" : `<div class="card empty-board">비어 있는 워크스페이스예요. <button class="btn primary" data-add="apps">앱 섹션 만들기</button></div>`);
  // Big pictures fill the tile; small favicons sit on a plate.
  $("board")
    .querySelectorAll<HTMLImageElement>(".tile .icon img")
    .forEach((img) => {
      const fit = () => img.closest(".icon")?.classList.toggle("full", img.naturalWidth >= 96);
      if (img.complete) fit();
      else img.onload = fit;
      img.onerror = () => img.replaceWith(Object.assign(document.createElement("span"), { className: "letter fallback", textContent: (img.closest(".tile") as HTMLElement).title[0] ?? "?" }));
    });
  applySearch();
}

/** Only one section's to-do list (keeps typing focus elsewhere intact). */
function renderTodos(s: Section) {
  const el = document.querySelector<HTMLElement>(`[data-sec="${s.id}"]`);
  if (!el) return renderBoard();
  el.querySelector(".todos")!.innerHTML = todosHtml(s);
  el.querySelector(".count")!.textContent = countText(s);
  const f = todoFilter.get(s.id) ?? "all";
  el.querySelectorAll<HTMLElement>(".filters [data-f]").forEach((b) => b.classList.toggle("on", b.dataset.f === f));
  (el.querySelector(".filters .clear") as HTMLElement).hidden = !s.todos?.some((t) => t.done);
}

// ---------------------------------------------------------------- clock and search

function tick() {
  const d = new Date();
  $("clock").textContent = `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
  const h = d.getHours();
  const hello = h < 5 ? "늦은 밤이에요" : h < 11 ? "좋은 아침이에요" : h < 17 ? "좋은 오후예요" : h < 21 ? "좋은 저녁이에요" : "편안한 밤 되세요";
  $("date").textContent = `${d.getMonth() + 1}월 ${d.getDate()}일 ${"일월화수목금토"[d.getDay()]}요일 · ${hello}`;
}
tick();
setInterval(tick, 10_000);

// The search box only searches this workspace's link icons: matches stay in color, the rest
// turn gray. Enter opens the first match in a new tab; Esc clears.
const CHO = "ㄱㄲㄴㄷㄸㄹㅁㅂㅃㅅㅆㅇㅈㅉㅊㅋㅌㅍㅎ";
/** "네이버" → "ㄴㅇㅂ" (Korean initial sounds), so "ㄴㅇㅂ" finds it too. */
const initials = (t: string) => [...t].map((c) => { const k = c.charCodeAt(0) - 0xac00; return k >= 0 && k < 11172 ? CHO[Math.floor(k / 588)] : c; }).join("");
function matches(a: App, q: string) {
  const hay = `${a.title} ${host(a.url)}`.toLowerCase();
  return hay.includes(q) || initials(a.title.toLowerCase()).includes(q);
}
function applySearch() {
  const q = $<HTMLInputElement>("q").value.trim().toLowerCase();
  const tiles = $("board").querySelectorAll<HTMLElement>(".tile");
  let hits = 0;
  for (const el of tiles) {
    const s = sec(el.closest<HTMLElement>("[data-sec]")?.dataset.sec);
    const a = el.dataset.id ? s?.apps?.find((x) => x.id === el.dataset.id) : undefined;
    const hit = !q || (!!a && matches(a, q));
    el.classList.toggle("dim", !hit);
    el.classList.toggle("hit", !!q && hit);
    if (q && hit) hits++;
  }
  $("hits").textContent = q ? (hits ? `${hits}개` : "없음") : "";
}
$("q").addEventListener("input", applySearch);
$("q").addEventListener("keydown", (e) => {
  if (e.key === "Escape") {
    $<HTMLInputElement>("q").value = "";
    applySearch();
  }
});
$<HTMLFormElement>("search").onsubmit = (e) => {
  e.preventDefault();
  if (!$<HTMLInputElement>("q").value.trim()) return;
  $("board").querySelector<HTMLAnchorElement>(".tile.hit")?.click();
};
window.addEventListener("keydown", (e) => {
  if (e.key === "/" && !(e.target as HTMLElement).closest("input, textarea, dialog")) {
    e.preventDefault();
    $("q").focus();
  }
});

// ---------------------------------------------------------------- workspaces and sections

$("spaces").addEventListener("click", async (e) => {
  const b = (e.target as HTMLElement).closest<HTMLElement>(".space");
  if (!b) return;
  if (b.id === "add-space") {
    const name = await ask("새 워크스페이스", "", "예: 업무, 공부, 취미");
    if (!name) return;
    const w: Workspace = { id: uid(), name, sections: [appsSection()] };
    data.workspaces.push(w);
    data.active = w.id;
  } else data.active = b.dataset.id!;
  save();
  render();
});
$("spaces").addEventListener("dblclick", (e) => {
  if ((e.target as HTMLElement).closest(".space.on")) menuAction("rename");
});

$("edit").onclick = () => {
  editing = !editing;
  render();
};
$("more").onclick = (e) => {
  e.stopPropagation();
  $("menu").hidden = !$("menu").hidden;
};
document.addEventListener("click", () => ($("menu").hidden = true));
$("menu").addEventListener("click", (e) => {
  const m = (e.target as HTMLElement).closest<HTMLElement>("[data-m]")?.dataset.m;
  if (m) menuAction(m);
});

function addSection(kind: "apps" | "todo") {
  const s = kind === "apps" ? appsSection([], "새 앱 섹션", 6, 4) : todoSection([], "할 일", 4, 6);
  ws().sections.push(s);
  save();
  renderBoard();
  document.querySelector(`[data-sec="${s.id}"]`)?.scrollIntoView({ behavior: "smooth", block: "center" });
  flash(kind === "apps" ? "앱 섹션을 추가했습니다. ‘추가’로 사이트를 넣으세요." : "할 일 섹션을 추가했습니다.");
}

async function removeSection(s: Section) {
  const what = s.kind === "apps" ? `앱 ${s.apps?.length ?? 0}개` : `할 일 ${s.todos?.length ?? 0}개`;
  const ok = await ask(`‘${s.title}’ 섹션 삭제`, null, `${what}도 함께 지워집니다.`);
  if (ok === null) return;
  ws().sections = ws().sections.filter((x) => x !== s);
  save();
  renderBoard();
}

async function menuAction(m: string) {
  const w = ws();
  $("menu").hidden = true;
  if (m === "add-apps" || m === "add-todo") addSection(m === "add-apps" ? "apps" : "todo");
  else if (m === "rename") {
    const name = await ask("워크스페이스 이름", w.name);
    if (name) {
      w.name = name;
      save();
      render();
    }
  } else if (m === "delete") {
    if (data.workspaces.length === 1) return flash("워크스페이스가 하나뿐이라 지울 수 없습니다.");
    const ok = await ask(`‘${w.name}’ 삭제`, null, `섹션 ${w.sections.length}개와 그 안의 앱·할 일도 함께 지워집니다. 먼저 내보내 두면 나중에 불러올 수 있습니다.`);
    if (ok === null) return;
    data.workspaces = data.workspaces.filter((x) => x.id !== w.id);
    data.active = data.workspaces[0].id;
    save();
    render();
  } else if (m === "export" || m === "export-all") {
    const one = m === "export";
    const file: Portal = { version: 2, active: one ? w.id : data.active, workspaces: one ? [w] : data.workspaces };
    const done = await call<boolean>("portal_export", { json: JSON.stringify(file, null, 1), name: one ? w.name : "Kite 홈 전체" });
    if (done) flash(one ? `‘${w.name}’을(를) 내보냈습니다.` : "모든 워크스페이스를 내보냈습니다.");
  } else if (m === "import") {
    const raw = await call<string | null>("portal_import");
    if (!raw) return;
    const file = JSON.parse(raw) as { workspaces: Workspace[] };
    const names = new Set(data.workspaces.map((x) => x.name));
    let first = "";
    for (const src of file.workspaces) {
      // New ids everywhere, so importing the same file twice never mixes things up.
      const w2 = normalize({ ...src, id: "" });
      w2.sections = w2.sections.map((s) => ({ ...s, id: uid(), apps: s.apps?.map((a) => ({ ...a, id: uid() })), todos: s.todos?.map((t) => ({ ...t, id: uid() })) }));
      if (names.has(w2.name)) w2.name = `${w2.name} (가져옴)`;
      data.workspaces.push(w2);
      first ||= w2.id;
    }
    if (first) data.active = first;
    save();
    render();
    fetchMissingIcons();
    flash(`워크스페이스 ${file.workspaces.length}개를 불러왔습니다.`);
  }
}

// ---------------------------------------------------------------- board: clicks

let dragged = false;
let appSection: Section | null = null; // the section the app dialog adds to / edits in

$("board").addEventListener("click", async (e) => {
  const t = e.target as HTMLElement;
  const add = t.closest<HTMLElement>("[data-add]");
  if (add) return addSection(add.dataset.add as "apps" | "todo");
  const s = sec(t.closest<HTMLElement>("[data-sec]")?.dataset.sec);
  if (!s) return;
  if (t.closest(".sec-btn.remove")) return removeSection(s);
  if (t.closest(".sec-btn.rename") || (editing && t.closest(".sec-title"))) {
    const name = await ask("섹션 이름", s.title);
    if (name) {
      s.title = name;
      save();
      renderBoard();
    }
    return;
  }
  if (s.kind === "apps") {
    if (t.closest(".tile.add")) return openApp(s, null);
    const tile = t.closest<HTMLElement>(".tile[data-id]");
    const a = tile && s.apps?.find((x) => x.id === tile.dataset.id);
    if (!a) return;
    if (t.closest(".del")) {
      e.preventDefault();
      s.apps = s.apps!.filter((x) => x !== a);
      save();
      renderBoard();
      return;
    }
    if (editing || dragged) {
      e.preventDefault();
      if (!dragged) openApp(s, a);
    }
    return;
  }
  // to-do section
  const fb = t.closest<HTMLElement>(".filters button");
  if (fb) {
    if (fb.classList.contains("clear")) {
      s.todos = s.todos!.filter((x) => !x.done);
      save();
    } else todoFilter.set(s.id, fb.dataset.f as "all" | "open" | "done");
    return renderTodos(s);
  }
  const li = t.closest<HTMLElement>("li[data-id]");
  const todo = li && s.todos?.find((x) => x.id === li.dataset.id);
  if (!todo) return;
  if (t.closest(".check")) {
    todo.done = !todo.done;
    // Done ones sink below the open ones.
    s.todos!.sort((a, b) => Number(a.done) - Number(b.done) || b.at - a.at);
  } else if (t.closest(".x")) s.todos = s.todos!.filter((x) => x !== todo);
  else return;
  save();
  renderTodos(s);
});

$("board").addEventListener("submit", (e) => {
  const formEl = (e.target as HTMLElement).closest<HTMLFormElement>(".todo-add");
  const s = sec(formEl?.closest<HTMLElement>("[data-sec]")?.dataset.sec);
  if (!formEl || !s) return;
  e.preventDefault();
  const input = formEl.querySelector("input")!;
  const text = input.value.trim();
  if (!text) return;
  s.todos!.unshift({ id: uid(), text, done: false, at: Date.now() });
  input.value = "";
  if (todoFilter.get(s.id) === "done") todoFilter.set(s.id, "all");
  save();
  renderTodos(s);
});

$("board").addEventListener("dblclick", (e) => {
  const span = (e.target as HTMLElement).closest<HTMLElement>(".todos .text");
  const li = span?.closest<HTMLElement>("li[data-id]");
  const s = sec(span?.closest<HTMLElement>("[data-sec]")?.dataset.sec);
  const t = li && s?.todos?.find((x) => x.id === li.dataset.id);
  if (!span || !s || !t) return;
  const input = Object.assign(document.createElement("input"), { type: "text", value: t.text, className: "edit" });
  span.replaceWith(input);
  input.focus();
  input.select();
  let finished = false;
  const done = (keep: boolean) => {
    if (finished) return;
    finished = true;
    if (keep && input.value.trim()) t.text = input.value.trim();
    save();
    renderTodos(s);
  };
  input.onkeydown = (k) => {
    if (k.key === "Enter") done(true);
    else if (k.key === "Escape") done(false);
  };
  input.onblur = () => done(true);
});

$("board").addEventListener("contextmenu", (e) => {
  const tile = (e.target as HTMLElement).closest<HTMLElement>(".tile[data-id]");
  const s = sec(tile?.closest<HTMLElement>("[data-sec]")?.dataset.sec);
  if (!tile || !s) return;
  e.preventDefault();
  openApp(s, s.apps?.find((x) => x.id === tile.dataset.id) ?? null);
});

// ---------------------------------------------------------------- board: dragging (edit mode)

/** Follows the pointer from `e`; `step` runs on each move after a small threshold, `end` on release. */
function drag(e: PointerEvent, step: (m: PointerEvent, dx: number, dy: number) => void, end: (movedAtAll: boolean) => void) {
  const sx = e.clientX;
  const sy = e.clientY;
  let moving = false;
  const move = (m: PointerEvent) => {
    if (!moving && Math.hypot(m.clientX - sx, m.clientY - sy) < 5) return;
    moving = true;
    step(m, m.clientX - sx, m.clientY - sy);
  };
  const up = () => {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", up);
    end(moving);
  };
  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", up);
}

/** What is under the pointer, ignoring the element being dragged. */
function under(el: HTMLElement, x: number, y: number) {
  el.style.pointerEvents = "none";
  const hit = document.elementFromPoint(x, y) as HTMLElement | null;
  el.style.pointerEvents = "";
  return hit;
}

/** Moves `el` in the DOM but keeps it under the pointer: `off` collects how far the move shifted it. */
function relocate(el: HTMLElement, off: { x: number; y: number }, place: () => void) {
  const old = el.getBoundingClientRect();
  place();
  const now = el.getBoundingClientRect();
  off.x += old.left - now.left;
  off.y += old.top - now.top;
}

$("board").addEventListener("pointerdown", (e) => {
  const t = e.target as HTMLElement;
  if (!editing || e.button !== 0) return;
  const secEl = t.closest<HTMLElement>("[data-sec]");
  if (!secEl) return;

  // Resize: the corner handle. Columns snap to twelfths of the board, rows to row units.
  if (t.closest(".resize")) {
    e.preventDefault();
    const s = sec(secEl.dataset.sec)!;
    const colW = ($("board").clientWidth - GAP * (COLS - 1)) / COLS;
    const [w0, h0] = [s.w, s.h];
    secEl.classList.add("resizing");
    const badge = secEl.querySelector<HTMLElement>(".size-badge")!;
    badge.textContent = `${s.w} × ${s.h}`;
    drag(
      e,
      (_m, dx, dy) => {
        s.w = clamp(w0 + dx / (colW + GAP), 1, COLS);
        s.h = clamp(h0 + dy / (ROW + GAP), 2, MAX_H);
        secEl.style.gridColumn = `span ${s.w}`;
        secEl.style.gridRow = `span ${s.h}`;
        badge.textContent = `${s.w} × ${s.h}`;
      },
      () => {
        secEl.classList.remove("resizing");
        save();
      },
    );
    return;
  }

  // Move a section: drag its header over another section.
  if (t.closest(".sec-head") && !t.closest(".sec-btn")) {
    e.preventDefault();
    const off = { x: 0, y: 0 };
    drag(
      e,
      (m, dx, dy) => {
        secEl.classList.add("dragging");
        const over = under(secEl, m.clientX, m.clientY)?.closest<HTMLElement>("[data-sec]");
        if (over && over !== secEl) {
          const r = over.getBoundingClientRect();
          // Left half (or upper third) of the other section: before it; otherwise after.
          const before = m.clientX < r.left + r.width / 2 || m.clientY < r.top + r.height / 3;
          relocate(secEl, off, () => $("board").insertBefore(secEl, before ? over : over.nextSibling));
        }
        secEl.style.transform = `translate(${dx + off.x}px, ${dy + off.y}px)`;
      },
      (moved) => {
        secEl.classList.remove("dragging");
        secEl.style.transform = "";
        if (!moved) return;
        const order = [...$("board").querySelectorAll<HTMLElement>("[data-sec]")].map((x) => x.dataset.sec);
        ws().sections.sort((x, y) => order.indexOf(x.id) - order.indexOf(y.id));
        save();
        renderBoard();
      },
    );
    return;
  }

  // Move an icon: within its grid, or into another app section.
  const tile = t.closest<HTMLElement>(".tile[data-id]");
  if (!tile || t.closest(".del")) return;
  e.preventDefault();
  dragged = false;
  const off = { x: 0, y: 0 };
  drag(
    e,
    (m, dx, dy) => {
      tile.classList.add("dragging");
      const hit = under(tile, m.clientX, m.clientY);
      const over = hit?.closest<HTMLElement>(".tile[data-id]");
      const grid = hit?.closest<HTMLElement>(".sec-apps")?.querySelector<HTMLElement>(".grid");
      if (over && over !== tile) {
        const r = over.getBoundingClientRect();
        relocate(tile, off, () => over.parentElement!.insertBefore(tile, m.clientX < r.left + r.width / 2 ? over : over.nextSibling));
      } else if (grid && !over && tile.parentElement !== grid) {
        relocate(tile, off, () => grid.insertBefore(tile, grid.querySelector(".tile.add")));
      }
      tile.style.transform = `translate(${dx + off.x}px, ${dy + off.y}px)`;
    },
    (moved) => {
      tile.classList.remove("dragging");
      tile.style.transform = "";
      if (!moved) return;
      dragged = true;
      setTimeout(() => (dragged = false), 0);
      // Rebuild every app section from what the DOM now shows.
      const pool = new Map(ws().sections.flatMap((x) => x.apps ?? []).map((a) => [a.id, a]));
      for (const el of $("board").querySelectorAll<HTMLElement>(".sec-apps")) {
        const s = sec(el.dataset.sec);
        if (s) s.apps = [...el.querySelectorAll<HTMLElement>(".tile[data-id]")].map((x) => pool.get(x.dataset.id!)).filter((a): a is App => !!a);
      }
      save();
      renderBoard();
    },
  );
});

// ---------------------------------------------------------------- the app dialog

// Name, address, icon (site's, letter + color, or an image file).
let current: App | null = null;
let draft: App = app("", "");
function previewTile() {
  $("preview").innerHTML = `<span class="icon${draft.icon ? " full" : ""}">${tileIcon(draft)}</span>`;
  $("swatches").innerHTML = COLORS.map(([c1, c2], i) => `<button type="button" data-c="${i}" class="${i === (draft.color ?? 0) ? "on" : ""}" style="background:linear-gradient(135deg,${c1},${c2})"></button>`).join("");
  $("swatches").hidden = !!draft.icon;
}
const form = () => $<HTMLFormElement>("app-form").elements as unknown as Record<string, HTMLInputElement>;
const kindOf = () => form().kind as unknown as RadioNodeList;
function openApp(s: Section, a: App | null) {
  appSection = s;
  current = a;
  draft = a ? { ...a } : app("", "https://");
  const f = form();
  f.title.value = draft.title;
  f.url.value = draft.url;
  f.letter.value = draft.letter ?? "";
  kindOf().value = draft.icon === "" ? "letter" : draft.uploaded ? "image" : "site";
  $("app-dlg-title").textContent = a ? "앱 편집" : "앱 추가";
  $("app-host").textContent = a ? host(a.url) : `‘${s.title}’에 사이트를 추가합니다`;
  $("app-delete").hidden = !a;
  previewTile();
  $<HTMLDialogElement>("app-dlg").showModal();
  (a ? f.title : f.url).focus();
}
$("app-form").addEventListener("input", (e) => {
  const t = e.target as HTMLInputElement;
  const f = form();
  draft.title = f.title.value;
  if (t.name === "letter" || t.name === "title") draft.letter = (f.letter.value || f.title.value[0] || "?").toUpperCase();
  if (t.name === "kind" && t.value === "letter") draft.icon = "";
  if (t.name === "kind" && t.value === "site") refetch();
  previewTile();
});
$("swatches").addEventListener("click", (e) => {
  const c = (e.target as HTMLElement).closest<HTMLElement>("[data-c]");
  if (!c) return;
  draft.color = Number(c.dataset.c);
  draft.icon = "";
  kindOf().value = "letter";
  previewTile();
});
async function refetch() {
  let url = form().url.value.trim();
  if (!/^https?:\/\//.test(url)) url = `https://${url}`;
  $("refetch").textContent = "받는 중…";
  const icon = await call<string | null>("portal_icon", { url });
  $("refetch").textContent = "다시 받기";
  if (icon) {
    draft.icon = icon;
    draft.uploaded = false;
    kindOf().value = "site";
  } else {
    flash("이 사이트의 아이콘을 찾지 못해 글자 아이콘으로 둡니다.");
    draft.icon = "";
    kindOf().value = "letter";
  }
  previewTile();
}
$("refetch").onclick = refetch;
$("pick").onclick = () => $("file").click();
$<HTMLInputElement>("file").onchange = () => {
  const file = $<HTMLInputElement>("file").files?.[0];
  if (!file) return;
  // Down to 192px: plenty for a tile, small to keep.
  const img = new Image();
  img.onload = () => {
    const c = document.createElement("canvas");
    c.width = c.height = 192;
    const g = c.getContext("2d")!;
    const side = Math.min(img.width, img.height);
    g.drawImage(img, (img.width - side) / 2, (img.height - side) / 2, side, side, 0, 0, 192, 192);
    draft.icon = c.toDataURL("image/png");
    draft.uploaded = true;
    kindOf().value = "image";
    previewTile();
    URL.revokeObjectURL(img.src);
  };
  img.src = URL.createObjectURL(file);
};
$("app-delete").onclick = () => {
  if (!current || !appSection) return;
  appSection.apps = appSection.apps!.filter((x) => x.id !== current!.id);
  $<HTMLDialogElement>("app-dlg").close();
  save();
  renderBoard();
};
$<HTMLDialogElement>("app-dlg").addEventListener("close", async () => {
  const s = appSection;
  if ($<HTMLDialogElement>("app-dlg").returnValue !== "ok" || !s) return;
  const f = form();
  let url = f.url.value.trim();
  if (!/^[a-z]+:\/\//i.test(url)) url = `https://${url}`;
  try {
    new URL(url);
  } catch {
    return flash("주소가 올바르지 않습니다.");
  }
  draft.url = url;
  draft.title = f.title.value.trim() || host(url);
  draft.letter = (f.letter.value || draft.title[0] || "?").toUpperCase();
  const existing = current && s.apps!.find((x) => x.id === current!.id);
  const target = existing ? Object.assign(existing, draft) : draft;
  if (!existing) s.apps!.push(draft);
  // Site icon chosen but not fetched (new app, or the address changed): get the site's own.
  const moved = current && host(current.url) !== host(url);
  if (kindOf().value === "site" && (target.icon == null || (moved && !target.uploaded))) target.icon = null;
  save();
  renderBoard();
  if (target.icon == null) {
    target.icon = (await call<string | null>("portal_icon", { url })) || "";
    save();
    renderBoard();
  }
});

// ---------------------------------------------------------------- a small prompt/confirm

/** Text prompt (`value` a string) or a confirm (`value` null): resolves to the text / "" or null. */
function ask(title: string, value: string | null, msg = ""): Promise<string | null> {
  const dlg = $<HTMLDialogElement>("ask");
  const input = $<HTMLInputElement>("ask-input");
  $("ask-title").textContent = title;
  $("ask-msg").textContent = msg;
  $("ask-msg").hidden = value !== null;
  input.hidden = value === null;
  input.placeholder = value === null ? "" : msg;
  input.value = value ?? "";
  $("ask-ok").textContent = value === null ? "삭제" : "확인";
  $("ask-ok").className = `btn ${value === null ? "danger-fill" : "primary"}`;
  return new Promise((resolve) => {
    dlg.onclose = () => resolve(dlg.returnValue === "ok" ? (value === null ? "" : input.value.trim() || null) : null);
    dlg.returnValue = "";
    dlg.showModal();
    if (value !== null) {
      input.focus();
      input.select();
    }
  });
}

load();
