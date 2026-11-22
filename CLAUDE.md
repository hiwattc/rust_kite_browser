# Kite

Rust 브라우저. Tauri v2 + OS 웹뷰(Windows WebView2 / macOS WKWebView) + Vite/TypeScript(vanilla).
스포트라이트 팔레트(Ctrl+Shift+P)와 AI 에이전트용 MCP 서버(Playwright MCP와 비슷한 도구)를 내장.
계획과 진행 상황은 `docs/PLAN.md`, 사용법은 `README.md`.

## 구조 (먼저 읽을 곳)
- `src-tauri/src/browser.rs` — 창(`main`, `win-<n>`)마다 웹뷰 여러 개: `ui`/`ui-<n>`(탭줄+툴바), `tab-<id>`(탭마다), `palette`/`palette-<n>`(오버레이), `omni`/`omni-<n>`(주소창 추천 목록 오버레이, 키보드는 주소창이 가짐 — `omnibox.rs`), `side`/`side-<n>`(세로 탭 목록, 열리면 `layout`이 탭을 오른쪽으로 밀어 배치; `browser-state`를 ui와 같이 받음), `cover`/`cover-<n>`(되살리는 탭 위의 마지막 화면). 탭 상태는 Rust가 소유(탭마다 `win`)하고 UI는 자기 창의 `browser-state` 이벤트를 그리기만 함. 창 지정 없는 동작(단축키·팔레트·새 탭)은 마지막으로 포커스된 창(`focused`) 기준.
- `src-tauri/src/inject/content.js` — 모든 탭에 주입(`window.__kite`): 단축키 전달, 콘솔/네트워크 기록, 스냅샷·클릭·입력 등 자동화 헬퍼.
- `src-tauri/src/automation.rs` — 탭에서 JS 실행 후 결과 받기(`eval_with_callback`, 비동기는 폴링). 페이지 쪽 IPC를 쓰지 않음.
- `src-tauri/src/mcp.rs` — MCP 서버(Streamable HTTP, 127.0.0.1, Bearer 토큰)와 도구 구현.
- `src-tauri/src/keymap.rs` — 단축키 표 하나(UI·팔레트·탭 공통). `KeyboardEvent.code` 기준(한글 IME에서도 동작).
- `src-tauri/src/palette.rs`, `hangul.rs` — 팔레트 검색(탭·북마크·기록·명령, 한/영 잘못 친 입력 보정).
- `src-tauri/src/platform.rs` — 네이티브 스크린샷(WKWebView takeSnapshot / WebView2 CapturePreview), Windows z-order.
- `src-tauri/src/launcher.rs` — 전역 단축키(tauri-plugin-global-shortcut)로 뜨는 떠 있는 검색 창 `launcher`(팔레트 페이지 `palette.html?launcher` 재사용; `palette_close/resize/run`이 라벨로 구분). macOS에서 다른 앱이 앞일 때는 활성화가 늦게 끝나므로 잠시 뒤 다시 `set_focus`.
- `src-tauri/src/neterror.rs` — 접속 불가: wry가 실패를 알려 주지 않아, Kite가 시작한 이동마다 `watch_failure`가 WKWebView `isLoading`을 봐서 commit 없이 멈추면 실패로 판단 → 직접 진단(DNS/TCP/TLS) → `neterror.html`(탭 `error`/`error_url`로 주소창엔 원래 주소). 9번 같은 WebKit 제한 포트는 about:blank로 commit되어 해당 없음. 인증서 예외: wry 내비게이션 델리게이트가 인증 챌린지를 처리하지 않아 `platform::install_cert_exceptions`가 그 클래스에 `webView:didReceiveAuthenticationChallenge:completionHandler:`를 추가하고 델리게이트를 다시 붙임(WebKit이 메서드 목록을 설정 시점에 캐시). 사용자가 `neterror_proceed`로 허용한 호스트만 서버 신뢰를 받아들이고 나머지는 기본 처리. 허용 목록은 메모리(재시작 시 초기화), 탭 `badCert`로 주소창 경고.
- `src-tauri/src/meet.rs` — 영상회의 서버(호스트 Kite가 TCP로 열고 UPnP로 공유기 포트 개방, 초대 코드 키로 프레임 암호화, 참가 요청·수락·시그널링 중계). 페이지(`src/meet`)는 네트워크를 직접 쓰지 않고 `meet_*` 명령과 `meet` 이벤트만 씀.
- `src-tauri/src/portal.rs` + `src/home` — `kite://home` 개인 포털. 페이지가 레이아웃·편집을 하고, 데이터(워크스페이스 → 섹션(`apps`|`todo`, 12칸 격자 `w`×`h`) JSON 하나, version 2; 예전 v1은 `normalize`가 변환)는 store kv `portal`에 저장. 사이트 아이콘은 Rust가 받아 data URL로(apple-touch-icon 우선). 내보내기/불러오기는 파일 대화상자.
- `src-tauri/src/downloads.rs` + `src/downloads` — 다운로드 진행률·기록·`kite://downloads`. Tauri는 Requested/Finished만 줘서, macOS는 `platform::mac_download`가 wry 다운로드 델리게이트의 destination 메서드를 감싸 `WKDownload`(retain, 메인 스레드에서만 접근)와 expectedContentLength를 넘김 → 진행률·취소. 취소한 다운로드는 WebKit이 알려 주지 않아 직접 종료 처리. Windows는 파일 크기로만.
- `src-tauri/src/pages.rs` — 내부 페이지(`kite://settings`, `kite://bookmarks`)용 명령.
- `src/index`, `src/palette` — 탭줄/툴바·북마크 바, 팔레트(각자 웹뷰). `src/newtab`, `src/settings`, `src/bookmarks` — 탭 안에 뜨는 내부 페이지(`src/shared/page.ts` 공통).

## 규칙
- 기술 스택 유지: Tauri v2 + OS 웹뷰. 엔진 교체(CEF 등)는 사용자와 상의.
- 웹 페이지(탭)가 부를 수 있는 앱 명령은 `content_event` 하나뿐 (`capabilities/content.json`). 내부 페이지 명령은 `capabilities/internal.json`(로컬 페이지만)에 넣고, 내부 페이지 추가 시 `browser.rs`의 `INTERNAL`·`kite_page`, `vite.config.ts` 입력에 등록. 새 명령은 `build.rs` 목록과 `capabilities/chrome.json`에 추가하고, 탭에는 열지 않는다. 페이지가 보낸 값은 믿지 않는다(URL은 웹뷰에서 다시 읽음).
- MCP는 루프백 + Bearer 토큰 + Origin 거부. 토큰을 로그·설정 파일에 남기지 않는다(토큰 파일 `mcp-token`만, 0600).
- 웹뷰를 만드는 Tauri 명령은 `async`로 (Windows에서 sync 명령이 웹뷰를 만들면 교착). 순서가 중요한 명령(크기 조정)은 sync.
- 플러그인이 모든 웹뷰에 스크립트를 주입하는지 확인(탭에도 들어감). 예: opener의 `open_js_links_on_click`은 꺼 둠 — 켜면 탭의 `target=_blank` 링크가 막힘.
- 팝업 창(`popup-<n>`, `browser.rs`의 `open_popup`)은 macOS에서 여는 탭의 WebKit 설정을 물려받아 탭의 `content.js`와 IPC(라벨 `tab-N`)가 그대로 들어감 → `content.js`는 팝업(`window.opener`)에서 IPC를 보내지 않는다. 팝업 빌더의 `initialization_script`는 macOS에서 적용되지 않음.
- `src-tauri/Info.plist`(카메라·마이크 사용 설명)가 있어야 WKWebView에 `navigator.mediaDevices`가 생김 — dev 빌드에도 tauri가 바이너리에 넣어 줌. 지우면 영상회의·웹 화상통화가 안 됨.
- macOS가 숨은 탭의 WebContent 프로세스를 끝내면 `lib.rs`의 `on_web_content_process_terminate` → `Browser::process_gone`(탭이면 `purged` 표시, 보일 때 `revive`: `cover` 웹뷰에 마지막 화면을 띄우고 reload 후 스크롤 복원). 프로세스를 `kill -9` 해서 재현 가능.
- 창 `Focused` 이벤트 처리에서 포커스를 다시 옮기지 않는다(Windows에선 자식 웹뷰로 포커스를 옮기면 창이 Focused(false)/(true)를 바로 다시 보내 무한 반복 → 응답 없음). 지금은 `refocus_after_activation`으로 250ms 넘게 떠나 있었을 때만.
- 로그는 앱 데이터 폴더의 `kite.log`(직전 실행은 `kite.prev.log`). `diag.rs`의 감시 스레드가 메인 스레드가 4초 넘게 멈추면 마지막 동작(`diag::mark`)을, 창 이벤트가 초당 300개를 넘으면 storm을 기록. Windows 실기 확인은 `.github/workflows/windows-smoke.yml`(diag/** 브랜치 푸시나 수동 실행: 빌드 → 실행 → MCP 조작·실제 클릭 → 응답 없음/storm이면 실패, kite.exe·로그·화면 업로드).
- `cargo fmt`를 돌리지 않는다(rustfmt 설정이 없고 기존 코드는 긴 줄 스타일이라 전체가 바뀜).
- Windows 창은 제목 표시줄 없음(`decorations(false)` + 그림자): 탭줄(`ui` 웹뷰)이 최소화·최대화·닫기 버튼과 이동 영역(`data-tauri-drag-region`), 위쪽 가장자리 크기 조절(`startResizeDragging`)을 맡고, 좌·우·아래 크기 조절은 창 그림자가 함. Tauri 기본 크기 조절 스크립트는 웹뷰 하나짜리 창에만 붙어서 직접 처리.
- 전역 단축키 기본값: macOS `Alt+Space`, Windows `F9`. 런처 창은 메인 스레드 콜백 안에서 만들지 않는다(백그라운드 작업에서 build).
- `gen`은 edition 2024 예약어. Windows 전용/맥 전용 코드는 `platform.rs`에 cfg로 모으고 양쪽 모두 컴파일 확인.
- 프론트에서 이벤트는 `getCurrentWebview().listen` (전역 `listen`은 다른 창으로 보낸 이벤트도 받음). 창마다 다른 이벤트는 `ui_label(n)`로 `emit_to`.
- UI 문자열은 한국어. 색·크기는 `src/styles/base.css` 토큰을 쓴다.

## 확인 명령
- Rust: `cd src-tauri && cargo clippy --all-targets && cargo test --lib`
- Windows 컴파일 확인: `cd src-tauri && CARGO_TARGET_DIR=target/xwin cargo xwin clippy --all-targets --target x86_64-pc-windows-msvc` (CI처럼 테스트 포함)
- 프론트: `npx tsc --noEmit`
- 실행: `npx tauri dev` (dev 서버 포트 1440)
- MCP 수동 호출: 토큰은 macOS `~/Library/Application Support/com.kite.browser/mcp-token`, Windows `%APPDATA%\com.kite.browser\mcp-token`
