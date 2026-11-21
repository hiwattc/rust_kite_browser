//! Running scripts in a tab and getting their results back, without any IPC from the page:
//! the app evaluates JS with `eval_with_callback` and reads the returned JSON string.
//!
//! - [`Browser::run_sync`]: the body runs synchronously; its return value comes back at once.
//! - [`Browser::run_async`]: the body may `await`; the page stores the result under an id and
//!   the app polls for it (WKWebView's evaluateJavaScript does not wait for promises).
//!
//! Every script is prefixed with the content script, so `window.__kite` exists even on pages
//! where the init script did not run.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::sync::oneshot;

use crate::browser::Browser;

static NEXT: AtomicU64 = AtomicU64::new(1);

/// Raw result of one evaluation: the JSON the webview returned ("" when the script threw or
/// did not parse).
async fn eval_raw(b: &Browser, tab: u64, js: String, timeout: Duration) -> Result<String, String> {
    // Kite's own pages can change settings and read the MCP token: no agent scripts there
    // (a web page could otherwise talk an agent into it). The start page is harmless.
    if let Some(t) = b.tab(tab)
        && t.url.starts_with("kite://")
        && t.url != crate::browser::NEWTAB
    {
        return Err(format!("{}는 Kite 내부 페이지라 자동화할 수 없습니다. 다른 주소로 이동하세요.", t.url));
    }
    let wv = b.ensure_live(tab).map_err(|e| e.user_message)?;
    let (tx, rx) = oneshot::channel::<String>();
    let tx = Arc::new(Mutex::new(Some(tx)));
    wv.eval_with_callback(js, move |r| {
        if let Some(tx) = tx.lock().unwrap().take() {
            let _ = tx.send(r);
        }
    })
    .map_err(|e| format!("스크립트를 실행하지 못했습니다: {e}"))?;
    match tokio::time::timeout(timeout, rx).await {
        Ok(Ok(r)) => Ok(r),
        Ok(Err(_)) => Err("페이지가 응답하지 않았습니다(탭이 닫혔거나 이동 중).".into()),
        Err(_) => Err(format!("페이지 응답 시간 초과({}초)", timeout.as_secs())),
    }
}

/// The webview returns the JSON encoding of our string; unwrap both layers.
fn decode(raw: &str) -> Option<Value> {
    let s: String = serde_json::from_str(raw).ok()?;
    serde_json::from_str(&s).ok()
}

fn outcome(v: Value) -> Result<Value, String> {
    if v.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(v.get("value").cloned().unwrap_or(Value::Null))
    } else {
        Err(v.get("error").and_then(Value::as_str).unwrap_or("알 수 없는 오류").to_string())
    }
}

impl Browser {
    fn guard(&self) -> &str {
        self.content_js()
    }

    /// Runs `body` (a function body using `__kite`, returning a JSON-able value) and returns
    /// its value.
    pub async fn run_sync(&self, tab: u64, body: &str) -> Result<Value, String> {
        let js = format!(
            "(function(){{{guard}\ntry{{var __r=(function(){{{body}\n}})();return JSON.stringify({{ok:true,value:__r===undefined?null:window.__kite.serialize(__r)}});}}catch(e){{return JSON.stringify({{ok:false,error:String(e&&e.message||e)}});}}}})()",
            guard = self.guard()
        );
        let raw = eval_raw(self, tab, js, Duration::from_secs(15)).await?;
        match decode(&raw) {
            Some(v) => outcome(v),
            None => Err("스크립트가 실행되지 않았습니다(문법 오류이거나 페이지가 바뀌는 중).".into()),
        }
    }

    /// Starts `body` as an async function and polls for its result.
    pub async fn run_async(&self, tab: u64, body: &str, timeout: Duration) -> Result<Value, String> {
        let id = NEXT.fetch_add(1, Ordering::SeqCst);
        let js = format!(
            "(function(){{{guard}\nwindow.__kite.start({id},async function(){{{body}\n}});return 'started';}})()",
            guard = self.guard()
        );
        let raw = eval_raw(self, tab, js, Duration::from_secs(15)).await?;
        if serde_json::from_str::<String>(&raw).ok().as_deref() != Some("started") {
            return Err("SYNTAX".into());
        }
        let poll = format!("(function(){{return window.__kite?window.__kite.take({id}):'__gone__';}})()");
        let deadline = tokio::time::Instant::now() + timeout;
        let mut wait = 15u64;
        loop {
            tokio::time::sleep(Duration::from_millis(wait)).await;
            wait = (wait * 2).min(150);
            let raw = eval_raw(self, tab, poll.clone(), Duration::from_secs(10)).await?;
            match serde_json::from_str::<Option<String>>(&raw) {
                Ok(Some(s)) if s == "__gone__" || s == "__missing__" => {
                    return Err("스크립트 실행 중 페이지가 바뀌었습니다.".into());
                }
                Ok(Some(s)) => {
                    return match serde_json::from_str(&s) {
                        Ok(v) => outcome(v),
                        Err(e) => Err(format!("결과 해석 실패: {e}")),
                    };
                }
                _ => {}
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(format!("스크립트가 {}초 안에 끝나지 않았습니다.", timeout.as_secs()));
            }
        }
    }

    /// Agent-supplied script: an expression, a function (called with the target element), or
    /// statements with `return`.
    pub async fn evaluate(&self, tab: u64, script: &str, target: Option<&str>, timeout: Duration) -> Result<Value, String> {
        let el = target.map(|t| format!("var el=window.__kite.resolve({t});")).unwrap_or_else(|| "var el=undefined;".into());
        let expr = format!("{el}var __v=({script}\n);if(typeof __v==='function')__v=await __v(el);return __v;");
        match self.run_async(tab, &expr, timeout).await {
            Err(e) if e == "SYNTAX" => {
                let stmts = format!("{el}var __v=await (async function(el){{{script}\n}})(el);if(typeof __v==='function')__v=await __v(el);return __v;");
                match self.run_async(tab, &stmts, timeout).await {
                    Err(e) if e == "SYNTAX" => Err("스크립트 문법 오류입니다(식, 함수, 또는 return이 있는 문장으로 써 주세요).".into()),
                    r => r,
                }
            }
            r => r,
        }
    }

    /// Polls `__kite.check_state(cond)` until true.
    pub async fn wait_for(&self, tab: u64, cond: &Value, timeout: Duration) -> Result<(), String> {
        let deadline = tokio::time::Instant::now() + timeout;
        let body = format!("return window.__kite.check_state({cond});");
        loop {
            // A page in the middle of navigating has no result: just try again.
            if let Ok(Value::Bool(true)) = self.run_sync(tab, &body).await {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(format!("{}초 안에 조건을 만족하지 않았습니다: {cond}", timeout.as_secs()));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// After a click/type/key: wait for a navigation the action may have started. wry reports
    /// a load only when it commits, so a pending one shows as "the page's URL is not the
    /// webview's URL yet".
    pub async fn settle(&self, tab: u64) {
        tokio::time::sleep(Duration::from_millis(150)).await;
        let quiet_needed = 3;
        let mut quiet = 0;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        while tokio::time::Instant::now() < deadline {
            if self.tab(tab).is_some_and(|t| t.loading) {
                self.wait_loaded(tab, deadline - tokio::time::Instant::now()).await;
                quiet = 0;
            }
            match self.probe(tab).await {
                Some((true, true, _)) => quiet += 1,
                Some(_) => quiet = 0,
                None => quiet += 1,
            }
            if quiet >= quiet_needed {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decode_double_encoded() {
        let inner = json!({"ok": true, "value": {"a": 1}}).to_string();
        let raw = serde_json::to_string(&inner).unwrap();
        assert_eq!(outcome(decode(&raw).unwrap()).unwrap(), json!({"a": 1}));
        let err = serde_json::to_string(&json!({"ok": false, "error": "boom"}).to_string()).unwrap();
        assert_eq!(outcome(decode(&err).unwrap()).unwrap_err(), "boom");
        assert!(decode("").is_none());
        assert!(decode("null").is_none());
    }
}
