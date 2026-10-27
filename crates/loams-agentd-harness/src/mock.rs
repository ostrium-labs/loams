//! Mock harness for engine/UI tests: replays a scripted event sequence.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;

use loams_agentd_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SteeringMode, ToolCall,
    ToolDiff, UserInputQuestion,
};

use crate::{Harness, HarnessError, RunControls};

#[derive(Debug)]
pub struct MockHarness {
    pub script: Vec<AgentEvent>,
}

/// The edit knobs (SD1 Task 8), so Studio's tests get real diffs and
/// approvals from a deterministic run.
///
/// - `LOAMS_DESKTOP_MOCK_EDIT=<path>[,<path>…]`: before the run's `Done`, edit
///   each path (relative to the run's cwd) in order: create it, or append a
///   line, `mock edit <n>`, where `n` is one more than the `mock edit` lines
///   the file already holds (so turn 1 writes `mock edit 1`, the next turn
///   `mock edit 2`). Each edit is an `editFile` tool call and its result,
///   with the whole-file diff. Absolute paths, `..`, and paths that resolve
///   outside the cwd through a symlink are refused with an error result.
/// - `LOAMS_DESKTOP_MOCK_APPROVE=1`: before each edit, ask for approval and
///   wait. A denial, or a dropped bridge, leaves the file untouched and makes
///   the result an error.
///
/// The approval rides the approval contract (SD1 Task 13; ruling T8-2 had it
/// on `request_input` until then): in a supervised thread each edit is an
/// [`crate::ApprovalAsk`] for the tool `editFile`, in the order call, ask,
/// write, result. A full-access thread has no approval bridge, and the edits
/// are written without asking, as every driver answers its own permissions
/// there. Title runs never edit.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MockEdits {
    pub paths: Vec<String>,
    pub approve: bool,
}

impl MockEdits {
    /// Read the knobs from the process environment.
    pub fn from_env() -> Self {
        Self::parse(
            std::env::var("LOAMS_DESKTOP_MOCK_EDIT").ok().as_deref(),
            std::env::var("LOAMS_DESKTOP_MOCK_APPROVE").ok().as_deref(),
        )
    }

    /// Parse the two knob values: a comma-separated path list (blank entries
    /// dropped) and a flag that is on unless empty or `0`.
    pub fn parse(edit: Option<&str>, approve: Option<&str>) -> Self {
        Self {
            paths: edit
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(str::to_owned)
                .collect(),
            approve: approve.is_some_and(|v| !v.is_empty() && v != "0"),
        }
    }
}

/// One planned edit, fixed when the stream reaches it: the id, the line and
/// the file's contents before the edit.
struct EditStep {
    id: String,
    path: String,
    line: String,
    /// The file to write, or why the path is refused.
    target: Result<PathBuf, String>,
    old: Option<String>,
}

const MOCK_EDIT_PREFIX: &str = "mock edit ";

fn plan_edit(cwd: &Path, index: usize, path: &str) -> EditStep {
    let relative = Path::new(path);
    let inside = relative
        .components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
        && relative
            .components()
            .any(|c| matches!(c, Component::Normal(_)));
    let refused = |reason: String| EditStep {
        id: format!("mock-edit-refused-{index}"),
        path: path.to_owned(),
        line: String::new(),
        target: Err(reason),
        old: None,
    };
    if !inside {
        return refused(format!(
            "refused: {path} is not a path inside the run's cwd"
        ));
    }
    let target = cwd.join(relative);
    let old = match std::fs::read_to_string(&target) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return refused(format!("refused: cannot read {path}: {e}")),
    };
    let turn = old.as_deref().map_or(0, |text| {
        text.lines()
            .filter(|l| l.starts_with(MOCK_EDIT_PREFIX))
            .count()
    }) + 1;
    EditStep {
        id: format!("mock-edit-t{turn}-{index}"),
        path: path.to_owned(),
        line: format!("{MOCK_EDIT_PREFIX}{turn}\n"),
        target: Ok(target),
        old,
    }
}

impl EditStep {
    fn call(&self) -> AgentEvent {
        AgentEvent::ToolCall {
            id: self.id.clone(),
            call: ToolCall::EditFile {
                path: self.path.clone(),
                old_string: None,
                new_string: (!self.line.is_empty()).then(|| self.line.clone()),
            },
        }
    }

    fn failed(&self, message: String) -> AgentEvent {
        AgentEvent::ToolResult {
            id: self.id.clone(),
            is_error: true,
            output: Some(message),
            diff: None,
        }
    }

    async fn finish(
        self,
        cwd: PathBuf,
        approve: bool,
        request_approval: Option<Arc<crate::RequestApproval>>,
    ) -> AgentEvent {
        let target = match &self.target {
            Ok(target) => target.clone(),
            Err(reason) => return self.failed(reason.clone()),
        };
        if approve && let Some(request_approval) = request_approval {
            let args = serde_json::json!({ "path": self.path, "content": self.line });
            let answer = request_approval(crate::ApprovalAsk {
                call_id: self.id.clone(),
                tool: "editFile".into(),
                risk: crate::approvals::tool_risk("editFile", &args),
                args,
            });
            if !crate::approval_allows(answer).await {
                return self.failed(format!("edit of {} denied by the user", self.path));
            }
        }
        let mut new = self.old.clone().unwrap_or_default();
        if !new.is_empty() && !new.ends_with('\n') {
            new.push('\n');
        }
        new.push_str(&self.line);
        if let Err(message) = write_inside(&cwd, &target, &new) {
            return self.failed(format!("cannot write {}: {message}", self.path));
        }
        AgentEvent::ToolResult {
            id: self.id.clone(),
            is_error: false,
            output: None,
            diff: Some(ToolDiff {
                path: self.path,
                old_text: self.old,
                new_text: new,
            }),
        }
    }
}

/// Write `content` to `target`, refusing a target that is a symlink or whose
/// directory resolves outside `cwd`.
fn write_inside(cwd: &Path, target: &Path, content: &str) -> Result<(), String> {
    let parent = target.parent().ok_or("no parent directory")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let root = cwd.canonicalize().map_err(|e| e.to_string())?;
    let dir = parent.canonicalize().map_err(|e| e.to_string())?;
    if !dir.starts_with(&root) {
        return Err("it resolves outside the run's cwd".into());
    }
    if std::fs::symlink_metadata(target).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err("it is a symlink".into());
    }
    std::fs::write(target, content).map_err(|e| e.to_string())
}

/// Splice the edits in before the script's `Done` (or at the end when the
/// script has none). Each edit is planned only when the stream reaches it, so
/// its tool call is out before the approval wait and the write.
fn with_edits(
    mut events: Vec<Result<AgentEvent, HarnessError>>,
    cwd: &str,
    edits: MockEdits,
    request_approval: Option<crate::RequestApproval>,
) -> BoxStream<'static, Result<AgentEvent, HarnessError>> {
    if edits.paths.is_empty() {
        return futures::stream::iter(events).boxed();
    }
    let at = events
        .iter()
        .position(|e| matches!(e, Ok(AgentEvent::Done { .. })))
        .unwrap_or(events.len());
    let tail = events.split_off(at);
    let cwd = PathBuf::from(cwd);
    let request_approval = request_approval.map(Arc::new);
    let approve = edits.approve;
    let edit_events = futures::stream::iter(edits.paths.into_iter().enumerate()).flat_map(
        move |(index, path)| {
            let step = plan_edit(&cwd, index, &path);
            let call = step.call();
            let finish = step.finish(cwd.clone(), approve, request_approval.clone());
            futures::stream::iter([Ok(call)])
                .chain(futures::stream::once(async move { Ok(finish.await) }))
        },
    );
    futures::stream::iter(events)
        .chain(edit_events)
        .chain(futures::stream::iter(tail))
        .boxed()
}

/// The scripted question set for the `LOAMS_DESKTOP_MOCK_QUESTION` variant (exercises
/// the QuestionPanel end-to-end: single-select page, multi-select page).
fn question_script() -> Vec<UserInputQuestion> {
    vec![
        UserInputQuestion {
            id: "q-sync".into(),
            header: "Question".into(),
            question: "Which sync strategy should the rewrite use?".into(),
            options: vec![
                "Poll the doc host every 120ms".into(),
                "Event-driven fold with coalesced commits".into(),
                "Hybrid: event-driven with a polling fallback".into(),
            ],
            prefill: None,
            multiline: false,
            multi_select: false,
        },
        UserInputQuestion {
            id: "q-gates".into(),
            header: "Question".into(),
            question: "Which suites should gate the merge?".into(),
            options: vec![
                "Unit tests".into(),
                "End-to-end (two-device)".into(),
                "Golden screenshots".into(),
            ],
            prefill: None,
            multiline: false,
            multi_select: true,
        },
    ]
}

#[async_trait]
impl Harness for MockHarness {
    fn supports_approvals(&self) -> bool {
        true
    }

    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Mock"
    }
    fn supports_steering(&self) -> bool {
        true
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::StepBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[ReasoningLevel::Medium]
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![
            Model {
                id: "mock-1".into(),
                label: "Mock 1".into(),
                description: None,
                reasoning_levels: vec![ReasoningLevel::Medium],
                options: vec![],
            },
            // Claude-mirroring demo model: lets scripted runs carry the same
            // chip labels ("Fable 5 · High") as a real Claude session.
            Model {
                id: "mock-fable-5".into(),
                label: "Fable 5".into(),
                description: None,
                reasoning_levels: vec![
                    ReasoningLevel::Low,
                    ReasoningLevel::Medium,
                    ReasoningLevel::High,
                    ReasoningLevel::XHigh,
                ],
                options: vec![],
            },
        ])
    }
    async fn run_title(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        // A title run never edits files.
        self.run_with(request, controls, MockEdits::default())
    }

    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        self.run_with(request, controls, MockEdits::from_env())
    }
}

impl MockHarness {
    /// [`Harness::run`] with the edit knobs given explicitly instead of read
    /// from the environment.
    pub fn run_with(
        &self,
        request: RunRequest,
        controls: RunControls,
        edits: MockEdits,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        // Optional pacing knob for demos/manual testing: `LOAMS_DESKTOP_MOCK_DELAY_MS`
        // spaces the scripted events out so live-run UI states (working
        // indicator, streaming fade, trailing tool-group auto-open) are
        // observable. Unset (the default, and in tests) streams instantly.
        let delay_ms = std::env::var("LOAMS_DESKTOP_MOCK_DELAY_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        let delay = std::time::Duration::from_millis(delay_ms);

        // Dev/testing knob: `LOAMS_DESKTOP_MOCK_QUESTION=1` swaps in a run that asks
        // the user questions mid-stream via `controls.request_input` (the
        // engine mints the request id, emits `InputRequested`, and resolves it
        // from the `RespondInput` doc command) — the only data-side way to put
        // the QuestionPanel on screen.
        let question_mode = std::env::var("LOAMS_DESKTOP_MOCK_QUESTION")
            .ok()
            .is_some_and(|v| !v.is_empty() && v != "0");
        if question_mode {
            let request_input = controls.request_input;
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();
            tokio::spawn(async move {
                let pause = if delay_ms == 0 {
                    std::time::Duration::from_millis(50)
                } else {
                    delay
                };
                tokio::time::sleep(pause).await;
                let _ = tx.send(AgentEvent::TextDelta {
                    text:
                        "Before I wire the reconciliation path I need two decisions from you.\n\n"
                            .into(),
                });
                tokio::time::sleep(pause).await;
                let answers = request_input(question_script()).await.unwrap_or_default();
                let picked: Vec<String> = answers
                    .iter()
                    .flat_map(|a| a.labels.iter().cloned())
                    .collect();
                tokio::time::sleep(pause).await;
                let _ = tx.send(AgentEvent::TextDelta {
                    text: format!(
                        "Locked in: **{}**. Proceeding with the plan.",
                        if picked.is_empty() {
                            "your defaults".to_string()
                        } else {
                            picked.join("**, **")
                        }
                    ),
                });
                let _ = tx.send(AgentEvent::Done {
                    status: DoneStatus::Completed,
                    result: None,
                    error: None,
                    session_id: None,
                });
            });
            let stream = futures::stream::unfold(rx, |mut rx| async move {
                rx.recv().await.map(|event| (Ok(event), rx))
            });
            return Ok(stream.boxed());
        }

        // Dev/testing knob: `LOAMS_DESKTOP_MOCK_REPEAT=N` loops the script body N times
        // before the final Done — long single-reply streams for frame-cost /
        // smoothness measurement (the terminal `Done` is emitted exactly once,
        // at the very end).
        let repeat = std::env::var("LOAMS_DESKTOP_MOCK_REPEAT")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(1)
            .max(1);
        // Dev/testing knob: `LOAMS_DESKTOP_MOCK_ERROR=1` appends a scripted error
        // before the terminal Done — the only data-side way to put the
        // transcript ErrorChip on screen with the mock harness.
        let mock_error = std::env::var("LOAMS_DESKTOP_MOCK_ERROR")
            .ok()
            .is_some_and(|v| !v.is_empty() && v != "0");
        // Dev/testing knob: `LOAMS_DESKTOP_MOCK_TABLE=1` appends scripted GFM tables
        // before the terminal Done — a plain 3-column grid plus a wide/uneven
        // one (long prose cell beside short cells, mixed alignment) for
        // table-styling checks against the reference app.
        let mock_table = std::env::var("LOAMS_DESKTOP_MOCK_TABLE")
            .ok()
            .is_some_and(|v| !v.is_empty() && v != "0");
        let done_ix = self
            .script
            .iter()
            .position(|e| matches!(e, AgentEvent::Done { .. }))
            .unwrap_or(self.script.len());
        let (body, tail) = self.script.split_at(done_ix);
        let error_event = mock_error.then(|| AgentEvent::Error {
            message: "Claude usage limit reached — try again after the limit resets.".into(),
        });
        // Dev/testing knob: `LOAMS_DESKTOP_MOCK_CODE=1` appends rust + ts code blocks
        // (keywords, strings, numbers, comments) plus inline code — for
        // syntax-palette and inline-code styling checks against the reference.
        let mock_code = std::env::var("LOAMS_DESKTOP_MOCK_CODE")
            .ok()
            .is_some_and(|v| !v.is_empty() && v != "0");
        let code_event = mock_code.then(|| AgentEvent::TextDelta {
            text: concat!(
                "\n### Code check\n\n",
                "The `fold_event_into_parts` helper feeds `writer.sync` on a `120ms` cadence:\n\n",
                "```rust\n",
                "// Fold one event into the accumulated parts.\n",
                "pub fn fold(mut acc: Vec<Part>, event: &AgentEvent) -> Vec<Part> {\n",
                "    let label = \"delta\";\n",
                "    if acc.len() > 128 {\n",
                "        acc.truncate(64); // keep the tail hot\n",
                "    }\n",
                "    acc\n",
                "}\n",
                "```\n\n",
                "```ts\n",
                "// Subscribe and fold on the client.\n",
                "const room = await connect(\"wss://mesh.local\", { retries: 3 });\n",
                "export function fold(parts: Part[], event: AgentEvent): Part[] {\n",
                "    return event.kind === \"delta\" ? [...parts, event] : parts;\n",
                "}\n",
                "```\n\n",
            )
            .into(),
        });
        let table_event = mock_table.then(|| AgentEvent::TextDelta {
            text: "\n### Table check\n\n\
                | Column A | Column B | Column C |\n\
                |---|---|---|\n\
                | a1 | b1 | c1 |\n\
                | a2 | b2 | c2 |\n\n\
                And a wide, uneven one:\n\n\
                | Stage | What happens | p95 |\n\
                |:--|:--|--:|\n\
                | Fold | Events fold into parts and diff into the Loro doc on a 120ms coalesced commit cadence, keeping the oplog RLE-merged across devices | 4.2ms |\n\
                | Sync | Session-room fan-out | 18ms |\n\n"
                .into(),
        });
        // Dev/testing knob: `LOAMS_DESKTOP_MOCK_MEND=1` appends a link/list-heavy
        // passage — bold-led list items, inline links, emphasis, strikethrough
        // — the shapes whose half-streamed markers the display mend
        // (crates/ui markdown/mend.rs) must hold steady while streaming.
        let mock_mend = std::env::var("LOAMS_DESKTOP_MOCK_MEND")
            .ok()
            .is_some_and(|v| !v.is_empty() && v != "0");
        let mend_event = mock_mend.then(|| AgentEvent::TextDelta {
            text: concat!(
                "\n### Streaming mend check\n\n",
                "Inline styles hold while text arrives: **bold stays bold**, ",
                "*italic stays italic*, `code stays code`, and ~~this stays struck~~.\n\n",
                "- **Fold** — parts diff into the [Loro doc](https://loro.dev) on a 120ms cadence\n",
                "- **Relay** — commits fan out through the [session room](https://developers.cloudflare.com/durable-objects/) to every device\n",
                "- **Paint** — the [display tree](https://github.com/pulldown-cmark/pulldown-cmark) mends hanging markers in the last block only\n\n",
                "Links above never flash their URLs, and closing markers never reflow the paragraph.\n",
            )
            .into(),
        });
        // Dev/testing knob: `LOAMS_DESKTOP_MOCK_THINKING=1` appends a markdown-heavy
        // reasoning stream plus a command — the thought-process chip inside a
        // tool-group accordion, for checking that thinking renders styled
        // (bold/lists/inline code) instead of literal `**` markers.
        let mock_thinking = std::env::var("LOAMS_DESKTOP_MOCK_THINKING")
            .ok()
            .is_some_and(|v| !v.is_empty() && v != "0");
        let thinking_events = mock_thinking
            .then(|| {
                [
                    AgentEvent::ReasoningDelta {
                        text: concat!(
                            "**Planning message rollback helper extraction**\n\n",
                            "The `discard_message` helper needs to roll back optimistic sends on error. ",
                            "Three constraints stand out:\n\n",
                            "1. **Rollback** — remove the user message only when the server never persisted it\n",
                            "2. **Credit state** — clear the exhaustion flag per workspace on auth changes\n",
                            "3. **Paywall copy** — resolve icon conflicts, *then* update the tests\n\n",
                            "Checking call sites before patching:\n\n",
                            "```rust\n",
                            "let rolled_back = ledger.discard(message_id)?;\n",
                            "```\n\n",
                            "The [ledger docs](https://example.com) say discard is idempotent, so ",
                            "re-running the rollback on a retry is safe.",
                        )
                        .into(),
                    },
                    AgentEvent::ToolCall {
                        id: "mock-think-tool".into(),
                        call: loams_agentd_proto::ToolCall::Exec {
                            command: "rg -n walletInsufficient apps/word/src | wc -l".into(),
                        },
                    },
                    AgentEvent::ToolResult {
                        id: "mock-think-tool".into(),
                        is_error: false,
                        output: None,
                        diff: None,
                    },
                ]
            })
            .into_iter()
            .flatten();
        // Dev/testing knob: `LOAMS_DESKTOP_MOCK_SUBAGENT=1` appends two spawn chips
        // whose nested traffic arrives as tagged `AgentEvent::Subagent`
        // events — the only data-side way to put spawn chips (running → done)
        // AND their openable subagent docs on screen with the mock harness.
        // The second subagent finishes after a beat of nested activity, so a
        // paced run (`LOAMS_DESKTOP_MOCK_DELAY_MS`) holds a Running chip long enough
        // to observe.
        let mock_subagent = std::env::var("LOAMS_DESKTOP_MOCK_SUBAGENT")
            .ok()
            .is_some_and(|v| !v.is_empty() && v != "0");
        let subagent_events = mock_subagent
            .then(|| {
                let tag = |parent: &str, event: AgentEvent| AgentEvent::Subagent {
                    parent_tool_use_id: parent.into(),
                    event: Box::new(event),
                };
                let spawn = |id: &str, description: &str, prompt: &str| AgentEvent::ToolCall {
                    id: id.into(),
                    // The claude-driver spawn shape: `Agent: {description}`
                    // with the task in the input (names the chip AND the tab).
                    call: loams_agentd_proto::ToolCall::Unknown {
                        name: format!("Agent: {description}"),
                        input: Some(serde_json::json!({
                            "description": description,
                            "prompt": prompt,
                        })),
                    },
                };
                let resolve = |id: &str| AgentEvent::ToolResult {
                    id: id.into(),
                    is_error: false,
                    output: None,
                    diff: None,
                };
                let done = AgentEvent::Done {
                    status: DoneStatus::Completed,
                    result: None,
                    error: None,
                    session_id: None,
                };
                vec![
                    AgentEvent::TextDelta {
                        text: "\n### Subagent check\n\nFanning out two scouts before the fold rewrite.\n\n".into(),
                    },
                    spawn(
                        "mock-sub-1",
                        "Audit the fold path",
                        "Read crates/doc and list every call site of fold_event_into_parts, checking each holds the byte cap.",
                    ),
                    spawn(
                        "mock-sub-2",
                        "Verify the commit cadence",
                        "Measure the 120ms coalesced commit cadence under a scripted delta burst.",
                    ),
                    // The spawn prompts seed each subagent's opening user
                    // entry (like the claude driver's Task-prompt seeding).
                    tag(
                        "mock-sub-1",
                        AgentEvent::UserMessage {
                            text: "Read crates/doc and list every call site of fold_event_into_parts, checking each holds the byte cap.".into(),
                        },
                    ),
                    tag(
                        "mock-sub-2",
                        AgentEvent::UserMessage {
                            text: "Measure the 120ms coalesced commit cadence under a scripted delta burst.".into(),
                        },
                    ),
                    tag(
                        "mock-sub-1",
                        AgentEvent::TextDelta {
                            text: "Scanning `crates/doc` for fold call sites.\n\n".into(),
                        },
                    ),
                    tag(
                        "mock-sub-1",
                        AgentEvent::ToolCall {
                            id: "sub1-grep".into(),
                            call: loams_agentd_proto::ToolCall::Exec {
                                command: "grep -rn fold_event_into_parts crates".into(),
                            },
                        },
                    ),
                    tag("mock-sub-1", resolve("sub1-grep")),
                    tag(
                        "mock-sub-1",
                        AgentEvent::TextDelta {
                            text: "Three call sites: the live fold, the rebuild, and the subagent sink — every one applies the byte cap before persisting.".into(),
                        },
                    ),
                    resolve("mock-sub-1"),
                    tag("mock-sub-1", done.clone()),
                    // A steer AFTER the subagent settled (claude's queued
                    // SendMessage shape): resurrects the chip and reopens
                    // the frozen transcript for the resumed segment.
                    tag(
                        "mock-sub-1",
                        AgentEvent::UserMessage {
                            text: "One more: confirm the rebuild path holds the byte cap too."
                                .into(),
                        },
                    ),
                    tag(
                        "mock-sub-1",
                        AgentEvent::TextDelta {
                            text: "Rebuild path checked — same cap, applied before persisting."
                                .into(),
                        },
                    ),
                    tag("mock-sub-1", done.clone()),
                    tag(
                        "mock-sub-2",
                        AgentEvent::TextDelta {
                            text: "Driving a 2k-delta burst through the writer.\n\n".into(),
                        },
                    ),
                    tag(
                        "mock-sub-2",
                        AgentEvent::ToolCall {
                            id: "sub2-burst".into(),
                            call: loams_agentd_proto::ToolCall::Exec {
                                command: "cargo test -p loams-desktop-doc cadence_burst -- --nocapture".into(),
                            },
                        },
                    ),
                    resolve("mock-sub-2"),
                    tag("mock-sub-2", resolve("sub2-burst")),
                    tag(
                        "mock-sub-2",
                        AgentEvent::TextDelta {
                            text: "Commits land on the 120ms cadence; no commit carried more than one burst.".into(),
                        },
                    ),
                    // A parent→subagent steer: splits the transcript into a
                    // user entry + fresh assistant segment (like the claude
                    // driver's tagged user text blocks).
                    tag(
                        "mock-sub-2",
                        AgentEvent::UserMessage {
                            text: "Also verify the cadence holds while a steer lands mid-burst.".into(),
                        },
                    ),
                    tag(
                        "mock-sub-2",
                        AgentEvent::TextDelta {
                            text: "Re-running with a mid-burst steer injected.\n\n".into(),
                        },
                    ),
                    tag(
                        "mock-sub-2",
                        AgentEvent::ToolCall {
                            id: "sub2-steer-burst".into(),
                            call: loams_agentd_proto::ToolCall::Exec {
                                command: "cargo test -p loams-desktop-doc cadence_steer -- --nocapture"
                                    .into(),
                            },
                        },
                    ),
                    tag("mock-sub-2", resolve("sub2-steer-burst")),
                    tag(
                        "mock-sub-2",
                        AgentEvent::TextDelta {
                            text: "Watching the commit log while the burst drains: ".into(),
                        },
                    ),
                    tag("mock-sub-2", AgentEvent::TextDelta { text: "batch 1 clean, ".into() }),
                    tag("mock-sub-2", AgentEvent::TextDelta { text: "batch 2 clean, ".into() }),
                    tag("mock-sub-2", AgentEvent::TextDelta { text: "batch 3 clean, ".into() }),
                    tag("mock-sub-2", AgentEvent::TextDelta { text: "batch 4 clean, ".into() }),
                    tag("mock-sub-2", AgentEvent::TextDelta { text: "batch 5 clean — ".into() }),
                    tag(
                        "mock-sub-2",
                        AgentEvent::TextDelta {
                            text: "every window under 120ms.\n\nSteer landed between commits; the cadence held.".into(),
                        },
                    ),
                    tag("mock-sub-2", done),
                ]
            })
            .into_iter()
            .flatten();
        // With the code knob, also exercise a MULTILINE Exec command — the
        // round-9 chip breaker shape ("set -e\nfixture_in_original=0"): the
        // Run chip must stay one 30px line.
        let code_tool_events = mock_code
            .then(|| {
                [
                    AgentEvent::ToolCall {
                        id: "mock-code-tool".into(),
                        call: loams_agentd_proto::ToolCall::Exec {
                            command: "set -e\nfixture_in_original=0\ngrep -rn \"veil\" crates/ui/src | wc -l".into(),
                        },
                    },
                    AgentEvent::ToolResult {
                        id: "mock-code-tool".into(),
                        is_error: false,
                        output: None,
                        diff: None,
                    },
                ]
            })
            .into_iter()
            .flatten();
        let events: Vec<Result<AgentEvent, HarnessError>> = body
            .iter()
            .cycle()
            .take(body.len() * repeat)
            .cloned()
            .chain(thinking_events)
            .chain(code_tool_events)
            .chain(subagent_events)
            .chain(code_event)
            .chain(table_event)
            .chain(mend_event)
            .chain(error_event)
            .chain(tail.iter().cloned())
            .map(Ok)
            .collect();
        // Dev/testing knob: `LOAMS_DESKTOP_MOCK_CHARS=N` re-chunks every TextDelta
        // into N-char deltas, so `LOAMS_DESKTOP_MOCK_DELAY_MS` paces *characters*
        // instead of whole scripted blocks — delta boundaries then land inside
        // inline markers and links, which is the streaming shape real
        // harnesses produce and the display mend exists for.
        let chunk_chars = std::env::var("LOAMS_DESKTOP_MOCK_CHARS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|&n| n > 0);
        let events: Vec<Result<AgentEvent, HarnessError>> = match chunk_chars {
            None => events,
            Some(n) => events
                .into_iter()
                .flat_map(|event| match event {
                    Ok(AgentEvent::TextDelta { text }) => {
                        let chars: Vec<char> = text.chars().collect();
                        chars
                            .chunks(n)
                            .map(|c| {
                                Ok(AgentEvent::TextDelta {
                                    text: c.iter().collect(),
                                })
                            })
                            .collect::<Vec<_>>()
                    }
                    // Reasoning streams the same char-paced way — thought
                    // chips have their own streaming display (mend + live
                    // tail), which whole-block deltas would never exercise.
                    Ok(AgentEvent::ReasoningDelta { text }) => {
                        let chars: Vec<char> = text.chars().collect();
                        chars
                            .chunks(n)
                            .map(|c| {
                                Ok(AgentEvent::ReasoningDelta {
                                    text: c.iter().collect(),
                                })
                            })
                            .collect::<Vec<_>>()
                    }
                    other => vec![other],
                })
                .collect(),
        };
        // Dev/testing knob: `LOAMS_DESKTOP_MOCK_SUBAGENT_DELAY_MS` paces TAGGED
        // (subagent) events on their own clock — the parent turn settles at
        // `LOAMS_DESKTOP_MOCK_DELAY_MS` speed while the background subagents stream
        // on slowly, which is exactly the eager-done shape live tabs are
        // observed under (and the only way a rig click can reliably land
        // inside a subagent's streaming window).
        let sub_delay_ms = std::env::var("LOAMS_DESKTOP_MOCK_SUBAGENT_DELAY_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok());
        let events = with_edits(events, &request.cwd, edits, controls.request_approval);
        if delay_ms == 0 && sub_delay_ms.is_none() {
            return Ok(events);
        }
        Ok(events
            .then(move |event| async move {
                let pause = match (&event, sub_delay_ms) {
                    (Ok(AgentEvent::Subagent { .. }), Some(ms)) => {
                        std::time::Duration::from_millis(ms)
                    }
                    _ => delay,
                };
                tokio::time::sleep(pause).await;
                event
            })
            .boxed())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod edit_tests {
    use std::sync::{Arc, Mutex};

    use futures::StreamExt;
    use loams_agentd_proto::approvals::{ApprovalDecision, ToolRisk};
    use loams_agentd_proto::{
        AgentEvent, DoneStatus, RunRequest, SandboxLevel, ToolCall, ToolDiff, UserInputQuestion,
    };
    use tokio::sync::{mpsc, oneshot};

    use super::{MockEdits, MockHarness};
    use crate::{CancellationToken, RunControls};

    fn request(cwd: &std::path::Path) -> RunRequest {
        RunRequest {
            mcp_servers: Vec::new(),
            mcp: None,
            prompt: "edit".into(),
            harness: None,
            model: None,
            reasoning: None,
            model_options: Default::default(),
            cwd: cwd.to_string_lossy().into_owned(),
            sandbox: SandboxLevel::WorkspaceWrite,
            auto_approve: false,
            attachments: Vec::new(),
            worktree: None,
            resume: None,
        }
    }

    type Asked = Arc<Mutex<Vec<(crate::ApprovalAsk, bool)>>>;

    /// Supervised controls (SD1 Task 13) whose approval bridge records each
    /// ask and whether `probe` existed when it was asked, and answers `Allow`
    /// (once) or anything else (deny). Questions are never expected.
    fn controls(probe: std::path::PathBuf, label: &'static str, asked: Asked) -> RunControls {
        let (_steer_tx, steering) = mpsc::channel(1);
        RunControls {
            request_approval: Some(Box::new(move |ask: crate::ApprovalAsk| {
                let (tx, rx) = oneshot::channel();
                asked.lock().unwrap().push((ask, probe.exists()));
                let _ = tx.send(if label == "Allow" {
                    ApprovalDecision::Once
                } else {
                    ApprovalDecision::Deny
                });
                rx
            })),
            execution_lease: None,
            request_input: Box::new(move |_questions: Vec<UserInputQuestion>| {
                panic!("the Mock's edits never ask questions");
            }),
            steering,
            interrupt: CancellationToken::new(),
        }
    }

    /// Full-access controls: no approval bridge.
    fn full_access() -> RunControls {
        let (_steer_tx, steering) = mpsc::channel(1);
        RunControls {
            request_approval: None,
            execution_lease: None,
            request_input: Box::new(move |_questions: Vec<UserInputQuestion>| {
                panic!("the Mock's edits never ask questions");
            }),
            steering,
            interrupt: CancellationToken::new(),
        }
    }

    fn harness() -> MockHarness {
        MockHarness {
            script: vec![
                AgentEvent::TextDelta {
                    text: "editing".into(),
                },
                AgentEvent::Done {
                    status: DoneStatus::Completed,
                    result: None,
                    error: None,
                    session_id: None,
                },
            ],
        }
    }

    async fn run(
        harness: &MockHarness,
        cwd: &std::path::Path,
        edits: &MockEdits,
        controls: RunControls,
    ) -> Vec<AgentEvent> {
        harness
            .run_with(request(cwd), controls, edits.clone())
            .unwrap()
            .map(Result::unwrap)
            .collect()
            .await
    }

    fn edit_call(id: &str, path: &str, line: &str) -> AgentEvent {
        AgentEvent::ToolCall {
            id: id.into(),
            call: ToolCall::EditFile {
                path: path.into(),
                old_string: None,
                new_string: Some(line.into()),
            },
        }
    }

    fn edit_result(id: &str, path: &str, old: Option<&str>, new: &str) -> AgentEvent {
        AgentEvent::ToolResult {
            id: id.into(),
            is_error: false,
            output: None,
            diff: Some(ToolDiff {
                path: path.into(),
                old_text: old.map(Into::into),
                new_text: new.into(),
            }),
        }
    }

    #[test]
    fn mock_edit_knobs_parse() {
        let edits = MockEdits::parse(Some(" notes.txt, ,src/new.txt "), Some("1"));
        assert_eq!(edits.paths, vec!["notes.txt", "src/new.txt"]);
        assert!(edits.approve);
        let off = MockEdits::parse(None, Some("0"));
        assert!(off.paths.is_empty());
        assert!(!off.approve);
        assert!(!MockEdits::parse(Some(""), None).approve);
    }

    #[tokio::test]
    async fn mock_edit_writes_files_and_emits_tool_calls() {
        let dir = tempfile::tempdir().unwrap();
        // An existing file without a trailing newline is appended to on a new line.
        std::fs::write(dir.path().join("notes.txt"), "kept").unwrap();
        let edits = MockEdits {
            paths: vec!["notes.txt".into(), "src/new.txt".into()],
            approve: false,
        };
        let asked = Asked::default();
        let mock = harness();
        let first = run(
            &mock,
            dir.path(),
            &edits,
            controls(dir.path().join("x"), "Allow", asked.clone()),
        )
        .await;
        assert_eq!(
            first,
            vec![
                AgentEvent::TextDelta {
                    text: "editing".into()
                },
                edit_call("mock-edit-t1-0", "notes.txt", "mock edit 1\n"),
                edit_result(
                    "mock-edit-t1-0",
                    "notes.txt",
                    Some("kept"),
                    "kept\nmock edit 1\n"
                ),
                edit_call("mock-edit-t1-1", "src/new.txt", "mock edit 1\n"),
                edit_result("mock-edit-t1-1", "src/new.txt", None, "mock edit 1\n"),
                AgentEvent::Done {
                    status: DoneStatus::Completed,
                    result: None,
                    error: None,
                    session_id: None,
                },
            ]
        );
        assert!(
            asked.lock().unwrap().is_empty(),
            "no approval without the knob"
        );

        // The next turn appends its own line: the turn number comes from the file.
        let second = run(
            &mock,
            dir.path(),
            &edits,
            controls(dir.path().join("x"), "Allow", asked),
        )
        .await;
        assert_eq!(
            std::fs::read_to_string(dir.path().join("notes.txt")).unwrap(),
            "kept\nmock edit 1\nmock edit 2\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("src/new.txt")).unwrap(),
            "mock edit 1\nmock edit 2\n"
        );
        assert_eq!(
            second[1],
            edit_call("mock-edit-t2-0", "notes.txt", "mock edit 2\n")
        );

        // Determinism: the same inputs give the same events.
        let again = tempfile::tempdir().unwrap();
        std::fs::write(again.path().join("notes.txt"), "kept").unwrap();
        let replay = run(
            &mock,
            again.path(),
            &edits,
            controls(again.path().join("x"), "Allow", Asked::default()),
        )
        .await;
        assert_eq!(replay, first);
    }

    #[tokio::test]
    async fn mock_edit_refuses_paths_outside_the_cwd() {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().join("repo");
        std::fs::create_dir_all(&cwd).unwrap();
        let outside = root.path().join("escape.txt");
        let edits = MockEdits {
            paths: vec![
                "../escape.txt".into(),
                outside.to_string_lossy().into_owned(),
            ],
            approve: true,
        };
        let asked = Asked::default();
        let events = run(
            &harness(),
            &cwd,
            &edits,
            controls(outside.clone(), "Allow", asked.clone()),
        )
        .await;
        assert!(!outside.exists(), "nothing written outside the cwd");
        assert!(
            asked.lock().unwrap().is_empty(),
            "a refused path asks nothing"
        );
        let errors = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::ToolResult { is_error: true, .. }))
            .count();
        assert_eq!(errors, 2);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn mock_edit_refuses_a_symlink_out_of_the_cwd() {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().join("repo");
        std::fs::create_dir_all(&cwd).unwrap();
        std::os::unix::fs::symlink(root.path(), cwd.join("link")).unwrap();
        let edits = MockEdits {
            paths: vec!["link/escape.txt".into()],
            approve: false,
        };
        let events = run(
            &harness(),
            &cwd,
            &edits,
            controls(cwd.join("x"), "Allow", Asked::default()),
        )
        .await;
        assert!(!root.path().join("escape.txt").exists());
        assert!(matches!(
            &events[2],
            AgentEvent::ToolResult { is_error: true, output: Some(o), .. } if o.contains("outside")
        ));
    }

    #[tokio::test]
    async fn mock_approve_requests_before_edit() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("notes.txt");
        let edits = MockEdits {
            paths: vec!["notes.txt".into()],
            approve: true,
        };
        let mock = harness();

        // Allowed: one approval per edit, asked while the file is still absent.
        let asked = Asked::default();
        let events = run(
            &mock,
            dir.path(),
            &edits,
            controls(target.clone(), "Allow", asked.clone()),
        )
        .await;
        {
            let asked = asked.lock().unwrap();
            assert_eq!(asked.len(), 1);
            let (ask, existed) = &asked[0];
            assert!(!existed, "approval requested before the write");
            assert_eq!(ask.call_id, "mock-edit-t1-0");
            assert_eq!(ask.tool, "editFile");
            assert_eq!(ask.args["path"], "notes.txt");
            assert_eq!(ask.risk, ToolRisk::Write);
        }
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "mock edit 1\n");
        // The order stays: the call, the ask, the write, the result.
        assert_eq!(
            events[1],
            edit_call("mock-edit-t1-0", "notes.txt", "mock edit 1\n")
        );
        assert_eq!(
            events[2],
            edit_result("mock-edit-t1-0", "notes.txt", None, "mock edit 1\n")
        );

        // Denied: the file is untouched and the result is an error.
        let asked = Asked::default();
        let events = run(
            &mock,
            dir.path(),
            &edits,
            controls(target.clone(), "Deny", asked.clone()),
        )
        .await;
        assert_eq!(asked.lock().unwrap().len(), 1);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "mock edit 1\n");
        assert!(matches!(
            &events[2],
            AgentEvent::ToolResult { is_error: true, output: Some(o), diff: None, .. }
                if o.contains("denied")
        ));
        assert!(matches!(events.last(), Some(AgentEvent::Done { .. })));
    }

    /// A full-access thread has no approval bridge: the knob's edits are not
    /// asked about, as every driver answers its own permissions there.
    #[tokio::test]
    async fn mock_approve_in_full_access_writes_without_asking() {
        let dir = tempfile::tempdir().unwrap();
        let edits = MockEdits {
            paths: vec!["notes.txt".into()],
            approve: true,
        };
        let events = run(&harness(), dir.path(), &edits, full_access()).await;
        assert_eq!(
            std::fs::read_to_string(dir.path().join("notes.txt")).unwrap(),
            "mock edit 1\n"
        );
        assert_eq!(
            events[2],
            edit_result("mock-edit-t1-0", "notes.txt", None, "mock edit 1\n")
        );
    }
}
