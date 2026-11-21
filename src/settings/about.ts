// Settings → 시스템 구성도 / 구현 아키텍처 / 오픈소스: static content, no commands needed.
import { esc } from "../shared/page";

// ---------------------------------------------------------------- 시스템 구성도

const pill = (y: number, name: string, what: string) =>
  `<rect class="d-pill" x="200" y="${y}" width="190" height="22" rx="6"/>` +
  `<text class="d-mono" x="210" y="${y + 15}">${name}</text><text class="d-dim" x="380" y="${y + 15}" text-anchor="end">${what}</text>`;

const view = (y: number, h: number, name: string, what: string) =>
  `<rect class="d-box" x="530" y="${y}" width="190" height="${h}" rx="8"/>` +
  `<text class="d-mono" x="542" y="${y + 18}">${name}</text><text class="d-dim" x="542" y="${y + 34}">${what}</text>`;

const arrow = (y: number, label: string, dir: "both" | "right" | "left") => {
  const [a, b] = dir === "left" ? [530, 400] : [400, 530];
  const start = dir === "both" ? ' marker-start="url(#d-head)"' : "";
  return `<line class="d-line" x1="${a}" y1="${y}" x2="${b}" y2="${y}" marker-end="url(#d-head)"${start}/><text class="d-label" x="465" y="${y - 5}" text-anchor="middle">${label}</text>`;
};

export const diagram = `
<svg class="diagram" viewBox="0 0 760 460" role="img" aria-label="Kite 시스템 구성도">
  <defs><marker id="d-head" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0L10 5L0 10z" class="d-headfill"/></marker></defs>

  <rect class="d-app" x="176" y="16" width="568" height="330" rx="14"/>
  <text class="d-title" x="192" y="40">Kite 앱 · Tauri v2 (프로세스 하나, 창 하나)</text>

  <rect class="d-box" x="190" y="56" width="210" height="276" rx="10"/>
  <text class="d-head" x="202" y="76">Rust 코어</text>
  ${pill(86, "browser.rs", "탭 상태·웹뷰 배치")}
  ${pill(116, "keymap.rs", "단축키 표")}
  ${pill(146, "palette.rs", "검색·한영 보정")}
  ${pill(176, "automation.rs", "탭에서 JS 실행")}
  ${pill(206, "mcp.rs", "MCP 서버·도구")}
  ${pill(236, "pages.rs", "내부 페이지 명령")}
  ${pill(266, "store.rs", "SQLite 저장소")}
  ${pill(296, "platform.rs", "스크린샷·OS 차이")}

  <text class="d-head" x="530" y="76">웹뷰 (OS 웹 엔진)</text>
  ${view(86, 40, "ui", "탭줄 · 툴바 · 북마크 바")}
  ${view(136, 40, "palette", "스포트라이트 오버레이")}
  <rect class="d-box d-stack" x="536" y="192" width="190" height="56" rx="8"/>
  ${view(186, 56, "tab-N", "웹 페이지 + content.js")}
  ${view(256, 60, "kite://", "새 탭 · 설정 · 북마크")}

  ${arrow(106, "invoke · 이벤트", "both")}
  ${arrow(156, "invoke · 이벤트", "both")}
  ${arrow(204, "JS 실행", "right")}
  ${arrow(230, "content_event", "left")}
  ${arrow(286, "invoke (로컬만)", "both")}

  <rect class="d-ext" x="16" y="186" width="140" height="58" rx="10"/>
  <text class="d-head" x="86" y="210" text-anchor="middle">AI 에이전트</text>
  <text class="d-dim" x="86" y="228" text-anchor="middle">Claude Code 등</text>
  <line class="d-line" x1="156" y1="217" x2="200" y2="217" marker-end="url(#d-head)"/>
  <text class="d-label" x="86" y="262" text-anchor="middle">HTTP · 127.0.0.1</text>
  <text class="d-label" x="86" y="276" text-anchor="middle">Bearer 토큰</text>

  <rect class="d-os" x="16" y="364" width="728" height="84" rx="12"/>
  <text class="d-title" x="30" y="438">운영체제 (macOS · Windows)</text>
  <rect class="d-box" x="30" y="378" width="140" height="38" rx="8"/>
  <text class="d-dim" x="100" y="401" text-anchor="middle">외부 앱 · f5-epi: 등</text>
  <rect class="d-box" x="190" y="378" width="210" height="38" rx="8"/>
  <text class="d-dim" x="295" y="401" text-anchor="middle">kite.db (SQLite) · mcp-token</text>
  <rect class="d-box" x="530" y="378" width="190" height="38" rx="8"/>
  <text class="d-dim" x="625" y="401" text-anchor="middle">WKWebView · WebView2</text>

  <line class="d-line" x1="295" y1="332" x2="295" y2="378" marker-end="url(#d-head)"/>
  <text class="d-label" x="301" y="356">읽기·쓰기</text>
  <path class="d-line" d="M190 320 H100 V378" fill="none" marker-end="url(#d-head)"/>
  <text class="d-label" x="106" y="356">확인 후 열기</text>
  <line class="d-line d-dash" x1="625" y1="332" x2="625" y2="378"/>
  <text class="d-label" x="631" y="356">렌더링</text>
</svg>`;

// ---------------------------------------------------------------- 구현 아키텍처

const ARCH: [string, string][] = [
  [
    "창 하나, 웹뷰 여러 개",
    "창 안에 <code>ui</code>(탭줄·툴바), 탭마다 <code>tab-N</code>, 숨겨 둔 <code>palette</code> 웹뷰를 겹쳐 놓습니다. 보이는 탭만 화면에 배치하고, 복원된 탭은 처음 볼 때 웹뷰를 만듭니다. 웹 엔진은 OS 것(macOS WKWebView, Windows WebView2)을 그대로 써서 앱이 가볍습니다.",
  ],
  [
    "상태는 Rust가 소유",
    "탭 목록·주소·로딩 상태는 Rust(<code>browser.rs</code>)가 가지고, 바뀔 때마다 <code>browser-state</code> 이벤트로 UI에 보냅니다. UI는 받은 상태를 그리기만 하고 버튼은 명령(<code>invoke</code>)으로 Rust에 요청합니다.",
  ],
  [
    "단축키",
    "단축키 표는 <code>keymap.rs</code> 하나입니다. 페이지에 포커스가 있어도 동작하도록 각 탭의 <code>content.js</code>가 키를 받아 앱에 넘깁니다. <code>KeyboardEvent.code</code>(물리 키) 기준이라 한글 입력 상태에서도 같습니다.",
  ],
  [
    "스포트라이트 팔레트",
    "열린 탭·북마크·방문 기록·명령을 한 번에 검색합니다. 한글 자판으로 잘못 친 영문(예: <code>햣</code> → <code>git</code>)도 함께 찾습니다(<code>hangul.rs</code>).",
  ],
  [
    "페이지 자동화",
    "모든 탭에 <code>content.js</code>(<code>window.__kite</code>)를 주입해 스냅샷·클릭·입력·콘솔/네트워크 기록을 제공합니다. 앱은 <code>eval_with_callback</code>으로 스크립트를 실행하고 결과를 받으며, 비동기 결과는 폴링으로 가져옵니다. 페이지 쪽에서 앱을 부르는 IPC는 쓰지 않습니다.",
  ],
  [
    "MCP 서버",
    "AI 에이전트용 Streamable HTTP 서버(<code>mcp.rs</code>)가 Playwright MCP와 같은 이름의 도구(<code>browser_snapshot</code>, <code>browser_click</code> …)를 제공합니다. 127.0.0.1에만 열리고, 요청마다 Bearer 토큰을 확인하며, 브라우저 <code>Origin</code>이 붙은 요청은 거부해 웹 페이지가 부를 수 없습니다.",
  ],
  [
    "권한 경계",
    "웹 페이지(탭)가 부를 수 있는 앱 명령은 <code>content_event</code> 하나뿐입니다. 설정·북마크 같은 내부 페이지 명령은 로컬 페이지에만 열려 있고, 앱 UI 명령은 탭에 열지 않습니다. 페이지가 보낸 값은 믿지 않고 주소는 웹뷰에서 다시 읽습니다. 웹 페이지가 외부 앱 링크(<code>f5-epi:</code> 등)를 열면 확인창을 거쳐 OS에 넘깁니다.",
  ],
  [
    "데이터 저장",
    "북마크·방문 기록·설정은 앱 데이터 폴더의 SQLite 파일 하나(<code>kite.db</code>)에 저장합니다. MCP 토큰은 별도 파일(<code>mcp-token</code>, 소유자만 읽기)에만 두고 로그나 설정에 남기지 않습니다.",
  ],
  [
    "플랫폼 차이",
    "스크린샷(WKWebView takeSnapshot / WebView2 CapturePreview)과 Windows 웹뷰 겹침 순서처럼 OS마다 다른 코드는 <code>platform.rs</code>에 모았습니다. 웹뷰를 만드는 명령은 Windows 교착을 피하려고 비동기로 실행합니다.",
  ],
];

export const architecture = ARCH.map(([h, body]) => `<div class="arch"><b>${h}</b><p>${body}</p></div>`).join("");

// ---------------------------------------------------------------- 오픈소스

type Oss = [name: string, version: string, license: string, use: string, url: string];

const OSS: [string, Oss[]][] = [
  [
    "앱 기반",
    [
      ["tauri", "2.12", "Apache-2.0 / MIT", "데스크톱 앱 프레임워크", "https://github.com/tauri-apps/tauri"],
      ["wry", "0.57", "Apache-2.0 / MIT", "OS 웹뷰 연결 (tauri 내부)", "https://github.com/tauri-apps/wry"],
      ["tao", "0.37", "Apache-2.0", "창·이벤트 루프 (tauri 내부)", "https://github.com/tauri-apps/tao"],
      ["tauri-plugin-dialog", "2.8", "Apache-2.0 / MIT", "확인창", "https://github.com/tauri-apps/plugins-workspace"],
      ["tauri-plugin-opener", "2.7", "Apache-2.0 / MIT", "외부 앱으로 링크 열기", "https://github.com/tauri-apps/plugins-workspace"],
    ],
  ],
  [
    "비동기 · 네트워크",
    [
      ["tokio", "1.54", "MIT", "비동기 런타임", "https://github.com/tokio-rs/tokio"],
      ["hyper", "1.12", "MIT", "MCP용 HTTP 서버", "https://github.com/hyperium/hyper"],
      ["hyper-util · http · http-body-util", "", "MIT / Apache-2.0", "HTTP 보조", "https://github.com/hyperium"],
      ["bytes · tokio-util · tower-service", "", "MIT", "비동기 보조", "https://github.com/tokio-rs"],
      ["url · percent-encoding", "2.5", "MIT / Apache-2.0", "주소 해석", "https://github.com/servo/rust-url"],
    ],
  ],
  [
    "AI 자동화",
    [["rmcp", "3.11", "Apache-2.0", "MCP(Model Context Protocol) 서버 SDK", "https://github.com/modelcontextprotocol/rust-sdk"]],
  ],
  [
    "데이터 · 보안",
    [
      ["SQLite", "3.54", "Public Domain", "로컬 데이터베이스 (앱에 포함)", "https://sqlite.org"],
      ["rusqlite", "0.42", "MIT", "SQLite 연결", "https://github.com/rusqlite/rusqlite"],
      ["ring", "0.17", "Apache-2.0 / ISC", "MCP 토큰 난수", "https://github.com/briansmith/ring"],
      ["serde · serde_json", "1.0", "MIT / Apache-2.0", "직렬화", "https://github.com/serde-rs/serde"],
      ["base64", "0.22", "MIT / Apache-2.0", "인코딩", "https://github.com/marshallpierce/rust-base64"],
      ["dirs", "6.0", "MIT / Apache-2.0", "다운로드·데이터 폴더 찾기", "https://github.com/soc/dirs-rs"],
    ],
  ],
  [
    "OS · 이미지",
    [
      ["objc2 · objc2-web-kit · objc2-app-kit", "0.6 / 0.3", "MIT / Zlib / Apache-2.0", "macOS WKWebView 호출", "https://github.com/madsmtm/objc2"],
      ["windows", "0.62", "MIT / Apache-2.0", "Windows API", "https://github.com/microsoft/windows-rs"],
      ["webview2-com", "0.39", "MIT", "WebView2 호출", "https://github.com/wravery/webview2-rs"],
      ["image", "0.25", "MIT / Apache-2.0", "스크린샷 PNG·JPEG 변환", "https://github.com/image-rs/image"],
      ["arboard", "3.7", "MIT / Apache-2.0", "클립보드", "https://github.com/1Password/arboard"],
    ],
  ],
  [
    "로그 · 오류",
    [
      ["tracing · tracing-subscriber", "0.1 / 0.3", "MIT", "로그", "https://github.com/tokio-rs/tracing"],
      ["thiserror", "2.0", "MIT / Apache-2.0", "오류 타입", "https://github.com/dtolnay/thiserror"],
    ],
  ],
  [
    "프론트엔드 · 빌드",
    [
      ["@tauri-apps/api", "2.12", "Apache-2.0 / MIT", "웹뷰 ↔ Rust 명령 호출", "https://github.com/tauri-apps/tauri"],
      ["TypeScript", "6.0", "Apache-2.0", "UI 코드 언어", "https://github.com/microsoft/TypeScript"],
      ["Vite", "8.3", "MIT", "UI 번들러 (빌드 시)", "https://github.com/vitejs/vite"],
      ["Vitest", "5.1", "MIT", "테스트 (개발 시)", "https://github.com/vitest-dev/vitest"],
      ["@tauri-apps/cli", "2.12", "Apache-2.0 / MIT", "빌드 도구 (개발 시)", "https://github.com/tauri-apps/tauri"],
    ],
  ],
];

export const oss = OSS.map(
  ([group, rows]) =>
    `<h3 class="oss-group">${group}</h3><table class="oss"><tbody>${rows
      .map(
        ([name, ver, lic, use, url]) =>
          `<tr><td><a href="${esc(url)}" target="_blank" rel="noopener">${esc(name)}</a>${ver ? ` <span class="ver">${esc(ver)}</span>` : ""}</td><td>${esc(use)}</td><td class="lic">${esc(lic)}</td></tr>`,
      )
      .join("")}</tbody></table>`,
).join("");
