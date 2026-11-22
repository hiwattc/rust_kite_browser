// Settings → 시스템 구성도 / 구현 아키텍처 / 오픈소스: static content, no commands needed.
import { esc } from "../shared/page";

// ---------------------------------------------------------------- 시스템 구성도

export { diagram } from "./diagram";

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
    "확장 프로그램",
    "크롬 웹 스토어의 CRX를 받아 압축을 풀어 <code>extensions/&lt;ID&gt;</code>에 두고(<code>extensions.rs</code>), OS 엔진에 올립니다. macOS는 탭마다 <code>WKWebViewConfiguration</code>에 WebKit의 <code>WKWebExtensionController</code>를 붙이고, Windows는 WebView2의 <code>AddBrowserExtension</code>을 씁니다. WebKit은 확장을 올린 뒤 만든 탭에 차단 규칙(declarativeNetRequest)을 주지 않아서, 새 탭이 생기면 차단 규칙이 있는 확장을 다시 올립니다.",
  ],
  [
    "비밀번호 관리자",
    "로그인 폼 제출을 <code>content.js</code>가 알리면, Rust가 사이트 주소를 웹뷰에서 다시 읽어 페이지가 말한 주소와 같을 때만 저장을 묻습니다. 비밀번호는 AES-256-GCM으로 암호화해 <code>kite.db</code>에 두고, 키는 OS 키체인(Windows 자격 증명 관리자)에만 있습니다. 자동 입력은 Rust가 탭에 스크립트를 실행해 하며, 페이지 안에서도 출처가 같은지 한 번 더 확인합니다(<code>passwords.rs</code>).",
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
      ["ring", "0.17", "Apache-2.0 / ISC", "MCP 토큰 난수, 비밀번호 암호화(AES-256-GCM)", "https://github.com/briansmith/ring"],
      ["keyring", "3.6", "MIT / Apache-2.0", "비밀번호 키를 OS 키체인에 보관", "https://github.com/open-source-cooperative/keyring-rs"],
      ["serde · serde_json", "1.0", "MIT / Apache-2.0", "직렬화", "https://github.com/serde-rs/serde"],
      ["base64", "0.22", "MIT / Apache-2.0", "인코딩", "https://github.com/marshallpierce/rust-base64"],
      ["dirs", "6.0", "MIT / Apache-2.0", "다운로드·데이터 폴더 찾기", "https://github.com/soc/dirs-rs"],
      ["ureq", "3.5", "MIT / Apache-2.0", "웹 스토어에서 확장 프로그램 받기", "https://github.com/algesten/ureq"],
      ["zip", "9.8", "MIT", "확장 프로그램(CRX) 압축 풀기", "https://github.com/zip-rs/zip2"],
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
