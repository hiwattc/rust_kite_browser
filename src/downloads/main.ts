// kite://downloads — download manager: what is downloading now (with progress) and the history,
// grouped by day. Open the file, show it in its folder, cancel, download again, remove from the
// list. Data and file actions live in `src-tauri/src/downloads.rs`.
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { call, esc } from "../shared/page";
import { icons } from "../shared/icons";
import { initTheme } from "../shared/theme";
import type { DownloadItem, DownloadsSnapshot } from "../shared/types";
import "./downloads.css";

initTheme();
document.title = "다운로드";

const isMac = navigator.platform.toLowerCase().includes("mac");
let snap: DownloadsSnapshot = { items: [], progress: 1, active: 0, dir: "" };
let query = "";
/** Speed per download: last (time, bytes) seen. */
const seen = new Map<number, { t: number; b: number; bps: number }>();

document.body.innerHTML = `
  <main class="dl-page">
    <header>
      <h1>${icons.download}<span>다운로드</span></h1>
      <div class="search"><span>${icons.search}</span><input type="search" id="q" placeholder="파일 이름이나 주소로 검색" /></div>
      <button class="btn" id="open-dir">${icons.folder_open}<span>다운로드 폴더 열기</span></button>
      <button class="btn" id="clear" title="목록만 지웁니다 (파일은 그대로)">목록 지우기</button>
    </header>
    <p class="where">저장 위치 <code id="dir"></code> · <a href="#" id="change">설정에서 바꾸기</a></p>
    <section id="list"></section>
  </main>`;

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

function size(n: number) {
  if (n < 0) return "";
  if (n >= 1 << 30) return `${(n / (1 << 30)).toFixed(2)} GB`;
  if (n >= 1 << 20) return `${(n / (1 << 20)).toFixed(1)} MB`;
  if (n >= 1024) return `${Math.round(n / 1024)} KB`;
  return `${n} B`;
}

function kind(name: string) {
  const ext = (name.split(".").pop() ?? "").toLowerCase();
  if (["png", "jpg", "jpeg", "gif", "webp", "svg", "heic", "bmp"].includes(ext)) return ["img", "이미지"];
  if (["mp4", "mov", "mkv", "avi", "webm"].includes(ext)) return ["vid", "동영상"];
  if (["mp3", "wav", "flac", "m4a", "aac", "ogg"].includes(ext)) return ["aud", "오디오"];
  if (["zip", "7z", "rar", "gz", "tgz", "xz", "bz2", "tar"].includes(ext)) return ["zip", "압축"];
  if (["pdf"].includes(ext)) return ["pdf", "PDF"];
  if (["doc", "docx", "hwp", "hwpx", "txt", "md", "rtf", "pages"].includes(ext)) return ["doc", "문서"];
  if (["xls", "xlsx", "csv", "numbers"].includes(ext)) return ["xls", "표"];
  if (["ppt", "pptx", "key"].includes(ext)) return ["ppt", "발표"];
  if (["dmg", "pkg", "exe", "msi", "app", "deb", "rpm"].includes(ext)) return ["bin", "설치"];
  return ["file", ext ? ext.toUpperCase() : "파일"];
}

function host(u: string) {
  try {
    return new URL(u).host;
  } catch {
    return u.slice(0, 60);
  }
}

function day(ms: number) {
  const d = new Date(ms);
  const today = new Date();
  const y = new Date(Date.now() - 86_400_000);
  const same = (a: Date, b: Date) => a.toDateString() === b.toDateString();
  if (same(d, today)) return "오늘";
  if (same(d, y)) return "어제";
  return `${d.getFullYear()}년 ${d.getMonth() + 1}월 ${d.getDate()}일`;
}

function status(x: DownloadItem) {
  if (x.state === "progress") {
    const s = seen.get(x.id);
    const pct = x.total > 0 ? `${Math.floor((x.received / x.total) * 100)}% · ` : "";
    const of = x.total > 0 ? `${size(x.received)} / ${size(x.total)}` : size(x.received);
    const speed = s && s.bps > 0 ? ` · ${size(s.bps)}/s` : "";
    const left = s && s.bps > 0 && x.total > 0 ? ` · ${Math.max(1, Math.round((x.total - x.received) / s.bps))}초 남음` : "";
    return `${pct}${of}${speed}${left}`;
  }
  const t = new Date(x.ended ?? x.started).toLocaleTimeString("ko-KR", { hour: "2-digit", minute: "2-digit" });
  if (x.state === "done") return x.missing ? `삭제되었거나 옮겨짐 · ${t}` : `${size(x.total)} · ${t}`;
  if (x.state === "canceled") return `취소됨 · ${t}`;
  return `실패 · ${t}`;
}

function row(x: DownloadItem) {
  const [k, label] = kind(x.name);
  const active = x.state === "progress";
  const pct = x.total > 0 ? (x.received / x.total) * 100 : -1;
  const actions = active
    ? x.cancelable
      ? `<button class="act" data-a="cancel">취소</button>`
      : ""
    : x.state === "done" && !x.missing
      ? `<button class="act primary" data-a="open">열기</button><button class="act" data-a="show">${isMac ? "Finder에서 보기" : "폴더에서 보기"}</button>`
      : `<button class="act" data-a="retry">다시 받기</button>`;
  return `<article class="item ${x.state}${x.missing ? " missing" : ""}" data-id="${x.id}">
    <div class="ficon ${k}"><span>${esc(label)}</span></div>
    <div class="body">
      <div class="name" title="${esc(x.path)}">${esc(x.name)}</div>
      <a class="src" href="#" data-a="retry" title="${esc(x.url)}">${esc(host(x.url))}</a>
      ${active ? `<div class="bar${pct < 0 ? " unknown" : ""}"><i style="width:${pct < 0 ? 30 : Math.max(2, pct)}%"></i></div>` : ""}
      <div class="meta">${status(x)}</div>
    </div>
    <div class="actions">${actions}<button class="x" data-a="remove" title="목록에서 삭제"${active ? " hidden" : ""}>${icons.close}</button></div>
  </article>`;
}

function render() {
  const now = Date.now();
  for (const x of snap.items.filter((i) => i.state === "progress")) {
    const s = seen.get(x.id);
    if (!s) seen.set(x.id, { t: now, b: x.received, bps: 0 });
    else if (now - s.t >= 900) {
      const bps = ((x.received - s.b) / (now - s.t)) * 1000;
      seen.set(x.id, { t: now, b: x.received, bps: s.bps ? s.bps * 0.6 + bps * 0.4 : bps });
    }
  }
  $("dir").textContent = snap.dir;
  const q = query.trim().toLowerCase();
  const items = snap.items.filter((x) => !q || x.name.toLowerCase().includes(q) || x.url.toLowerCase().includes(q));
  if (!items.length) {
    $("list").innerHTML = `<div class="empty">${icons.download}<p>${q ? "검색 결과가 없습니다" : "아직 받은 파일이 없습니다"}</p></div>`;
    return;
  }
  let html = "";
  let last = "";
  const active = items.filter((x) => x.state === "progress");
  if (active.length) html += `<h2>받는 중 <small>${active.length}</small></h2>` + active.map(row).join("");
  for (const x of items.filter((i) => i.state !== "progress")) {
    const d = day(x.ended ?? x.started);
    if (d !== last) {
      html += `<h2>${d}</h2>`;
      last = d;
    }
    html += row(x);
  }
  $("list").innerHTML = html;
}

async function refresh() {
  const s = await call<DownloadsSnapshot>("downloads_list");
  if (s) {
    snap = s;
    render();
  }
}

$("list").addEventListener("click", async (e) => {
  const a = (e.target as HTMLElement).closest<HTMLElement>("[data-a]");
  const item = (e.target as HTMLElement).closest<HTMLElement>(".item");
  if (!a || !item) return;
  e.preventDefault();
  await call("download_action", { id: Number(item.dataset.id), what: a.dataset.a });
  if (a.dataset.a === "remove") refresh();
});
$("list").addEventListener("dblclick", (e) => {
  const item = (e.target as HTMLElement).closest<HTMLElement>(".item.done:not(.missing)");
  if (item && !(e.target as HTMLElement).closest("button, a")) call("download_action", { id: Number(item.dataset.id), what: "open" });
});
$("q").addEventListener("input", () => {
  query = $<HTMLInputElement>("q").value;
  render();
});
$("open-dir").onclick = () => call("downloads_open_dir");
$("clear").onclick = () => call("downloads_clear");
$("change").onclick = (e) => {
  e.preventDefault();
  call("content_event", { kind: "shortcut", action: "open_settings" });
};

getCurrentWebview().listen<DownloadsSnapshot>("downloads", (e) => {
  // `missing` is only checked on a full read: keep what we knew.
  const gone = new Set(snap.items.filter((x) => x.missing).map((x) => x.id));
  snap = { ...e.payload, items: e.payload.items.map((x) => ({ ...x, missing: x.missing || gone.has(x.id) })) };
  render();
});
refresh();
// Files can be moved or deleted while this page is open.
window.addEventListener("focus", refresh);
