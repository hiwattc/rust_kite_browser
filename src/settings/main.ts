// kite://settings
import { call, esc, flash, invoke } from "../shared/page";
import { icons } from "../shared/icons";
import { architecture, diagram, oss } from "./about";
import { THEMES, applyTheme, initTheme } from "../shared/theme";
import "./settings.css";

initTheme();

interface ExtInfo {
  id: string;
  name: string;
  version: string;
  description: string;
  popup: string | null;
  options: string | null;
  icon: string | null;
  source: string;
  manifestVersion: number;
}
interface Extensions {
  supported: boolean;
  items: ExtInfo[];
}

interface SavedLogin {
  id: number;
  origin: string;
  username: string;
  lastUsed: number;
}
interface Passwords {
  enabled: boolean;
  items: SavedLogin[];
  never: string[];
}

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
  theme: string;
  launcherEnabled: boolean;
  launcherShortcut: string;
  launcherLabel: string;
  launcherDefault: string;
  launcherDefaultLabel: string;
  section: string | null;
}

document.title = "설정";

/** A small browser drawing in the theme's colors. `default` shows its light and dark halves. */
function themeCard(id: string, name: string, c: [string, string, string, string], dark: boolean, on: boolean) {
  const mini = (strip: string, bar: string, accent: string, text: string) =>
    `<span class="tp-win" style="--s:${strip};--b:${bar};--a:${accent};--t:${text}"><span class="tp-strip"><i></i><i class="on"></i></span><span class="tp-bar"><span class="tp-addr"></span><span class="tp-dot"></span></span><span class="tp-body"><span class="tp-line"></span><span class="tp-line short"></span></span></span>`;
  const art = id === "default" ? `<span class="tp-split">${mini(...c)}${mini("#17191f", "#24272f", "#9b84ff", "#e8eaf0")}</span>` : mini(...c);
  const sub = id === "default" ? "시스템 따라감" : dark ? "다크" : "라이트";
  return `<button class="theme${on ? " on" : ""}" data-theme-id="${id}" aria-pressed="${on}">${art}<span class="tp-name">${esc(name)}${on ? `<span class="tp-check">✓</span>` : ""}</span><span class="tp-sub">${sub}</span></button>`;
}

const SECTIONS = [
  ["general", "일반", icons.globe],
  ["appearance", "모양", icons.tab],
  ["bookmarks", "북마크", icons.bookmark],
  ["extensions", "확장 프로그램", icons.puzzle],
  ["passwords", "비밀번호", icons.key],
  ["ai", "AI 자동화 (MCP)", icons.bot],
  ["privacy", "개인정보", icons.lock],
  ["shortcuts", "단축키", icons.command],
  ["system", "시스템 구성도", icons.diagram],
  ["architecture", "구현 아키텍처", icons.layers],
  ["opensource", "오픈소스", icons.code],
  ["about", "Kite 정보", icons.settings],
] as const;

let s: Settings;
let ext: Extensions = { supported: true, items: [] };
let extBusy = false;
let pw: Passwords = { enabled: true, items: [], never: [] };
let pwQuery = "";
const revealed = new Map<number, string>();
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
        <div class="row"><div class="label"><b>다운로드 위치</b><small id="dl-dir">…</small></div>
          <button class="btn" id="dl-open">폴더 열기</button><button class="btn" id="dl-pick">변경…</button><button class="btn" id="dl-reset" title="운영체제의 다운로드 폴더로">기본값</button></div>
        <div class="row"><div class="label"><b>어디서나 스포트라이트 열기</b><small>다른 앱을 쓰는 중에도 이 단축키를 누르면 Kite 검색창이 화면 위에 뜹니다(Spotlight·Everything처럼). Esc나 다른 곳을 누르면 닫힙니다. 단축키를 눌러 바꿀 수 있고, F1~F24는 하나만으로도 됩니다${isMac ? "(맥북 키보드는 fn과 함께)" : ""}.</small></div>
          <div class="shortcut-ctl">${s.launcherEnabled ? `<button class="btn kbd-btn" id="launcher-key" title="눌러서 다른 키로 바꾸기"><kbd>${esc(s.launcherLabel)}</kbd></button>${s.launcherShortcut !== s.launcherDefault ? `<button class="link-btn" id="launcher-reset" title="${esc(s.launcherDefaultLabel)}">기본값</button>` : ""}` : ""}${sw("launcher", s.launcherEnabled)}</div></div>
      </section>

      <section class="card" id="appearance">
        <h2>모양</h2>
        <div class="row"><div class="label"><b>북마크 바 표시</b><small>툴바 아래에 북마크를 보여 줍니다 · 단축키 <kbd>${isMac ? "⇧⌘B" : "Ctrl+Shift+B"}</kbd></small></div>${sw("bbar", s.bookmarkBar)}</div>
        <div class="row theme-row"><div class="label"><b>테마</b><small>브라우저 전체(탭줄·툴바·스포트라이트·Kite 페이지)의 색 · ‘기본’은 운영체제의 라이트/다크 설정을 따릅니다</small></div></div>
        <div class="themes">${THEMES.map((t) => themeCard(t.id, t.name, t.colors, t.dark, t.id === s.theme)).join("")}</div>
      </section>

      <section class="card" id="bookmarks">
        <h2>북마크</h2>
        <div class="row"><div class="label"><b>북마크 관리자</b><small>북마크 ${s.bookmarkCount}개 · 폴더 정리, 편집, 삭제</small></div>
          <a class="btn" href="#" id="open-bm">${icons.bookmark}열기</a></div>
        <div class="row"><div class="label"><b>다른 브라우저에서 가져오기</b><small>이 PC의 Chrome · Edge · Whale · Brave 북마크 (이미 있는 주소는 건너뜀)</small></div>
          <button class="btn" id="import">가져오기</button></div>
      </section>

      <section class="card" id="extensions">
        <h2>확장 프로그램</h2>
        <p class="desc">크롬 웹 스토어의 확장 프로그램을 설치합니다. 엔진은 ${isMac ? "macOS WebKit(Safari와 같은 확장 엔진)" : "WebView2(Edge와 같은 확장 엔진)"}이라 크롬과 똑같이 동작하지 않는 확장도 있습니다. 툴바 팝업은 새 탭으로 열립니다.</p>
        <div id="ext-body">${extBody()}</div>
      </section>

      <section class="card" id="passwords">
        <h2>비밀번호</h2>
        <p class="desc">로그인하면 저장할지 묻고, 다음에 같은 사이트(주소의 출처가 정확히 같을 때)에 들어오면 아이디와 비밀번호를 자동으로 입력합니다. 비밀번호는 암호화해서 저장하고, 키는 ${isMac ? "macOS 키체인" : "Windows 자격 증명 관리자"}에 둡니다.</p>
        <div id="pw-body">${pwBody()}</div>
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
  bindExt();
  bindPw();
  markNav();
}

const isMac = navigator.platform.toLowerCase().includes("mac");

function extBody() {
  if (!ext.supported) return `<div class="row"><div class="label"><b>지원하지 않는 운영체제 버전</b><small>macOS 15.4 이상에서 쓸 수 있습니다</small></div></div>`;
  const list = ext.items.length
    ? ext.items
        .map(
          (e) => `<div class="row ext">
        ${e.icon ? `<img class="ext-icon" src="${esc(e.icon)}" alt=""/>` : `<span class="ext-icon">${icons.puzzle}</span>`}
        <div class="label"><b>${esc(e.name)} <span class="ver">${esc(e.version)}</span>${e.source === "local" ? ` <span class="ver">폴더</span>` : ""}</b><small>${esc(e.description)}</small><small class="mono">${esc(e.id)}</small></div>
        ${e.popup ? `<button class="btn" data-open="${esc(e.id)}" data-page="popup">팝업</button>` : ""}
        ${e.options ? `<button class="btn" data-open="${esc(e.id)}" data-page="options">옵션</button>` : ""}
        <button class="btn danger" data-remove="${esc(e.id)}">삭제</button>
      </div>`,
        )
        .join("")
    : `<div class="row"><div class="label"><small>설치한 확장 프로그램이 없습니다.</small></div></div>`;
  return `
    <div class="row"><div class="label"><b>크롬 웹 스토어에서 설치</b><small>스토어의 확장 프로그램 주소(또는 32자 ID)를 붙여 넣으세요. 스토어 페이지를 보면서 스포트라이트 <kbd>&gt; 확장 프로그램 설치</kbd>도 됩니다</small></div></div>
    <div class="codebox ext-install"><input type="text" id="ext-input" placeholder="https://chromewebstore.google.com/detail/…" ${extBusy ? "disabled" : ""}/>
      <button class="btn primary" id="ext-install" ${extBusy ? "disabled" : ""}>${extBusy ? "설치 중…" : "설치"}</button></div>
    <div class="row"><div class="label"><b>폴더에서 설치</b><small>압축을 푼 확장 프로그램(manifest.json이 있는 폴더) · 개발 중인 확장</small></div>
      <a class="btn" href="https://chromewebstore.google.com/" target="_blank" rel="noopener">웹 스토어 열기</a><button class="btn" id="ext-folder">폴더 고르기</button></div>
    ${list}`;
}

function host(origin: string) {
  try {
    return new URL(origin).host;
  } catch {
    return origin;
  }
}

function pwBody() {
  const q = pwQuery.toLowerCase();
  const items = pw.items.filter((l) => !q || l.origin.toLowerCase().includes(q) || l.username.toLowerCase().includes(q));
  const rows = items.length
    ? items
        .map(
          (l) => `<div class="row pw">
        <div class="label"><b>${esc(host(l.origin))}</b><small>${esc(l.origin)}</small></div>
        <div class="pw-user">${esc(l.username || "(아이디 없음)")}</div>
        <code class="pw-secret">${revealed.has(l.id) ? esc(revealed.get(l.id)!) : "••••••••"}</code>
        <button class="btn" data-pw-show="${l.id}">${revealed.has(l.id) ? "가리기" : "보기"}</button>
        <button class="btn" data-pw-copy="${l.id}">복사</button>
        <button class="btn danger" data-pw-del="${l.id}">삭제</button>
      </div>`,
        )
        .join("")
    : `<div class="row"><div class="label"><small>${pw.items.length ? "찾는 비밀번호가 없습니다." : "저장한 비밀번호가 없습니다. 사이트에 로그인하면 저장할지 묻습니다."}</small></div></div>`;
  const never = pw.never.length
    ? `<div class="row"><div class="label"><b>저장하지 않는 사이트</b><small>${pw.never.map((o) => `<span class="chip">${esc(host(o))}<button data-pw-never="${esc(o)}" title="목록에서 빼기">×</button></span>`).join("")}</small></div></div>`
    : "";
  return `
    <div class="row"><div class="label"><b>비밀번호 저장 제안과 자동 입력</b><small>끄면 저장을 묻지 않고 자동 입력도 하지 않습니다 (저장된 비밀번호는 그대로)</small></div>${sw("pw-enabled", pw.enabled)}</div>
    ${pw.items.length > 5 ? `<div class="codebox"><input type="search" id="pw-search" placeholder="사이트나 아이디 검색" value="${esc(pwQuery)}"/></div>` : ""}
    ${rows}
    ${never}`;
}

function renderPw() {
  const el = document.getElementById("pw-body");
  if (!el) return;
  el.innerHTML = pwBody();
  bindPw();
}

function bindPw() {
  const en = document.getElementById("pw-enabled") as HTMLInputElement | null;
  if (en)
    en.onchange = async () => {
      await call("settings_set", { key: "passwords_enabled", value: String(en.checked) });
      pw = (await call<Passwords>("passwords_list")) ?? pw;
      renderPw();
    };
  const search = document.getElementById("pw-search") as HTMLInputElement | null;
  if (search)
    search.oninput = () => {
      pwQuery = search.value;
      const pos = search.selectionStart;
      renderPw();
      const again = document.getElementById("pw-search") as HTMLInputElement | null;
      again?.focus();
      again?.setSelectionRange(pos, pos);
    };
  document.querySelectorAll<HTMLElement>("[data-pw-show]").forEach(
    (b) =>
      (b.onclick = async () => {
        const id = Number(b.dataset.pwShow);
        if (revealed.has(id)) revealed.delete(id);
        else {
          const v = await call<string>("password_reveal", { id });
          if (v == null) return;
          revealed.set(id, v);
          // Do not leave passwords on screen.
          setTimeout(() => {
            revealed.delete(id);
            renderPw();
          }, 20000);
        }
        renderPw();
      }),
  );
  document.querySelectorAll<HTMLElement>("[data-pw-copy]").forEach(
    (b) =>
      (b.onclick = async () => {
        const v = await call<string>("password_reveal", { id: Number(b.dataset.pwCopy) });
        if (v == null) return;
        await call("copy_text", { text: v });
        flash("비밀번호를 복사했습니다.");
      }),
  );
  document.querySelectorAll<HTMLElement>("[data-pw-del]").forEach(
    (b) =>
      (b.onclick = () =>
        confirmTwice(b, async () => {
          const r = await call<Passwords>("password_delete", { id: Number(b.dataset.pwDel) });
          if (r) {
            pw = r;
            renderPw();
            flash("비밀번호를 삭제했습니다.");
          }
        })),
  );
  document.querySelectorAll<HTMLElement>("[data-pw-never]").forEach(
    (b) =>
      (b.onclick = async () => {
        const r = await call<Passwords>("password_never_remove", { origin: b.dataset.pwNever });
        if (r) {
          pw = r;
          renderPw();
        }
      }),
  );
}

function renderExt() {
  const el = document.getElementById("ext-body");
  if (!el) return;
  el.innerHTML = extBody();
  bindExt();
}

function bindExt() {
  const input = document.getElementById("ext-input") as HTMLInputElement | null;
  const install = async () => {
    const v = input?.value.trim();
    if (!v || extBusy) return;
    extBusy = true;
    renderExt();
    const r = await call<Extensions>("extension_install", { input: v });
    extBusy = false;
    if (r) ext = r;
    renderExt();
  };
  document.getElementById("ext-install")?.addEventListener("click", install);
  input?.addEventListener("keydown", (e) => e.key === "Enter" && install());
  document.getElementById("ext-folder")?.addEventListener("click", async () => {
    const r = await call<Extensions | null>("extension_install_folder");
    if (r) {
      ext = r;
      renderExt();
    }
  });
  document.querySelectorAll<HTMLElement>("[data-open]").forEach((b) => (b.onclick = () => call("extension_open", { id: b.dataset.open, page: b.dataset.page })));
  document.querySelectorAll<HTMLElement>("[data-remove]").forEach(
    (b) =>
      (b.onclick = () =>
        confirmTwice(b, async () => {
          const r = await call<Extensions>("extension_remove", { id: b.dataset.remove });
          if (r) {
            ext = r;
            renderExt();
            flash("확장 프로그램을 삭제했습니다.");
          }
        })),
  );
}

/** Next key combination pressed (with at least one modifier) becomes the global shortcut. */
function recordShortcut() {
  const btn = document.getElementById("launcher-key")!;
  btn.classList.add("recording");
  btn.innerHTML = "<kbd>키를 누르세요… (Esc: 취소)</kbd>";
  const done = () => {
    window.removeEventListener("keydown", onKey, true);
    btn.classList.remove("recording");
  };
  const onKey = (e: KeyboardEvent) => {
    e.preventDefault();
    e.stopPropagation();
    if (e.key === "Escape") {
      done();
      return render();
    }
    if (["Shift", "Control", "Alt", "Meta"].includes(e.key)) return;
    const key = e.code === "Space" ? "Space" : e.code.startsWith("Key") ? e.code.slice(3) : e.code.startsWith("Digit") ? e.code.slice(5) : /^F\d+$/.test(e.code) ? e.code : "";
    if (!key) return;
    const mods = [e.ctrlKey && "Ctrl", e.altKey && "Alt", e.shiftKey && "Shift", e.metaKey && (isMac ? "Cmd" : "Super")].filter(Boolean);
    // A function key alone (F9) is fine; any other key needs a modifier, or typing it would be lost.
    if (!mods.length && !/^F\d+$/.test(key)) {
      btn.innerHTML = `<kbd>${isMac ? "⌘·⌥·⌃·⇧" : "Ctrl·Alt·Shift·Win"} 중 하나와 함께, 또는 F1~F24 하나만</kbd>`;
      return;
    }
    done();
    set("launcher_shortcut", [...mods, key].join("+"), "전역 단축키를 바꿨습니다.");
  };
  window.addEventListener("keydown", onKey, true);
}

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
  document.querySelectorAll<HTMLElement>(".theme").forEach((b) => {
    b.onclick = () => {
      const id = b.dataset.themeId!;
      if (id === s.theme) return;
      applyTheme(id); // at once here; the other pages follow the event
      set("theme", id, `테마: ${THEMES.find((t) => t.id === id)?.name ?? id}`);
    };
  });
  $<HTMLSelectElement>("engine").onchange = (e) => set("search_engine", (e.target as HTMLSelectElement).value, "검색 엔진을 바꿨습니다.");
  $<HTMLInputElement>("launcher").onchange = (e) => set("launcher_enabled", String((e.target as HTMLInputElement).checked), (e.target as HTMLInputElement).checked ? "전역 단축키를 켰습니다." : "전역 단축키를 껐습니다.");
  document.getElementById("launcher-key")?.addEventListener("click", recordShortcut);
  document.getElementById("launcher-reset")?.addEventListener("click", () => set("launcher_shortcut", "", "전역 단축키를 기본값으로 되돌렸습니다."));
  $<HTMLInputElement>("restore").onchange = (e) => set("restore_session", String((e.target as HTMLInputElement).checked));
  call<{ dir: string }>("downloads_list").then((d) => d && ($("dl-dir").textContent = d.dir));
  $("dl-open").onclick = () => call("downloads_open_dir");
  const pickDir = async (reset: boolean) => {
    const dir = await call<string>("downloads_pick_dir", { reset });
    if (dir) $("dl-dir").textContent = dir;
  };
  $("dl-pick").onclick = () => pickDir(false);
  $("dl-reset").onclick = () => pickDir(true);
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
  ext = (await call<Extensions>("extensions_list")) ?? ext;
  pw = (await call<Passwords>("passwords_list")) ?? pw;
  render();
  const sec = r.section || location.hash.slice(1);
  if (sec) {
    document.getElementById(sec)?.scrollIntoView();
    markNav(sec);
  }
}
main();
