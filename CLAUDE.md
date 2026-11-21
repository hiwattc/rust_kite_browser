# Kite

Rust 브라우저. Tauri v2 + OS 웹뷰(Windows WebView2 / macOS WKWebView) + Vite/TypeScript(vanilla).
스포트라이트 팔레트(Ctrl+Shift+P)와 AI 에이전트용 MCP 서버(Playwright MCP와 비슷한 도구)를 내장.
계획과 진행 상황은 `docs/PLAN.md`, 사용법은 `README.md`.

## 구조 (먼저 읽을 곳)
- `src-tauri/src/browser.rs` — 창 하나에 웹뷰 여러 개: `ui`(탭줄+툴바), `tab-<id>`(탭마다), `palette`(오버레이). 탭 상태는 Rust가 소유하고 UI는 `browser-state` 이벤트를 그리기만 함.
- `src-tauri/src/inject/content.js` — 모든 탭에 주입(`window.__kite`): 단축키 전달, 콘솔/네트워크 기록, 스냅샷·클릭·입력 등 자동화 헬퍼.
- `src-tauri/src/automation.rs` — 탭에서 JS 실행 후 결과 받기(`eval_with_callback`, 비동기는 폴링). 페이지 쪽 IPC를 쓰지 않음.
- `src-tauri/src/mcp.rs` — MCP 서버(Streamable HTTP, 127.0.0.1, Bearer 토큰)와 도구 구현.
- `src-tauri/src/keymap.rs` — 단축키 표 하나(UI·팔레트·탭 공통). `KeyboardEvent.code` 기준(한글 IME에서도 동작).
- `src-tauri/src/palette.rs`, `hangul.rs` — 팔레트 검색(탭·북마크·기록·명령, 한/영 잘못 친 입력 보정).
- `src-tauri/src/platform.rs` — 네이티브 스크린샷(WKWebView takeSnapshot / WebView2 CapturePreview), Windows z-order.
- `src-tauri/src/pages.rs` — 내부 페이지(`kite://settings`, `kite://bookmarks`)용 명령.
- `src/index`, `src/palette` — 탭줄/툴바·북마크 바, 팔레트(각자 웹뷰). `src/newtab`, `src/settings`, `src/bookmarks` — 탭 안에 뜨는 내부 페이지(`src/shared/page.ts` 공통).

## 규칙
- 기술 스택 유지: Tauri v2 + OS 웹뷰. 엔진 교체(CEF 등)는 사용자와 상의.
- 웹 페이지(탭)가 부를 수 있는 앱 명령은 `content_event` 하나뿐 (`capabilities/content.json`). 내부 페이지 명령은 `capabilities/internal.json`(로컬 페이지만)에 넣고, 내부 페이지 추가 시 `browser.rs`의 `INTERNAL`·`kite_page`, `vite.config.ts` 입력에 등록. 새 명령은 `build.rs` 목록과 `capabilities/chrome.json`에 추가하고, 탭에는 열지 않는다. 페이지가 보낸 값은 믿지 않는다(URL은 웹뷰에서 다시 읽음).
- MCP는 루프백 + Bearer 토큰 + Origin 거부. 토큰을 로그·설정 파일에 남기지 않는다(토큰 파일 `mcp-token`만, 0600).
- 웹뷰를 만드는 Tauri 명령은 `async`로 (Windows에서 sync 명령이 웹뷰를 만들면 교착). 순서가 중요한 명령(크기 조정)은 sync.
- `gen`은 edition 2024 예약어. Windows 전용/맥 전용 코드는 `platform.rs`에 cfg로 모으고 양쪽 모두 컴파일 확인.
- UI 문자열은 한국어. 색·크기는 `src/styles/base.css` 토큰을 쓴다.

## 확인 명령
- Rust: `cd src-tauri && cargo clippy --all-targets && cargo test --lib`
- Windows 컴파일 확인: `cd src-tauri && CARGO_TARGET_DIR=target/xwin cargo xwin clippy --target x86_64-pc-windows-msvc`
- 프론트: `npx tsc --noEmit`
- 실행: `npx tauri dev` (dev 서버 포트 1440)
- MCP 수동 호출: 토큰은 macOS `~/Library/Application Support/com.kite.browser/mcp-token`, Windows `%APPDATA%\com.kite.browser\mcp-token`
