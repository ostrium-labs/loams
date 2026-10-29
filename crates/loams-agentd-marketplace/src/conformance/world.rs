//! The agent's world during a run: one loopback HTTP server that is
//! - the **scripted model** (`/openai/v1/chat/completions`, OpenAI Chat
//!   Completions, plain or streamed), which answers from a script instead of
//!   a model;
//! - the **fake MCP servers** (`/mcp/<name>`, streamable HTTP with JSON
//!   answers), one per declared MCP-server dependency, offering exactly the
//!   tools the listing declares from it;
//! - the **egress proxy** (`HTTPS_PROXY`, `HTTP_PROXY`), which records every
//!   host asked for and refuses it; requests for the server itself pass;
//! - a stub **Loams endpoint** (`/loams`), which answers 503: Core runs have
//!   no Loams (the Compatible level brings one).
//!
//! The server reads at most 64 KiB of headers and 8 MiB of body, answers one
//! request per connection and closes it.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use loams_agentd_harness::CancellationToken;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;

/// The text the scripted model ends a turn with; C3 looks for it in what
/// the agent sends the client.
pub(super) const SCRIPTED_REPLY: &str = "LAPC-SCRIPTED-REPLY: the scripted turn is complete.";

const MAX_HEAD: usize = 64 * 1024;
const MAX_BODY: usize = 8 * 1024 * 1024;
/// At most this many tool calls per scripted turn.
const MAX_SCRIPTED_CALLS: usize = 4;

/// The phase of the run a request arrived in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
    Start,
    Turn,
    Cancel,
}

#[derive(Debug, Clone)]
pub(super) struct ModelRequest {
    pub seq: u64,
    pub phase: Phase,
    pub offered: Vec<String>,
    /// `tool_call_id`s of the tool results since the last user message.
    pub tool_results: Vec<String>,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

#[derive(Debug, Clone)]
pub(super) struct ScriptedCall {
    pub id: String,
    pub tool: String,
}

#[derive(Debug, Clone)]
pub(super) struct McpCall {
    pub seq: u64,
    pub server: String,
    pub tool: String,
    pub declared: bool,
}

#[derive(Debug, Clone)]
pub(super) struct ProxyRequest {
    pub host: String,
    pub port: u16,
    pub allowed: bool,
}

#[derive(Debug, Clone, Default)]
pub(super) struct Records {
    pub model: Vec<ModelRequest>,
    pub scripted: Vec<ScriptedCall>,
    pub mcp: Vec<McpCall>,
    pub proxy: Vec<ProxyRequest>,
    /// Model API paths the scripted model does not speak.
    pub unsupported: Vec<String>,
}

/// What the scripted model and the fake MCP servers work from.
#[derive(Debug, Clone, Default)]
pub(super) struct Script {
    /// Declared tool names, in listing order.
    pub declared: Vec<String>,
    pub always_ask: Vec<String>,
    /// Declared egress hosts (`*.` for subdomains).
    pub egress: Vec<String>,
    /// MCP server name → (listing id, its declared tool names).
    pub mcp: BTreeMap<String, (String, Vec<String>)>,
}

impl Script {
    /// The tool calls a turn makes: the offered declared tools, the ones
    /// that always ask first, at most four.
    fn plan(&self, offered: &[String]) -> Vec<String> {
        let offered_declared = |name: &&String| offered.contains(name);
        let mut plan: Vec<String> = self
            .declared
            .iter()
            .filter(offered_declared)
            .filter(|n| self.always_ask.contains(n))
            .cloned()
            .collect();
        plan.extend(
            self.declared
                .iter()
                .filter(offered_declared)
                .filter(|n| !self.always_ask.contains(n))
                .cloned(),
        );
        plan.truncate(MAX_SCRIPTED_CALLS);
        plan
    }

    pub(super) fn host_allowed(&self, host: &str) -> bool {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        self.egress.iter().any(|pattern| {
            let pattern = pattern.to_ascii_lowercase();
            match pattern.strip_prefix("*.") {
                Some(domain) => host.ends_with(&format!(".{domain}")),
                None => host == pattern,
            }
        })
    }
}

#[derive(Debug)]
struct Shared {
    script: Script,
    records: Mutex<Records>,
    seq: Arc<AtomicU64>,
    phase: Mutex<Phase>,
    hang: AtomicBool,
    hung: Notify,
    release: CancellationToken,
    calls: AtomicU64,
}

#[derive(Debug)]
pub(super) struct World {
    addr: SocketAddr,
    shared: Arc<Shared>,
    accept: tokio::task::JoinHandle<()>,
}

impl World {
    pub(super) async fn start(script: Script, seq: Arc<AtomicU64>) -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let addr = listener.local_addr()?;
        let shared = Arc::new(Shared {
            script,
            records: Mutex::default(),
            seq,
            phase: Mutex::new(Phase::Start),
            hang: AtomicBool::new(false),
            hung: Notify::new(),
            release: CancellationToken::new(),
            calls: AtomicU64::new(0),
        });
        let serving = Arc::clone(&shared);
        let accept = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(serve(stream, Arc::clone(&serving), addr));
            }
        });
        Ok(Self {
            addr,
            shared,
            accept,
        })
    }

    /// `http://127.0.0.1:<port>`.
    pub(super) fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub(super) fn set_phase(&self, phase: Phase) {
        *lock(&self.shared.phase) = phase;
    }

    /// From now on the scripted model holds every request until
    /// [`World::release`]: the cancel probe of C7.
    pub(super) fn hang_model(&self) {
        self.shared.hang.store(true, Ordering::SeqCst);
    }

    /// Resolves when a model request is being held.
    pub(super) async fn model_held(&self) {
        self.shared.hung.notified().await;
    }

    pub(super) fn release(&self) {
        self.shared.release.cancel();
    }

    pub(super) fn records(&self) -> Records {
        lock(&self.shared.records).clone()
    }
}

impl Drop for World {
    fn drop(&mut self) {
        self.shared.release.cancel();
        self.accept.abort();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct Request {
    method: String,
    target: String,
    body: Vec<u8>,
}

struct Response {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
}

impl Response {
    fn json(status: u16, value: &Value) -> Self {
        Self {
            status,
            content_type: "application/json",
            body: value.to_string().into_bytes(),
        }
    }

    fn empty(status: u16) -> Self {
        Self {
            status,
            content_type: "text/plain",
            body: Vec::new(),
        }
    }

    fn error(status: u16, message: &str) -> Self {
        Self::json(status, &json!({ "error": { "message": message } }))
    }
}

async fn serve(mut stream: TcpStream, shared: Arc<Shared>, own: SocketAddr) {
    let Ok(Some(request)) = read_request(&mut stream).await else {
        return;
    };
    let Some(response) = route(request, &shared, own).await else {
        return;
    };
    let reason = match response.status {
        200 => "OK",
        202 => "Accepted",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        _ => "Bad Request",
    };
    let head = format!(
        "HTTP/1.1 {} {reason}\r\ncontent-type: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        response.status,
        response.content_type,
        response.body.len()
    );
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(&response.body).await;
    let _ = stream.shutdown().await;
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

async fn read_request(stream: &mut TcpStream) -> std::io::Result<Option<Request>> {
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        if let Some(at) = find(&buf, b"\r\n\r\n") {
            break at;
        }
        if buf.len() > MAX_HEAD {
            return Ok(None);
        }
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Ok(None);
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut rest = buf.split_off(head_end + 4);
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or_default().split_whitespace();
    let method = first.next().unwrap_or_default().to_owned();
    let target = first.next().unwrap_or_default().to_owned();
    let mut length = 0usize;
    let mut chunked = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            length = value.parse().unwrap_or(0);
        } else if name.eq_ignore_ascii_case("transfer-encoding")
            && value.to_ascii_lowercase().contains("chunked")
        {
            chunked = true;
        }
    }
    let body = if chunked {
        read_chunked(stream, rest).await?
    } else {
        if length > MAX_BODY {
            return Ok(None);
        }
        while rest.len() < length {
            let n = stream.read(&mut chunk).await?;
            if n == 0 {
                return Ok(None);
            }
            rest.extend_from_slice(&chunk[..n]);
        }
        rest.truncate(length);
        rest
    };
    Ok(Some(Request {
        method,
        target,
        body,
    }))
}

/// Reads more bytes into `buf`; the end of the stream is an error here.
async fn fill(stream: &mut TcpStream, buf: &mut Vec<u8>) -> std::io::Result<()> {
    let mut chunk = [0u8; 8192];
    let n = stream.read(&mut chunk).await?;
    if n == 0 {
        return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
    }
    buf.extend_from_slice(&chunk[..n]);
    Ok(())
}

/// Decodes a chunked body (`size\r\ndata\r\n` … `0\r\n\r\n`).
async fn read_chunked(stream: &mut TcpStream, mut buf: Vec<u8>) -> std::io::Result<Vec<u8>> {
    let mut body = Vec::new();
    loop {
        let line_end = loop {
            if let Some(at) = find(&buf, b"\r\n") {
                break at;
            }
            if buf.len() > MAX_HEAD {
                return Err(std::io::Error::from(std::io::ErrorKind::InvalidData));
            }
            fill(stream, &mut buf).await?;
        };
        let size_text = String::from_utf8_lossy(&buf[..line_end]).into_owned();
        let size = usize::from_str_radix(size_text.split(';').next().unwrap_or("").trim(), 16)
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidData))?;
        buf.drain(..line_end + 2);
        if size == 0 {
            return Ok(body);
        }
        if body.len() + size > MAX_BODY {
            return Err(std::io::Error::from(std::io::ErrorKind::InvalidData));
        }
        while buf.len() < size + 2 {
            fill(stream, &mut buf).await?;
        }
        body.extend_from_slice(&buf[..size]);
        buf.drain(..size + 2);
    }
}

/// `host:port` (with `[v6]:port`), the port defaulting to `default`.
fn split_authority(authority: &str, default: u16) -> (String, u16) {
    if let Some(rest) = authority.strip_prefix('[')
        && let Some((host, after)) = rest.split_once(']')
    {
        let port = after
            .strip_prefix(':')
            .and_then(|p| p.parse().ok())
            .unwrap_or(default);
        return (host.to_owned(), port);
    }
    match authority.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => {
            (host.to_owned(), port.parse().unwrap_or(default))
        }
        _ => (authority.to_owned(), default),
    }
}

fn is_self(host: &str, port: u16, own: SocketAddr) -> bool {
    port == own.port() && matches!(host, "127.0.0.1" | "localhost" | "::1")
}

async fn route(request: Request, shared: &Shared, own: SocketAddr) -> Option<Response> {
    let record_proxy = |host: String, port: u16| {
        let allowed = shared.script.host_allowed(&host);
        lock(&shared.records).proxy.push(ProxyRequest {
            host,
            port,
            allowed,
        });
        Some(Response::error(
            403,
            "the LAPC egress proxy records hosts and forwards none",
        ))
    };
    if request.method.eq_ignore_ascii_case("CONNECT") {
        let (host, port) = split_authority(&request.target, 443);
        if is_self(&host, port, own) {
            return Some(Response::error(403, "no tunnels to the runner itself"));
        }
        return record_proxy(host, port);
    }
    let mut path = request.target.clone();
    for (scheme, default) in [("http://", 80u16), ("https://", 443)] {
        if let Some(rest) = request.target.strip_prefix(scheme) {
            let (authority, tail) = match rest.find('/') {
                Some(at) => (&rest[..at], &rest[at..]),
                None => (rest, "/"),
            };
            let (host, port) = split_authority(authority, default);
            if !is_self(&host, port, own) {
                return record_proxy(host, port);
            }
            path = tail.to_owned();
        }
    }
    let path = path.split('?').next().unwrap_or_default().to_owned();
    if path == "/openai/v1/chat/completions" && request.method == "POST" {
        return model(&request.body, shared).await;
    }
    if path.starts_with("/openai/") || path.starts_with("/anthropic/") || path == "/anthropic" {
        lock(&shared.records).unsupported.push(path);
        return Some(Response::error(
            501,
            "the LAPC scripted model speaks OpenAI Chat Completions only",
        ));
    }
    if let Some(name) = path.strip_prefix("/mcp/") {
        return Some(match request.method.as_str() {
            "POST" => mcp(name, &request.body, shared),
            "DELETE" => Response::empty(200),
            _ => Response::empty(405),
        });
    }
    if path == "/loams" || path.starts_with("/loams/") {
        return Some(Response::error(503, "LAPC Core runs have no Loams"));
    }
    Some(Response::error(404, "not found"))
}

/// The runner's own token estimate: four characters per token, the same
/// figure the scripted model reports in `usage`.
pub(super) fn estimate_tokens(chars: usize) -> u64 {
    chars.div_ceil(4) as u64
}

async fn model(body: &[u8], shared: &Shared) -> Option<Response> {
    let Ok(request) = serde_json::from_slice::<Value>(body) else {
        return Some(Response::error(400, "the body is not JSON"));
    };
    let phase = *lock(&shared.phase);
    let messages = request["messages"].as_array().cloned().unwrap_or_default();
    let offered: Vec<String> = request["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| {
            t["function"]["name"]
                .as_str()
                .or(t["name"].as_str())
                .map(str::to_owned)
        })
        .collect();
    let last_user = messages
        .iter()
        .rposition(|m| m["role"] == "user")
        .unwrap_or(0);
    let tool_results: Vec<String> = messages[last_user..]
        .iter()
        .filter(|m| m["role"] == "tool")
        .filter_map(|m| m["tool_call_id"].as_str().map(str::to_owned))
        .collect();
    let prompt_tokens =
        estimate_tokens(request["messages"].to_string().len() + request["tools"].to_string().len());
    let seq = shared.seq.fetch_add(1, Ordering::SeqCst);

    if shared.hang.load(Ordering::SeqCst) {
        lock(&shared.records).model.push(ModelRequest {
            seq,
            phase,
            offered,
            tool_results,
            prompt_tokens,
            completion_tokens: 0,
        });
        shared.hung.notify_one();
        let _ = tokio::time::timeout(Duration::from_secs(600), shared.release.cancelled()).await;
        return None;
    }

    let plan = shared.script.plan(&offered);
    let step = tool_results.len();
    let message = if step < plan.len() {
        let tool = plan[step].clone();
        let n = shared.calls.fetch_add(1, Ordering::SeqCst);
        let id = format!("lapc_call_{n}");
        let arguments = arguments_for(&request, &tool);
        lock(&shared.records).scripted.push(ScriptedCall {
            id: id.clone(),
            tool: tool.clone(),
        });
        json!({
            "role": "assistant",
            "content": null,
            "tool_calls": [{
                "id": id,
                "type": "function",
                "function": { "name": tool, "arguments": arguments.to_string() },
            }],
        })
    } else {
        json!({ "role": "assistant", "content": SCRIPTED_REPLY })
    };
    let finish = if message["tool_calls"].is_array() {
        "tool_calls"
    } else {
        "stop"
    };
    let completion_tokens = estimate_tokens(message.to_string().len());
    lock(&shared.records).model.push(ModelRequest {
        seq,
        phase,
        offered,
        tool_results,
        prompt_tokens,
        completion_tokens,
    });
    let usage = json!({
        "prompt_tokens": prompt_tokens,
        "completion_tokens": completion_tokens,
        "total_tokens": prompt_tokens + completion_tokens,
    });
    let model = request["model"]
        .as_str()
        .unwrap_or("lapc-scripted")
        .to_owned();
    if request["stream"].as_bool() == Some(true) {
        return Some(stream_response(&model, &message, finish, &usage));
    }
    Some(Response::json(
        200,
        &json!({
            "id": format!("chatcmpl-lapc-{seq}"),
            "object": "chat.completion",
            "created": 0,
            "model": model,
            "choices": [{ "index": 0, "message": message, "finish_reason": finish }],
            "usage": usage,
        }),
    ))
}

/// The same answer as server-sent events, as `stream: true` asks.
fn stream_response(model: &str, message: &Value, finish: &str, usage: &Value) -> Response {
    let chunk = |delta: Value, finish: Value, usage: Option<&Value>| {
        let mut value = json!({
            "id": "chatcmpl-lapc",
            "object": "chat.completion.chunk",
            "created": 0,
            "model": model,
            "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }],
        });
        if let Some(usage) = usage {
            value["usage"] = usage.clone();
        }
        format!("data: {value}\n\n")
    };
    let mut body = chunk(json!({ "role": "assistant" }), Value::Null, None);
    if let Some(calls) = message["tool_calls"].as_array() {
        let calls: Vec<Value> = calls
            .iter()
            .enumerate()
            .map(|(index, call)| {
                let mut call = call.clone();
                call["index"] = json!(index);
                call
            })
            .collect();
        body.push_str(&chunk(json!({ "tool_calls": calls }), Value::Null, None));
    } else if let Some(text) = message["content"].as_str() {
        body.push_str(&chunk(json!({ "content": text }), Value::Null, None));
    }
    body.push_str(&chunk(json!({}), json!(finish), Some(usage)));
    body.push_str("data: [DONE]\n\n");
    Response {
        status: 200,
        content_type: "text/event-stream",
        body: body.into_bytes(),
    }
}

/// Arguments that satisfy the offered tool's JSON Schema: each required
/// property gets a plain value of its type.
fn arguments_for(request: &Value, tool: &str) -> Value {
    let parameters = request["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|t| t["function"]["name"] == tool || t["name"] == tool)
        .map(|t| {
            if t["function"].is_object() {
                t["function"]["parameters"].clone()
            } else {
                t["parameters"].clone()
            }
        })
        .unwrap_or(Value::Null);
    let mut arguments = serde_json::Map::new();
    for name in parameters["required"].as_array().into_iter().flatten() {
        let Some(name) = name.as_str() else { continue };
        let schema = &parameters["properties"][name];
        let value = match schema["type"].as_str() {
            Some("number" | "integer") => json!(1),
            Some("boolean") => json!(true),
            Some("array") => json!([]),
            Some("object") => json!({}),
            _ => match schema["enum"].as_array().and_then(|v| v.first()) {
                Some(first) => first.clone(),
                None => json!("lapc"),
            },
        };
        arguments.insert(name.to_owned(), value);
    }
    Value::Object(arguments)
}

fn mcp(name: &str, body: &[u8], shared: &Shared) -> Response {
    let Some((server_id, tools)) = shared.script.mcp.get(name) else {
        return Response::error(404, "no such MCP server");
    };
    let Ok(request) = serde_json::from_slice::<Value>(body) else {
        return Response::json(
            200,
            &json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": "parse error" } }),
        );
    };
    let id = request["id"].clone();
    let method = request["method"].as_str().unwrap_or_default();
    if method.starts_with("notifications/") || request.get("id").is_none() {
        return Response::empty(202);
    }
    let answer = |result: Value| {
        Response::json(
            200,
            &json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        )
    };
    let error = |code: i64, message: &str| {
        Response::json(
            200,
            &json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
        )
    };
    match method {
        "initialize" => answer(json!({
            "protocolVersion": request["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": format!("lapc-fake-{name}"), "version": "1.0.0" },
        })),
        "ping" => answer(json!({})),
        "tools/list" => answer(json!({
            "tools": tools.iter().map(|tool| json!({
                "name": tool,
                "description": format!("{tool}, from the fake {server_id} server of the LAPC runner."),
                "inputSchema": { "type": "object", "properties": {} },
            })).collect::<Vec<_>>(),
        })),
        "tools/call" => {
            let tool = request["params"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            let declared = tools.contains(&tool);
            lock(&shared.records).mcp.push(McpCall {
                seq: shared.seq.fetch_add(1, Ordering::SeqCst),
                server: server_id.clone(),
                tool: tool.clone(),
                declared,
            });
            if declared {
                answer(json!({
                    "content": [{ "type": "text", "text": format!("{tool}: done (LAPC fake)") }],
                    "isError": false,
                }))
            } else {
                error(-32602, "unknown tool")
            }
        }
        _ => error(-32601, "method not found"),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn script() -> Script {
        Script {
            declared: vec!["a".into(), "b".into(), "c".into()],
            always_ask: vec!["c".into()],
            egress: vec!["api.example.com".into(), "*.example.org".into()],
            mcp: BTreeMap::new(),
        }
    }

    #[test]
    fn plan_asks_first_and_skips_what_is_not_offered() {
        let offered = vec!["a".into(), "c".into(), "x".into()];
        assert_eq!(script().plan(&offered), vec!["c", "a"]);
    }

    #[test]
    fn hosts_match_exactly_or_by_subdomain() {
        let s = script();
        assert!(s.host_allowed("api.example.com"));
        assert!(s.host_allowed("API.Example.com."));
        assert!(!s.host_allowed("evil.api.example.com"));
        assert!(s.host_allowed("cdn.example.org"));
        assert!(!s.host_allowed("example.org"));
    }

    #[test]
    fn authorities_split() {
        assert_eq!(split_authority("a.b:8443", 443), ("a.b".into(), 8443));
        assert_eq!(split_authority("a.b", 443), ("a.b".into(), 443));
        assert_eq!(split_authority("[::1]:80", 443), ("::1".into(), 80));
    }

    #[test]
    fn arguments_fill_required_properties() {
        let request = json!({ "tools": [{ "type": "function", "function": {
            "name": "t",
            "parameters": { "type": "object", "required": ["s", "n"], "properties": {
                "s": { "type": "string" }, "n": { "type": "integer" }, "o": { "type": "string" } } } } }] });
        assert_eq!(arguments_for(&request, "t"), json!({ "s": "lapc", "n": 1 }));
    }

    #[tokio::test]
    async fn the_scripted_model_streams_when_asked() {
        let world = World::start(script(), Arc::default()).await.unwrap();
        let mut stream = TcpStream::connect(world.addr).await.unwrap();
        let body = json!({ "model": "m", "stream": true, "messages": [{ "role": "user", "content": "hi" }] }).to_string();
        let request = format!(
            "POST /openai/v1/chat/completions HTTP/1.1\r\nhost: x\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).await.unwrap();
        assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
        assert!(answer.contains("text/event-stream"));
        assert!(answer.contains(SCRIPTED_REPLY));
        assert!(answer.trim_end().ends_with("data: [DONE]"));
    }

    #[tokio::test]
    async fn the_proxy_records_and_refuses() {
        let world = World::start(script(), Arc::default()).await.unwrap();
        let mut stream = TcpStream::connect(world.addr).await.unwrap();
        stream
            .write_all(b"CONNECT exfil.example:443 HTTP/1.1\r\nhost: exfil.example:443\r\n\r\n")
            .await
            .unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).await.unwrap();
        assert!(answer.starts_with("HTTP/1.1 403"), "{answer}");
        let records = world.records();
        assert_eq!(records.proxy.len(), 1);
        assert_eq!(records.proxy[0].host, "exfil.example");
        assert!(!records.proxy[0].allowed);
    }
}
