//! MCP server for Claude Code and other agents: Streamable HTTP on 127.0.0.1.
//!
//! Security model (same as termtree):
//! - Loopback only, `Authorization: Bearer <token>` on every request, any browser `Origin`
//!   refused (so web pages, including the ones open in Kite, cannot call it).
//! - The token is a random 256-bit hex string in `<config>/mcp-token`, readable by the owner only.
//! - Everything the agent does happens in visible tabs; the tab it works on is marked in the UI.
//!
//! Tools are named like Playwright MCP's (`browser_snapshot`, `browser_click`, …) so agents
//! that know those use them the same way: take a snapshot, act on `ref`s from it.

use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation, InitializeResult, ListToolsResult, PaginatedRequestParams,
    ServerCapabilities, Tool, ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use serde::Serialize;
use serde_json::{Value, json};
use tauri::{AppHandle, Manager};
use tokio_util::sync::CancellationToken;

use crate::browser::{Browser, NEWTAB};
use crate::error::AppError;

pub const DEFAULT_PORT: u16 = 17321;
const RESULT_MAX: usize = 400 * 1024;

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub running: bool,
    pub port: u16,
    pub url: String,
}

struct Running {
    cancel: CancellationToken,
    port: u16,
}

#[derive(Default)]
struct Inner {
    app: Mutex<Option<AppHandle>>,
    running: Mutex<Option<Running>>,
}

#[derive(Clone, Default)]
pub struct McpHub(Arc<Inner>);

pub fn url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/mcp")
}

impl McpHub {
    pub fn set_app(&self, app: AppHandle) {
        *self.0.app.lock().unwrap() = Some(app);
    }

    fn browser(&self) -> Option<Browser> {
        self.0.app.lock().unwrap().as_ref().and_then(|a| a.try_state::<Browser>().map(|b| b.inner().clone()))
    }

    pub fn status(&self) -> Status {
        match &*self.0.running.lock().unwrap() {
            Some(r) => Status { running: true, port: r.port, url: url(r.port) },
            None => Status::default(),
        }
    }

    pub fn stop(&self) {
        if let Some(r) = self.0.running.lock().unwrap().take() {
            r.cancel.cancel();
            tracing::info!(port = r.port, "mcp server stopped");
        }
    }

    pub async fn start(&self, port: u16, token: String) -> Result<Status, AppError> {
        self.stop();
        let mut tries = 0;
        let listener = loop {
            match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
                Ok(l) => break l,
                Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && tries < 20 => {
                    tries += 1;
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(e) => return Err(AppError::new(format!("MCP 포트 {port}을(를) 열 수 없습니다. 다른 프로그램이 쓰고 있을 수 있습니다."), e)),
            }
        };
        let cancel = CancellationToken::new();
        let config = StreamableHttpServerConfig::default().with_cancellation_token(cancel.child_token()).enforce_origin_validation();
        let hub = self.clone();
        let service = StreamableHttpService::new(move || Ok(Handler { hub: hub.clone() }), Arc::new(LocalSessionManager::default()), config);
        let token = Arc::new(token);
        let stop = cancel.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                let stream = tokio::select! {
                    _ = stop.cancelled() => break,
                    r = listener.accept() => match r {
                        Ok((s, _)) => s,
                        Err(e) => { tracing::warn!(error = %e, "mcp accept failed"); continue; }
                    },
                };
                let (service, token, stop) = (service.clone(), token.clone(), stop.clone());
                tauri::async_runtime::spawn(async move {
                    let svc = service_fn(move |req: http::Request<Incoming>| {
                        let mut service = service.clone();
                        let token = token.clone();
                        async move {
                            if !authorized(&req, &token) {
                                return Ok::<_, Infallible>(text_response(401, "missing or wrong bearer token"));
                            }
                            use tower_service::Service;
                            service.call(req).await
                        }
                    });
                    let conn = http1::Builder::new().serve_connection(TokioIo::new(stream), svc);
                    tokio::select! {
                        r = conn => if let Err(e) = r { tracing::debug!(error = %e, "mcp connection ended") },
                        _ = stop.cancelled() => {}
                    }
                });
            }
        });
        *self.0.running.lock().unwrap() = Some(Running { cancel, port });
        tracing::info!(port, "mcp server listening on 127.0.0.1");
        Ok(self.status())
    }
}

/// Constant-time comparison of the bearer token.
fn authorized<B>(req: &http::Request<B>, token: &str) -> bool {
    let Some(got) = req.headers().get(http::header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")) else {
        return false;
    };
    let (a, b) = (got.trim().as_bytes(), token.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn text_response(status: u16, msg: &'static str) -> http::Response<http_body_util::combinators::BoxBody<Bytes, Infallible>> {
    let mut r = http::Response::new(Full::new(Bytes::from_static(msg.as_bytes())).boxed());
    *r.status_mut() = http::StatusCode::from_u16(status).unwrap_or(http::StatusCode::UNAUTHORIZED);
    r
}

// ---------- token ----------

fn new_token() -> Result<String, AppError> {
    use ring::rand::SecureRandom;
    let mut b = [0u8; 32];
    ring::rand::SystemRandom::new().fill(&mut b).map_err(|_| AppError::new("토큰을 만들지 못했습니다.", "rng failed"))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// The saved token (`<config>/mcp-token`, owner-only), made on first use or when `renew`.
pub fn token(path: &std::path::Path, renew: bool) -> Result<String, AppError> {
    if !renew && let Ok(t) = std::fs::read_to_string(path) {
        let t = t.trim().to_string();
        if t.len() >= 32 {
            return Ok(t);
        }
    }
    let t = new_token()?;
    let fail = |e: std::io::Error| AppError::new("MCP 토큰 파일을 저장하지 못했습니다.", e);
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(fail)?;
    }
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    let mut f = o.open(path).map_err(fail)?;
    #[cfg(unix)]
    f.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600)).map_err(fail)?;
    use std::io::Write;
    f.write_all(t.as_bytes()).map_err(fail)?;
    Ok(t)
}

/// `claude mcp add …` line for the user to paste.
pub fn claude_command(port: u16, token: &str) -> String {
    format!("claude mcp add --transport http --scope user kite {} --header \"Authorization: Bearer {token}\"", url(port))
}

// ---------- tools ----------

fn obj(v: Value) -> Arc<serde_json::Map<String, Value>> {
    match v {
        Value::Object(m) => Arc::new(m),
        _ => Arc::new(serde_json::Map::new()),
    }
}

pub fn tools() -> Vec<Tool> {
    let tab = json!({ "type": "integer", "description": "탭 id (browser_tabs 결과). 비우면 에이전트가 마지막으로 쓴 탭, 없으면 사용자가 보고 있는 탭" });
    let r#ref = json!({ "type": "string", "description": "browser_snapshot 결과의 [ref=…] 값 (예: e12). 가장 정확함" });
    let selector = json!({ "type": "string", "description": "CSS selector (ref 대신)" });
    let snapshot = json!({ "type": "boolean", "description": "true면 작업 후 페이지 스냅샷을 결과에 붙임" });
    let element = json!({ "type": "string", "description": "사람이 읽을 대상 설명 (기록용, 선택)" });
    let ro = |t: Tool| t.with_annotations(ToolAnnotations::new().read_only(true));
    let s = |props: Value, req: &[&str]| obj(json!({ "type": "object", "properties": props, "required": req }));
    vec![
        Tool::new(
            "browser_tabs",
            "탭 목록 보기/새 탭/선택/닫기. action=list는 id·제목·주소·로딩 여부. new는 url(선택)로 새 탭을 열고 그 탭을 이후 기본 대상으로 삼음. select는 탭을 화면에 보여 주고 기본 대상으로 삼음.",
            s(json!({
                "action": { "type": "string", "enum": ["list", "new", "select", "close"] },
                "tab_id": tab, "url": { "type": "string", "description": "new에서 열 주소" }
            }), &["action"]),
        ),
        Tool::new(
            "browser_navigate",
            "탭에서 주소로 이동하고 로딩이 끝날 때까지 기다림. url에 검색어를 주면 기본 검색엔진으로 검색. new_tab=true면 새 탭에서 엶.",
            s(json!({ "url": { "type": "string" }, "tab_id": tab, "new_tab": { "type": "boolean" } }), &["url"]),
        ),
        Tool::new("browser_navigate_back", "뒤로 가기.", s(json!({ "tab_id": tab }), &[])),
        Tool::new("browser_navigate_forward", "앞으로 가기.", s(json!({ "tab_id": tab }), &[])),
        Tool::new("browser_reload", "새로고침하고 로딩을 기다림.", s(json!({ "tab_id": tab }), &[])),
        ro(Tool::new(
            "browser_snapshot",
            "페이지 구조를 접근성 트리 형태의 텍스트로 가져옴(역할, 이름, 상태, 값). 클릭·입력 가능한 요소에는 [ref=eN]이 붙고, 다른 도구에서 ref로 대상을 지정함. 스크린샷보다 빠르고 정확하므로 먼저 이것을 쓸 것. 큰 페이지는 selector로 범위를 좁히거나 interactive_only=true.",
            s(json!({
                "tab_id": tab, "selector": { "type": "string", "description": "이 요소 안만 (CSS selector)" },
                "interactive_only": { "type": "boolean", "description": "ref가 있는 요소만 나열" },
                "max_chars": { "type": "integer", "minimum": 1000, "maximum": 200000 }
            }), &[]),
        )),
        Tool::new(
            "browser_click",
            "요소를 클릭(마우스 이벤트 + click). 대상은 ref(권장)·selector·text(보이는 글자) 중 하나. 클릭으로 페이지 이동이 시작되면 로딩을 기다림.",
            s(json!({
                "ref": r#ref, "selector": selector, "text": { "type": "string", "description": "이 글자를 포함한 가장 작은 보이는 요소" },
                "element": element, "double_click": { "type": "boolean" }, "button": { "type": "string", "enum": ["left", "right", "middle"] },
                "tab_id": tab, "snapshot": snapshot
            }), &[]),
        ),
        Tool::new(
            "browser_type",
            "입력칸에 글자를 입력. 기본은 기존 값을 지우고 입력(clear=false로 이어 쓰기). submit=true면 입력 후 Enter(폼 제출). slowly=true면 한 글자씩 키 이벤트를 보냄(자동완성 등 키 입력에 반응하는 페이지).",
            s(json!({
                "ref": r#ref, "selector": selector, "text": { "type": "string" }, "element": element,
                "clear": { "type": "boolean" }, "submit": { "type": "boolean" }, "slowly": { "type": "boolean" },
                "tab_id": tab, "snapshot": snapshot
            }), &["text"]),
        ),
        Tool::new(
            "browser_fill_form",
            "여러 입력칸을 한 번에 채움. 각 항목은 ref 또는 selector와 value. 체크박스·라디오는 value에 true/false, select는 옵션 값이나 보이는 글자.",
            s(json!({
                "fields": { "type": "array", "items": { "type": "object", "properties": {
                    "ref": { "type": "string" }, "selector": { "type": "string" }, "value": { "type": "string" }, "name": { "type": "string", "description": "설명(선택)" }
                }, "required": ["value"] } },
                "tab_id": tab, "snapshot": snapshot
            }), &["fields"]),
        ),
        Tool::new(
            "browser_select_option",
            "<select>에서 옵션 선택(값 또는 보이는 글자, 여러 개 가능).",
            s(json!({ "ref": r#ref, "selector": selector, "values": { "type": "array", "items": { "type": "string" } }, "element": element, "tab_id": tab, "snapshot": snapshot }), &["values"]),
        ),
        Tool::new(
            "browser_press_key",
            "키 누르기 (Enter, Tab, Escape, ArrowDown, Backspace, PageDown, a, Control+a …). ref/selector를 주면 그 요소에 포커스한 뒤. 페이지 스크립트가 받는 합성 키 이벤트이며, Enter는 폼 제출, Tab은 포커스 이동을 흉내 냄.",
            s(json!({ "key": { "type": "string" }, "ref": r#ref, "selector": selector, "tab_id": tab, "snapshot": snapshot }), &["key"]),
        ),
        Tool::new(
            "browser_hover",
            "요소 위에 마우스를 올림(메뉴 펼치기 등).",
            s(json!({ "ref": r#ref, "selector": selector, "text": { "type": "string" }, "element": element, "tab_id": tab, "snapshot": snapshot }), &[]),
        ),
        Tool::new(
            "browser_scroll",
            "스크롤. ref/selector만 주면 그 요소가 보이게, direction(up/down/left/right)과 amount(px, 기본 화면의 80%)로 이동, to=top/bottom으로 끝까지. ref와 direction을 함께 주면 그 요소 안을 스크롤.",
            s(json!({
                "ref": r#ref, "selector": selector, "direction": { "type": "string", "enum": ["up", "down", "left", "right"] },
                "amount": { "type": "integer" }, "to": { "type": "string", "enum": ["top", "bottom"] }, "tab_id": tab
            }), &[]),
        ),
        Tool::new(
            "browser_evaluate",
            "페이지에서 JavaScript 실행 후 결과(JSON)를 돌려줌. script는 식(document.title), 함수(() => …, el => … : ref/selector를 주면 그 요소가 인자), 또는 return이 있는 문장. await 사용 가능.",
            s(json!({ "script": { "type": "string" }, "ref": r#ref, "selector": selector, "timeout_ms": { "type": "integer", "minimum": 100, "maximum": 120000 }, "tab_id": tab }), &["script"]),
        ),
        Tool::new(
            "browser_wait_for",
            "조건이 될 때까지 기다림: text(나타남) / text_gone(사라짐) / selector(보임) / selector_gone(안 보임) / time_ms(그냥 대기). 여러 개를 주면 모두 만족할 때까지.",
            s(json!({
                "text": { "type": "string" }, "text_gone": { "type": "string" }, "selector": { "type": "string" }, "selector_gone": { "type": "string" },
                "time_ms": { "type": "integer", "minimum": 0, "maximum": 60000 }, "timeout_ms": { "type": "integer", "minimum": 100, "maximum": 120000 }, "tab_id": tab
            }), &[]),
        ),
        ro(Tool::new(
            "browser_take_screenshot",
            "탭의 보이는 영역 스크린샷(이미지). ref/selector를 주면 그 요소만 잘라냄. 기본 JPEG·너비 1280px로 줄임(full_resolution=true면 원본 PNG). save_path를 주면 파일로도 저장. 탭이 화면에 없으면 먼저 그 탭으로 전환함. 구조 파악은 browser_snapshot이 더 나음.",
            s(json!({
                "ref": r#ref, "selector": selector, "full_resolution": { "type": "boolean" }, "format": { "type": "string", "enum": ["jpeg", "png"] },
                "save_path": { "type": "string", "description": "저장할 파일 경로 (선택)" }, "tab_id": tab
            }), &[]),
        )),
        ro(Tool::new(
            "browser_get_text",
            "페이지(또는 selector 요소)의 보이는 글자(innerText). 긴 글 읽기에 좋음.",
            s(json!({ "selector": { "type": "string" }, "max_chars": { "type": "integer", "minimum": 100, "maximum": 500000 }, "tab_id": tab }), &[]),
        )),
        ro(Tool::new(
            "browser_get_html",
            "페이지(또는 selector 요소)의 HTML(outerHTML).",
            s(json!({ "selector": { "type": "string" }, "max_chars": { "type": "integer", "minimum": 100, "maximum": 500000 }, "tab_id": tab }), &[]),
        )),
        ro(Tool::new(
            "browser_console_messages",
            "페이지의 콘솔 메시지(log/warn/error, 잡히지 않은 오류). 최근 300개.",
            s(json!({ "level": { "type": "string", "enum": ["all", "error", "warn"] }, "clear": { "type": "boolean" }, "tab_id": tab }), &[]),
        )),
        ro(Tool::new(
            "browser_network_requests",
            "페이지가 보낸 fetch/XHR 요청(메서드, 주소, 상태, 시간). 최근 300개. filter는 주소에 들어간 글자.",
            s(json!({ "filter": { "type": "string" }, "clear": { "type": "boolean" }, "tab_id": tab }), &[]),
        )),
        Tool::new(
            "browser_handle_dialog",
            "이 페이지에서 앞으로 뜨는 alert/confirm/prompt에 자동으로 답하도록 정함(accept=true: 확인, false: 취소, prompt_text: prompt 입력값). 대화상자를 띄우는 동작 전에 호출. 이미 뜬 대화상자 기록도 돌려줌. 페이지를 이동하면 초기화됨.",
            s(json!({ "accept": { "type": "boolean" }, "prompt_text": { "type": "string" }, "tab_id": tab }), &["accept"]),
        ),
        Tool::new(
            "bookmarks",
            "Kite 북마크 검색/추가/삭제. search는 query의 단어가 모두 들어간 북마크(제목·주소·폴더). add는 url(비우면 현재 탭)과 title.",
            s(json!({
                "action": { "type": "string", "enum": ["search", "add", "remove"] }, "query": { "type": "string" },
                "url": { "type": "string" }, "title": { "type": "string" }, "folder": { "type": "string" }, "limit": { "type": "integer", "minimum": 1, "maximum": 500 }
            }), &["action"]),
        ),
        ro(Tool::new(
            "history_search",
            "방문 기록 검색(단어가 모두 들어간 제목·주소, 자주·최근 방문 순).",
            s(json!({ "query": { "type": "string" }, "limit": { "type": "integer", "minimum": 1, "maximum": 500 } }), &[]),
        )),
    ]
}

const INSTRUCTIONS: &str = "Kite는 사용자의 데스크톱 브라우저입니다(사용자의 로그인 상태 그대로). 모든 작업은 사용자가 보는 탭에서 일어납니다. \
흐름: browser_navigate로 열고 → browser_snapshot으로 구조와 ref를 얻고 → browser_click/browser_type 등에 ref를 넘기세요. \
작업 후 페이지가 바뀌었을 수 있으니 다시 snapshot하세요(액션 도구에 snapshot=true로 한 번에 받을 수 있음). \
입력은 합성 이벤트(isTrusted=false)라 일부 사이트에서는 막힐 수 있습니다. 결제·삭제·메일 발송 같은 되돌리기 어려운 동작 전에는 사용자에게 확인하세요.";

#[derive(Clone)]
struct Handler {
    hub: McpHub,
}

impl ServerHandler for Handler {
    fn get_info(&self) -> InitializeResult {
        let mut r = InitializeResult::new(ServerCapabilities::builder().enable_tools().build());
        r.server_info = Implementation::new("kite", env!("CARGO_PKG_VERSION"));
        r.instructions = Some(INSTRUCTIONS.into());
        r
    }

    async fn list_tools(&self, _request: Option<PaginatedRequestParams>, _context: RequestContext<RoleServer>) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(tools()))
    }

    async fn call_tool(&self, request: CallToolRequestParams, _context: RequestContext<RoleServer>) -> Result<CallToolResponse, McpError> {
        let tool = request.name.to_string();
        if !tools().iter().any(|t| t.name == tool) {
            return Err(McpError::invalid_params(format!("unknown tool: {tool}"), None));
        }
        let args = Value::Object(request.arguments.unwrap_or_default());
        let Some(b) = self.hub.browser() else {
            return Ok(CallToolResult::error(vec![ContentBlock::text("Kite 창이 준비되지 않았습니다.")]).into());
        };
        tracing::info!(tool, "mcp call");
        let result = match run(&b, &tool, &args).await {
            Ok(blocks) => CallToolResult::success(blocks),
            Err(msg) => CallToolResult::error(vec![ContentBlock::text(msg)]),
        };
        Ok(result.into())
    }
}

// ---------- tool implementations ----------

fn arg_str<'a>(a: &'a Value, k: &str) -> Option<&'a str> {
    a.get(k).and_then(Value::as_str).filter(|s| !s.is_empty())
}

fn arg_bool(a: &Value, k: &str) -> bool {
    a.get(k).and_then(Value::as_bool).unwrap_or(false)
}

fn arg_u64(a: &Value, k: &str) -> Option<u64> {
    a.get(k).and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
}

/// JS literal for `__kite.resolve(...)` from ref / selector / text arguments.
fn target(a: &Value, with_text: bool) -> Option<String> {
    let mut t = serde_json::Map::new();
    if let Some(r) = arg_str(a, "ref") {
        t.insert("ref".into(), r.trim_start_matches("ref=").into());
    } else if let Some(s) = arg_str(a, "selector") {
        t.insert("selector".into(), s.into());
    } else if with_text && let Some(s) = arg_str(a, "text") {
        t.insert("text".into(), s.into());
    } else {
        return None;
    }
    Some(Value::Object(t).to_string())
}

fn need_target(a: &Value, with_text: bool) -> Result<String, String> {
    target(a, with_text).ok_or_else(|| if with_text { "ref, selector, text 중 하나가 필요합니다." } else { "ref 또는 selector가 필요합니다." }.to_string())
}

/// The tab to act on: `tab_id`, else the agent's tab, else the visible one. Marks it as the
/// agent's tab and tells the UI.
fn pick_tab(b: &Browser, a: &Value) -> Result<u64, String> {
    let id = match arg_u64(a, "tab_id") {
        Some(id) => {
            if b.tab(id).is_none() {
                return Err(format!("탭 {id}이(가) 없습니다. browser_tabs로 목록을 확인하세요."));
            }
            id
        }
        None => b.agent_tab().or_else(|| b.active()).ok_or("열린 탭이 없습니다.")?,
    };
    if b.agent_tab() != Some(id) {
        b.set_agent_tab(Some(id));
    }
    Ok(id)
}

fn activity(b: &Browser, tab: u64, tool: &str) {
    b.emit_all_ui("agent-activity", json!({ "tabId": tab, "tool": tool }));
}

fn page_line(b: &Browser, tab: u64) -> String {
    match b.tab(tab) {
        Some(t) => format!("탭 {} · {} — {}{}", t.id, if t.title.is_empty() { "(제목 없음)" } else { &t.title }, t.url, if t.loading { " (로딩 중)" } else { "" }),
        None => format!("탭 {tab} (닫힘)"),
    }
}

fn text(s: impl Into<String>) -> Vec<ContentBlock> {
    let mut s = s.into();
    if s.len() > RESULT_MAX {
        let mut cut = RESULT_MAX;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push_str("\n…(잘림)");
    }
    vec![ContentBlock::text(s)]
}

fn pretty(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        v => serde_json::to_string_pretty(v).unwrap_or_default(),
    }
}

async fn snapshot_text(b: &Browser, tab: u64, a: &Value) -> Result<String, String> {
    let opts = json!({
        "selector": arg_str(a, "selector"),
        "interactiveOnly": arg_bool(a, "interactive_only"),
        "maxChars": arg_u64(a, "max_chars").unwrap_or(40000),
    });
    let v = b.run_sync(tab, &format!("return window.__kite.snapshot({opts});")).await?;
    Ok(format!("{}\n```yaml\n{}\n```", page_line(b, tab), v.as_str().unwrap_or_default()))
}

/// Result of an action: what happened, dialogs it raised, page line, optional snapshot.
async fn after_action(b: &Browser, tab: u64, a: &Value, what: String) -> Result<Vec<ContentBlock>, String> {
    b.settle(tab).await;
    b.refresh_url(tab);
    let mut out = what;
    if let Ok(Value::Array(d)) = b.run_sync(tab, "var d=window.__kite.dialogs.splice(0);return d;").await
        && !d.is_empty()
    {
        for x in d {
            out.push_str(&format!("\n대화상자 {}: {} → {}", x["kind"].as_str().unwrap_or(""), x["message"].as_str().unwrap_or(""), x["answer"]));
        }
    }
    out.push('\n');
    out.push_str(&page_line(b, tab));
    if arg_bool(a, "snapshot") {
        match snapshot_text(b, tab, &json!({})).await {
            Ok(s) => {
                out.push_str("\n\n");
                out.push_str(&s);
            }
            Err(e) => out.push_str(&format!("\n(스냅샷 실패: {e})")),
        }
    }
    Ok(text(out))
}

fn describe(v: &Value) -> String {
    v.get("element").and_then(Value::as_str).unwrap_or("").to_string()
}

async fn run(b: &Browser, tool: &str, a: &Value) -> Result<Vec<ContentBlock>, String> {
    let load_timeout = Duration::from_secs(30);
    match tool {
        "browser_tabs" => {
            let action = arg_str(a, "action").unwrap_or("list");
            match action {
                "new" => {
                    let url = arg_str(a, "url").map(|u| {
                        let engine = b.0.store.get("search_engine").unwrap_or_else(|| "google".into());
                        crate::palette::resolve(u, &engine)
                    });
                    let has_url = url.is_some();
                    let id = b.new_tab(url, true).map_err(|e| e.user_message)?;
                    b.set_agent_tab(Some(id));
                    activity(b, id, tool);
                    if has_url {
                        b.wait_loaded(id, load_timeout).await;
                    }
                    Ok(text(format!("새 탭을 열었습니다.\n{}", page_line(b, id))))
                }
                "select" => {
                    let id = arg_u64(a, "tab_id").ok_or("tab_id가 필요합니다.")?;
                    b.activate(id).map_err(|e| e.user_message)?;
                    b.set_agent_tab(Some(id));
                    Ok(text(format!("탭을 선택했습니다.\n{}", page_line(b, id))))
                }
                "close" => {
                    let id = match arg_u64(a, "tab_id") {
                        Some(id) => id,
                        None => b.agent_tab().ok_or("닫을 tab_id가 필요합니다.")?,
                    };
                    b.close_tab(id).map_err(|e| e.user_message)?;
                    Ok(text(format!("탭 {id}을(를) 닫았습니다.")))
                }
                _ => {
                    let active = b.active();
                    let agent = b.agent_tab();
                    // grouped by window (several windows: each tab says which)
                    let mut tabs = b.tabs();
                    tabs.sort_by_key(|t| t.win);
                    let wins = b.wins();
                    let lines: Vec<String> = tabs
                        .iter()
                        .enumerate()
                        .map(|(i, t)| {
                            let mut flags = Vec::new();
                            let win_flag = format!("창 {}", wins.iter().position(|w| *w == t.win).unwrap_or(0) + 1);
                            if wins.len() > 1 {
                                flags.push(win_flag.as_str());
                            }
                            if Some(t.id) == active {
                                flags.push("보는 중");
                            }
                            if Some(t.id) == agent {
                                flags.push("에이전트 대상");
                            }
                            if t.loading {
                                flags.push("로딩 중");
                            }
                            let f = if flags.is_empty() { String::new() } else { format!(" [{}]", flags.join(", ")) };
                            format!("{}. tab_id={} {}{} — {}", i + 1, t.id, if t.title.is_empty() { "(제목 없음)" } else { &t.title }, f, t.url)
                        })
                        .collect();
                    Ok(text(lines.join("\n")))
                }
            }
        }
        "browser_navigate" => {
            let input = arg_str(a, "url").ok_or("url이 필요합니다.")?;
            let tab = if arg_bool(a, "new_tab") {
                let id = b.new_tab(None, true).map_err(|e| e.user_message)?;
                b.set_agent_tab(Some(id));
                id
            } else {
                pick_tab(b, a)?
            };
            activity(b, tab, tool);
            b.navigate(tab, input).map_err(|e| e.user_message)?;
            let done = b.wait_loaded(tab, load_timeout).await;
            b.refresh_url(tab);
            Ok(text(format!("{}{}", if done { "" } else { "로딩이 30초 안에 끝나지 않았습니다(계속 진행 중일 수 있음).\n" }, page_line(b, tab))))
        }
        "browser_navigate_back" | "browser_navigate_forward" | "browser_reload" => {
            let tab = pick_tab(b, a)?;
            activity(b, tab, tool);
            match tool {
                "browser_navigate_back" => b.back(tab),
                "browser_navigate_forward" => b.forward(tab),
                _ => b.reload(tab, false),
            }
            .map_err(|e| e.user_message)?;
            after_action(b, tab, a, "완료.".into()).await
        }
        "browser_snapshot" => {
            let tab = pick_tab(b, a)?;
            activity(b, tab, tool);
            if b.tab(tab).is_some_and(|t| t.loading) {
                b.wait_loaded(tab, Duration::from_secs(15)).await;
            }
            Ok(text(snapshot_text(b, tab, a).await?))
        }
        "browser_click" => {
            let tab = pick_tab(b, a)?;
            activity(b, tab, tool);
            let t = need_target(a, true)?;
            let opts = json!({ "double": arg_bool(a, "double_click"), "button": arg_str(a, "button").unwrap_or("left") });
            let r = b.run_sync(tab, &format!("return window.__kite.click({t},{opts});")).await?;
            let mut what = format!("클릭: {}", r["element"].as_str().unwrap_or(&describe(a)));
            if let Some(c) = r["covered"].as_str() {
                what.push_str(&format!(" (다른 요소에 가려져 있어 그 요소({c})가 클릭을 받음)"));
            }
            after_action(b, tab, a, what).await
        }
        "browser_type" => {
            let tab = pick_tab(b, a)?;
            activity(b, tab, tool);
            let t = need_target(a, false)?;
            let s = a.get("text").and_then(Value::as_str).ok_or("text가 필요합니다.")?;
            let opts = json!({ "clear": a.get("clear").and_then(Value::as_bool).unwrap_or(true), "submit": arg_bool(a, "submit"), "slowly": arg_bool(a, "slowly") });
            let r = b.run_sync(tab, &format!("return window.__kite.type({t},{},{opts});", Value::String(s.into()))).await?;
            after_action(b, tab, a, format!("입력: {} = {}", r["element"].as_str().unwrap_or(""), r["value"])).await
        }
        "browser_fill_form" => {
            let tab = pick_tab(b, a)?;
            activity(b, tab, tool);
            let fields = a.get("fields").and_then(Value::as_array).ok_or("fields가 필요합니다.")?;
            let mut out = Vec::new();
            for f in fields {
                let t = need_target(f, false)?;
                let value = match f.get("value") {
                    Some(Value::String(s)) => s.clone(),
                    Some(v) => v.to_string(),
                    None => String::new(),
                };
                let v = Value::String(value.clone());
                // Checkbox/radio take true/false, selects an option, the rest is typed.
                let body = format!(
                    "var el=window.__kite.resolve({t});var ty=(el.type||'').toLowerCase();\
                     if(ty==='checkbox'||ty==='radio'||el.getAttribute('role')==='checkbox'||el.getAttribute('role')==='switch'){{return window.__kite.check({t},/^(true|1|on|yes|checked)$/i.test({v}));}}\
                     if(el.tagName==='SELECT'){{return window.__kite.select({t},[{v}]);}}\
                     return window.__kite.type({t},{v},{{clear:true}});"
                );
                match b.run_sync(tab, &body).await {
                    Ok(r) => out.push(format!("✓ {} = {}", r["element"].as_str().unwrap_or(""), if r["value"].is_null() { Value::String(value) } else { r["value"].clone() })),
                    Err(e) => out.push(format!("✗ {}: {e}", f.get("name").and_then(Value::as_str).unwrap_or(&t))),
                }
            }
            after_action(b, tab, a, out.join("\n")).await
        }
        "browser_select_option" => {
            let tab = pick_tab(b, a)?;
            activity(b, tab, tool);
            let t = need_target(a, false)?;
            let values = a.get("values").cloned().unwrap_or(json!([]));
            let r = b.run_sync(tab, &format!("return window.__kite.select({t},{values});")).await?;
            after_action(b, tab, a, format!("선택: {} → {}", r["element"].as_str().unwrap_or(""), r["selected"])).await
        }
        "browser_press_key" => {
            let tab = pick_tab(b, a)?;
            activity(b, tab, tool);
            let key = arg_str(a, "key").ok_or("key가 필요합니다.")?;
            let t = target(a, false).unwrap_or_else(|| "null".into());
            let r = b.run_sync(tab, &format!("return window.__kite.press({},{t});", Value::String(key.into()))).await?;
            after_action(b, tab, a, format!("키 {key} → {} ({})", r["target"].as_str().unwrap_or(""), r["result"].as_str().unwrap_or(""))).await
        }
        "browser_hover" => {
            let tab = pick_tab(b, a)?;
            activity(b, tab, tool);
            let t = need_target(a, true)?;
            let r = b.run_sync(tab, &format!("return window.__kite.hover({t});")).await?;
            after_action(b, tab, a, format!("마우스 올림: {}", r["element"].as_str().unwrap_or(""))).await
        }
        "browser_scroll" => {
            let tab = pick_tab(b, a)?;
            activity(b, tab, tool);
            let t = target(a, false).unwrap_or_else(|| "null".into());
            let amount = a.get("amount").and_then(Value::as_i64);
            let (dx, dy) = match arg_str(a, "direction") {
                Some("up") => ("0".to_string(), format!("-{}", amount.map(|x| x.to_string()).unwrap_or("Math.round(innerHeight*0.8)".into()))),
                Some("down") => ("0".to_string(), amount.map(|x| x.to_string()).unwrap_or("Math.round(innerHeight*0.8)".into())),
                Some("left") => (format!("-{}", amount.map(|x| x.to_string()).unwrap_or("Math.round(innerWidth*0.8)".into())), "0".to_string()),
                Some("right") => (amount.map(|x| x.to_string()).unwrap_or("Math.round(innerWidth*0.8)".into()), "0".to_string()),
                _ => ("0".to_string(), "0".to_string()),
            };
            let to = arg_str(a, "to").map(|s| Value::String(s.into())).unwrap_or(Value::Null);
            let r = b.run_sync(tab, &format!("return window.__kite.scroll({t},{dx},{dy},{to});")).await?;
            Ok(text(format!("스크롤 위치 {} / 전체 {} (화면 높이 {})", r["scrollY"], r["scrollHeight"], r["viewport"])))
        }
        "browser_evaluate" => {
            let tab = pick_tab(b, a)?;
            activity(b, tab, tool);
            let script = arg_str(a, "script").ok_or("script가 필요합니다.")?;
            let t = target(a, false);
            let timeout = Duration::from_millis(arg_u64(a, "timeout_ms").unwrap_or(30000));
            let v = b.evaluate(tab, script, t.as_deref(), timeout).await?;
            Ok(text(pretty(&v)))
        }
        "browser_wait_for" => {
            let tab = pick_tab(b, a)?;
            if let Some(ms) = arg_u64(a, "time_ms") {
                tokio::time::sleep(Duration::from_millis(ms.min(60000))).await;
            }
            let cond = json!({
                "text": arg_str(a, "text"), "textGone": arg_str(a, "text_gone"),
                "selector": arg_str(a, "selector"), "selectorGone": arg_str(a, "selector_gone"),
            });
            let any = cond.as_object().is_some_and(|o| o.values().any(|v| !v.is_null()));
            if any {
                activity(b, tab, tool);
                let timeout = Duration::from_millis(arg_u64(a, "timeout_ms").unwrap_or(10000));
                b.wait_for(tab, &cond, timeout).await?;
            }
            Ok(text(format!("조건 만족.\n{}", page_line(b, tab))))
        }
        "browser_take_screenshot" => {
            let tab = pick_tab(b, a)?;
            activity(b, tab, tool);
            let mut note = String::new();
            if b.active() != Some(tab) {
                b.activate(tab).map_err(|e| e.user_message)?;
                tokio::time::sleep(Duration::from_millis(300)).await;
                note = "(스크린샷을 위해 이 탭으로 전환함) ".into();
            }
            let crop = match target(a, false) {
                Some(t) => {
                    let r = b.run_sync(tab, &format!("return window.__kite.rect({t});")).await?;
                    tokio::time::sleep(Duration::from_millis(120)).await;
                    let f = |k: &str| r[k].as_f64().unwrap_or(0.0);
                    Some((f("x"), f("y"), f("width"), f("height"), f("vw")))
                }
                None => None,
            };
            let wv = b.ensure_live(tab).map_err(|e| e.user_message)?;
            let png = crate::platform::capture_png(&wv).await?;
            let full = arg_bool(a, "full_resolution");
            let jpeg = match arg_str(a, "format") {
                Some("png") => false,
                Some("jpeg") => true,
                _ => !full,
            };
            let (bytes, w, h) = crate::platform::encode(&png, crop, if full { 0 } else { 1280 }, jpeg)?;
            let mut msg = format!("{note}스크린샷 {w}×{h} {}", page_line(b, tab));
            if let Some(p) = arg_str(a, "save_path") {
                std::fs::write(p, &bytes).map_err(|e| format!("파일 저장 실패: {e}"))?;
                msg.push_str(&format!("\n저장: {p}"));
            }
            let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
            Ok(vec![ContentBlock::text(msg), ContentBlock::image(data, if jpeg { "image/jpeg" } else { "image/png" })])
        }
        "browser_get_text" | "browser_get_html" => {
            let tab = pick_tab(b, a)?;
            activity(b, tab, tool);
            let sel = arg_str(a, "selector").map(|s| Value::String(s.into())).unwrap_or(Value::Null);
            let max = arg_u64(a, "max_chars").unwrap_or(if tool == "browser_get_text" { 30000 } else { 50000 });
            let f = if tool == "browser_get_text" { "text" } else { "html" };
            let v = b.run_sync(tab, &format!("return window.__kite.{f}({sel},{max});")).await?;
            Ok(text(format!("{}\n\n{}", page_line(b, tab), v.as_str().unwrap_or_default())))
        }
        "browser_console_messages" => {
            let tab = pick_tab(b, a)?;
            let clear = if arg_bool(a, "clear") { "window.__kite.console.length=0;" } else { "" };
            let v = b.run_sync(tab, &format!("var c=window.__kite.console.slice();{clear}return c;")).await?;
            let level = arg_str(a, "level").unwrap_or("all");
            let lines: Vec<String> = v
                .as_array()
                .into_iter()
                .flatten()
                .filter(|m| match level {
                    "error" => m["level"] == "error",
                    "warn" => m["level"] == "error" || m["level"] == "warn",
                    _ => true,
                })
                .map(|m| format!("[{}] {}", m["level"].as_str().unwrap_or(""), m["text"].as_str().unwrap_or("")))
                .collect();
            Ok(text(if lines.is_empty() { "콘솔 메시지 없음 (이 페이지를 연 뒤 기록된 것만 보임)".into() } else { lines.join("\n") }))
        }
        "browser_network_requests" => {
            let tab = pick_tab(b, a)?;
            let clear = if arg_bool(a, "clear") { "window.__kite.network.length=0;" } else { "" };
            let v = b.run_sync(tab, &format!("var c=window.__kite.network.slice();{clear}return c;")).await?;
            let filter = arg_str(a, "filter").unwrap_or("");
            let lines: Vec<String> = v
                .as_array()
                .into_iter()
                .flatten()
                .filter(|m| m["url"].as_str().is_some_and(|u| u.contains(filter)))
                .map(|m| {
                    let status = if m.get("error").is_some() { format!("오류 {}", m["error"].as_str().unwrap_or("")) } else { m["status"].to_string() };
                    format!("{} {} {} {} → {} ({}ms)", m["type"].as_str().unwrap_or(""), m["method"].as_str().unwrap_or(""), m["url"].as_str().unwrap_or(""), "", status, m["ms"])
                })
                .collect();
            Ok(text(if lines.is_empty() { "fetch/XHR 요청 없음 (이 페이지를 연 뒤 보낸 것만 보임)".into() } else { lines.join("\n") }))
        }
        "browser_handle_dialog" => {
            let tab = pick_tab(b, a)?;
            let policy = json!({ "accept": a.get("accept").and_then(Value::as_bool).unwrap_or(true), "promptText": arg_str(a, "prompt_text") });
            let v = b.run_sync(tab, &format!("window.__kite.dialogPolicy={policy};return window.__kite.dialogs.slice();")).await?;
            let past: Vec<String> = v.as_array().into_iter().flatten().map(|d| format!("{}: {} → {}", d["kind"].as_str().unwrap_or(""), d["message"].as_str().unwrap_or(""), d["answer"])).collect();
            Ok(text(format!(
                "이 페이지의 대화상자는 이제 자동으로 {}합니다.{}",
                if policy["accept"] == true { "확인" } else { "취소" },
                if past.is_empty() { String::new() } else { format!("\n이전 기록:\n{}", past.join("\n")) }
            )))
        }
        "bookmarks" => {
            let store = &b.0.store;
            match arg_str(a, "action").unwrap_or("search") {
                "add" => {
                    let (url, title) = match arg_str(a, "url") {
                        Some(u) => (u.to_string(), arg_str(a, "title").unwrap_or(u).to_string()),
                        None => {
                            let t = b.tab(b.agent_tab().or(b.active()).ok_or("열린 탭이 없습니다.")?).ok_or("탭이 없습니다.")?;
                            if t.url == NEWTAB {
                                return Err("새 탭 페이지는 북마크할 수 없습니다.".into());
                            }
                            (t.url, arg_str(a, "title").map(str::to_string).unwrap_or(t.title))
                        }
                    };
                    let added = store.bookmark_add(&url, &title, arg_str(a, "folder").unwrap_or("")).map_err(|e| e.user_message)?;
                    b.refresh_bookmark_flags();
                    Ok(text(if added { format!("북마크 추가: {title} — {url}") } else { format!("이미 북마크에 있음: {url}") }))
                }
                "remove" => {
                    let url = arg_str(a, "url").ok_or("url이 필요합니다.")?;
                    let removed = store.bookmark_remove(url).map_err(|e| e.user_message)?;
                    b.refresh_bookmark_flags();
                    Ok(text(if removed { format!("북마크 삭제: {url}") } else { format!("북마크에 없음: {url}") }))
                }
                _ => {
                    let limit = arg_u64(a, "limit").unwrap_or(50) as usize;
                    let list = store.bookmarks_search(arg_str(a, "query").unwrap_or(""), limit);
                    let lines: Vec<String> =
                        list.iter().map(|x| format!("- {} — {}{}", x.title, x.url, if x.folder.is_empty() { String::new() } else { format!(" [{}]", x.folder) })).collect();
                    Ok(text(if lines.is_empty() { "북마크 없음".into() } else { lines.join("\n") }))
                }
            }
        }
        "history_search" => {
            let limit = arg_u64(a, "limit").unwrap_or(50) as usize;
            let list = b.0.store.history_search(arg_str(a, "query").unwrap_or(""), limit);
            let lines: Vec<String> = list.iter().map(|x| format!("- {} — {} (방문 {}회)", if x.title.is_empty() { "(제목 없음)" } else { &x.title }, x.url, x.visit_count)).collect();
            Ok(text(if lines.is_empty() { "기록 없음".into() } else { lines.join("\n") }))
        }
        _ => Err(format!("알 수 없는 도구: {tool}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_check() {
        let req = |h: Option<&str>| {
            let mut b = http::Request::builder();
            if let Some(h) = h {
                b = b.header("Authorization", h);
            }
            b.body(()).unwrap()
        };
        assert!(authorized(&req(Some("Bearer abc123")), "abc123"));
        assert!(!authorized(&req(Some("Bearer abc124")), "abc123"));
        assert!(!authorized(&req(Some("abc123")), "abc123"));
        assert!(!authorized(&req(None), "abc123"));
    }

    #[test]
    fn tools_have_object_schemas_and_unique_names() {
        let t = tools();
        let mut names = std::collections::HashSet::new();
        for x in &t {
            assert!(names.insert(x.name.to_string()), "dup {}", x.name);
            assert_eq!(x.input_schema.get("type").and_then(Value::as_str), Some("object"));
        }
        assert!(names.contains("browser_snapshot") && names.contains("browser_click"));
    }

    #[test]
    fn targets_are_json_literals() {
        assert_eq!(target(&json!({"ref": "e3"}), true).unwrap(), r#"{"ref":"e3"}"#);
        assert_eq!(target(&json!({"ref": "ref=e3"}), true).unwrap(), r#"{"ref":"e3"}"#);
        assert_eq!(target(&json!({"selector": "a\"b"}), false).unwrap(), r#"{"selector":"a\"b"}"#);
        assert!(target(&json!({"text": "로그인"}), false).is_none());
        assert_eq!(target(&json!({"text": "로그인"}), true).unwrap(), r#"{"text":"로그인"}"#);
    }

    #[test]
    fn token_file() {
        let dir = std::env::temp_dir().join(format!("kite-mcp-{}", std::process::id()));
        let p = dir.join("mcp-token");
        let a = token(&p, false).unwrap();
        assert_eq!(a.len(), 64);
        assert_eq!(token(&p, false).unwrap(), a);
        assert_ne!(token(&p, true).unwrap(), a);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn claude_command_line() {
        let c = claude_command(17321, "t0k");
        assert_eq!(c, "claude mcp add --transport http --scope user kite http://127.0.0.1:17321/mcp --header \"Authorization: Bearer t0k\"");
    }
}
