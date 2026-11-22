// System diagram (Settings → 시스템 구성도). No imports: `docs/architecture.svg` is generated from
// this with `node scripts/diagram.mjs`, so the README and the app show the same picture.

const W = 900;
const H = 728;

/** Rust modules, top to bottom (name, what it does). */
const CORE: [string, string][] = [
  ["browser.rs", "탭·창·웹뷰 배치, 상태"],
  ["keymap.rs", "단축키 표"],
  ["palette.rs · hangul.rs", "검색·한영 보정"],
  ["omnibox.rs", "주소창 자동완성"],
  ["launcher.rs", "전역 스포트라이트"],
  ["automation.rs", "탭에서 JS 실행"],
  ["mcp.rs", "MCP 서버·도구"],
  ["pages.rs", "내부 페이지 명령"],
  ["portal.rs", "홈(개인 포털)·사이트 아이콘"],
  ["neterror.rs", "접속 불가 진단·인증서 예외"],
  ["siteinfo.rs", "사이트 인증서 정보"],
  ["extensions.rs", "확장 프로그램"],
  ["passwords.rs", "비밀번호 (키체인)"],
  ["meet.rs", "영상회의 서버·중계"],
  ["session_cookies.rs", "로그인 유지"],
  ["downloads.rs", "다운로드 진행률·기록"],
  ["store.rs", "SQLite 저장소"],
  ["platform.rs", "스크린샷·확장 엔진·OS 차이"],
];
const PILL_Y0 = 86;
const PILL_STEP = 27;
const pillY = (name: string) => PILL_Y0 + CORE.findIndex(([n]) => n === name) * PILL_STEP;

const pill = ([name, what]: [string, string], i: number) => {
  const y = PILL_Y0 + i * PILL_STEP;
  return (
    `<rect class="d-pill" x="200" y="${y}" width="310" height="22" rx="6"/>` +
    `<text class="d-mono" x="210" y="${y + 15}">${name}</text><text class="d-dim" x="500" y="${y + 15}" text-anchor="end">${what}</text>`
  );
};

/** Webviews (label, what it shows, height). */
const VIEWS: [string, string, number][] = [
  ["ui", "탭줄 · 툴바 · 북마크 바", 38],
  ["side", "세로 탭 · 북마크 목록", 38],
  ["omni", "주소창 추천 목록", 38],
  ["palette · launcher", "스포트라이트 (창 안 · 떠 있는 창)", 38],
  ["siteinfo", "사이트 정보 말풍선 창", 38],
  ["cover", "되살리는 탭의 마지막 화면", 38],
  ["tab-N", "웹 페이지 + content.js", 50],
  ["popup-N", "로그인·결제 팝업 창", 38],
  ["kite://", "home · settings · bookmarks · downloads · meet …", 50],
];
const viewTop: number[] = [];
{
  let y = 86;
  for (const v of VIEWS) {
    viewTop.push(y);
    y += v[2] + 8;
  }
}
const view = ([name, what, h]: [string, string, number], i: number) => {
  const y = viewTop[i];
  const stack = name === "tab-N" || name === "popup-N" ? `<rect class="d-box d-stack" x="616" y="${y + 5}" width="254" height="${h}" rx="8"/>` : "";
  return `${stack}<rect class="d-box" x="610" y="${y}" width="254" height="${h}" rx="8"/><text class="d-mono" x="622" y="${y + 16}">${name}</text><text class="d-dim" x="622" y="${y + 31}">${what}</text>`;
};
const vy = (name: string, dy = 0) => viewTop[VIEWS.findIndex(([n]) => n === name)] + dy;

const arrow = (y: number, label: string, dir: "both" | "right" | "left") => {
  const [a, b] = dir === "left" ? [610, 520] : [520, 610];
  const start = dir === "both" ? ' marker-start="url(#d-head)"' : "";
  return `<line class="d-line" x1="${a}" y1="${y}" x2="${b}" y2="${y}" marker-end="url(#d-head)"${start}/>${label ? `<text class="d-label" x="565" y="${y - 5}" text-anchor="middle">${label}</text>` : ""}`;
};

/** A box outside the app on the left, with arrows into the given modules. */
const outside = (y: number, h: number, title: string, sub: string, note: string, to: string[], both = false) =>
  `<rect class="d-ext" x="16" y="${y}" width="150" height="${h}" rx="10"/>` +
  `<text class="d-head" x="91" y="${y + 22}" text-anchor="middle">${title}</text>` +
  `<text class="d-dim" x="91" y="${y + 39}" text-anchor="middle">${sub}</text>` +
  `<text class="d-label" x="91" y="${y + h + 14}" text-anchor="middle">${note}</text>` +
  to.map((m) => `<line class="d-line" x1="166" y1="${pillY(m) + 11}" x2="200" y2="${pillY(m) + 11}" marker-end="url(#d-head)"${both ? ' marker-start="url(#d-head)"' : ""}/>`).join("");

const coreBottom = PILL_Y0 + CORE.length * PILL_STEP + 4;
const appBottom = Math.max(coreBottom, viewTop[viewTop.length - 1] + VIEWS[VIEWS.length - 1][2]) + 18;
const osY = appBottom + 18;

export const diagram = `
<svg class="diagram" viewBox="0 0 ${W} ${H}" role="img" aria-label="Kite 시스템 구성도">
  <defs><marker id="d-head" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0L10 5L0 10z" class="d-headfill"/></marker></defs>

  <rect class="d-app" x="182" y="16" width="${W - 198}" height="${appBottom - 16}" rx="14"/>
  <text class="d-title" x="198" y="40">Kite 앱 · Tauri v2 (프로세스 하나, 창 여러 개)</text>

  <rect class="d-box" x="190" y="56" width="330" height="${coreBottom - 56 + 8}" rx="10"/>
  <text class="d-head" x="202" y="76">Rust 코어</text>
  ${CORE.map(pill).join("\n  ")}

  <text class="d-head" x="610" y="76">웹뷰 (OS 웹 엔진, 창마다)</text>
  ${VIEWS.map(view).join("\n  ")}

  ${arrow(vy("ui", 19), "invoke · 이벤트", "both")}
  ${arrow(vy("side", 19), "", "both")}
  ${arrow(vy("omni", 19), "", "both")}
  ${arrow(vy("palette · launcher", 19), "", "both")}
  ${arrow(vy("siteinfo", 19), "", "both")}
  ${arrow(vy("cover", 19), "", "both")}
  ${arrow(vy("tab-N", 18), "JS 실행", "right")}
  ${arrow(vy("tab-N", 40), "content_event", "left")}
  ${arrow(vy("kite://", 28), "invoke (로컬만)", "both")}

  ${outside(pillY("mcp.rs") - 34, 56, "AI 에이전트", "Claude Code 등", "HTTP · 127.0.0.1 · Bearer", ["mcp.rs"])}
  ${outside(pillY("portal.rs") - 8, 84, "웹사이트", "아이콘 · 인증서 · 진단", "Kite가 직접 연결 (HTTPS)", ["portal.rs", "neterror.rs", "siteinfo.rs"])}
  ${outside(pillY("meet.rs") - 20, 56, "회의 참가자", "다른 Kite", "TCP · UPnP · WebRTC", ["meet.rs"], true)}

  <rect class="d-os" x="16" y="${osY}" width="${W - 32}" height="92" rx="12"/>
  <text class="d-title" x="30" y="${osY + 82}">운영체제 (macOS · Windows)</text>
  <rect class="d-box" x="30" y="${osY + 14}" width="140" height="40" rx="8"/>
  <text class="d-dim" x="100" y="${osY + 38}" text-anchor="middle">외부 앱 · f5-epi: 등</text>
  <rect class="d-box" x="190" y="${osY + 14}" width="330" height="40" rx="8"/>
  <text class="d-dim" x="355" y="${osY + 31}" text-anchor="middle">kite.db (SQLite) · mcp-token · session-cookies.json</text>
  <text class="d-dim" x="355" y="${osY + 46}" text-anchor="middle">키체인 / 자격 증명 관리자 (비밀번호 키)</text>
  <rect class="d-box" x="610" y="${osY + 14}" width="254" height="40" rx="8"/>
  <text class="d-dim" x="737" y="${osY + 38}" text-anchor="middle">WKWebView · WebView2</text>

  <line class="d-line" x1="355" y1="${coreBottom + 8}" x2="355" y2="${osY + 14}" marker-end="url(#d-head)"/>
  <text class="d-label" x="361" y="${osY - 4}">읽기·쓰기</text>
  <path class="d-line" d="M190 ${coreBottom - 6} H100 V${osY + 14}" fill="none" marker-end="url(#d-head)"/>
  <text class="d-label" x="106" y="${osY - 4}">확인 후 열기</text>
  <line class="d-line d-dash" x1="737" y1="${appBottom}" x2="737" y2="${osY + 14}"/>
  <text class="d-label" x="743" y="${osY + 6}">렌더링</text>
</svg>`;
