//! The v2 WebMCP tools: `list_webmcp_tools` and `call_webmcp_tool` (D570).
//!
//! A page can register tools with `document.modelContext`, so an agent calls a
//! function instead of clicking. That is an **enhancement, never the only
//! path** (D568), and D635 makes the rule load-bearing rather than prudent:
//! WebKit's standards position on WebMCP is closed and `oppose`, so Safari is
//! not a browser that will grow the API later — it is a browser that has said
//! no. Firefox is implementing behind a pref; Chrome is trialling.
//!
//! So every rule here is written for the absent case being the *normal* one:
//!
//! * **Detection is per call, never a capability claim.** The injected script
//!   feature-detects `document.modelContext` itself, which is also the only way
//!   to be right: the API is `[SecureContext]`, and a page can register,
//!   unregister and re-register tools between two calls.
//! * **Absence is an answer, not an error.** [`WebmcpAbsent`] says which of the
//!   reasons it was — the browser has no API, the page is not a secure context,
//!   the `tools` Permissions Policy refuses this origin, the driver cannot
//!   evaluate a script, the page refused — and every message names the way on
//!   (D504 rule 9). The rest of the toolbox is untouched: a Safari page costs a
//!   sentence, not a failed session.
//! * **`call_webmcp_tool` requires a name `list_webmcp_tools` would have
//!   returned.** The name is validated against the draft's rule (1–128
//!   characters of `[A-Za-z0-9_.-]`) before anything is sent, and the page is
//!   asked for its own list before the call, so a stale or invented name comes
//!   back as "not registered; list again" rather than as a silent no-op.
//! * **Outputs obey D504.** A listing is one line per tool with the
//!   description truncated and the whole inside the token budget with a marker
//!   naming the way to continue; a call answers in one line plus the change
//!   summary, then the tool's own return value, also budgeted. Page text is
//!   untrusted (D509): it is escaped and truncated here, and it is never
//!   concatenated into the script that reads it.

use serde_json::Value;

use crate::error::BridgeError;
use crate::tool::{ChangeSummary, DEFAULT_BUDGET_CHARS, MAX_NAME_CHARS};

mod script;

/// The longest a WebMCP tool name may be, from the draft: `name` is 1 to 128
/// characters of `[A-Za-z0-9_.-]`.
pub const MAX_TOOL_NAME_CHARS: usize = 128;

/// How many tool entries are read out of a page's answer.
///
/// A page's list is untrusted input (D509), so it is bounded before it is
/// parsed rather than after: a hostile page should cost a capped answer, not
/// memory.
pub const MAX_TOOLS_PARSED: usize = 512;

/// How long `call_webmcp_tool` waits for a tool, in milliseconds.
///
/// A tool is page code, so it can never return. The bound is in the injected
/// script as well as in the transport, so a hung tool cannot wedge a session.
pub const DEFAULT_CALL_TIMEOUT_MS: u64 = 30_000;

/// A validated WebMCP tool name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WebmcpToolName(String);

impl WebmcpToolName {
    /// Validate `raw` against the draft's rule for `ModelContextTool.name`.
    pub fn parse(raw: &str) -> Result<Self, BridgeError> {
        if raw.is_empty() || raw.chars().count() > MAX_TOOL_NAME_CHARS {
            return Err(BridgeError::policy(format!(
                "{raw:?} is not a WebMCP tool name: it must be 1 to {MAX_TOOL_NAME_CHARS} \
                 characters; call list_webmcp_tools for the names this page registered"
            )));
        }
        if !raw
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
        {
            return Err(BridgeError::policy(format!(
                "{raw:?} is not a WebMCP tool name: letters, digits, `.`, `_` and `-` only; \
                 call list_webmcp_tools for the names this page registered"
            )));
        }
        Ok(Self(raw.to_string()))
    }

    /// The name as written.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for WebmcpToolName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The `annotations` a tool may carry. Every hint defaults to false, so an
/// absent annotation and a false one read the same.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WebmcpAnnotations {
    /// `readOnlyHint`: the tool does not change the page.
    pub read_only: bool,
    /// `untrustedContentHint`: the result is content, not an instruction.
    pub untrusted_content: bool,
    /// `consequentialHint`: the tool has an effect a person would want to see.
    pub consequential: bool,
    /// `debuggingHint`: the tool is not for production use.
    pub debugging: bool,
}

impl WebmcpAnnotations {
    /// Read the four hints out of a page's JSON, defensively: a missing object,
    /// a missing member or a member of the wrong type all mean false.
    pub fn from_value(value: Option<&Value>) -> Self {
        let flag = |key: &str| {
            value
                .and_then(|object| object.get(key))
                .and_then(Value::as_bool)
                .unwrap_or(false)
        };
        Self {
            read_only: flag("readOnlyHint"),
            untrusted_content: flag("untrustedContentHint"),
            consequential: flag("consequentialHint"),
            debugging: flag("debuggingHint"),
        }
    }

    /// The hints that are set, in the draft's order.
    pub fn render(&self) -> String {
        [
            (self.read_only, "readOnly"),
            (self.untrusted_content, "untrustedContent"),
            (self.consequential, "consequential"),
            (self.debugging, "debugging"),
        ]
        .into_iter()
        .filter(|(set, _)| *set)
        .map(|(_, name)| name)
        .collect::<Vec<&str>>()
        .join(", ")
    }
}

/// One tool the page has registered.
#[derive(Debug, Clone, PartialEq)]
pub struct WebmcpTool {
    /// The validated name, which is what `call_webmcp_tool` takes.
    pub name: WebmcpToolName,
    /// `title`, the human-facing name, when the page gave one.
    pub title: Option<String>,
    /// `description`, from the page: untrusted text, escaped when rendered.
    pub description: String,
    /// `inputSchema`, when the page gave one.
    pub input_schema: Option<Value>,
    /// The four hints.
    pub annotations: WebmcpAnnotations,
}

impl WebmcpTool {
    /// Read one entry of a page's answer.
    ///
    /// `None` when the entry is not an object or its `name` is not a name this
    /// draft would accept: an unusable entry is dropped and counted, never
    /// rendered as if it were a tool.
    pub fn parse(value: &Value) -> Option<Self> {
        let name = WebmcpToolName::parse(value.get("name")?.as_str()?).ok()?;
        let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_string);
        Some(Self {
            name,
            title: text("title").filter(|title| !title.is_empty()),
            description: text("description").unwrap_or_default(),
            input_schema: value.get("inputSchema").cloned(),
            annotations: WebmcpAnnotations::from_value(value.get("annotations")),
        })
    }

    /// The properties `inputSchema` marks as required.
    pub fn required_inputs(&self) -> Vec<String> {
        self.input_schema
            .as_ref()
            .and_then(|schema| schema.get("required"))
            .and_then(Value::as_array)
            .map(|required| {
                required
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// One line: `name "title" — description [readOnly] requires=a,b`.
    pub fn render(&self) -> String {
        let mut line = self.name.to_string();
        if let Some(title) = &self.title {
            line.push_str(&format!(" \"{}\"", crate::tool::escape(title)));
        }
        if !self.description.is_empty() {
            let text = crate::tool::truncate(&self.description, MAX_NAME_CHARS);
            line.push_str(&format!(" — {}", crate::tool::escape(&text)));
        }
        let hints = self.annotations.render();
        if !hints.is_empty() {
            line.push_str(&format!(" [{hints}]"));
        }
        let required = self.required_inputs();
        if !required.is_empty() {
            line.push_str(&format!(" requires={}", required.join(",")));
        }
        line
    }
}

/// Why a page's `document.modelContext` is out of reach.
///
/// Every variant is a sentence an agent can act on, because the alternative —
/// an empty list with no explanation — is indistinguishable from a page that
/// chose not to register anything, which is the one case where the agent should
/// give up and drive the UI instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebmcpAbsent {
    /// The browser does not implement the API. Safari and every WebKit engine
    /// is the expected case (D635).
    NotExposed,
    /// The page is not a secure context, and the API is `[SecureContext]`.
    InsecureContext,
    /// The page's `tools` Permissions Policy does not allow this origin.
    BlockedByPermissionsPolicy,
    /// The driver cannot evaluate a script in the page at all.
    DriverCannotEvaluate,
    /// The provider has no WebMCP path.
    ProviderCannotReach,
    /// The page answered with an error.
    Rejected(String),
}

impl WebmcpAbsent {
    /// The reason as a short slug, for a trace field.
    pub fn as_str(&self) -> &str {
        match self {
            WebmcpAbsent::NotExposed => "not-exposed",
            WebmcpAbsent::InsecureContext => "insecure-context",
            WebmcpAbsent::BlockedByPermissionsPolicy => "blocked-by-permissions-policy",
            WebmcpAbsent::DriverCannotEvaluate => "driver-cannot-evaluate",
            WebmcpAbsent::ProviderCannotReach => "provider-cannot-reach",
            WebmcpAbsent::Rejected(_) => "rejected",
        }
    }

    /// The reason as one sentence that names the way on.
    pub fn note(&self) -> String {
        match self {
            WebmcpAbsent::NotExposed => "this browser does not expose document.modelContext, \
                 which is the normal state in Safari and any WebKit engine, since WebKit opposes \
                 WebMCP; drive the page with take_snapshot, find and click instead"
                .to_string(),
            WebmcpAbsent::InsecureContext => "the page is not a secure context and \
                 document.modelContext is [SecureContext]; open the page over https, or drive it \
                 with take_snapshot and click"
                .to_string(),
            WebmcpAbsent::BlockedByPermissionsPolicy => "the page's tools Permissions Policy does \
                 not allow this origin, and its default allowlist is ['self']; the page must opt \
                 the agent origin in, or the agent drives the page with take_snapshot and click"
                .to_string(),
            WebmcpAbsent::DriverCannotEvaluate => "this driver cannot evaluate a script in the \
                 page, so document.modelContext is out of reach; drive the page with \
                 take_snapshot and click"
                .to_string(),
            WebmcpAbsent::ProviderCannotReach => "this provider has no WebMCP path; drive the \
                 page with take_snapshot and click"
                .to_string(),
            WebmcpAbsent::Rejected(reason) => format!(
                "the page refused to answer ({reason}); drive the page with take_snapshot and \
                 click"
            ),
        }
    }

    /// Map the slug the injected script reports onto a reason.
    ///
    /// `NotAllowedError` is the Permissions Policy refusal D635 records, and it
    /// is the one error name worth naming precisely: it is the page's
    /// configuration, not a browser limitation, and only the page can change
    /// it. Anything else is reported verbatim (truncated, because it is page
    /// text) rather than guessed at.
    fn from_reason(reason: &str) -> Self {
        match reason {
            "" | "not-exposed" => WebmcpAbsent::NotExposed,
            "insecure-context" => WebmcpAbsent::InsecureContext,
            other => {
                // `nameOf` in the injected script names an exception with its
                // message when it has one, and the message is the page's to
                // write — a real `NotAllowedError` carries one. So the typed
                // reason is read off the error's *name*, which is the part the
                // page cannot rename, and only the name.
                let name = other.split(':').next().unwrap_or(other).trim();
                if name == "NotAllowedError" {
                    return WebmcpAbsent::BlockedByPermissionsPolicy;
                }
                WebmcpAbsent::Rejected(crate::tool::truncate(other, MAX_NAME_CHARS))
            }
        }
    }

    /// Whether this reason is one of the *named* absences rather than a page's
    /// own refusal.
    ///
    /// A refusal keeps the page's own words because they are the only thing
    /// that says what went wrong; a named absence is the bridge's own fact and
    /// has a message of its own that names the way on.
    fn is_named(&self) -> bool {
        !matches!(self, WebmcpAbsent::Rejected(_))
    }
}

/// Whether a page's WebMCP surface is usable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    /// The page exposes `document.modelContext`.
    Available,
    /// It does not, for a named reason.
    Absent(WebmcpAbsent),
}

impl Availability {
    /// Whether the tools can be listed or called.
    pub fn is_available(&self) -> bool {
        matches!(self, Availability::Available)
    }

    /// The reason, when it is absent.
    pub fn absent(&self) -> Option<&WebmcpAbsent> {
        match self {
            Availability::Available => None,
            Availability::Absent(reason) => Some(reason),
        }
    }
}

/// What `list_webmcp_tools` takes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListWebmcpToolsRequest {
    /// Keep only the tools whose name contains this, case-insensitively. The
    /// filter is applied in the page *and* here: the page's copy saves the
    /// bytes, and this crate's copy is the one that counts, because the answer
    /// is untrusted (D509).
    pub filter: Option<String>,
    /// Override the character budget.
    pub budget_chars: Option<usize>,
}

impl ListWebmcpToolsRequest {
    /// The whole page, unfiltered, within the default budget.
    pub fn new() -> Self {
        Self::default()
    }

    /// Only the tools whose name contains `needle`.
    pub fn with_filter(mut self, needle: impl Into<String>) -> Self {
        self.filter = Some(needle.into());
        self
    }

    /// With an explicit budget.
    pub fn with_budget_chars(mut self, budget: usize) -> Self {
        self.budget_chars = Some(budget);
        self
    }

    /// The budget in force.
    pub fn budget(&self) -> usize {
        self.budget_chars.unwrap_or(DEFAULT_BUDGET_CHARS)
    }
}

/// What `list_webmcp_tools` answers.
#[derive(Debug, Clone, PartialEq)]
pub struct WebmcpListing {
    /// Whether the page exposes `document.modelContext`.
    pub availability: Availability,
    /// The tools, parsed and filtered.
    pub tools: Vec<WebmcpTool>,
    /// How many entries the page's answer held, before the cap.
    pub total: usize,
    /// Entries dropped because they were not a tool this draft would accept.
    pub malformed: usize,
    /// The filter in force, echoed so the truncation marker can name it.
    pub filter: Option<String>,
    /// The character budget in force.
    pub budget_chars: usize,
}

impl WebmcpListing {
    /// An answer for a page that exposes no WebMCP surface.
    pub fn absent(availability: WebmcpAbsent, request: &ListWebmcpToolsRequest) -> Self {
        Self {
            availability: Availability::Absent(availability),
            tools: Vec::new(),
            total: 0,
            malformed: 0,
            filter: request.filter.clone(),
            budget_chars: request.budget(),
        }
    }

    /// The whole answer as one string, inside the budget.
    pub fn render(&self) -> String {
        let Availability::Absent(reason) = &self.availability else {
            return self.render_tools();
        };
        format!("no WebMCP tools: {}", reason.note())
    }

    fn render_tools(&self) -> String {
        if self.tools.is_empty() {
            if self.malformed > 0 {
                return format!(
                    "this page returned {} WebMCP entries and none of them named a tool this \
                     draft accepts; drive it with take_snapshot and click",
                    self.malformed
                );
            }
            return "this page registered no WebMCP tools; drive it with take_snapshot and click"
                .to_string();
        }
        let mut lines = Vec::new();
        let header = match &self.filter {
            None => format!("{} WebMCP tools registered", self.total),
            Some(filter) => format!(
                "{} of {} WebMCP tools match {filter:?}",
                self.tools.len(),
                self.total
            ),
        };
        let mut used = header.len() + 1;
        let mut emitted = 0usize;
        lines.push(header);
        for tool in &self.tools {
            let line = format!("  {}", tool.render());
            let cost = line.len() + 1;
            if used + cost > self.budget_chars {
                break;
            }
            used += cost;
            lines.push(line);
            emitted += 1;
        }
        let remaining = self.tools.len().saturating_sub(emitted);
        if remaining > 0 {
            lines.push(format!(
                "… {remaining} more not shown: call list_webmcp_tools with filter= to narrow, or \
                 call call_webmcp_tool with one of the names above"
            ));
        }
        if self.malformed > 0 {
            lines.push(format!(
                "… {} of the page's entries named no usable tool and are not shown",
                self.malformed
            ));
        }
        lines.join("\n")
    }
}

/// What `call_webmcp_tool` takes.
#[derive(Debug, Clone, PartialEq)]
pub struct CallWebmcpToolRequest {
    /// The tool's name, validated before anything is sent.
    pub name: WebmcpToolName,
    /// `inputObject`, which the draft defines as an object.
    pub input: Value,
    /// How long to wait for the tool.
    pub timeout_ms: u64,
    /// Override the character budget on the tool's return value.
    pub budget_chars: Option<usize>,
}

impl CallWebmcpToolRequest {
    /// A call to `name` with `input`.
    pub fn new(name: &str, input: Value) -> Result<Self, BridgeError> {
        let request = Self {
            name: WebmcpToolName::parse(name)?,
            input: Value::Object(serde_json::Map::new()),
            timeout_ms: DEFAULT_CALL_TIMEOUT_MS,
            budget_chars: None,
        };
        request.with_input(input)
    }

    /// A call to `name` with no input.
    pub fn bare(name: &str) -> Result<Self, BridgeError> {
        Self::new(name, Value::Object(serde_json::Map::new()))
    }

    /// With `input` as the `inputObject`.
    ///
    /// The draft's `inputObject` is an object, so anything else is refused here
    /// with the tool's own name in the message: a scalar or an array reaches
    /// no tool, and saying so before the round trip beats a page-side
    /// `TypeError` after it.
    pub fn with_input(mut self, input: Value) -> Result<Self, BridgeError> {
        if !input.is_object() {
            return Err(BridgeError::policy(format!(
                "{} takes an object as its inputObject, and {} is not one; call \
                 list_webmcp_tools to see the tool's required inputs",
                self.name,
                kind_of(&input)
            )));
        }
        self.input = input;
        Ok(self)
    }

    /// With a timeout for this call.
    pub fn with_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    /// With an explicit budget on the return value.
    pub fn with_budget_chars(mut self, budget: usize) -> Self {
        self.budget_chars = Some(budget);
        self
    }

    /// The budget in force on the tool's return value.
    pub fn budget(&self) -> usize {
        self.budget_chars.unwrap_or(DEFAULT_BUDGET_CHARS)
    }
}

fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// What `call_webmcp_tool` answers.
#[derive(Debug, Clone, PartialEq)]
pub struct WebmcpCallOutcome {
    /// Whether the page exposes `document.modelContext`.
    pub availability: Availability,
    /// The tool the call named.
    pub name: WebmcpToolName,
    /// One line: what happened.
    pub message: String,
    /// What the call changed in the page (D504 rule 3). Meaningful only when
    /// `executed` is true.
    pub change: ChangeSummary,
    /// The tool's return value, cut at the budget.
    pub result: Option<String>,
    /// Whether the return value was cut at the budget.
    pub truncated: bool,
    /// Whether the tool actually ran.
    pub executed: bool,
    /// The names the page does have, when the tool is not registered.
    pub available: Vec<String>,
    /// The character budget the return value was cut at.
    pub budget_chars: usize,
}

impl WebmcpCallOutcome {
    /// An answer for a page that exposes no WebMCP surface.
    pub fn absent(request: &CallWebmcpToolRequest, availability: WebmcpAbsent) -> Self {
        Self {
            availability: Availability::Absent(availability.clone()),
            name: request.name.clone(),
            message: format!("{} was not called: {}", request.name, availability.note()),
            change: ChangeSummary::default(),
            result: None,
            truncated: false,
            executed: false,
            available: Vec::new(),
            budget_chars: request.budget(),
        }
    }

    /// The whole answer as one string.
    ///
    /// An action that ran answers in one line plus its change summary, then the
    /// tool's own words. An action that did **not** run answers in one line
    /// only: a change summary beside a call that never happened would be a lie
    /// about the page.
    pub fn render(&self) -> String {
        let head = if self.executed {
            format!("{} ({})", self.message, self.change.render())
        } else {
            self.message.clone()
        };
        let Some(result) = &self.result else {
            return head;
        };
        let mut out = format!("{head}\n\n{result}");
        if self.truncated {
            out.push_str(&format!(
                "\n… cut at {} characters: narrow the tool's input, or use extract to read the \
                 page",
                self.budget_chars
            ));
        }
        out
    }
}

/// What a driver or a CDP session is asked about a page's WebMCP surface.
///
/// This is the seam the local provider uses: the crate builds the script and
/// parses the answer, and the host only runs the string. A driver that has not
/// been taught to evaluate scripts therefore needs no knowledge of WebMCP at
/// all — it says so, and the tool degrades.
#[derive(Debug, Clone, PartialEq)]
pub enum WebmcpRequest {
    /// Enumerate the page's tools.
    List {
        /// Keep only the tools whose name contains this.
        filter: Option<String>,
    },
    /// Execute one tool.
    Call {
        /// The tool's name.
        name: WebmcpToolName,
        /// The `inputObject`.
        input: Value,
        /// How long the page should wait for the tool.
        timeout_ms: u64,
    },
}

impl WebmcpRequest {
    /// The listing request for `request`.
    pub fn listing(request: &ListWebmcpToolsRequest) -> Self {
        WebmcpRequest::List {
            filter: request.filter.clone(),
        }
    }

    /// The call request for `request`.
    pub fn call(request: &CallWebmcpToolRequest) -> Self {
        WebmcpRequest::Call {
            name: request.name.clone(),
            input: request.input.clone(),
            timeout_ms: request.timeout_ms,
        }
    }

    /// The expression to evaluate in the page.
    pub fn script(&self) -> String {
        match self {
            WebmcpRequest::List { filter } => script::listing(filter.as_deref()),
            WebmcpRequest::Call {
                name,
                input,
                timeout_ms,
            } => script::call(name, input, *timeout_ms),
        }
    }

    /// How long the page should be given for this request. A listing has no
    /// single tool to wait for, but it is `getTools()` in both cases and that
    /// is page code either way, so it gets the same bound.
    pub fn timeout_ms(&self) -> u64 {
        match self {
            WebmcpRequest::List { .. } => DEFAULT_CALL_TIMEOUT_MS,
            WebmcpRequest::Call { timeout_ms, .. } => *timeout_ms,
        }
    }

    /// A description for a log line: the name, never the input.
    ///
    /// The `inputObject` may carry a credential — an agent typing a password
    /// into a tool is the ordinary case, not the exotic one — so the shape of
    /// the request is logged and its bytes are counted, exactly as a `fill`
    /// does.
    pub fn summary(&self) -> String {
        match self {
            WebmcpRequest::List { .. } => "list_webmcp_tools".to_string(),
            WebmcpRequest::Call { name, input, .. } => format!(
                "call_webmcp_tool {name} ({} bytes of input)",
                input_bytes(input)
            ),
        }
    }
}

fn input_bytes(input: &Value) -> usize {
    serde_json::to_string(input).map_or(0, |json| json.len())
}

/// An answer that says the page has no WebMCP surface.
pub fn envelope_absent(reason: &str) -> Value {
    serde_json::json!({
        "loams": "webmcp/1",
        "supported": false,
        "reason": reason,
    })
}

/// An answer that lists tools.
pub fn envelope_tools(tools: &[Value]) -> Value {
    serde_json::json!({
        "loams": "webmcp/1",
        "supported": true,
        "tools": tools,
    })
}

/// An answer that a tool returned `text`.
pub fn envelope_call_ok(text: &str) -> Value {
    serde_json::json!({ "loams": "webmcp/1", "state": "ok", "text": text })
}

/// An answer that the page has no such tool, naming the ones it does have.
pub fn envelope_call_unknown(names: &[String]) -> Value {
    serde_json::json!({ "loams": "webmcp/1", "state": "unknown-tool", "tools": names })
}

/// An answer that the tool did not answer in time.
pub fn envelope_call_timeout() -> Value {
    serde_json::json!({ "loams": "webmcp/1", "state": "timeout" })
}

/// An answer that the tool refused.
pub fn envelope_call_error(reason: &str) -> Value {
    serde_json::json!({ "loams": "webmcp/1", "state": "error", "reason": reason })
}

fn malformed(what: &str) -> BridgeError {
    BridgeError::unavailable(format!(
        "the page's WebMCP answer is not the shape this bridge expects: {what} is missing; this \
         page may have replaced the bridge's script"
    ))
}

/// Whether the bridge's own script threw in the page.
///
/// A thrown script is the one failure the page's answer cannot describe, so it
/// is read from the protocol's own `exceptionDetails` rather than from the
/// envelope: Windows drops a thrown exception's message entirely
/// (design §37 §18.14.3), and a silent empty list is exactly the failure mode
/// D568 rules out.
pub fn thrown_in_page(raw: &Value) -> Result<(), BridgeError> {
    let Some(details) = raw.get("exceptionDetails") else {
        return Ok(());
    };
    let text = details
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("the page threw");
    Err(BridgeError::unavailable(format!(
        "the bridge's WebMCP script threw in the page ({text}); this page may have replaced it"
    )))
}

/// Read the `supported: false` envelope, if that is what came back.
///
/// Both scripts answer this way when the page has no `document.modelContext`,
/// so a call that finds none and a call the policy refuses are the same kind of
/// absence as a listing — which is what keeps `call_webmcp_tool` from claiming
/// a tool ran when the API was never there.
fn absent_answer(raw: &Value) -> Option<WebmcpAbsent> {
    if raw.get("supported").and_then(Value::as_bool) != Some(false) {
        return None;
    }
    Some(WebmcpAbsent::from_reason(
        raw.get("reason")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    ))
}

/// Turn a page's answer to a listing request into a listing.
pub fn parse_listing(
    raw: &Value,
    request: &ListWebmcpToolsRequest,
) -> Result<WebmcpListing, BridgeError> {
    thrown_in_page(raw)?;
    if let Some(reason) = absent_answer(raw) {
        return Ok(WebmcpListing::absent(reason, request));
    }
    if raw.get("supported").and_then(Value::as_bool) != Some(true) {
        return Err(malformed("supported"));
    }
    let entries = raw
        .get("tools")
        .and_then(Value::as_array)
        .ok_or_else(|| malformed("tools"))?;
    let total = entries.len();
    let mut malformed_entries = 0usize;
    let mut tools: Vec<WebmcpTool> = Vec::new();
    for entry in entries.iter().take(MAX_TOOLS_PARSED) {
        match WebmcpTool::parse(entry) {
            None => malformed_entries += 1,
            Some(tool) => {
                if matches_filter(request.filter.as_deref(), tool.name.as_str()) {
                    tools.push(tool);
                }
            }
        }
    }
    Ok(WebmcpListing {
        availability: Availability::Available,
        tools,
        total,
        malformed: malformed_entries,
        filter: request.filter.clone(),
        budget_chars: request.budget(),
    })
}

/// Turn a page's answer to a call request into an outcome.
pub fn parse_call(
    raw: &Value,
    request: &CallWebmcpToolRequest,
    change: ChangeSummary,
) -> Result<WebmcpCallOutcome, BridgeError> {
    thrown_in_page(raw)?;
    if let Some(reason) = absent_answer(raw) {
        return Ok(WebmcpCallOutcome::absent(request, reason));
    }
    let base = |message: String, executed: bool| WebmcpCallOutcome {
        availability: Availability::Available,
        name: request.name.clone(),
        message,
        change: ChangeSummary::default(),
        result: None,
        truncated: false,
        executed,
        available: Vec::new(),
        budget_chars: request.budget(),
    };
    match raw.get("state").and_then(Value::as_str) {
        Some("ok") => {
            let text = raw.get("text").and_then(Value::as_str).unwrap_or_default();
            let budget = request.budget();
            let truncated = text.chars().count() > budget;
            let kept: String = text.chars().take(budget).collect();
            let mut outcome = base(format!("called {} via webmcp", request.name), true);
            outcome.change = change;
            outcome.result = Some(kept);
            outcome.truncated = truncated;
            Ok(outcome)
        }
        Some("unknown-tool") => {
            let names: Vec<String> = raw
                .get("tools")
                .and_then(Value::as_array)
                .map(|names| {
                    names
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .take(MAX_TOOLS_PARSED)
                        .collect()
                })
                .unwrap_or_default();
            let have = if names.is_empty() {
                "the page has none".to_string()
            } else {
                names.join(", ")
            };
            let mut outcome = base(
                format!(
                    "{} is not registered on this page (it has: {have}); call list_webmcp_tools \
                     again",
                    request.name
                ),
                false,
            );
            outcome.available = names;
            Ok(outcome)
        }
        Some("timeout") => Ok(base(
            format!(
                "{} did not answer within {} ms; it may still be running, so take_snapshot before \
                 calling again",
                request.name, request.timeout_ms
            ),
            false,
        )),
        Some("error") => {
            let reason = raw
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("the page gave no reason");
            // The API being out of reach is not a tool that failed, so it is
            // read out of whatever envelope it arrived in and answered as the
            // typed absence it is. Only a refusal the bridge cannot name keeps
            // the page's own words.
            let absent = WebmcpAbsent::from_reason(reason);
            if absent.is_named() {
                return Ok(WebmcpCallOutcome::absent(request, absent));
            }
            Ok(base(
                format!(
                    "{} failed: {}; take_snapshot to see the page",
                    request.name,
                    crate::tool::escape(&crate::tool::truncate(reason, MAX_NAME_CHARS))
                ),
                false,
            ))
        }
        _ => Err(malformed("state")),
    }
}

/// Whether a tool's name matches a filter, case-insensitively.
fn matches_filter(filter: Option<&str>, name: &str) -> bool {
    match filter {
        None => true,
        // An empty filter is no filter: `filter=` is how a caller asks for
        // everything, not how a caller asks for nothing.
        Some("") => true,
        Some(needle) => name.to_lowercase().contains(&needle.to_lowercase()),
    }
}

/// Turn a driver's answer into a listing, degrading an incapable driver to an
/// honest answer rather than an error.
pub fn listing_from_evaluation(
    result: Result<Value, BridgeError>,
    request: &ListWebmcpToolsRequest,
) -> Result<WebmcpListing, BridgeError> {
    match result {
        Err(BridgeError::Unsupported { .. }) => Ok(WebmcpListing::absent(
            WebmcpAbsent::DriverCannotEvaluate,
            request,
        )),
        Err(error) => Err(error),
        Ok(raw) => parse_listing(&raw, request),
    }
}

/// Turn a driver's answer into an outcome, degrading an incapable driver to an
/// honest answer rather than an error.
pub fn call_from_evaluation(
    result: Result<Value, BridgeError>,
    request: &CallWebmcpToolRequest,
    change: ChangeSummary,
) -> Result<WebmcpCallOutcome, BridgeError> {
    match result {
        Err(BridgeError::Unsupported { .. }) => Ok(WebmcpCallOutcome::absent(
            request,
            WebmcpAbsent::DriverCannotEvaluate,
        )),
        Err(error) => Err(error),
        Ok(raw) => parse_call(&raw, request, change),
    }
}

#[cfg(test)]
mod tests;
