// kite://settings
import { call, esc, flash, invoke } from "../shared/page";
import { icons } from "../shared/icons";
import { architecture, diagram, oss } from "./about";

interface Settings {
  searchEngine: string;
  engines: [string, string][];
  restoreSession: boolean;
  bookmarkBar: boolean;
  mcpEnabled: boolean;
  mcpRunning: boolean;
  mcpPort: number;
  mcpUrl: string;
  mcpCommand: string;
  dataDir: string;
  version: string;
  shortcuts: { title: string; keys: string[] }[];
  bookmarkCount: number;
}

document.title = "설정";

const SECTIONS = [
  ["general", "일반", icons.globe],
  ["appearance", "모양", icons.tab],
  ["bookmarks", "북마크", icons.bookmark],
  ["ai", "AI 자동화 (MCP)", icons.bot],
  ["privacy", "개인정보", icons.lock],
  ["shortcuts", "단축키", icons.command],
  ["system", "시스템 구성도", icons.diagram],
  ["architecture", "구현 아키텍처", icons.layers],
  ["opensource", "오픈소스", icons.code],
  ["about", "Kite 정보", icons.settings],
] as const;

let s: Settings;
let showToken = false;

const sw = (id: string, on: boolean) => `<label class="switch"><input type="checkbox" id="${id}" ${on ? "checked" : ""}/><span></span></label>`;
const masked = (cmd: string) => (showToken ? cmd : cmd.replace(/Bearer [0-9a-f]+/, "Bearer ••••••••••••••••"));

function render() {
  const nav = SECTIONS.map(([id, name, ic]) => `<a class="nav" href="#${id}" data-sec="${id}">${ic}<span>${name}</span></a>`).join("");
  document.body.innerHTML = `
  <div class="page">
    <aside class="side"><h1>${icons.settings}설정</h1>${nav}</aside>
    <main>
      <section class="card" id="general">
        <h2>일반</h2>
        <div class="row"><div class="label"><b>검색 엔진</b><small>주소창과 스포트라이트에서 주소가 아닌 글자를 입력했을 때</small></div>
          <select id="engine">${s.engines.map(([id, name]) => `<option value="${id}" ${id === s.searchEngine ? "selected" : ""}>${esc(name)}</option>`).join("")}</select></div>
        <div class="row"><div class="label"><b>시작할 때 이전 탭 다시 열기</b><small>끄면 새 탭 하나로 시작합니다</small></div>${sw("restore", s.restoreSession)}</div>
      </section>

      <section class="card" id="appearance">
        <h2>모양</h2>
        <div class="row"><div class="label"><b>북마크 바 표시</b><small>툴바 아래에 북마크를 보여 줍니다 · 단축키 <kbd>${isMac ? "⇧⌘B" : "Ctrl+Shift+B"}</kbd></small></div>${sw("bbar", s.bookmarkBar)}</div>
        <div class="row"><div class="label"><b>테마</b><small>운영체제의 라이트/다크 설정을 따릅니다</small></div></div>
      </section>

      <section class="card" id="bookmarks">
        <h2>북마크</h2>
        <div class="row"><div class="label"><b>북마크 관리자</b><small>북마크 ${s.bookmarkCount}개 · 폴더 정리, 편집, 삭제</small></div>
          <a class="btn" href="#" id="open-bm">${icons.bookmark}열기</a></div>
        <div class="row"><div class="label"><b>다른 브라우저에서 가져오기</b><small>이 PC의 Chrome · Edge · Whale · Brave 북마크 (이미 있는 주소는 건너뜀)</small></div>
          <button class="btn" id="import">가져오기</button></div>
      </section>

      <section class="card" id="ai">
        <h2>AI 자동화 (MCP)</h2>
        <p class="desc">Claude Code 같은 AI 에이전트가 이 브라우저의 탭을 조작합니다. 이 PC 안(127.0.0.1)에서 토큰을 가진 프로그램만 접속할 수 있습니다.</p>
        <div class="row"><div class="label"><b>MCP 서버</b><small><span class="status ${s.mcpRunning ? "on" : ""}">${s.mcpRunning ? `실행 중 · ${esc(s.mcpUrl)}` : "꺼짐"}</span></small></div>${sw("mcp", s.mcpEnabled)}</div>
        <div class="row"><div class="label"><b>포트</b><small>바꾸면 서버를 다시 시작하고 연결 명령도 바뀝니다</small></div>
          <input type="number" id="port" min="1024" max="65535" value="${s.mcpPort}" style="width:110px"/><button class="btn" id="port-save">적용</button></div>
        <div class="row"><div class="label"><b>Claude Code 연결 명령</b><small>터미널에 붙여 넣으면 Claude Code에 kite 서버가 등록됩니다</small></div>
          <button class="btn" id="show-token">${showToken ? "토큰 가리기" : "토큰 보기"}</button><button class="btn primary" id="copy-cmd">복사</button></div>
        <div class="codebox"><code>${esc(masked(s.mcpCommand))}</code></div>
        <div class="row"><div class="label"><b>토큰 새로 만들기</b><small>예전 토큰으로 등록한 에이전트는 연결이 끊어집니다 (다시 등록 필요)</small></div>
          <button class="btn danger" id="renew">새 토큰</button></div>
      </section>

      <section class="card" id="privacy">
        <h2>개인정보</h2>
        <div class="row"><div class="label"><b>방문 기록 지우기</b><small>스포트라이트의 방문 기록이 모두 지워집니다</small></div><button class="btn danger" id="clear-history">지우기</button></div>
        <div class="row"><div class="label"><b>쿠키 및 사이트 데이터 지우기</b><small>모든 사이트의 쿠키·캐시·저장소를 지웁니다. 로그인한 사이트에서 로그아웃됩니다</small></div><button class="btn danger" id="clear-site">지우기</button></div>
      </section>

      <section class="card" id="shortcuts">
        <h2>단축키</h2>
        <p class="desc">페이지 안에 커서가 있어도, 한글 입력 상태에서도 동작합니다.</p>
        ${s.shortcuts.map((k) => `<div class="row" style="min-height:40px"><div class="label">${esc(k.title)}</div><div>${k.keys.map((x) => `<kbd>${esc(x)}</kbd>`).join("")}</div></div>`).join("")}
      </section>

      <section class="card" id="system">
        <h2>시스템 구성도</h2>
        <p class="desc">Kite는 Rust 코어 하나와 OS 웹 엔진으로 그리는 웹뷰 여러 개로 이루어져 있습니다. 웹 페이지는 탭 웹뷰 안에서만 돌고, 앱 기능에는 정해진 통로로만 닿습니다.</p>
        <div class="diagram-wrap">${diagram}</div>
      </section>

      <section class="card" id="architecture">
        <h2>구현 아키텍처</h2>
        <p class="desc">Tauri v2 + OS 웹뷰(macOS WKWebView, Windows WebView2) + TypeScript(프레임워크 없음)</p>
        ${architecture}
      </section>

      <section class="card" id="opensource">
        <h2>오픈소스</h2>
        <p class="desc">Kite가 직접 사용하는 오픈소스와 라이선스입니다(간접 의존성 포함 Rust 크레이트 약 540개). 웹 엔진 WKWebView·WebView2는 운영체제 구성 요소입니다. 이름을 누르면 프로젝트 페이지가 새 탭에서 열립니다.</p>
        ${oss}
      </section>

      <section class="card" id="about">
        <h2>Kite 정보</h2>
        <div class="row"><div class="label"><b>버전</b></div><span class="mono">${esc(s.version)}</span></div>
        <div class="row"><div class="label"><b>데이터 폴더</b><small class="mono">${esc(s.dataDir)}</small></div></div>
      </section>
    </main>
  </div>`;
  bind();
  markNav();
}

const isMac = navigator.platform.toLowerCase().includes("mac");

async function set(key: string, value: string, msg?: string) {
  const r = await call<Settings>("settings_set", { key, value });
  if (r) {
    s = r;
    render();
    if (msg) flash(msg);
  } else {
    render(); // put the control back to the saved value
  }
}

function confirmTwice(btn: HTMLElement, run: () => void) {
  if (btn.dataset.armed) {
    run();
    return;
  }
  btn.dataset.armed = "1";
  const old = btn.textContent;
  btn.textContent = "한 번 더 누르면 실행";
  setTimeout(() => {
    delete btn.dataset.armed;
    btn.textContent = old;
  }, 3000);
}

function bind() {
  const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
  $<HTMLSelectElement>("engine").onchange = (e) => set("search_engine", (e.target as HTMLSelectElement).value, "검색 엔진을 바꿨습니다.");
  $<HTMLInputElement>("restore").onchange = (e) => set("restore_session", String((e.target as HTMLInputElement).checked));
  $<HTMLInputElement>("bbar").onchange = (e) => set("bookmark_bar", String((e.target as HTMLInputElement).checked));
  $<HTMLInputElement>("mcp").onchange = (e) => set("mcp_enabled", String((e.target as HTMLInputElement).checked));
  $("port-save").onclick = () => set("mcp_port", $<HTMLInputElement>("port").value);
  $("show-token").onclick = () => {
    showToken = !showToken;
    render();
  };
  $("copy-cmd").onclick = async () => {
    try {
      await invoke("copy_text", { text: s.mcpCommand });
      flash("연결 명령을 복사했습니다. 터미널에 붙여 넣으세요.");
    } catch (e) {
      flash(String(e));
    }
  };
  $("renew").onclick = (e) =>
    confirmTwice(e.currentTarget as HTMLElement, async () => {
      const r = await call<Settings>("mcp_renew_token");
      if (r) {
        s = r;
        render();
        flash("새 토큰을 만들었습니다. 연결 명령을 다시 복사해 등록하세요.");
      }
    });
  $("clear-history").onclick = (e) => confirmTwice(e.currentTarget as HTMLElement, async () => flash((await call<string>("clear_data", { what: "history" })) ?? ""));
  $("clear-site").onclick = (e) => confirmTwice(e.currentTarget as HTMLElement, async () => flash((await call<string>("clear_data", { what: "site_data" })) ?? ""));
  $("import").onclick = async () => {
    const msg = await call<string>("bookmarks_import");
    if (msg) {
      flash(msg);
      s = (await call<Settings>("settings_get")) ?? s;
      render();
    }
  };
  $("open-bm").onclick = (e) => {
    e.preventDefault();
    invoke("content_event", { kind: "shortcut", action: "open_bookmarks" });
  };
  document.querySelectorAll<HTMLAnchorElement>(".nav").forEach((a) => {
    a.onclick = (e) => {
      e.preventDefault();
      document.getElementById(a.dataset.sec!)?.scrollIntoView({ behavior: "smooth", block: "start" });
      history.replaceState(null, "", `#${a.dataset.sec}`);
      markNav(a.dataset.sec);
    };
  });
}

function markNav(sec?: string) {
  const cur = sec ?? (location.hash.slice(1) || "general");
  document.querySelectorAll<HTMLElement>(".nav").forEach((a) => a.classList.toggle("on", a.dataset.sec === cur));
}

window.addEventListener("scroll", () => {
  let cur = "general";
  for (const [id] of SECTIONS) {
    const el = document.getElementById(id);
    if (el && el.getBoundingClientRect().top < 120) cur = id;
  }
  markNav(cur);
});

async function main() {
  const r = await call<Settings>("settings_get");
  if (!r) return;
  s = r;
  render();
  if (location.hash) document.getElementById(location.hash.slice(1))?.scrollIntoView();
}
main();
