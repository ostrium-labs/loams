//! Page-level helpers over raw CDP, identical for both providers.
//!
//! Everything here uses methods the pinned engine documents (Page, Runtime,
//! DOM, Input, Fetch, Storage, Target). Where a scenario depends on a method
//! the engine may not have, the helper says which path it took so the report
//! can record it rather than hide it.

use std::time::{Duration, Instant};

use loams_sidebar_browser::{CdpClient, CdpEvent, SidebarBrowserError};
use serde_json::{Value, json};

use crate::provider::PageHandle;

/// Evaluate `expression` in the page and return its JSON value.
pub async fn eval(
    client: &mut CdpClient,
    page: &PageHandle,
    expression: &str,
) -> Result<Value, String> {
    let result = client
        .call(
            "Runtime.evaluate",
            json!({ "expression": expression, "returnByValue": true, "awaitPromise": false }),
            Some(&page.session_id),
        )
        .await
        .map_err(|error| format!("Runtime.evaluate: {error}"))?;
    if let Some(details) = result.get("exceptionDetails") {
        return Err(format!("the page threw: {details}"));
    }
    Ok(result
        .get("result")
        .and_then(|result| result.get("value"))
        .cloned()
        .unwrap_or(Value::Null))
}

/// Navigate and wait until the document at `url` has finished loading.
///
/// Readiness is polled (`document.readyState` plus `location.href`) rather
/// than taken from `Page.loadEventFired`, because the poll means the same
/// thing on every engine build and does not depend on event delivery.
pub async fn navigate(
    client: &mut CdpClient,
    page: &PageHandle,
    url: &str,
    within: Duration,
) -> Result<(), String> {
    let started = Instant::now();
    let navigated = client
        .call(
            "Page.navigate",
            json!({ "url": url }),
            Some(&page.session_id),
        )
        .await
        .map_err(|error| format!("Page.navigate {url}: {error}"))?;
    if let Some(error) = navigated.get("errorText").and_then(Value::as_str)
        && !error.is_empty()
    {
        return Err(format!("Page.navigate {url}: {error}"));
    }
    wait_until(
        client,
        page,
        within.saturating_sub(started.elapsed()),
        |value| value.as_bool() == Some(true),
        &format!(
            "document.readyState === 'complete' && location.href.startsWith({})",
            json!(url.trim_end_matches('/'))
        ),
    )
    .await
    .map(|_| ())
    .map_err(|error| format!("waiting for {url} to load: {error}"))
}

/// Poll `expression` until `done` accepts its value.
pub async fn wait_until(
    client: &mut CdpClient,
    page: &PageHandle,
    within: Duration,
    done: impl Fn(&Value) -> bool,
    expression: &str,
) -> Result<Value, String> {
    let deadline = Instant::now() + within;
    let mut last = Value::Null;
    loop {
        match eval(client, page, expression).await {
            Ok(value) if done(&value) => return Ok(value),
            Ok(value) => last = value,
            // A navigation in flight can destroy the context mid-evaluate;
            // keep polling until the deadline.
            Err(error) => last = Value::String(error),
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "{expression:?} did not hold within {}ms; last value {last}",
                within.as_millis()
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// The centre of the element matching `selector`, in CSS pixels.
pub async fn center(
    client: &mut CdpClient,
    page: &PageHandle,
    selector: &str,
) -> Result<(f64, f64), String> {
    let value = eval(
        client,
        page,
        &format!(
            "(() => {{ const el = document.querySelector({}); if (!el) return null; \
             el.scrollIntoView && el.scrollIntoView({{block: 'center'}}); \
             const r = el.getBoundingClientRect(); \
             return {{ x: r.left + r.width / 2, y: r.top + r.height / 2, w: r.width, h: r.height }}; }})()",
            json!(selector)
        ),
    )
    .await?;
    let x = value.get("x").and_then(Value::as_f64);
    let y = value.get("y").and_then(Value::as_f64);
    let w = value.get("w").and_then(Value::as_f64).unwrap_or(0.0);
    let h = value.get("h").and_then(Value::as_f64).unwrap_or(0.0);
    match (x, y) {
        (Some(x), Some(y)) if w > 0.0 && h > 0.0 => Ok((x, y)),
        _ => Err(format!(
            "{selector} has no layout box (got {value}); the engine laid it out at zero size"
        )),
    }
}

/// Click the element matching `selector` with real mouse events.
pub async fn click(
    client: &mut CdpClient,
    page: &PageHandle,
    selector: &str,
) -> Result<(), String> {
    let (x, y) = center(client, page, selector).await?;
    for kind in ["mouseMoved", "mousePressed", "mouseReleased"] {
        let mut params = json!({ "type": kind, "x": x, "y": y });
        if kind != "mouseMoved" {
            params["button"] = json!("left");
            params["clickCount"] = json!(1);
        }
        client
            .call("Input.dispatchMouseEvent", params, Some(&page.session_id))
            .await
            .map_err(|error| format!("Input.dispatchMouseEvent {kind}: {error}"))?;
    }
    Ok(())
}

/// Type `text` into the focused element with key events, one per character.
pub async fn type_text(
    client: &mut CdpClient,
    page: &PageHandle,
    text: &str,
) -> Result<(), String> {
    for ch in text.chars() {
        let key = ch.to_string();
        client
            .call(
                "Input.dispatchKeyEvent",
                json!({ "type": "keyDown", "key": key, "text": key, "unmodifiedText": key }),
                Some(&page.session_id),
            )
            .await
            .map_err(|error| format!("Input.dispatchKeyEvent keyDown {key:?}: {error}"))?;
        client
            .call(
                "Input.dispatchKeyEvent",
                json!({ "type": "keyUp", "key": key }),
                Some(&page.session_id),
            )
            .await
            .map_err(|error| format!("Input.dispatchKeyEvent keyUp {key:?}: {error}"))?;
    }
    Ok(())
}

/// The next event, or `None` after `within`.
pub async fn next_event(
    client: &mut CdpClient,
    within: Duration,
) -> Result<Option<CdpEvent>, String> {
    if let Some(event) = client.take_event() {
        return Ok(Some(event));
    }
    match tokio::time::timeout(within, client.next_event()).await {
        Ok(Ok(event)) => Ok(Some(event)),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Ok(None),
    }
}

/// What a Fetch interceptor decided about one paused request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Continue,
    Fail(&'static str),
}

/// One paused request, as the interceptor sees it.
#[derive(Clone, Debug)]
pub struct Paused {
    pub request_id: String,
    pub url: String,
    /// Present at the response stage.
    pub response_headers: Option<Vec<(String, String)>>,
}

impl Paused {
    fn from_event(event: &CdpEvent) -> Option<Self> {
        let request_id = event.str_param("requestId")?.to_string();
        let url = event
            .param("request")
            .and_then(|request| request.get("url"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let response_headers = event
            .param("responseHeaders")
            .and_then(Value::as_array)
            .map(|headers| {
                headers
                    .iter()
                    .filter_map(|header| {
                        Some((
                            header.get("name")?.as_str()?.to_ascii_lowercase(),
                            header.get("value")?.as_str()?.to_string(),
                        ))
                    })
                    .collect()
            });
        Some(Self {
            request_id,
            url,
            response_headers,
        })
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.response_headers
            .as_ref()?
            .iter()
            .find(|(found, _)| found == name)
            .map(|(_, value)| value.as_str())
    }
}

/// Start `expression` without awaiting it, then serve Fetch pauses with
/// `decide` until `result_expression` stops being `null` or `within` passes.
///
/// The expression is not awaited on purpose: a request it makes is paused
/// until this loop answers it, and an awaited evaluate would deadlock against
/// its own interception on a client that serialises calls.
pub async fn run_intercepted(
    client: &mut CdpClient,
    page: &PageHandle,
    expression: &str,
    result_expression: &str,
    within: Duration,
    mut decide: impl FnMut(&Paused) -> Verdict,
) -> Result<(Value, Vec<(Paused, Verdict)>), String> {
    eval(client, page, expression).await?;
    let deadline = Instant::now() + within;
    let mut decisions = Vec::new();
    loop {
        while let Some(event) = next_event(client, Duration::from_millis(150)).await? {
            if event.method != "Fetch.requestPaused" {
                continue;
            }
            let Some(paused) = Paused::from_event(&event) else {
                continue;
            };
            let verdict = decide(&paused);
            let session = event.session_id.clone();
            let session = session.as_deref().or(Some(page.session_id.as_str()));
            let answered = match verdict {
                Verdict::Continue => {
                    client
                        .call(
                            "Fetch.continueRequest",
                            json!({ "requestId": paused.request_id }),
                            session,
                        )
                        .await
                }
                Verdict::Fail(reason) => {
                    client
                        .call(
                            "Fetch.failRequest",
                            json!({ "requestId": paused.request_id, "errorReason": reason }),
                            session,
                        )
                        .await
                }
            };
            answered.map_err(|error| format!("answering a paused request: {error}"))?;
            decisions.push((paused, verdict));
        }
        let value = eval(client, page, result_expression).await?;
        if !value.is_null() {
            return Ok((value, decisions));
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "{result_expression} stayed null for {}ms; decisions so far: {decisions:?}",
                within.as_millis()
            ));
        }
    }
}

/// Whether an error is a CDP call timeout.
pub fn is_timeout(error: &SidebarBrowserError) -> bool {
    matches!(error, SidebarBrowserError::Timeout { .. })
}
