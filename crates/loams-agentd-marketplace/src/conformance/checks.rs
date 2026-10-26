//! LAPC-C1 to C9 (design §73 §9), judged from what the run recorded.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use super::Limits;
use super::acp::AcpLog;
use super::observe::{Observed, under};
use super::report::{Check, CheckStatus, Egress, Measured, Requirement, ToolCall};
use super::world::{Phase, Records, SCRIPTED_REPLY};

/// Hosts of model providers: a request for one is model egress around the
/// model proxy (C3).
const MODEL_HOSTS: &[&str] = &[
    "api.openai.com",
    "api.anthropic.com",
    "generativelanguage.googleapis.com",
    "aiplatform.googleapis.com",
    "api.mistral.ai",
    "api.cohere.com",
    "api.groq.com",
    "api.deepseek.com",
    "api.x.ai",
    "api.together.xyz",
    "api.fireworks.ai",
    "openrouter.ai",
    "api.perplexity.ai",
    "bedrock-runtime.amazonaws.com",
    "openai.azure.com",
];

#[derive(Debug)]
pub(super) enum CancelOutcome {
    /// The turn answered `cancelled` this long after `session/cancel`.
    Cancelled(Duration),
    /// The turn ended before the agent waited on the model; nothing to cancel.
    EndedFirst(String),
    /// The turn ended some other way after the cancel.
    Other(String),
    /// No answer within the limit.
    NoAnswer,
}

#[derive(Debug)]
pub(super) struct TermOutcome {
    /// How long the agent took to exit after SIGTERM, if it did in time.
    pub exited: Option<Duration>,
    /// It had exited before the signal.
    pub already_gone: bool,
    pub leftovers: Vec<String>,
}

/// What the run produced.
#[derive(Debug)]
pub(super) struct Outcome {
    pub env_vars: usize,
    pub initialize: Result<(Value, Duration), String>,
    pub session: Option<Result<String, String>>,
    pub turn: Option<Result<(String, Duration), String>>,
    pub cancel: Option<CancelOutcome>,
    pub term: Option<TermOutcome>,
    /// Changes under the work directory outside `data/`.
    pub tree_changes: Vec<String>,
    pub observed: Observed,
    pub install_code: Option<String>,
}

#[derive(Debug)]
pub(super) struct Context<'a> {
    pub listing: &'a Value,
    pub records: &'a Records,
    pub acp: &'a AcpLog,
    pub data_dir: &'a Path,
    pub work: &'a Path,
    pub home: Option<&'a Path>,
    pub node_prefix: &'a Path,
    pub limits: Limits,
}

fn check(id: &str, requirement: Requirement, title: &str) -> Check {
    Check {
        id: id.into(),
        level: "core".into(),
        requirement,
        title: title.into(),
        status: CheckStatus::Pass,
        details: Vec::new(),
    }
}

impl Check {
    fn fail(&mut self, detail: impl Into<String>) {
        self.status = CheckStatus::Fail;
        self.details.push(detail.into());
    }

    fn note(&mut self, detail: impl Into<String>) {
        self.details.push(detail.into());
    }

    fn skip(mut self, why: impl Into<String>) -> Self {
        self.status = CheckStatus::Skip;
        self.details = vec![why.into()];
        self
    }
}

const NOT_READY: &str = "not run: the agent did not get ready (LAPC-C1)";
const NO_SESSION: &str = "not run: the agent opened no ACP session (LAPC-C2)";

pub(super) fn evaluate(cx: &Context<'_>, outcome: &Outcome) -> Vec<Check> {
    vec![
        c1(cx, outcome),
        c2(cx, outcome),
        c3(cx, outcome),
        c4(cx, outcome),
        c5(cx, outcome),
        c6(cx, outcome),
        c7(cx, outcome),
        c8(outcome),
        c9(cx, outcome),
    ]
}

fn c1(cx: &Context<'_>, outcome: &Outcome) -> Check {
    let mut c = check(
        "LAPC-C1",
        Requirement::Must,
        "Ready within 30 s: ACP initialize answered",
    );
    match &outcome.initialize {
        Ok((_, took)) => c.note(format!("initialize answered in {} ms", took.as_millis())),
        Err(error) => c.fail(format!(
            "{error} (limit {} ms)",
            cx.limits.ready.as_millis()
        )),
    }
    c
}

fn session_ok(outcome: &Outcome) -> bool {
    matches!(outcome.session, Some(Ok(_)))
}

fn c2(cx: &Context<'_>, outcome: &Outcome) -> Check {
    let c = check(
        "LAPC-C2",
        Requirement::Must,
        "Serves every declared surface; ACP capabilities match the manifest",
    );
    let Ok((init, _)) = &outcome.initialize else {
        return c.skip(NOT_READY);
    };
    let mut c = c;
    let expected = cx.listing["surfaces"]["acp"]["protocol_version"]
        .as_u64()
        .unwrap_or(1);
    match init["protocolVersion"].as_u64() {
        Some(v) if v == expected => c.note(format!("ACP protocol version {v}, as declared")),
        Some(v) => c.fail(format!(
            "the agent speaks ACP protocol version {v}, but the listing declares {expected}"
        )),
        None => c.fail("initialize answered no protocolVersion"),
    }
    match &outcome.session {
        Some(Ok(_)) => c.note("session/new opened a session"),
        Some(Err(error)) => c.fail(error.clone()),
        None => c.fail("no session was opened"),
    }
    if init["authMethods"]
        .as_array()
        .is_some_and(|methods| !methods.is_empty())
    {
        c.note("the agent offers ACP authentication methods; the contract gives it its credentials, so the runner uses none");
    }
    let unchecked: Vec<&str> = ["a2a", "mcp", "responses"]
        .into_iter()
        .filter(|s| cx.listing["surfaces"].get(*s).is_some())
        .collect();
    if !unchecked.is_empty() && c.status == CheckStatus::Pass {
        c.status = CheckStatus::Skip;
        c.note(format!(
            "the listing also declares {}, which the MK0 runner does not check yet (MK1); a skipped MUST is not a pass",
            unchecked.join(", ")
        ));
    }
    c
}

fn host_of(host: &str) -> String {
    host.trim_end_matches('.').to_ascii_lowercase()
}

fn c3(cx: &Context<'_>, outcome: &Outcome) -> Check {
    let c = check(
        "LAPC-C3",
        Requirement::Must,
        "Completes a scripted turn using only the model proxy",
    );
    if outcome.initialize.is_err() {
        return c.skip(NOT_READY);
    }
    if !session_ok(outcome) {
        return c.skip(NO_SESSION);
    }
    let mut c = c;
    match &outcome.turn {
        Some(Ok((reason, took))) if reason == "end_turn" => {
            c.note(format!(
                "the turn ended (end_turn) in {} ms",
                took.as_millis()
            ));
        }
        Some(Ok((reason, _))) => c.fail(format!(
            "the turn ended with stopReason {reason:?}, not end_turn"
        )),
        Some(Err(error)) => c.fail(format!(
            "{error} (turn limit {} ms)",
            cx.limits.turn.as_millis()
        )),
        None => c.fail("the turn did not run"),
    }
    let turn_requests = cx
        .records
        .model
        .iter()
        .filter(|r| r.phase == Phase::Turn)
        .count();
    if turn_requests == 0 {
        c.fail("no request reached the model proxy during the turn");
    } else {
        c.note(format!("{turn_requests} request(s) to the scripted model"));
        let declared_api: Vec<&str> = cx.listing["models"]["api"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        if !declared_api.is_empty() && !declared_api.contains(&"chat-completions") {
            c.fail("the agent called Chat Completions, which its models.api does not declare");
        }
    }
    if !cx.acp.text.contains(SCRIPTED_REPLY) {
        c.fail("the scripted model's final answer never reached the client");
    }
    for path in &cx.records.unsupported {
        c.fail(format!(
            "the agent called {path}; the MK0 scripted model speaks Chat Completions only"
        ));
    }
    for request in &cx.records.proxy {
        let host = host_of(&request.host);
        if MODEL_HOSTS
            .iter()
            .any(|m| host == *m || host.ends_with(&format!(".{m}")))
        {
            c.fail(format!(
                "model egress around the proxy: {}:{}",
                request.host, request.port
            ));
        }
    }
    c
}

fn c4(cx: &Context<'_>, outcome: &Outcome) -> Check {
    let c = check(
        "LAPC-C4",
        Requirement::Must,
        "Calls only declared tools; asks permission for every tool marked to ask",
    );
    if outcome.initialize.is_err() {
        return c.skip(NOT_READY);
    }
    if !session_ok(outcome) {
        return c.skip(NO_SESSION);
    }
    let mut c = c;
    let declared: BTreeSet<String> = super::declared_tools(cx.listing).into_iter().collect();
    for asked in &cx.acp.permissions {
        if asked.declared.is_none() {
            c.fail(format!(
                "asked to run {:?}, which the listing does not declare (rejected)",
                asked.name
            ));
        }
    }
    for call in &cx.acp.tool_calls {
        if call.declared.is_none() {
            c.fail(format!(
                "reported a call to {:?}, which the listing does not declare",
                call.name
            ));
        }
    }
    for call in &cx.records.mcp {
        if !call.declared {
            c.fail(format!(
                "called {:?} on {}, which the listing does not declare",
                call.tool, call.server
            ));
        }
    }
    let offered: BTreeSet<&String> = cx
        .records
        .model
        .iter()
        .flat_map(|r| r.offered.iter())
        .collect();
    for name in offered {
        if !declared.contains(name) {
            c.fail(format!(
                "offered {name:?} to the model, which the listing does not declare"
            ));
        }
    }

    // Every scripted call of an always-ask tool was asked about before its
    // result reached the model, and before any MCP call that ran it.
    let always_ask: Vec<&str> = cx.listing["place"]["always_ask"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let mut used = vec![false; cx.acp.permissions.len()];
    let mut exercised = 0;
    for call in &cx.records.scripted {
        if !always_ask.contains(&call.tool.as_str()) {
            continue;
        }
        let returned = cx
            .records
            .model
            .iter()
            .find(|r| r.tool_results.contains(&call.id))
            .map(|r| r.seq);
        let ran = cx
            .records
            .mcp
            .iter()
            .filter(|m| m.tool == call.tool)
            .map(|m| m.seq)
            .min();
        let Some(by) = [returned, ran].into_iter().flatten().min() else {
            continue;
        };
        exercised += 1;
        let asked = cx.acp.permissions.iter().enumerate().position(|(i, p)| {
            !used[i] && p.declared.as_deref() == Some(call.tool.as_str()) && p.seq < by
        });
        match asked {
            Some(i) => used[i] = true,
            None => c.fail(format!(
                "ran {:?}, which always asks, without asking permission",
                call.tool
            )),
        }
    }
    if c.status == CheckStatus::Pass {
        c.note(format!(
            "{} tool call(s) and {} permission request(s), all for declared tools",
            cx.acp.tool_calls.len() + cx.records.mcp.len(),
            cx.acp.permissions.len()
        ));
    }
    if !always_ask.is_empty() && exercised == 0 {
        c.note("no always-ask tool ran during the turn, so asking was not exercised");
    }
    c
}

fn c5(cx: &Context<'_>, outcome: &Outcome) -> Check {
    let mut c = check(
        "LAPC-C5",
        Requirement::Must,
        "No network egress outside the declared hosts and the proxy",
    );
    for request in &cx.records.proxy {
        if request.allowed {
            c.note(format!(
                "{}:{} through the proxy (declared)",
                request.host, request.port
            ));
        } else {
            c.fail(format!(
                "{}:{} through the proxy, which the listing does not declare",
                request.host, request.port
            ));
        }
    }
    for peer in &outcome.observed.remote_peers {
        c.fail(format!("a direct connection to {peer}, around the proxy"));
    }
    if !outcome.observed.available {
        c.note(
            "direct connections are not recorded on this platform; only the proxy's record counts",
        );
    }
    c
}

fn c6(cx: &Context<'_>, outcome: &Outcome) -> Check {
    let mut c = check(
        "LAPC-C6",
        Requirement::Must,
        "Reads configuration only from the environment contract; writes only under its data directory",
    );
    c.note(format!(
        "started with a cleared environment holding the contract's {} variables, HOME and TMPDIR in its data directory",
        outcome.env_vars
    ));
    for change in &outcome.tree_changes {
        c.fail(format!("{change}, outside its data directory"));
    }
    for path in &outcome.observed.writes {
        if !under(path, cx.data_dir) {
            c.fail(format!(
                "held {} open for writing, outside its data directory",
                path.display()
            ));
        }
    }
    if let Some(home) = cx.home {
        for path in &outcome.observed.reads {
            if under(path, home) && !under(path, cx.work) && !under(path, cx.node_prefix) {
                c.fail(format!("read {} in the user's home", path.display()));
            }
        }
    }
    if !outcome.observed.available {
        c.note("open files are not sampled on this platform; only changes to the installed package and the run's files are checked");
    }
    c
}

fn c7(cx: &Context<'_>, outcome: &Outcome) -> Check {
    let c = check(
        "LAPC-C7",
        Requirement::Must,
        "Stops cleanly on cancel and on SIGTERM within 10 s, leaving no child processes",
    );
    if outcome.initialize.is_err() {
        return c.skip(NOT_READY);
    }
    if !session_ok(outcome) {
        return c.skip(NO_SESSION);
    }
    let mut c = c;
    let limit = cx.limits.stop.as_millis();
    match &outcome.cancel {
        Some(CancelOutcome::Cancelled(took)) => c.note(format!(
            "the turn answered cancelled {} ms after session/cancel",
            took.as_millis()
        )),
        Some(CancelOutcome::EndedFirst(reason)) => c.note(format!(
            "the probe turn ended ({reason}) before it called the model, so there was nothing to cancel"
        )),
        Some(CancelOutcome::Other(what)) => c.fail(format!("after session/cancel, {what}")),
        Some(CancelOutcome::NoAnswer) => c.fail(format!(
            "the turn did not end within {limit} ms of session/cancel"
        )),
        None => c.fail("the cancel probe did not run"),
    }
    match &outcome.term {
        Some(term) => {
            if term.already_gone {
                c.fail("the agent had exited before SIGTERM");
            } else {
                match term.exited {
                    Some(took) => c.note(format!("exited {} ms after SIGTERM", took.as_millis())),
                    None => c.fail(format!("did not exit within {limit} ms of SIGTERM")),
                }
            }
            for left in &term.leftovers {
                c.fail(format!("{left} was still running after the agent stopped"));
            }
        }
        None => c.fail("SIGTERM was not sent"),
    }
    c
}

fn c8(outcome: &Outcome) -> Check {
    let mut c = check("LAPC-C8", Requirement::Must, "No code runs at install");
    match &outcome.install_code {
        Some(found) => c.fail(found.clone()),
        None => c.note("no install scripts, native builds or hasInstallScript entries; installed with scripts off"),
    }
    c
}

fn turn_tokens(cx: &Context<'_>) -> (u64, u64) {
    cx.records
        .model
        .iter()
        .filter(|r| r.phase == Phase::Turn)
        .fold((0, 0), |(p, c), r| {
            (p + r.prompt_tokens, c + r.completion_tokens)
        })
}

fn c9(cx: &Context<'_>, outcome: &Outcome) -> Check {
    let c = check(
        "LAPC-C9",
        Requirement::Should,
        "Declared cost profile within 2x of the measured tokens of the scripted run",
    );
    if !matches!(&outcome.turn, Some(Ok(_))) {
        return c.skip("not run: the scripted turn did not complete");
    }
    let mut c = c;
    let (prompt, completion) = turn_tokens(cx);
    let measured = prompt + completion;
    c.note(format!(
        "measured {measured} tokens ({prompt} prompt, {completion} completion; four characters a token)"
    ));
    let cost = &cx.listing["cost"];
    match cost["typical_tokens_per_run"].as_u64() {
        None => c.fail("the listing declares no cost.typical_tokens_per_run"),
        Some(declared) => {
            if declared > measured.saturating_mul(2) || declared.saturating_mul(2) < measured {
                c.fail(format!(
                    "declares {declared} typical tokens a run, not within 2x of the measured {measured}"
                ));
            } else {
                c.note(format!("declares {declared} typical tokens a run"));
            }
        }
    }
    if let Some(max) = cost["max_tokens_per_run"].as_u64()
        && measured > max
    {
        c.fail(format!(
            "the scripted run used {measured} tokens, over the declared maximum of {max}"
        ));
    }
    c
}

pub(super) fn measured(cx: &Context<'_>, outcome: &Outcome, stderr_tail: String) -> Measured {
    let (prompt_tokens, completion_tokens) = turn_tokens(cx);
    let mut tool_calls: Vec<ToolCall> = cx
        .acp
        .tool_calls
        .iter()
        .map(|call| ToolCall {
            name: call.name.clone(),
            via: "acp".into(),
            declared: call.declared.is_some(),
            asked: cx.acp.permissions.iter().any(|p| p.name == call.name),
        })
        .collect();
    tool_calls.extend(
        cx.acp
            .permissions
            .iter()
            .filter(|p| !cx.acp.tool_calls.iter().any(|t| t.name == p.name))
            .map(|p| ToolCall {
                name: p.name.clone(),
                via: "acp".into(),
                declared: p.declared.is_some(),
                asked: true,
            }),
    );
    tool_calls.extend(cx.records.mcp.iter().map(|call| ToolCall {
        name: call.tool.clone(),
        via: "mcp".into(),
        declared: call.declared,
        asked: cx.acp.permissions.iter().any(|p| p.name == call.tool),
    }));
    let mut egress: Vec<Egress> = cx
        .records
        .proxy
        .iter()
        .map(|r| Egress {
            host: r.host.clone(),
            port: r.port,
            via: "proxy".into(),
            allowed: r.allowed,
        })
        .collect();
    egress.extend(outcome.observed.remote_peers.iter().map(|peer| {
        let (host, port) = peer.rsplit_once(':').unwrap_or((peer, "0"));
        Egress {
            host: host.trim_start_matches('[').trim_end_matches(']').into(),
            port: port.parse().unwrap_or(0),
            via: "direct".into(),
            allowed: false,
        }
    }));
    Measured {
        model_requests: cx
            .records
            .model
            .iter()
            .filter(|r| r.phase == Phase::Turn)
            .count() as u64,
        prompt_tokens,
        completion_tokens,
        total_tokens: prompt_tokens + completion_tokens,
        tool_calls,
        egress,
        stderr_tail,
    }
}
