// kite://bookmarks — bookmark manager: folders on the left, list on the right, edit/delete/add.
import { call, esc, flash } from "../shared/page";
import { icons } from "../shared/icons";
import type { Bookmark } from "../shared/types";
import "./bookmarks.css";

document.title = "북마크";

let all: Bookmark[] = [];
/** null = every bookmark, "" = bookmark bar (top level), "a/b" = that folder and below */
let folder: string | null = null;
let query = "";

const BAR = "";
const folderName = (f: string) => (f === BAR ? "북마크 바" : f.split("/").pop()!);

function folders(): string[] {
  const set = new Set<string>();
  for (const b of all) {
    const parts = b.folder.split("/").filter(Boolean);
    for (let i = 1; i <= parts.length; i++) set.add(parts.slice(0, i).join("/"));
  }
  return [...set].sort((a, b) => a.localeCompare(b, "ko"));
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

function favicon(u: string) {
  try {
    const x = new URL(u);
    return /^https?:$/.test(x.protocol) ? `${x.origin}/favicon.ico` : "";
  } catch {
    return "";
  }
}

function host(u: string) {
  try {
    return decodeURI(u.replace(/^https?:\/\/(www\.)?/, "").replace(/\/$/, ""));
  } catch {
    return u;
  }
}

function render() {
  const fl = folders();
  const count = (f: string | null) => all.filter((b) => inFolder(b, f)).length;
  const nav = [
    `<a class="nav ${folder === null ? "on" : ""}" data-f="*">${icons.bookmark}<span>모든 북마크</span><span class="count">${all.length}</span></a>`,
    `<a class="nav ${folder === BAR ? "on" : ""}" data-f="">${icons.star}<span>북마크 바</span><span class="count">${count(BAR)}</span></a>`,
    ...fl.map((f) => {
      const depth = f.split("/").length - 1;
      return `<a class="nav ${folder === f ? "on" : ""}" data-f="${esc(f)}" style="padding-left:${12 + depth * 16}px">${icons.folder}<span>${esc(folderName(f))}</span><span class="count">${count(f)}</span></a>`;
    }),
  ].join("");
  const list = visible();
  const rows = list
    .map(
      (b) => `<div class="bm" data-id="${b.id}">
        <span class="bm-ic">${favicon(b.url) ? `<img src="${esc(favicon(b.url))}" alt="" />` : icons.globe}</span>
        <a class="bm-txt" href="${esc(b.url)}" title="${esc(b.url)}"><b>${esc(b.title || host(b.url))}</b><small>${esc(host(b.url))}</small></a>
        ${b.folder && folder === null ? `<span class="bm-folder">${icons.folder}${esc(b.folder)}</span>` : ""}
        <button class="ib" data-act="edit" title="편집">${icons.edit}</button>
        <button class="ib" data-act="del" title="삭제">${icons.trash}</button>
      </div>`,
    )
    .join("");
  const title = folder === null ? "모든 북마크" : folderName(folder);
  document.body.innerHTML = `
  <div class="page">
    <aside class="side"><h1>${icons.bookmark}북마크</h1>${nav}</aside>
    <main>
      <div class="bar">
        <input type="search" id="q" placeholder="북마크 검색 (제목·주소·폴더)" value="${esc(query)}" />
        <button class="btn primary" id="add">${icons.plus}추가</button>
        <button class="btn" id="import">가져오기</button>
      </div>
      <section class="card">
        <h2>${esc(title)} <span class="sub">${list.length}개</span></h2>
        ${rows || `<div class="empty">${query ? "검색 결과가 없습니다" : "북마크가 없습니다. 페이지에서 ☆ 를 누르거나 가져오기를 써 보세요."}</div>`}
      </section>
      <p class="hint">클릭: 이 탭에서 열기 · ${isMac ? "⌘" : "Ctrl"}+클릭: 새 탭 · 폴더 이름에 <code>/</code>를 넣으면 하위 폴더 (예: <code>개발/러스트</code>) · 폴더가 비어 있으면 북마크 바에 표시</p>
    </main>
  </div>
  <dialog id="dlg"><form method="dialog" id="form">
    <h3 id="dlg-title"></h3>
    <label>이름<input type="text" name="title" /></label>
    <label>주소<input type="text" name="url" required spellcheck="false" /></label>
    <label>폴더<input type="text" name="folder" list="folders" placeholder="비우면 북마크 바" /></label>
    <datalist id="folders">${fl.map((f) => `<option value="${esc(f)}"></option>`).join("")}</datalist>
    <div class="actions"><button class="btn" value="cancel" formnovalidate>취소</button><button class="btn primary" value="ok">저장</button></div>
  </form></dialog>`;
  bind();
}

const isMac = navigator.platform.toLowerCase().includes("mac");

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
      flash(b ? "저장했습니다." : "추가했습니다.");
      await load();
    }
  };
  dlg.showModal();
  (b ? f.title : f.url).focus();
}

function bind() {
  const q = document.getElementById("q") as HTMLInputElement;
  q.oninput = () => {
    query = q.value;
    const pos = q.selectionStart;
    render();
    const nq = document.getElementById("q") as HTMLInputElement;
    nq.focus();
    nq.setSelectionRange(pos, pos);
  };
  document.querySelectorAll<HTMLElement>(".nav").forEach((a) => {
    a.onclick = () => {
      folder = a.dataset.f === "*" ? null : a.dataset.f!;
      render();
    };
  });
  document.getElementById("add")!.onclick = () => openDialog();
  document.getElementById("import")!.onclick = async () => {
    const msg = await call<string>("bookmarks_import");
    if (msg) {
      flash(msg);
      await load();
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
        flash(`삭제했습니다: ${b.title || b.url}`);
        await load();
      }
    };
  });
}

async function load() {
  all = (await call<Bookmark[]>("bookmarks_all")) ?? [];
  if (folder && folder !== BAR && !all.some((b) => inFolder(b, folder))) folder = null;
  render();
}

load();
