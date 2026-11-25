// kite://bookmarks — bookmark manager: folders on the left, list on the right, edit/delete/add.
import { call, esc, flash } from "../shared/page";
import { icons } from "../shared/icons";
import type { Bookmark } from "../shared/types";
import "./bookmarks.css";
import { initTheme } from "../shared/theme";
import { loadIcons, savedIcon } from "../shared/favicons";

initTheme();

document.title = "북마크";

let all: Bookmark[] = [];
/** null = every bookmark, "" = bookmark bar (top level), "a/b" = that folder and below */
let folder: string | null = null;
let query = "";
/** Filter for the folder tree (keeps the tree shape: matches with their parents and children). */
let treeQuery = "";

const BAR = "";
const folderName = (f: string) => (f === BAR ? "북마크 바" : f.split("/").pop()!);

/** Folder order set by dragging (full paths; siblings sort by position, others alphabetically). */
let order: string[] = [];
/** Folders kept on their own (empty ones too). */
let kept: string[] = [];
const parentOf = (f: string) => f.split("/").slice(0, -1).join("/");

/** Every folder, depth first, siblings in the user's order. */
function folders(): string[] {
  const set = new Set<string>();
  for (const path of [...all.map((b) => b.folder), ...kept]) {
    const parts = path.split("/").filter(Boolean);
    for (let i = 1; i <= parts.length; i++) set.add(parts.slice(0, i).join("/"));
  }
  const kids = new Map<string, string[]>();
  for (const f of set) {
    const p = parentOf(f);
    if (!kids.has(p)) kids.set(p, []);
    kids.get(p)!.push(f);
  }
  const rank = (f: string) => {
    const i = order.indexOf(f);
    return i < 0 ? Number.MAX_SAFE_INTEGER : i;
  };
  const out: string[] = [];
  const walk = (p: string) => {
    const list = (kids.get(p) ?? []).sort((a, b) => rank(a) - rank(b) || a.localeCompare(b, "ko"));
    for (const f of list) {
      out.push(f);
      walk(f);
    }
  };
  walk("");
  return out;
}

function inFolder(b: Bookmark, f: string | null) {
  if (f === null) return true;
  if (f === BAR) return b.folder === "";
  return b.folder === f || b.folder.startsWith(f + "/");
}

function visible(): Bookmark[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  return all.filter((b) => {
    if (!inFolder(b, folder)) return false;
    const hay = `${b.title} ${b.url} ${b.folder}`.toLowerCase();
    return words.every((w) => hay.includes(w));
  });
}

/** Only icons kept while the site was open (never fetched from the bookmarked site). */
const favicon = savedIcon;

function host(u: string) {
  try {
    return decodeURI(u.replace(/^https?:\/\/(www\.)?/, "").replace(/\/$/, ""));
  } catch {
    return u;
  }
}

const count = (f: string | null) => all.filter((b) => inFolder(b, f)).length;

/** `name` with the first match of `q` marked. */
function marked(name: string, q: string) {
  const i = q ? name.toLowerCase().indexOf(q) : -1;
  if (i < 0) return esc(name);
  return `${esc(name.slice(0, i))}<mark>${esc(name.slice(i, i + q.length))}</mark>${esc(name.slice(i + q.length))}`;
}

// Folded folders (remembered in this browser). A search shows its matches unfolded.
const FOLD_KEY = "kite.bookmarks.collapsed";
const collapsed = new Set<string>(
  (() => {
    try {
      const v = JSON.parse(localStorage.getItem(FOLD_KEY) || "[]");
      return Array.isArray(v) ? v.filter((x) => typeof x === "string") : [];
    } catch {
      return [];
    }
  })(),
);
function saveFolds() {
  try {
    localStorage.setItem(FOLD_KEY, JSON.stringify([...collapsed]));
  } catch {
    /* not kept: fine */
  }
}
const chevron = `<svg class="icon" viewBox="0 0 24 24"><path d="M9 6l6 6-6 6"/></svg>`;
const newFolderIcon = `<svg class="icon" viewBox="0 0 24 24"><path d="M3.5 7.5a2 2 0 0 1 2-2h4l2 2h7a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2z"/><path d="M12 11v5M9.5 13.5h5"/></svg>`;
const expandAllIcon = `<svg class="icon" viewBox="0 0 24 24"><path d="M7 5l5 5 5-5M7 13l5 5 5-5"/></svg>`;
const collapseAllIcon = `<svg class="icon" viewBox="0 0 24 24"><path d="M7 11l5-5 5 5M7 19l5-5 5 5"/></svg>`;

function treeHtml(): string {
  const fl = folders();
  const q = treeQuery.trim().toLowerCase();
  // A query with `/` matches whole paths (개발/러), otherwise folder names.
  const hit = (f: string) => (q.includes("/") ? f.toLowerCase().includes(q) : folderName(f).toLowerCase().includes(q));
  const matches = q ? fl.filter(hit) : fl;
  const shown = new Set<string>();
  const context = new Set<string>();
  for (const m of matches) {
    shown.add(m);
    const parts = m.split("/");
    for (let i = 1; i < parts.length; i++) {
      const a = parts.slice(0, i).join("/");
      if (!shown.has(a)) context.add(a);
    }
    if (q) for (const f of fl) if (f.startsWith(m + "/")) shown.add(f);
  }
  for (const f of shown) context.delete(f);
  const parents = new Set(fl.map((f) => f.split("/").slice(0, -1).join("/")).filter(Boolean));
  // Hidden under a folded ancestor (not while searching).
  const folded = (f: string) => {
    if (q) return false;
    const parts = f.split("/");
    for (let i = 1; i < parts.length; i++) if (collapsed.has(parts.slice(0, i).join("/"))) return true;
    return false;
  };
  const rows = fl
    .filter((f) => (shown.has(f) || context.has(f)) && !folded(f))
    .map((f) => {
      const depth = f.split("/").length - 1;
      const name = q && !q.includes("/") && hit(f) ? marked(folderName(f), q) : esc(folderName(f));
      const open = q || !collapsed.has(f);
      const tw = parents.has(f) ? `<span class="tw${open ? " open" : ""}" data-tw title="${open ? "접기" : "펼치기"}">${chevron}</span>` : `<span class="tw-gap"></span>`;
      return `<a class="nav ${folder === f ? "on" : ""}${context.has(f) ? " ctx" : ""}" data-f="${esc(f)}" title="${esc(f)}" style="padding-left:${4 + depth * 16}px">${tw}${icons.folder}<span>${name}</span><span class="count">${count(f)}</span><span class="nav-act"><button data-act="rename" title="이름 바꾸기">${icons.edit}</button><button data-act="fdel" title="폴더 삭제">${icons.trash}</button></span></a>`;
    });
  const fixed = q
    ? []
    : [
        `<a class="nav ${folder === null ? "on" : ""}" data-f="*">${icons.bookmark}<span>모든 북마크</span><span class="count">${all.length}</span></a>`,
        `<a class="nav ${folder === BAR ? "on" : ""}" data-f="">${icons.star}<span>북마크 바</span><span class="count">${count(BAR)}</span></a>`,
      ];
  const none = q && !matches.length ? `<div class="tree-empty">일치하는 폴더가 없습니다</div>` : "";
  return [...fixed, ...rows].join("") + none;
}

function bookmarkRow(b: Bookmark) {
  return `<div class="bm" data-id="${b.id}">
        <span class="bm-ic">${favicon(b.url) ? `<img src="${esc(favicon(b.url))}" alt="" draggable="false" />` : icons.globe}</span>
        <a class="bm-txt" href="${esc(b.url)}" target="_blank" rel="noopener" title="${esc(b.url)}" draggable="false"><b>${esc(b.title || host(b.url))}</b><small>${esc(host(b.url))}</small></a>
        <button class="ib" data-act="edit" title="편집">${icons.edit}</button>
        <button class="ib" data-act="del" title="삭제">${icons.trash}</button>
      </div>`;
}

/** Category row for a folder: its path (parents faint), count; a click opens it in the tree. */
function categoryRow(f: string, n: number) {
  const parts = f === BAR ? [] : f.split("/");
  const label =
    f === BAR
      ? `<b>북마크 바</b>`
      : parts
          .map((p, i) => (i === parts.length - 1 ? `<b>${esc(p)}</b>` : `<span class="cat-parent">${esc(p)}</span><span class="cat-sep">›</span>`))
          .join("");
  return `<div class="cat" data-f="${esc(f)}" title="${f === BAR ? "북마크 바" : esc(f)} 열기">
        <span class="cat-ic">${f === BAR ? icons.star : icons.folder}</span>
        <span class="cat-path">${label}</span>
        <span class="cat-count">${n}</span>
        <span class="cat-go">${icons.forward}</span>
      </div>`;
}

/** The list grouped by folder, groups in tree order (bookmark bar first). */
function listHtml(list: Bookmark[], fl: string[]) {
  const groups = new Map<string, Bookmark[]>();
  for (const b of list) {
    if (!groups.has(b.folder)) groups.set(b.folder, []);
    groups.get(b.folder)!.push(b);
  }
  const rank = new Map(fl.map((f, i) => [f, i]));
  const at = (f: string) => (f === BAR ? -1 : (rank.get(f) ?? Number.MAX_SAFE_INTEGER));
  const keys = [...groups.keys()].sort((a, b) => at(a) - at(b));
  // One group that is just the open folder: the card title already says it.
  const plain = keys.length === 1 && keys[0] === folder;
  return keys.map((k) => (plain ? "" : categoryRow(k, groups.get(k)!.length)) + groups.get(k)!.map(bookmarkRow).join("")).join("");
}

/** Folder shown by the last render: the list keeps its scroll position until another one opens. */
let renderedFolder: string | null | undefined;

function render() {
  // The page is rebuilt: keep where the tree (always) and the list (same folder) were scrolled.
  const sideTop = document.querySelector<HTMLElement>(".side")?.scrollTop ?? 0;
  const mainTop = renderedFolder === folder ? (document.querySelector<HTMLElement>("main")?.scrollTop ?? 0) : 0;
  renderedFolder = folder;
  const fl = folders();
  const list = visible();
  const rows = listHtml(list, fl);
  const title = folder === null ? "모든 북마크" : folderName(folder);
  document.body.innerHTML = `
  <div class="page split" style="--side-w:${sideWidth}px">
    <aside class="side">
      <div class="side-head"><h1>${icons.bookmark}북마크</h1><div class="tree-tools"><input type="search" id="tq" placeholder="폴더 검색" value="${esc(treeQuery)}" spellcheck="false" /><button class="tool" id="new-folder" title="새 폴더 (선택한 폴더 안에)">${newFolderIcon}</button><button class="tool" id="expand-all" title="모두 펼치기">${expandAllIcon}</button><button class="tool" id="collapse-all" title="모두 접기">${collapseAllIcon}</button></div></div>
      <nav class="tree" id="tree">${treeHtml()}</nav>
    </aside>
    <div class="splitter" id="splitter" title="끌어서 폭 조절 · 더블클릭: 기본 폭"></div>
    <main>
      <div class="bar">
        <input type="search" id="q" placeholder="북마크 검색 (제목·주소·폴더)" value="${esc(query)}" />
        <button class="btn primary" id="add">${icons.plus}추가</button>
        <button class="btn" id="import">가져오기</button>
      </div>
      <section class="card bm-card">
        <h2>${esc(title)} <span class="sub">${list.length}개</span></h2>
        ${rows || `<div class="empty">${query ? "검색 결과가 없습니다" : folder && folder !== BAR ? "빈 폴더입니다. 북마크를 끌어다 놓거나 편집에서 이 폴더를 고르세요." : "북마크가 없습니다. 페이지에서 ☆ 를 누르거나 가져오기를 써 보세요."}</div>`}
      </section>
      <p class="hint">클릭: 이 탭에서 열기 · ${isMac ? "⌘" : "Ctrl"}+클릭: 새 탭 · 끌어서 왼쪽 폴더에 놓으면 이동 · 폴더도 끌어서 다른 폴더 안(가운데)이나 위·아래(순서)로 · 폴더 이름에 <code>/</code>를 넣으면 하위 폴더 (예: <code>개발/러스트</code>) · 폴더가 비어 있으면 북마크 바에 표시</p>
    </main>
  </div>
  <dialog id="dlg"><form method="dialog" id="form">
    <h3 id="dlg-title"></h3>
    <label>이름<input type="text" name="title" /></label>
    <label>주소<input type="text" name="url" required spellcheck="false" /></label>
    <label>폴더<input type="text" name="folder" list="folders" placeholder="비우면 북마크 바" /></label>
    <datalist id="folders">${fl.map((f) => `<option value="${esc(f)}"></option>`).join("")}</datalist>
    <div class="actions"><button class="btn" value="cancel" formnovalidate>취소</button><button class="btn primary" value="ok">저장</button></div>
  </form></dialog>
  <dialog id="ask"><form method="dialog" id="ask-form">
    <h3 id="ask-title"></h3>
    <p class="ask-msg" id="ask-msg"></p>
    <label id="ask-label">이름<input type="text" name="v" id="ask-input" spellcheck="false" /></label>
    <div class="actions"><button class="btn" value="cancel" formnovalidate>취소</button><button class="btn primary" value="ok" id="ask-ok">확인</button></div>
  </form></dialog>`;
  document.querySelector<HTMLElement>(".side")!.scrollTop = sideTop;
  document.querySelector<HTMLElement>("main")!.scrollTop = mainTop;
  bind();
}

const isMac = navigator.platform.toLowerCase().includes("mac");

// Folder tree width: drag the bar between the tree and the list (remembered in this browser).
const SIDE_DEFAULT = 220;
const SIDE_MIN = 160;
const SIDE_MAX = 600;
const SIDE_KEY = "kite.bookmarks.sideWidth";
let sideWidth = (() => {
  try {
    const v = Number(localStorage.getItem(SIDE_KEY));
    return v >= SIDE_MIN && v <= SIDE_MAX ? v : SIDE_DEFAULT;
  } catch {
    return SIDE_DEFAULT;
  }
})();
function setSideWidth(w: number, save: boolean) {
  sideWidth = Math.round(Math.min(SIDE_MAX, Math.max(SIDE_MIN, w)));
  document.querySelector<HTMLElement>(".page.split")?.style.setProperty("--side-w", `${sideWidth}px`);
  if (save) {
    try {
      localStorage.setItem(SIDE_KEY, String(sideWidth));
    } catch {
      /* not kept: fine */
    }
  }
}
function bindSplitter() {
  const bar = document.getElementById("splitter");
  if (!bar) return;
  bar.onpointerdown = (e) => {
    if (e.button !== 0) return;
    e.preventDefault();
    try {
      bar.setPointerCapture(e.pointerId);
    } catch {
      /* moves still arrive while over the bar */
    }
    const startX = e.clientX;
    const startW = sideWidth;
    document.body.classList.add("resizing");
    bar.onpointermove = (m) => setSideWidth(startW + m.clientX - startX, false);
    bar.onpointerup = bar.onpointercancel = () => {
      bar.onpointermove = bar.onpointerup = bar.onpointercancel = null;
      document.body.classList.remove("resizing");
      setSideWidth(sideWidth, true);
    };
  };
  bar.ondblclick = () => setSideWidth(SIDE_DEFAULT, true);
}

function openDialog(b?: Bookmark) {
  const dlg = document.getElementById("dlg") as HTMLDialogElement;
  const form = document.getElementById("form") as HTMLFormElement;
  (document.getElementById("dlg-title") as HTMLElement).textContent = b ? "북마크 편집" : "북마크 추가";
  const f = form.elements as unknown as Record<string, HTMLInputElement>;
  f.title.value = b?.title ?? "";
  f.url.value = b?.url ?? "";
  f.folder.value = b ? b.folder : folder && folder !== BAR ? folder : "";
  dlg.onclose = async () => {
    if (dlg.returnValue !== "ok") return;
    const ok = await call("bookmark_save", { id: b?.id ?? null, url: f.url.value, title: f.title.value, folder: f.folder.value });
    if (ok !== undefined) {
      await load();
      flash(b ? "저장했습니다." : "추가했습니다.");
    }
  };
  dlg.showModal();
  (b ? f.title : f.url).focus();
}

function bind() {
  bindSplitter();
  // Category rows stick right below the search bar.
  const barEl = document.querySelector<HTMLElement>("main .bar");
  if (barEl) document.querySelector<HTMLElement>("main")!.style.setProperty("--bar-h", `${barEl.offsetHeight}px`);
  document.querySelectorAll<HTMLElement>(".cat").forEach((c) => {
    c.onclick = () => {
      folder = c.dataset.f!;
      if (folder !== BAR) {
        // Open the folders above it so it shows in the tree.
        const parts = folder.split("/");
        for (let i = 1; i < parts.length; i++) collapsed.delete(parts.slice(0, i).join("/"));
        saveFolds();
      }
      render();
      document.querySelector<HTMLElement>(".nav.on")?.scrollIntoView({ block: "nearest" });
      document.querySelector<HTMLElement>("main")!.scrollTop = 0;
    };
  });
  bindDrag();
  bindFolderDrag();
  const q = document.getElementById("q") as HTMLInputElement;
  q.oninput = () => {
    query = q.value;
    const pos = q.selectionStart;
    render();
    const nq = document.getElementById("q") as HTMLInputElement;
    nq.focus();
    nq.setSelectionRange(pos, pos);
  };
  const tree = document.getElementById("tree")!;
  tree.onclick = (e) => {
    const a = (e.target as HTMLElement).closest<HTMLElement>(".nav");
    if (!a) return;
    const act = (e.target as HTMLElement).closest<HTMLElement>("[data-act]")?.dataset.act;
    if (act === "rename" || act === "fdel") {
      e.preventDefault();
      if (act === "rename") renameFolder(a.dataset.f!);
      else deleteFolder(a.dataset.f!);
      return;
    }
    // The ▸ toggles the folder (does not open it); not while searching (all shown).
    if ((e.target as HTMLElement).closest("[data-tw]")) {
      e.preventDefault();
      if (treeQuery.trim()) return;
      const f = a.dataset.f!;
      if (!collapsed.delete(f)) collapsed.add(f);
      saveFolds();
      tree.innerHTML = treeHtml();
      return;
    }
    folder = a.dataset.f === "*" ? null : a.dataset.f!;
    render();
  };
  document.getElementById("new-folder")!.onclick = () => newFolder();
  document.getElementById("expand-all")!.onclick = () => {
    collapsed.clear();
    saveFolds();
    tree.innerHTML = treeHtml();
  };
  document.getElementById("collapse-all")!.onclick = () => {
    for (const f of folders()) if (folders().some((g) => g.startsWith(f + "/"))) collapsed.add(f);
    saveFolds();
    tree.innerHTML = treeHtml();
    document.querySelector<HTMLElement>(".side")!.scrollTop = 0;
  };
  // Only the tree is redrawn while filtering, so the box keeps focus.
  const tq = document.getElementById("tq") as HTMLInputElement;
  tq.oninput = () => {
    treeQuery = tq.value;
    tree.innerHTML = treeHtml();
    document.querySelector<HTMLElement>(".side")!.scrollTop = 0;
  };
  tq.onkeydown = (e) => {
    if (e.key === "Escape" && tq.value) {
      e.preventDefault();
      tq.value = "";
      tq.dispatchEvent(new Event("input"));
    } else if (e.key === "Enter") {
      // The first folder that matches (not a parent shown for context).
      const first = tree.querySelector<HTMLElement>(".nav:not(.ctx)");
      if (first) first.click();
    }
  };
  document.getElementById("add")!.onclick = () => openDialog();
  document.getElementById("import")!.onclick = async () => {
    const msg = await call<string>("bookmarks_import");
    if (msg) {
      await load();
      flash(msg);
    }
  };
  document.querySelectorAll<HTMLImageElement>(".bm-ic img").forEach((img) => {
    img.onerror = () => (img.parentElement!.innerHTML = icons.globe);
  });
  document.querySelectorAll<HTMLElement>(".bm").forEach((row) => {
    const b = all.find((x) => x.id === Number(row.dataset.id))!;
    row.querySelector<HTMLElement>('[data-act="edit"]')!.onclick = () => openDialog(b);
    row.querySelector<HTMLElement>('[data-act="del"]')!.onclick = async () => {
      if ((await call("bookmark_delete", { id: b.id })) !== undefined) {
        await load();
        flash(`삭제했습니다: ${b.title || b.url}`);
      }
    };
  });
}

// ---- folders: new / rename / delete (the page's own dialog: macOS webviews show no prompt()) ----
function ask(opts: { title: string; msg?: string; value?: string; ok: string; danger?: boolean; input?: boolean }): Promise<string | null> {
  const dlg = document.getElementById("ask") as HTMLDialogElement;
  const input = document.getElementById("ask-input") as HTMLInputElement;
  const okBtn = document.getElementById("ask-ok") as HTMLButtonElement;
  (document.getElementById("ask-title") as HTMLElement).textContent = opts.title;
  const msg = document.getElementById("ask-msg") as HTMLElement;
  msg.textContent = opts.msg ?? "";
  msg.hidden = !opts.msg;
  (document.getElementById("ask-label") as HTMLElement).hidden = opts.input === false;
  input.required = opts.input !== false;
  input.value = opts.value ?? "";
  okBtn.textContent = opts.ok;
  okBtn.className = `btn ${opts.danger ? "danger" : "primary"}`;
  return new Promise((resolve) => {
    dlg.onclose = () => resolve(dlg.returnValue === "ok" ? input.value.trim() : null);
    dlg.returnValue = "";
    dlg.showModal();
    if (opts.input !== false) {
      input.focus();
      input.select();
    } else okBtn.focus();
  });
}

async function newFolder() {
  const parent = folder && folder !== BAR ? folder : "";
  const name = await ask({ title: "새 폴더", msg: parent ? `위치: ${parent.split("/").join(" › ")}` : "위치: 최상위", ok: "만들기" });
  if (!name) return;
  if (name.includes("/")) return flash("폴더 이름에는 / 를 쓸 수 없습니다.");
  const path = await call<string>("bookmark_folder_create", { path: parent ? `${parent}/${name}` : name });
  if (path === undefined) return;
  if (parent) collapsed.delete(parent);
  saveFolds();
  folder = path;
  await load();
  document.querySelector<HTMLElement>(".nav.on")?.scrollIntoView({ block: "nearest" });
  flash(`폴더를 만들었습니다: ${name}`);
}

async function renameFolder(f: string) {
  const name = await ask({ title: "폴더 이름 바꾸기", value: folderName(f), ok: "바꾸기" });
  if (!name || name === folderName(f)) return;
  const to = await call<string>("bookmark_folder_rename", { path: f, name });
  if (to === undefined) return;
  for (const c of [...collapsed]) {
    collapsed.delete(c);
    collapsed.add(renamePrefix(c, f, to));
  }
  saveFolds();
  if (folder !== null) folder = renamePrefix(folder, f, to);
  await load();
  flash(`이름을 바꿨습니다: ${name}`);
}

async function deleteFolder(f: string) {
  const n = count(f);
  const subs = folders().filter((g) => g.startsWith(f + "/")).length;
  const what = [subs ? `하위 폴더 ${subs}개` : "", n ? `북마크 ${n}개` : ""].filter(Boolean).join("와 ");
  const ok = await ask({
    title: `'${folderName(f)}' 폴더 삭제`,
    msg: what ? `안에 있는 ${what}도 함께 삭제됩니다. 되돌릴 수 없습니다.` : "빈 폴더를 삭제합니다.",
    ok: "삭제",
    danger: true,
    input: false,
  });
  if (ok === null) return;
  if ((await call<number>("bookmark_folder_delete", { path: f })) === undefined) return;
  for (const c of [...collapsed]) if (c === f || c.startsWith(f + "/")) collapsed.delete(c);
  saveFolds();
  if (folder !== null && (folder === f || folder.startsWith(f + "/"))) folder = null;
  await load();
  flash(`폴더를 삭제했습니다: ${folderName(f)}`);
}

// ---- drag a bookmark onto a folder in the tree (pointer events: in a tab, the engine's file
// drop handling gets in the way of HTML5 drag and drop) ----
let suppressClick = false;
document.addEventListener(
  "click",
  (e) => {
    if (suppressClick) {
      e.preventDefault();
      e.stopPropagation();
    }
  },
  true,
);

async function moveTo(b: Bookmark, to: string) {
  const ok = await call("bookmark_save", { id: b.id, url: b.url, title: b.title, folder: to });
  if (ok === undefined) return;
  await load();
  flash(`옮겼습니다: ${b.title || host(b.url)} → ${to === BAR ? "북마크 바" : to}`);
}

function bindDrag() {
  const tree = document.getElementById("tree")!;
  const side = document.querySelector<HTMLElement>(".side")!;
  const head = document.querySelector<HTMLElement>(".side-head")!;
  document.querySelectorAll<HTMLElement>(".bm").forEach((row) => {
    row.onpointerdown = (e) => {
      if (e.button !== 0 || (e.target as HTMLElement).closest("button")) return;
      const b = all.find((x) => x.id === Number(row.dataset.id));
      if (!b) return;
      e.preventDefault(); // no text selection, no native link drag
      const sx = e.clientX;
      const sy = e.clientY;
      let ghost: HTMLElement | null = null;
      let target: HTMLElement | null = null;
      let openTimer = 0;
      const setTarget = (el: HTMLElement | null) => {
        if (el === target) return;
        target?.classList.remove("drop");
        target = el;
        target?.classList.add("drop");
        clearTimeout(openTimer);
        // Hovering a folded folder opens it.
        const f = target?.dataset.f;
        if (f && collapsed.has(f) && !treeQuery.trim()) {
          openTimer = window.setTimeout(() => {
            collapsed.delete(f);
            saveFolds();
            tree.innerHTML = treeHtml();
            target = null;
            setTarget(tree.querySelector<HTMLElement>(`.nav[data-f="${CSS.escape(f)}"]`));
          }, 700);
        }
      };
      const move = (m: PointerEvent) => {
        if (!ghost) {
          if (Math.hypot(m.clientX - sx, m.clientY - sy) < 6) return;
          ghost = document.createElement("div");
          ghost.className = "drag-ghost";
          ghost.innerHTML = `${icons.globe}<span>${esc(b.title || host(b.url))}</span>`;
          document.body.append(ghost);
          document.body.classList.add("dragging-bm");
          row.classList.add("drag-src");
        }
        ghost.style.transform = `translate(${m.clientX + 14}px, ${m.clientY + 10}px)`;
        const el = document.elementFromPoint(m.clientX, m.clientY)?.closest<HTMLElement>("#tree .nav") ?? null;
        // Any folder or the bookmark bar, but not "all bookmarks" or where it already is.
        setTarget(el && el.dataset.f !== "*" && el.dataset.f !== b.folder ? el : null);
        // Near the tree's top or bottom edge: scroll it.
        const r = side.getBoundingClientRect();
        if (m.clientX >= r.left && m.clientX <= r.right) {
          const top = head.getBoundingClientRect().bottom;
          if (m.clientY < top + 30) side.scrollTop -= 14;
          else if (m.clientY > r.bottom - 40) side.scrollTop += 14;
        }
      };
      const finish = (drop: boolean) => {
        window.removeEventListener("pointermove", move);
        window.removeEventListener("pointerup", up);
        window.removeEventListener("pointercancel", cancel);
        window.removeEventListener("keydown", key, true);
        clearTimeout(openTimer);
        if (!ghost) return;
        ghost.remove();
        document.body.classList.remove("dragging-bm");
        row.classList.remove("drag-src");
        const to = target?.dataset.f;
        target?.classList.remove("drop");
        // The release ends a drag, not a click on the link under it.
        suppressClick = true;
        setTimeout(() => (suppressClick = false), 0);
        if (drop && to !== undefined) moveTo(b, to);
      };
      const up = () => finish(true);
      const cancel = () => finish(false);
      const key = (k: KeyboardEvent) => {
        if (k.key === "Escape") {
          k.preventDefault();
          finish(false);
        }
      };
      window.addEventListener("pointermove", move);
      window.addEventListener("pointerup", up);
      window.addEventListener("pointercancel", cancel);
      window.addEventListener("keydown", key, true);
    };
  });
}

// ---- drag a folder in the tree: onto a folder = into it, near a row's top/bottom edge = before/
// after it (same parent). Folders exist only as bookmark paths, so a move rewrites those paths
// (Rust, one statement) and the order is kept separately. ----
type FolderDrop = { el: HTMLElement; mode: "into" | "before" | "after" };

/** Paths after `from` became `to` (fold state, the open folder). */
function renamePrefix(f: string, from: string, to: string) {
  return f === from ? to : f.startsWith(from + "/") ? to + f.slice(from.length) : f;
}

async function dropFolder(from: string, d: FolderDrop) {
  const target = d.el.dataset.f!;
  const parent = d.mode === "into" ? target : parentOf(target);
  let path = from;
  if (parentOf(from) !== parent) {
    const to = await call<string>("bookmark_folder_move", { from, parent });
    if (to === undefined) return;
    path = to;
    for (const f of [...collapsed]) {
      collapsed.delete(f);
      collapsed.add(renamePrefix(f, from, to));
    }
    saveFolds();
    if (folder !== null) folder = renamePrefix(folder, from, to);
    all = (await call<Bookmark[]>("bookmarks_all")) ?? all;
    kept = (await call<string[]>("bookmark_folders")) ?? kept;
    order = order.map((f) => renamePrefix(f, from, to));
  }
  // New sibling order under `parent`, then the whole tree as the saved order.
  const tree = folders();
  const siblings = tree.filter((f) => parentOf(f) === parent && f !== path);
  const at = d.mode === "into" ? siblings.length : siblings.indexOf(target) + (d.mode === "after" ? 1 : 0);
  siblings.splice(Math.max(0, at), 0, path);
  // Ranks: these siblings first (in their new order), everything else keeps its relative order;
  // folders() then lays the whole tree out, and that list is what gets saved.
  order = [...siblings, ...tree.filter((f) => parentOf(f) !== parent)];
  order = folders();
  await call("bookmark_folder_order_set", { order });
  if (d.mode === "into") {
    collapsed.delete(target);
    saveFolds();
  }
  await load();
  const where = parent === "" ? "최상위" : parent;
  flash(path === from && parentOf(from) === parent ? `순서를 바꿨습니다: ${folderName(path)}` : `옮겼습니다: ${folderName(path)} → ${where}`);
}

function bindFolderDrag() {
  const tree = document.getElementById("tree")!;
  const side = document.querySelector<HTMLElement>(".side")!;
  const head = document.querySelector<HTMLElement>(".side-head")!;
  tree.onpointerdown = (e) => {
    const src = (e.target as HTMLElement).closest<HTMLElement>(".nav");
    const from = src?.dataset.f;
    // Real folders only (not "all bookmarks" / the bar), not the ▸, not while searching.
    if (!src || !from || from === "*" || e.button !== 0 || (e.target as HTMLElement).closest("[data-tw], .nav-act") || treeQuery.trim()) return;
    e.preventDefault();
    const sx = e.clientX;
    const sy = e.clientY;
    let ghost: HTMLElement | null = null;
    let drop: FolderDrop | null = null;
    let openTimer = 0;
    const show = (d: FolderDrop | null) => {
      if (drop && (!d || d.el !== drop.el || d.mode !== drop.mode)) drop.el.classList.remove("drop", "drop-before", "drop-after");
      const sameTarget = d && drop && d.el === drop.el && d.mode === drop.mode;
      drop = d;
      if (!d || sameTarget) return;
      d.el.classList.add(d.mode === "into" ? "drop" : `drop-${d.mode}`);
      clearTimeout(openTimer);
      const f = d.el.dataset.f!;
      if (d.mode === "into" && collapsed.has(f)) {
        openTimer = window.setTimeout(() => {
          collapsed.delete(f);
          saveFolds();
          tree.innerHTML = treeHtml();
          tree.querySelector<HTMLElement>(`.nav[data-f="${CSS.escape(from)}"]`)?.classList.add("drag-src");
          drop = null;
          show({ el: tree.querySelector<HTMLElement>(`.nav[data-f="${CSS.escape(f)}"]`)!, mode: "into" });
        }, 700);
      }
    };
    const move = (m: PointerEvent) => {
      if (!ghost) {
        if (Math.hypot(m.clientX - sx, m.clientY - sy) < 6) return;
        ghost = document.createElement("div");
        ghost.className = "drag-ghost";
        ghost.innerHTML = `${icons.folder}<span>${esc(folderName(from))}</span>`;
        document.body.append(ghost);
        document.body.classList.add("dragging-bm");
        src.classList.add("drag-src");
      }
      ghost.style.transform = `translate(${m.clientX + 14}px, ${m.clientY + 10}px)`;
      const el = document.elementFromPoint(m.clientX, m.clientY)?.closest<HTMLElement>("#tree .nav") ?? null;
      const f = el?.dataset.f;
      let d: FolderDrop | null = null;
      if (el && f !== undefined && f !== "*" && f !== from && !f.startsWith(from + "/")) {
        if (f === "") {
          d = { el, mode: "into" }; // the bookmark bar row = top level
        } else {
          const r = el.getBoundingClientRect();
          const y = (m.clientY - r.top) / r.height;
          d = { el, mode: y < 0.28 ? "before" : y > 0.72 ? "after" : "into" };
          // Already right there: nothing to do.
          if (d.mode === "into" && parentOf(from) === f) d = null;
        }
      }
      show(d);
      const r = side.getBoundingClientRect();
      if (m.clientX >= r.left && m.clientX <= r.right) {
        const top = head.getBoundingClientRect().bottom;
        if (m.clientY < top + 30) side.scrollTop -= 14;
        else if (m.clientY > r.bottom - 40) side.scrollTop += 14;
      }
    };
    const finish = (ok: boolean) => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", cancel);
      window.removeEventListener("keydown", key, true);
      clearTimeout(openTimer);
      if (!ghost) return;
      ghost.remove();
      document.body.classList.remove("dragging-bm");
      tree.querySelectorAll(".drag-src").forEach((x) => x.classList.remove("drag-src"));
      const d = drop;
      show(null);
      suppressClick = true;
      setTimeout(() => (suppressClick = false), 0);
      if (ok && d) dropFolder(from, d);
    };
    const up = () => finish(true);
    const cancel = () => finish(false);
    const key = (k: KeyboardEvent) => {
      if (k.key === "Escape") {
        k.preventDefault();
        finish(false);
      }
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", cancel);
    window.addEventListener("keydown", key, true);
  };
}

async function load() {
  all = (await call<Bookmark[]>("bookmarks_all")) ?? [];
  order = (await call<string[]>("bookmark_folder_order")) ?? [];
  kept = (await call<string[]>("bookmark_folders")) ?? [];
  if (folder && folder !== BAR && !folders().includes(folder)) folder = null;
  render();
  if (await loadIcons(all.map((b) => b.url))) render();
}

load();
