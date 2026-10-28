//! Typed constructors of the canonical fixtures (design §50 §8.3).
//!
//! One fixture per type the method table names, and one per variant of the
//! tagged unions a client switches on (`AgentEvent`, `ToolCall`,
//! `MessagePart`, `SessionCommandPayload`, `MutateParams`). A field added in
//! Rust fails to compile here until its fixture says what it holds.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, TimeZone, Utc};
use loams_agentd_proto::chat::{
    BeginQueueEditOutcome, FinishQueueEditOutcome, FinishQueuedMessageEditAction, MessagePart,
    MessageRole, MessageStatus, QueueDeliveryGate, QueuedMessage, RenewQueueEditOutcome,
    SessionCommandPayload, SessionMessageEntry, SubagentStatus, ToolDiffStat,
};
use loams_agentd_proto::daemon::DaemonMode;
use loams_agentd_proto::invocation::{Skill, SkillCommand};
use loams_agentd_proto::rpc::{ClientKind, Hello, HelloReply, Role};
use loams_agentd_proto::transcript::{
    TextAppend, TranscriptBaseline, TranscriptFrame, TranscriptUpdate, TranscriptUpsert,
};
use loams_agentd_proto::*;

use super::Fixture;

fn at() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 12, 9, 30, 0).unwrap()
}

const MS: i64 = 1_791_797_400_000;

fn s(text: &str) -> String {
    text.to_owned()
}

fn some(text: &str) -> Option<String> {
    Some(text.to_owned())
}

// ---------------------------------------------------------------------------
// Shared sample values
// ---------------------------------------------------------------------------

fn review_comment() -> ReviewComment {
    ReviewComment {
        id: s("comment-1"),
        checkout_id: s("checkout-1"),
        path: s("src/main.rs"),
        side: DiffSide::New,
        line: 12,
        end_line: Some(14),
        body: s("This loop never ends."),
        diff_checksum: s("sum-1"),
        created_at_ms: MS,
        resolved: false,
        outdated: false,
    }
}

fn chat_config() -> ChatConfig {
    let mut model_options = serde_json::Map::new();
    model_options.insert(s("effort"), serde_json::json!("high"));
    ChatConfig {
        harness: HarnessId::ClaudeCode,
        model: some("claude-sonnet"),
        reasoning: Some(ReasoningLevel::High),
        model_options,
        sandbox: SandboxLevel::WorkspaceWrite,
    }
}

fn chat() -> Chat {
    Chat {
        id: s("chat-1"),
        device_id: s("device-1"),
        title: some("Fix the build"),
        archived: false,
        cwd: some("/home/dev/project"),
        branch: some("main"),
        checkout_id: some("checkout-1"),
        source_context: Some(ConversationSourceContext {
            checkout_id: s("checkout-1"),
            repo_root: s("/home/dev/project"),
            cwd: s("/home/dev/project"),
            branch: s("main"),
            head_sha: some("0123abcd"),
            observed_at: at(),
        }),
        config: Some(chat_config()),
        last_message_preview: some("Done."),
        last_message_at: Some(at()),
        created_at: at(),
        harness_session_id: some("claude-session-1"),
        harness_session_cwd: some("/home/dev/project"),
        space_id: some("space-1"),
        last_seen_at: Some(at()),
        room_gen: Some(2),
        parent_chat_id: None,
    }
}

fn space() -> Space {
    Space {
        id: s("space-1"),
        device_id: s("device-1"),
        path: s("/home/dev/project"),
        name: some("project"),
        git_detected: true,
        git_checked_at: Some(at()),
        checkout_id: some("checkout-1"),
        created_at: at(),
    }
}

fn session() -> Session {
    Session {
        last_completed_turn: some("turn-1"),
        chat_id: s("chat-1"),
        device_id: s("device-1"),
        status: SessionStatus::Working,
        started_at: Some(at()),
        updated_at: at(),
    }
}

fn repo() -> Repo {
    Repo {
        path: s("/home/dev/project"),
        name: s("project"),
        default_branch: some("main"),
    }
}

fn target() -> WorkspaceTarget {
    WorkspaceTarget {
        chat_id: some("chat-1"),
        space_id: None,
        checkout_path: None,
    }
}

fn workspace_entry() -> WorkspaceEntry {
    WorkspaceEntry {
        mutation_revision: some("rev-1"),
        path: s("src/main.rs"),
        name: s("main.rs"),
        kind: WorkspaceEntryKind::File,
        size: Some(1024),
        modified_at: Some(at()),
        ignored: false,
        read_only: false,
    }
}

fn file_change() -> WorkspaceFileChange {
    WorkspaceFileChange {
        operation_id: some("op-1"),
        kind: WorkspaceFileChangeKind::Renamed,
        path: s("src/lib.rs"),
        old_path: some("src/main.rs"),
    }
}

fn terminal() -> TerminalSession {
    TerminalSession {
        id: s("term-1"),
        cwd: s("/home/dev/project"),
        shell: s("/bin/bash"),
    }
}

fn action_run() -> ProjectActionRun {
    ProjectActionRun {
        action_id: s("action-1"),
        action_name: s("Test"),
        terminal: terminal(),
    }
}

fn action_draft() -> ProjectActionDraft {
    ProjectActionDraft {
        name: s("Test"),
        command: s("cargo test"),
        icon: ProjectActionIcon::Test,
        run_on_worktree_create: false,
    }
}

fn actions_snapshot() -> ProjectActionsSnapshot {
    ProjectActionsSnapshot {
        space_id: s("space-1"),
        actions: vec![ProjectAction {
            id: s("action-1"),
            name: s("Test"),
            command: s("cargo test"),
            icon: ProjectActionIcon::Test,
            run_on_worktree_create: true,
        }],
        importable_actions: vec![action_draft()],
        project_file_issue: None,
    }
}

fn checkout_diff() -> CheckoutDiff {
    CheckoutDiff {
        checkout_id: s("checkout-1"),
        device_id: s("device-1"),
        cwd: s("/home/dev/project"),
        patch: s("--- a/src/main.rs\n+++ b/src/main.rs\n"),
        files: vec![DiffFileSummary {
            path: s("src/main.rs"),
            old_path: None,
            status: s("modified"),
            additions: 3,
            deletions: 1,
            binary: false,
        }],
        additions: 3,
        deletions: 1,
        truncated: false,
        checksum: s("sum-1"),
        updated_at: at(),
    }
}

fn history_commit() -> GitHistoryCommit {
    GitHistoryCommit {
        sha: s("0123abcd"),
        parent_shas: vec![s("fedc9876")],
        subject: s("Fix the build"),
        author_name: s("Dev"),
        author_email: s("dev@example.com"),
        authored_at: s("2026-10-12T09:30:00Z"),
        refs: vec![GitHistoryRef {
            kind: GitHistoryRefKind::Branch,
            label: s("main"),
        }],
    }
}

fn agent_accounts() -> AgentAccountsSnapshot {
    AgentAccountsSnapshot {
        accounts: vec![AgentAccount {
            id: s("account-1"),
            harness: HarnessId::Codex,
            email: some("dev@example.com"),
            plan_label: some("Pro"),
            active: true,
            usage_windows: vec![AgentUsageWindow {
                label: s("5h"),
                used_fraction: 0.5,
                resets_at: Some(at()),
            }],
            usage_fetched_at: Some(MS),
            usage_error: None,
            display_name: some("Dev"),
            organization: None,
            auth_kind: Some(AgentAuthKind::Oauth),
            switchable: true,
            saved_at: Some(MS),
            provider: None,
        }],
        warnings: vec![AgentAccountWarning {
            harness: HarnessId::Pi,
            message: s("Pi is not signed in"),
        }],
    }
}

fn harness_update() -> HarnessUpdateStatus {
    HarnessUpdateStatus {
        harness: HarnessId::Codex,
        installed_version: some("0.40.0"),
        latest_version: some("0.41.0"),
        channel: some("stable"),
        source: HarnessInstallSource::Npm,
        phase: HarnessUpdatePhase::Downloading,
        progress: Some(HarnessUpdateProgress {
            completed_bytes: Some(512),
            total_bytes: Some(1024),
            message: some("Downloading"),
        }),
        checked_at: Some(MS),
        error: None,
        can_apply: true,
        manual_command: some("npm install -g @openai/codex"),
    }
}

fn sidebar() -> SidebarPreferencesState {
    SidebarPreferencesState {
        revision: 3,
        synced: false,
        initialized: true,
        pinned_session_ids: vec![s("chat-1")],
        sections: vec![SidebarSection {
            id: s("section-1"),
            name: s("Work"),
            session_ids: vec![s("chat-2")],
            collapsed: false,
        }],
    }
}

fn user_input_question() -> UserInputQuestion {
    UserInputQuestion {
        id: s("q-1"),
        header: s("Branch"),
        question: s("Which branch?"),
        options: vec![s("main"), s("dev")],
        multi_select: false,
        prefill: None,
        multiline: false,
    }
}

fn tool_diff() -> ToolDiff {
    ToolDiff {
        path: s("src/main.rs"),
        old_text: some("fn main() {}\n"),
        new_text: s("fn main() {\n    run();\n}\n"),
    }
}

fn run_request() -> RunRequest {
    RunRequest {
        prompt: s("Fix the build"),
        harness: Some(HarnessId::ClaudeCode),
        model: some("claude-sonnet"),
        reasoning: Some(ReasoningLevel::High),
        model_options: serde_json::Map::new(),
        cwd: s("/home/dev/project"),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: false,
        resume: some("claude-session-1"),
        attachments: vec![s("/home/dev/.uploads/shot.png")],
        worktree: Some(WorktreeSpec {
            repo_path: s("/home/dev/project"),
            base: s("main"),
            space_id: some("space-1"),
        }),
        mcp: Some(McpServer {
            name: s("loams-desktop"),
            command: s("/opt/loams/loams-agentd"),
            args: vec![s("mcp")],
            env: BTreeMap::from([(s("LOAMS_AGENTD_PORT"), s("50217"))]),
        }),
        // MK1 Task 4b: an installed marketplace MCP server.
        mcp_servers: vec![McpServer {
            name: s("marketplace-ostrium-github-mcp-server"),
            command: s(
                "/home/dev/.loams-agentd/agents/ostrium/github-mcp-server/2.0.3/github-mcp-server",
            ),
            args: vec![s("stdio")],
            env: BTreeMap::from([(s("GITHUB_PERSONAL_ACCESS_TOKEN"), s("ghp_example"))]),
        }],
    }
}

// ---------------------------------------------------------------------------
// Tagged unions, one value per variant
// ---------------------------------------------------------------------------

fn tool_calls() -> Vec<(&'static str, ToolCall)> {
    vec![
        (
            "exec",
            ToolCall::Exec {
                command: s("cargo test"),
            },
        ),
        (
            "readFile",
            ToolCall::ReadFile {
                path: s("src/main.rs"),
            },
        ),
        (
            "writeFile",
            ToolCall::WriteFile {
                path: s("src/new.rs"),
                content: some("pub fn new() {}\n"),
            },
        ),
        (
            "editFile",
            ToolCall::EditFile {
                path: s("src/main.rs"),
                old_string: some("{}"),
                new_string: some("{ run(); }"),
            },
        ),
        ("applyPatch", ToolCall::ApplyPatch { path: None }),
        (
            "search",
            ToolCall::Search {
                pattern: s("fn main"),
                path: some("src"),
            },
        ),
        (
            "glob",
            ToolCall::Glob {
                pattern: s("**/*.rs"),
            },
        ),
        (
            "webFetch",
            ToolCall::WebFetch {
                url: s("https://example.com"),
                prompt: some("Summarize"),
            },
        ),
        (
            "webSearch",
            ToolCall::WebSearch {
                query: s("loro crdt"),
            },
        ),
        (
            "todo",
            ToolCall::Todo {
                items: vec![TodoItem {
                    text: s("Write tests"),
                    done: true,
                }],
            },
        ),
        (
            "mcp",
            ToolCall::Mcp {
                server: s("loams-desktop"),
                tool: s("list_chats"),
                input: Some(serde_json::json!({ "limit": 10 })),
            },
        ),
        (
            "unknown",
            ToolCall::Unknown {
                name: s("Agent: review"),
                input: Some(serde_json::json!({ "model": "claude-haiku" })),
            },
        ),
    ]
}

fn message_parts() -> Vec<(&'static str, MessagePart)> {
    vec![
        (
            "text",
            MessagePart::Text {
                id: s("t0"),
                text: s("Building now."),
            },
        ),
        (
            "image",
            MessagePart::Image {
                id: s("img-1"),
                path: s("/home/dev/.uploads/shot.png"),
                name: s("shot.png"),
                mime_type: s("image/png"),
            },
        ),
        (
            "reasoning",
            MessagePart::Reasoning {
                id: s("r1"),
                text: s("The linker flag is missing."),
            },
        ),
        (
            "tool",
            MessagePart::Tool {
                id: s("call-1"),
                call: ToolCall::Unknown {
                    name: s("Agent: review"),
                    input: None,
                },
                is_error: false,
                resolved: true,
                output: some("ok"),
                diff: Some(tool_diff()),
                output_ref: some("chat-1/call-1"),
                output_bytes: Some(2048),
                diff_ref: some("chat-1/call-1.diff"),
                diff_stats: Some(vec![ToolDiffStat {
                    path: s("src/main.rs"),
                    additions: 2,
                    deletions: 1,
                }]),
                subagent_ref: some("chat-1:sub-1"),
                subagent_status: Some(SubagentStatus::Running),
                subagent_tail: some("Reading files"),
            },
        ),
        (
            "input",
            MessagePart::Input {
                id: s("in-req-1"),
                request_id: s("req-1"),
                questions: vec![user_input_question()],
                resolved: false,
            },
        ),
        (
            "error",
            MessagePart::Error {
                id: s("e5"),
                message: s("rate limited"),
            },
        ),
        (
            "fork",
            MessagePart::Fork {
                id: s("fork:chat-2"),
                source_chat_id: s("chat-1"),
                source_title: s("Fix the build"),
            },
        ),
    ]
}

fn agent_events() -> Vec<(&'static str, AgentEvent)> {
    vec![
        (
            "sessionStarted",
            AgentEvent::SessionStarted {
                harness: HarnessId::ClaudeCode,
                model: s("claude-sonnet"),
                tools: vec![s("Bash"), s("Read")],
                cwd: s("/home/dev/project"),
                session_id: s("claude-session-1"),
                assistant_message_id: s("msg-1"),
            },
        ),
        (
            "textDelta",
            AgentEvent::TextDelta {
                text: s("Building"),
            },
        ),
        (
            "generatedImage",
            AgentEvent::GeneratedImage {
                id: s("img-1"),
                path: s("/home/dev/.uploads/chart.png"),
                name: s("chart.png"),
                mime_type: s("image/png"),
            },
        ),
        (
            "reasoningDelta",
            AgentEvent::ReasoningDelta {
                text: s("Check the linker."),
            },
        ),
        (
            "assistantMessageCompleted",
            AgentEvent::AssistantMessageCompleted {
                assistant_message_id: s("msg-1"),
            },
        ),
        (
            "toolCall",
            AgentEvent::ToolCall {
                id: s("call-1"),
                call: ToolCall::Exec {
                    command: s("cargo build"),
                },
            },
        ),
        (
            "toolResult",
            AgentEvent::ToolResult {
                id: s("call-1"),
                is_error: false,
                output: some("Finished"),
                diff: Some(tool_diff()),
            },
        ),
        (
            "contextUsage",
            AgentEvent::ContextUsage {
                tokens: Some(12_000),
                window: Some(200_000),
            },
        ),
        (
            "usage",
            AgentEvent::Usage {
                input_tokens: 1200,
                output_tokens: 300,
                cache_tokens: None,
                cost_usd: None,
            },
        ),
        (
            "availableCommands",
            AgentEvent::AvailableCommands {
                commands: vec![SlashCommand {
                    name: s("review"),
                    description: s("Review the diff"),
                    input_hint: some("path"),
                }],
            },
        ),
        (
            "error",
            AgentEvent::Error {
                message: s("rate limited"),
            },
        ),
        (
            "inputRequested",
            AgentEvent::InputRequested {
                request_id: s("req-1"),
                questions: vec![user_input_question()],
            },
        ),
        (
            "inputResolved",
            AgentEvent::InputResolved {
                request_id: s("req-1"),
            },
        ),
        (
            "approvalRequested",
            AgentEvent::ApprovalRequested {
                call_id: s("call-1"),
                tool: s("Bash"),
                args: serde_json::json!({ "command": "cargo test" }),
                risk: loams_agentd_proto::approvals::ToolRisk::Write,
            },
        ),
        (
            "approvalResolved",
            AgentEvent::ApprovalResolved {
                call_id: s("call-1"),
                decision: loams_agentd_proto::approvals::ApprovalDecision::Once,
            },
        ),
        (
            "steered",
            AgentEvent::Steered {
                assistant_message_id: some("msg-1"),
                next_assistant_message_id: some("msg-2"),
            },
        ),
        (
            "done",
            AgentEvent::Done {
                status: DoneStatus::Completed,
                result: some("Fixed."),
                error: None,
                session_id: some("claude-session-1"),
            },
        ),
        (
            "userMessage",
            AgentEvent::UserMessage {
                text: s("Also run the tests"),
            },
        ),
        (
            "subagent",
            AgentEvent::Subagent {
                parent_tool_use_id: s("call-1"),
                event: Box::new(AgentEvent::TextDelta { text: s("Reading") }),
            },
        ),
    ]
}

fn commands() -> Vec<(&'static str, SessionCommandPayload)> {
    vec![
        (
            "run",
            SessionCommandPayload::Run {
                request: run_request(),
                message_id: s("msg-u1"),
            },
        ),
        (
            "steer",
            SessionCommandPayload::Steer {
                prompt: s("Also run the tests"),
                message_id: some("msg-u2"),
            },
        ),
        ("interrupt", SessionCommandPayload::Interrupt {}),
        (
            "respondInput",
            SessionCommandPayload::RespondInput {
                request_id: s("req-1"),
                answers: vec![UserInputAnswer {
                    question_id: s("q-1"),
                    labels: vec![s("main")],
                }],
            },
        ),
    ]
}

fn mutations() -> Vec<(&'static str, MutateParams)> {
    vec![
        (
            "createChat",
            MutateParams::CreateChat {
                chat_id: s("chat-2"),
                space_id: some("space-1"),
                device_id: None,
                config: Some(chat_config()),
                branch: some("main"),
                cwd: None,
                parent_chat_id: some("chat-1"),
            },
        ),
        (
            "createSpace",
            MutateParams::CreateSpace {
                space_id: s("space-2"),
                device_id: s("device-1"),
                path: s("/home/dev/other"),
                name: None,
                git_detected: true,
            },
        ),
        (
            "renameSpace",
            MutateParams::RenameSpace {
                space_id: s("space-1"),
                name: some("Project"),
            },
        ),
        (
            "deleteSpace",
            MutateParams::DeleteSpace {
                space_id: s("space-2"),
            },
        ),
        (
            "renameChat",
            MutateParams::RenameChat {
                chat_id: s("chat-1"),
                title: s("Fix the linker"),
            },
        ),
        (
            "setChatBranch",
            MutateParams::SetChatBranch {
                chat_id: s("chat-1"),
                branch: s("fix-linker"),
            },
        ),
        (
            "setChatCwd",
            MutateParams::SetChatCwd {
                chat_id: s("chat-1"),
                cwd: s("/home/dev/project-wt/fix-linker"),
            },
        ),
        (
            "setChatActivity",
            MutateParams::SetChatActivity {
                chat_id: s("chat-1"),
                last_message_at: Some(MS),
                created_at: Some(MS),
            },
        ),
        (
            "setChatHost",
            MutateParams::SetChatHost {
                chat_id: s("chat-1"),
                device_id: s("device-1"),
            },
        ),
        (
            "setChatArchived",
            MutateParams::SetChatArchived {
                chat_id: s("chat-1"),
                archived: true,
            },
        ),
        (
            "changeSidebarPin",
            MutateParams::ChangeSidebarPin {
                change: SidebarPinChange::Section {
                    change: SidebarSectionChange::Assign {
                        session_id: s("chat-1"),
                        section_id: some("section-1"),
                    },
                },
            },
        ),
        (
            "setChatConfig",
            MutateParams::SetChatConfig {
                chat_id: s("chat-1"),
                config: chat_config(),
            },
        ),
        (
            "deleteChat",
            MutateParams::DeleteChat {
                chat_id: s("chat-2"),
            },
        ),
        (
            "markChatSeen",
            MutateParams::MarkChatSeen {
                chat_id: s("chat-1"),
                at: None,
            },
        ),
    ]
}

fn entries() -> Vec<SessionMessageEntry> {
    vec![
        SessionMessageEntry {
            id: s("msg-u1"),
            role: MessageRole::User,
            parts: vec![MessagePart::Text {
                id: s("t0"),
                text: s("Fix the build"),
            }],
            created_at: MS,
            device_id: s("device-1"),
            status: None,
            continuation_of: None,
            duration_ms: None,
        },
        SessionMessageEntry {
            id: s("msg-1"),
            role: MessageRole::Assistant,
            parts: message_parts().into_iter().map(|(_, part)| part).collect(),
            created_at: MS + 1,
            device_id: s("device-1"),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
            duration_ms: Some(4200),
        },
    ]
}

fn queued() -> QueuedMessage {
    QueuedMessage {
        id: s("queued-1"),
        text: s("Then update the docs"),
        attachments: vec![s("/home/dev/.uploads/shot.png")],
        hold_for_turn_end: true,
        issued_by: s("device-1"),
        issued_at: MS,
        edited_at: Some(MS + 5),
        delivery_gate: Some(QueueDeliveryGate::Editing {
            lease_id: s("lease-1"),
            owner_device_id: s("device-1"),
            owner_instance_id: s("window-1"),
            acquired_at_ms: MS,
            expires_at_ms: MS + 30_000,
            base_text_hash: s("hash-1"),
        }),
    }
}

// ---------------------------------------------------------------------------
// Every fixture
// ---------------------------------------------------------------------------

/// Every fixture, in a stable order.
pub fn all() -> Vec<Fixture> {
    let mut out = Vec::new();
    let mut add = |fixture: Fixture| out.push(fixture);

    // The handshake and the daemon's own methods.
    add(Fixture::new(
        "Hello",
        Hello {
            client: ClientKind::ElectronMain,
            client_version: s("0.2.1"),
            protocol: 1,
        },
    ));
    add(Fixture::new(
        "HelloReply",
        HelloReply {
            protocol: 1,
            version: s("0.2.1"),
            build_sha: s("abc123"),
            mode: DaemonMode::Service,
            pid: 41234,
            started_at_ms: MS,
            role: Role::Owner,
            features: vec![],
        },
    ));
    add(Fixture::new("Empty", Empty {}));
    add(Fixture::new("OkReply", OkReply::OK));

    // Harnesses.
    add(Fixture::new(
        "HarnessCatalog",
        HarnessCatalog(vec![
            HarnessDescriptor {
                id: HarnessId::Codex,
                name: s("Codex"),
                supports_steering: true,
                steering_mode: SteeringMode::StepBoundary,
                reasoning_levels: vec![ReasoningLevel::Low, ReasoningLevel::High],
                installed: true,
                can_install: false,
                marketplace_listing: None,
                enabled: Some(true),
            },
            // MK1 Task 4b: an installed marketplace ACP harness.
            HarnessDescriptor {
                id: HarnessId::Marketplace,
                name: s("fx"),
                supports_steering: true,
                steering_mode: SteeringMode::TurnBoundary,
                reasoning_levels: vec![],
                installed: true,
                can_install: false,
                marketplace_listing: some("vercel-labs/fx"),
                enabled: Some(true),
            },
        ]),
    ));
    add(Fixture::new(
        "HarnessParams",
        HarnessParams {
            harness: HarnessId::Unsupported,
        },
    ));
    let forgejo = || loams_agentd_proto::factory::FactoryAppInfo {
        id: s("forgejo"),
        label: s("Forgejo"),
        url: some("https://git.example.com"),
        health: loams_agentd_proto::factory::FactoryHealth::Ok,
        has_panels: true,
        credential_fields: vec![
            loams_agentd_proto::factory::FactoryCredentialField {
                key: s("token"),
                label: s("Access token"),
                secret: true,
            },
            loams_agentd_proto::factory::FactoryCredentialField {
                key: s("ssoOrigin"),
                label: s("SSO origin (optional)"),
                secret: false,
            },
        ],
        fields: Some(BTreeMap::from([(
            s("ssoOrigin"),
            s("https://sso.example.com"),
        )])),
        persistent: true,
        preview: None,
    };
    add(Fixture::new(
        "FactoryAppList",
        loams_agentd_proto::factory::FactoryAppList {
            apps: vec![forgejo()],
        },
    ));
    add(Fixture::new(
        "FactoryAppParams",
        loams_agentd_proto::factory::FactoryAppParams { app: s("forgejo") },
    ));
    add(Fixture::new(
        "FactoryAppReply",
        loams_agentd_proto::factory::FactoryAppReply {
            app: Some(forgejo()),
            error: None,
        },
    ));
    add(Fixture::new(
        "FactoryConfigureParams",
        loams_agentd_proto::factory::FactoryConfigureParams {
            app: s("forgejo"),
            url: s("https://git.example.com"),
            fields: BTreeMap::from([(s("token"), s("pat-1"))]),
        },
    ));
    add(Fixture::new(
        "FactoryQueryParams",
        loams_agentd_proto::factory::FactoryQueryParams {
            app: s("forgejo"),
            op: s("repos"),
            params: serde_json::json!({ "limit": 20 }),
        },
    ));
    add(Fixture::new(
        "FactoryQueryReply",
        loams_agentd_proto::factory::FactoryQueryReply {
            value: None,
            error: Some(loams_agentd_proto::factory::FactoryError {
                code: s("auth_failed"),
                message: s("GET /api/v1/repos/search failed with 401"),
            }),
        },
    ));
    add(Fixture::new(
        "TitleSettings",
        TitleSettings {
            harness: Some(HarnessId::ClaudeCode),
            model: some("claude-haiku"),
        },
    ));
    add(Fixture::new(
        "SetHarnessEnabledParams",
        SetHarnessEnabledParams {
            harness: HarnessId::Pi,
            enabled: false,
        },
    ));
    add(Fixture::new(
        "ListModelsParams",
        ListModelsParams {
            harness: HarnessId::Codex,
            force: true,
        },
    ));
    add(Fixture::new(
        "ModelList",
        ModelList(vec![Model {
            id: s("gpt-5"),
            label: s("GPT-5"),
            description: some("Most capable"),
            reasoning_levels: vec![ReasoningLevel::Medium],
            options: vec![ModelOption {
                id: s("speed"),
                label: s("Speed"),
                choices: vec![ModelOptionChoice {
                    id: s("fast"),
                    label: s("Fast"),
                }],
                default_choice: s("fast"),
            }],
        }]),
    ));
    add(Fixture::new(
        "HarnessCatalogParams",
        HarnessCatalogParams {
            harness: HarnessId::ClaudeCode,
            chat_id: some("chat-1"),
            space_id: None,
            path: None,
        },
    ));
    add(Fixture::new(
        "SkillList",
        SkillList(Some(vec![Skill {
            name: s("review"),
            path: s("/home/dev/.claude/skills/review/SKILL.md"),
            description: s("Review a diff"),
            enabled: true,
            command: Some(SkillCommand {
                name: s("review"),
                harness: HarnessId::ClaudeCode,
            }),
        }])),
    ));
    add(Fixture::new(
        "SlashCommandList",
        SlashCommandList(vec![SlashCommand {
            name: s("compact"),
            description: s("Compact the context"),
            input_hint: None,
        }]),
    ));
    add(Fixture::new(
        "HarnessUpdateList",
        HarnessUpdateList(vec![harness_update()]),
    ));
    add(Fixture::new("HarnessUpdateStatus", harness_update()));
    add(Fixture::new(
        "HarnessUpdateParams",
        HarnessUpdateParams {
            harness: Some(HarnessId::Codex),
        },
    ));
    add(Fixture::new(
        "DismissHarnessUpdateParams",
        DismissHarnessUpdateParams {
            harness: HarnessId::Codex,
            version: some("0.41.0"),
        },
    ));
    add(Fixture::new(
        "ApplyHarnessUpdateReply",
        ApplyHarnessUpdateReply {
            ok: true,
            version: s("0.41.0"),
        },
    ));
    add(Fixture::new(
        "CancelledReply",
        CancelledReply { cancelled: true },
    ));

    // Commands, the transcript and the queue.
    for (name, command) in commands() {
        add(Fixture::new(
            &format!("SessionCommandPayload.{name}"),
            command,
        ));
    }
    add(Fixture::new(
        "QueueCommandParams",
        QueueCommandParams {
            chat_id: s("chat-1"),
            command: SessionCommandPayload::Interrupt {},
        },
    ));
    add(Fixture::new(
        "QueueCommandReply",
        QueueCommandReply {
            command_id: s("cmd-1"),
        },
    ));
    add(Fixture::new(
        "ChatIdParams",
        ChatIdParams {
            chat_id: s("chat-1"),
        },
    ));
    add(Fixture::new(
        "TakeProjectActionSetupParams",
        TakeProjectActionSetupParams {
            chat_id: s("chat-1"),
            command_id: s("cmd-1"),
        },
    ));
    add(Fixture::new(
        "ProjectActionSetup",
        ProjectActionSetup {
            ready: true,
            setup_action: Some(action_run()),
            setup_error: None,
        },
    ));
    add(Fixture::new(
        "ForkSideChatParams",
        ForkSideChatParams {
            chat_id: s("chat-2"),
            source_chat_id: s("chat-1"),
            parent_chat_id: None,
        },
    ));
    add(Fixture::new("Chat", chat()));
    add(Fixture::new(
        "ListCheckpointsParams",
        loams_agentd_proto::checkpoints::ListCheckpointsParams {
            chat_id: s("chat-1"),
        },
    ));
    add(Fixture::new(
        "CheckpointList",
        loams_agentd_proto::checkpoints::CheckpointList {
            checkpoints: vec![loams_agentd_proto::checkpoints::Checkpoint {
                turn_id: s("msg-1"),
                git_ref: s("refs/loams/checkpoints/chat-1/turn/1/pre"),
                created_at_ms: MS,
                files_changed: 2,
            }],
        },
    ));
    add(Fixture::new(
        "RestoreCheckpointParams",
        loams_agentd_proto::checkpoints::RestoreCheckpointParams {
            chat_id: s("chat-1"),
            turn_id: s("msg-1"),
            expected_checksum: s("sum-1"),
            scope: loams_agentd_proto::checkpoints::CheckpointScope::WorktreeAndTranscript,
        },
    ));
    add(Fixture::new(
        "GetThreadUsageParams",
        loams_agentd_proto::usage::GetThreadUsageParams {
            chat_id: s("chat-1"),
        },
    ));
    add(Fixture::new(
        "ThreadUsage",
        loams_agentd_proto::usage::ThreadUsage::new(vec![
            loams_agentd_proto::usage::TurnUsage {
                turn_id: s("msg-1"),
                input_tokens: Some(1200),
                output_tokens: Some(340),
                cache_tokens: Some(800),
                cost_usd: Some(0.0125),
                duration_ms: 4200,
            },
            loams_agentd_proto::usage::TurnUsage {
                turn_id: s("msg-2"),
                input_tokens: None,
                output_tokens: None,
                cache_tokens: None,
                cost_usd: None,
                duration_ms: 900,
            },
        ]),
    ));
    add(Fixture::new(
        "WatchApprovalsParams",
        loams_agentd_proto::approvals::WatchApprovalsParams {
            chat_id: some("chat-1"),
        },
    ));
    add(Fixture::new(
        "ApprovalList",
        loams_agentd_proto::approvals::ApprovalList {
            pending: vec![loams_agentd_proto::approvals::Approval {
                id: s("call-1"),
                chat_id: s("chat-1"),
                turn_id: some("msg-1"),
                tool: s("Bash"),
                args: serde_json::json!({ "command": "rm -rf build" }),
                risk: loams_agentd_proto::approvals::ToolRisk::Destructive,
                created_at_ms: MS,
            }],
            mode: Some(loams_agentd_proto::approvals::ApprovalMode::Supervised),
        },
    ));
    add(Fixture::new(
        "ResolveApprovalParams",
        loams_agentd_proto::approvals::ResolveApprovalParams {
            call_id: s("call-1"),
            chat_id: some("chat-1"),
            decision: loams_agentd_proto::approvals::ApprovalDecision::Always,
        },
    ));
    add(Fixture::new(
        "SetThreadApprovalModeParams",
        loams_agentd_proto::approvals::SetThreadApprovalModeParams {
            chat_id: s("chat-1"),
            mode: loams_agentd_proto::approvals::ApprovalMode::Supervised,
        },
    ));
    add(Fixture::new(
        "Notification",
        loams_agentd_proto::notify::Notification {
            id: s("notification-1"),
            kind: loams_agentd_proto::notify::NotificationKind::TurnDone,
            chat_id: some("chat-1"),
            group_id: None,
            title: s("Fix the login"),
            created_at_ms: MS,
        },
    ));
    add(Fixture::new(
        "RestoreCheckpointReply",
        loams_agentd_proto::checkpoints::RestoreCheckpointReply {
            restored_to: s("0123456789abcdef0123456789abcdef01234567"),
            forked_chat_id: some("chat-2"),
        },
    ));
    add(Fixture::new(
        "WatchDocMessagesParams",
        WatchDocMessagesParams {
            chat_id: s("chat-1"),
            opening_tail: true,
        },
    ));
    for (name, part) in message_parts() {
        add(Fixture::new(&format!("MessagePart.{name}"), part));
    }
    add(Fixture::new(
        "TranscriptUpdate.reset",
        TranscriptUpdate {
            frame: TranscriptFrame::reset(&entries()),
            context_usage: Some(ContextUsage {
                tokens: Some(12_000),
                window: Some(200_000),
            }),
            replay_baseline: Some(TranscriptBaseline::capture(&entries())),
            history_pending: true,
        },
    ));
    add(Fixture::new(
        "TranscriptUpdate.delta",
        TranscriptUpdate {
            frame: TranscriptFrame::Delta {
                upsert: vec![TranscriptUpsert {
                    after: some("msg-u1"),
                    entry: entries().remove(1),
                }],
                append: vec![TextAppend {
                    entry: s("msg-1"),
                    part: s("t0"),
                    text: s(" Done."),
                    len: 19,
                }],
                remove: vec![s("msg-0")],
                count: 2,
            },
            context_usage: None,
            replay_baseline: None,
            history_pending: false,
        },
    ));
    add(Fixture::new(
        "QueueSnapshot",
        QueueSnapshot {
            items: vec![queued()],
        },
    ));
    add(Fixture::new(
        "QueueMessageParams",
        QueueMessageParams {
            chat_id: s("chat-1"),
            text: s("Then update the docs"),
            attachments: vec![s("/home/dev/.uploads/shot.png")],
            hold_for_turn_end: true,
        },
    ));
    add(Fixture::new("IdReply", IdReply { id: s("queued-1") }));
    add(Fixture::new(
        "UpdateQueuedMessageParams",
        UpdateQueuedMessageParams {
            chat_id: s("chat-1"),
            id: s("queued-1"),
            text: s("Then update the README"),
        },
    ));
    add(Fixture::new("ChangedReply", ChangedReply { changed: true }));
    add(Fixture::new(
        "MoveQueuedMessageParams",
        MoveQueuedMessageParams {
            chat_id: s("chat-1"),
            id: s("queued-1"),
            to_index: 0,
        },
    ));
    add(Fixture::new(
        "QueuedMessageRef",
        QueuedMessageRef {
            chat_id: s("chat-1"),
            id: s("queued-1"),
        },
    ));
    add(Fixture::new("RemovedReply", RemovedReply { removed: true }));
    add(Fixture::new("SentReply", SentReply { sent: false }));
    add(Fixture::new(
        "BeginQueuedMessageEditParams",
        BeginQueuedMessageEditParams {
            chat_id: s("chat-1"),
            id: s("queued-1"),
            editor_device_id: s("device-1"),
            editor_instance_id: s("window-1"),
        },
    ));
    add(Fixture::new(
        "BeginQueueEditOutcome.acquired",
        BeginQueueEditOutcome::Acquired {
            lease_id: s("lease-1"),
            text: s("Then update the docs"),
            attachments: vec![],
            base_text_hash: s("hash-1"),
            expires_at_ms: MS + 30_000,
        },
    ));
    add(Fixture::new(
        "BeginQueueEditOutcome.locked",
        BeginQueueEditOutcome::Locked {
            owner_device_id: s("device-2"),
            expires_at_ms: MS + 30_000,
        },
    ));
    add(Fixture::new(
        "RenewQueuedMessageEditParams",
        RenewQueuedMessageEditParams {
            chat_id: s("chat-1"),
            id: s("queued-1"),
            lease_id: s("lease-1"),
        },
    ));
    add(Fixture::new(
        "RenewQueueEditOutcome",
        RenewQueueEditOutcome::Renewed {
            expires_at_ms: MS + 60_000,
        },
    ));
    add(Fixture::new(
        "FinishQueuedMessageEditParams",
        FinishQueuedMessageEditParams {
            chat_id: s("chat-1"),
            id: s("queued-1"),
            lease_id: s("lease-1"),
            action: FinishQueuedMessageEditAction::Commit,
            text: some("Then update the README"),
            expected_text_hash: some("hash-1"),
            attachments: Some(vec![]),
        },
    ));
    add(Fixture::new(
        "FinishQueueEditOutcome",
        FinishQueueEditOutcome::Conflict {
            current_text: s("Then update the docs"),
        },
    ));

    // The workspace doc.
    add(Fixture::new("ChatList", ChatList(vec![chat()])));
    add(Fixture::new("SpaceList", SpaceList(vec![space()])));
    add(Fixture::new("SessionList", SessionList(vec![session()])));
    add(Fixture::new("SidebarPreferencesState", sidebar()));
    for (name, mutation) in mutations() {
        add(Fixture::new(&format!("MutateParams.{name}"), mutation));
    }
    add(Fixture::new(
        "MutateReply",
        MutateReply {
            ok: true,
            sidebar_preferences: Some(sidebar()),
        },
    ));
    add(Fixture::new(
        "LocalDeviceReply",
        LocalDeviceReply {
            device_id: s("device-1"),
        },
    ));
    add(Fixture::new(
        "EngineInfo",
        EngineInfo {
            device_id: s("device-1"),
            workspace_scope: WorkspaceScope::Local,
            capabilities: capabilities::current(),
        },
    ));
    add(Fixture::new(
        "EngineReadyReply",
        EngineReadyReply { ready: true },
    ));
    add(Fixture::new(
        "WatchPreviewsParams",
        WatchPreviewsParams {
            chat_id: s("chat-1"),
        },
    ));
    add(Fixture::new(
        "PreviewSnapshot",
        PreviewSnapshot {
            services: vec![PreviewService {
                id: s("preview-1"),
                project_id: s("space-1"),
                project_name: s("project"),
                project_cwd: s("/home/dev/project"),
                device_id: s("device-1"),
                device_name: s("workstation"),
                hostname: s("web.project.localhost"),
                name: s("web"),
                port: 5173,
                pid: 4242,
                cwd: s("/home/dev/project/web"),
                started_at: 1_791_797_400_000,
                loams_desktop_owned: false,
            }],
            proxy_port: PREVIEW_PROXY_PORT,
            error: None,
            project_name: some("project"),
            remote: false,
        },
    ));

    // Repositories and checkouts.
    add(Fixture::new("RepoList", RepoList(vec![repo()])));
    add(Fixture::new("Repo", repo()));
    add(Fixture::new(
        "AddRepoParams",
        AddRepoParams {
            path: s("/home/dev/project"),
        },
    ));
    add(Fixture::new(
        "CloneRepoParams",
        CloneRepoParams {
            url: s("https://github.com/example/project.git"),
        },
    ));
    add(Fixture::new(
        "CreateRepoParams",
        CreateRepoParams { name: s("project") },
    ));
    add(Fixture::new(
        "RepoPathParams",
        RepoPathParams {
            repo_path: s("/home/dev/project"),
        },
    ));
    add(Fixture::new(
        "BranchList",
        BranchList(vec![s("main"), s("dev")]),
    ));
    add(Fixture::new(
        "RepoRefList",
        RepoRefList(vec![RepoRef {
            name: s("main"),
            current: true,
            worktree_path: some("/home/dev/project"),
        }]),
    ));
    add(Fixture::new(
        "ListGitHistoryParams",
        ListGitHistoryParams {
            cwd: s("/home/dev/project"),
            cursor: 0,
            limit: GIT_HISTORY_DEFAULT_LIMIT,
        },
    ));
    add(Fixture::new(
        "SearchGitHistoryParams",
        SearchGitHistoryParams {
            cwd: s("/home/dev/project"),
            query: s("build"),
            cursor: 0,
            limit: 50,
        },
    ));
    add(Fixture::new(
        "GitHistoryPage",
        GitHistoryPage {
            commits: vec![history_commit()],
            branch_tips: vec![],
            head_sha: some("0123abcd"),
            next_cursor: Some(100),
            total_count: Some(240),
            head_commit_count: Some(240),
            comparison: Some(GitHistoryComparison {
                base: s("origin/main"),
                ahead: 1,
                behind: 0,
            }),
        },
    ));
    add(Fixture::new(
        "ResolveGitAvatarsParams",
        ResolveGitAvatarsParams {
            cwd: s("/home/dev/project"),
            authors: vec![GitAvatarAuthor {
                sha: s("0123abcd"),
                email: s("dev@example.com"),
            }],
            cursor: 0,
            limit: GIT_HISTORY_DEFAULT_LIMIT,
        },
    ));
    add(Fixture::new(
        "GitAvatars",
        GitAvatars(HashMap::from([(s("dev@example.com"), s("iVBORw0KGgo="))])),
    ));
    add(Fixture::new(
        "SwitchRefParams",
        SwitchRefParams {
            repo_path: s("/home/dev/project"),
            ref_name: s("origin/dev"),
        },
    ));
    add(Fixture::new(
        "SwitchRefReply",
        SwitchRefReply { branch: s("dev") },
    ));
    add(Fixture::new(
        "ListFoldersParams",
        ListFoldersParams {
            path: some("/home/dev"),
        },
    ));
    add(Fixture::new(
        "FolderListing",
        FolderListing {
            path: s("/home/dev"),
            entries: vec![FolderEntry {
                name: s("project"),
                is_dir: true,
                is_repo: true,
            }],
            truncated: false,
        },
    ));
    add(Fixture::new(
        "DriveListing",
        DriveListing {
            drives: vec![DriveEntry {
                name: s("Home"),
                path: s("/home/dev"),
            }],
        },
    ));
    add(Fixture::new(
        "SearchFilesParams",
        SearchFilesParams {
            query: s("main"),
            chat_id: some("chat-1"),
            space_id: None,
            path: None,
        },
    ));
    add(Fixture::new(
        "FileSearchMatchList",
        FileSearchMatchList(vec![FileSearchMatch {
            path: s("src/main.rs"),
            is_dir: false,
        }]),
    ));
    add(Fixture::new(
        "CreateWorktreeParams",
        CreateWorktreeParams {
            repo_path: s("/home/dev/project"),
            branch: s("fix-linker"),
            space_id: some("space-1"),
        },
    ));
    add(Fixture::new(
        "CreateWorktreeOutcome",
        CreateWorktreeOutcome {
            worktree: Worktree {
                repo_path: s("/home/dev/project"),
                path: s("/home/dev/project-wt/fix-linker"),
                branch: s("loams-desktop/fix-linker"),
                name: s("fix-linker"),
                checkout_id: some("checkout-2"),
            },
            setup_action: Some(action_run()),
            setup_error: None,
        },
    ));
    add(Fixture::new(
        "DeleteWorktreeParams",
        DeleteWorktreeParams {
            repo_path: s("/home/dev/project"),
            worktree_path: s("/home/dev/project-wt/fix-linker"),
        },
    ));
    add(Fixture::new(
        "CheckoutDiffList",
        CheckoutDiffList(vec![checkout_diff()]),
    ));
    add(Fixture::new("CheckoutDiff", checkout_diff()));
    add(Fixture::new(
        "WorkspaceGitStatusFrame",
        WorkspaceGitStatusFrame {
            status: Some(CheckoutGitStatus {
                checkout_id: s("checkout-1"),
                device_id: s("device-1"),
                revision: s("rev-7"),
                complete: true,
                files: vec![GitFileStatus {
                    path: s("src/main.rs"),
                    old_path: None,
                    index: GitFileState::Unchanged,
                    worktree: GitFileState::Modified,
                }],
            }),
        },
    ));
    add(Fixture::new(
        "WatchCheckoutChangeRequestParams",
        WatchCheckoutChangeRequestParams {
            cwd: s("/home/dev/project"),
            branch: some("fix-linker"),
        },
    ));
    add(Fixture::new(
        "CheckoutChangeRequestStatus",
        CheckoutChangeRequestStatus {
            checkout_id: s("checkout-1"),
            device_id: s("device-1"),
            cwd: s("/home/dev/project"),
            branch: s("fix-linker"),
            change_request: Some(ChangeRequestSummary {
                provider: s("github"),
                number: 42,
                title: s("Fix the linker"),
                url: s("https://github.com/example/project/pull/42"),
                state: ChangeRequestState::Open,
                base_ref: s("main"),
                head_ref: s("fix-linker"),
            }),
            updated_at: at(),
        },
    ));
    add(Fixture::new(
        "GetCheckoutDiffParams",
        GetCheckoutDiffParams {
            cwd: s("/home/dev/project"),
            mode: s("branch"),
            base_ref: some("main"),
            chat_id: None,
            commit_sha: None,
        },
    ));
    add(Fixture::new(
        "DiscardWorkingTreeParams",
        DiscardWorkingTreeParams {
            chat_id: s("chat-1"),
            checkout_id: s("checkout-1"),
            expected_checksum: s("sum-1"),
        },
    ));
    add(Fixture::new(
        "DiscardWorkingTreeReply",
        DiscardWorkingTreeReply {
            ok: true,
            checksum: s("sum-2"),
        },
    ));

    // Worktrees and git writes (SD1 Task 9, §53 §6.5 G1–G5, G21).
    add(Fixture::new(
        "WorktreeList",
        WorktreeList(vec![WorktreeInfo {
            path: s("/home/dev/.loams-desktop/worktrees/project/fix-linker"),
            branch: s("loams-desktop/fix-linker"),
            head: s("4b825dc642cb6eb9a060e54bf8d69288fbee4904"),
            checkout_id: some("checkout-2"),
            chat_ids: vec![s("chat-1")],
            dirty: true,
            ahead: 2,
            behind: 1,
            last_activity_ms: MS,
        }]),
    ));
    add(Fixture::new(
        "CommitWorktreeParams",
        CommitWorktreeParams {
            checkout_id: s("checkout-1"),
            cwd: s("/home/dev/project"),
            message: s("Fix the linker"),
            paths: vec![s("src/main.rs")],
            all: false,
            amend: false,
            signoff: true,
            expected_checksum: s("sum-1"),
        },
    ));
    add(Fixture::new(
        "CommitWorktreeReply",
        CommitWorktreeReply {
            sha: s("4b825dc642cb6eb9a060e54bf8d69288fbee4904"),
            summary: s("Fix the linker"),
        },
    ));
    add(Fixture::new(
        "PushBranchParams",
        PushBranchParams {
            cwd: s("/home/dev/project"),
            remote: some("origin"),
            branch: s("fix-linker"),
            set_upstream: true,
            force_with_lease: false,
        },
    ));
    add(Fixture::new(
        "PushBranchReply",
        PushBranchReply {
            remote: s("origin"),
            ref_name: s("refs/heads/fix-linker"),
            url: some("https://forgejo.example.com/dev/project.git"),
        },
    ));
    add(Fixture::new(
        "CreateChangeRequestParams",
        CreateChangeRequestParams {
            cwd: s("/home/dev/project"),
            provider: ChangeRequestProvider::Forgejo,
            base: s("main"),
            head: s("fix-linker"),
            title: s("Fix the linker"),
            body: s("Links again."),
            draft: false,
            link_issue: some("42"),
        },
    ));
    add(Fixture::new(
        "CreateChangeRequestReply",
        CreateChangeRequestReply {
            provider: ChangeRequestProvider::Forgejo,
            number: 7,
            url: s("https://forgejo.example.com/dev/project/pulls/7"),
            state: ChangeRequestState::Open,
        },
    ));
    add(Fixture::new(
        "MergeBranchParams",
        MergeBranchParams {
            repo_path: s("/home/dev/project"),
            source: s("loams-desktop/fix-linker"),
            target: s("main"),
            strategy: MergeStrategy::Squash,
            message: some("Fix the linker"),
            delete_source: true,
            remove_worktree: true,
            expected_source_head: s("4b825dc642cb6eb9a060e54bf8d69288fbee4904"),
        },
    ));
    add(Fixture::new(
        "MergeBranchReply",
        MergeBranchReply {
            sha: s("4b825dc642cb6eb9a060e54bf8d69288fbee4904"),
        },
    ));
    add(Fixture::new(
        "DiscardPathsParams",
        DiscardPathsParams {
            checkout_id: s("checkout-1"),
            paths: vec![s("src/main.rs")],
            expected_checksum: s("sum-1"),
        },
    ));

    // Review comments and content search (SD1 Task 11, §53 §6.5 G8–G11, G23).
    add(Fixture::new("ReviewComment", review_comment()));
    add(Fixture::new(
        "WatchReviewCommentsParams",
        WatchReviewCommentsParams {
            checkout_id: s("checkout-1"),
        },
    ));
    add(Fixture::new(
        "ReviewCommentsFrame.reset",
        ReviewCommentsFrame::Reset {
            reset: vec![review_comment()],
        },
    ));
    add(Fixture::new(
        "ReviewCommentsFrame.delta",
        ReviewCommentsFrame::Delta {
            upsert: vec![ReviewComment {
                outdated: true,
                ..review_comment()
            }],
            remove: vec![s("comment-2")],
        },
    ));
    add(Fixture::new(
        "DeleteReviewCommentParams",
        DeleteReviewCommentParams {
            checkout_id: s("checkout-1"),
            id: s("comment-1"),
        },
    ));
    add(Fixture::new(
        "ResolveReviewCommentParams",
        ResolveReviewCommentParams {
            checkout_id: s("checkout-1"),
            id: s("comment-1"),
            resolved: true,
        },
    ));
    add(Fixture::new(
        "SendReviewToThreadParams",
        SendReviewToThreadParams {
            chat_id: s("chat-1"),
            comment_ids: vec![s("comment-1")],
        },
    ));
    add(Fixture::new(
        "PostReviewToChangeRequestParams",
        PostReviewToChangeRequestParams {
            checkout_id: s("checkout-1"),
            comment_ids: vec![s("comment-1")],
            event: ReviewEvent::RequestChanges,
        },
    ));
    add(Fixture::new(
        "PostReviewToChangeRequestReply",
        PostReviewToChangeRequestReply {
            url: s("https://forgejo.example.com/dev/project/pulls/7#issuecomment-3"),
        },
    ));
    add(Fixture::new(
        "SearchWorkspaceContentParams",
        SearchWorkspaceContentParams {
            space_id: some("space-1"),
            chat_id: None,
            query: s("fn main"),
            regex: false,
            case_sensitive: true,
            globs: vec![s("*.rs")],
            max_results: 200,
        },
    ));
    add(Fixture::new(
        "ContentSearchMatch",
        ContentSearchMatch {
            path: s("src/main.rs"),
            line: 3,
            column: 1,
            preview: s("fn main() {"),
        },
    ));
    run_group_fixtures(&mut add);
    marketplace_fixtures(&mut add);
    add(Fixture::new(
        "GetCheckoutFileDiffTextRequest",
        GetCheckoutFileDiffTextRequest {
            checkout_id: s("checkout-1"),
            cwd: s("/home/dev/project"),
            path: s("src/main.rs"),
            mode: s("turn"),
            base_ref: None,
            chat_id: some("chat-1"),
            commit_sha: None,
            diff_checksum: s("sum-1"),
        },
    ));
    add(Fixture::new(
        "CheckoutFileDiffText",
        CheckoutFileDiffText {
            diff_checksum: s("sum-1"),
            old_text: some("fn main() {}\n"),
            new_text: some("fn main() { run(); }\n"),
            old_content_hash: some("h-old"),
            new_content_hash: some("h-new"),
            binary: false,
            truncated: false,
            stale: false,
        },
    ));

    // Workspace files.
    add(Fixture::new(
        "ListWorkspaceDirectoryRequest",
        ListWorkspaceDirectoryRequest {
            target: target(),
            directory: s("src"),
            include_ignored: false,
            cursor: None,
        },
    ));
    add(Fixture::new(
        "WorkspaceDirectoryPage",
        WorkspaceDirectoryPage {
            checkout_id: some("checkout-1"),
            mutation_capabilities: Some(WorkspaceMutationCapabilities {
                move_entry: true,
                delete_entry: true,
            }),
            directory: s("src"),
            entries: vec![workspace_entry()],
            next_cursor: some("cursor-2"),
            truncated: false,
        },
    ));
    add(Fixture::new(
        "SearchWorkspaceFilesRequest",
        SearchWorkspaceFilesRequest {
            target: target(),
            query: s("main"),
            include_ignored: false,
            limit: Some(20),
        },
    ));
    add(Fixture::new(
        "WorkspaceFileSearchMatchList",
        WorkspaceFileSearchMatchList(vec![WorkspaceFileSearchMatch {
            path: s("src/main.rs"),
            name: s("main.rs"),
            kind: WorkspaceEntryKind::File,
            score: 96,
        }]),
    ));
    add(Fixture::new(
        "ReadWorkspaceImageRequest",
        ReadWorkspaceImageRequest {
            target: target(),
            path: s("docs/shot.png"),
            expected_checkout_id: s("checkout-1"),
            offset: 0,
            expected_content_hash: None,
        },
    ));
    add(Fixture::new(
        "WorkspaceImageChunk",
        WorkspaceImageChunk {
            checkout_id: s("checkout-1"),
            content_hash: s("h-img"),
            mime_type: s("image/png"),
            data: s("iVBORw0KGgo="),
            next_offset: 8,
            size: 8,
            done: true,
        },
    ));
    add(Fixture::new(
        "ReadWorkspaceFileRequest",
        ReadWorkspaceFileRequest {
            target: target(),
            path: s("src/main.rs"),
        },
    ));
    add(Fixture::new(
        "WorkspaceFileText",
        WorkspaceFileText {
            checkout_id: s("checkout-1"),
            path: s("src/main.rs"),
            text: some("fn main() {}\n"),
            content_hash: some("h-1"),
            size: 13,
            modified_at: Some(at()),
            encoding: WorkspaceTextEncoding::Utf8,
            line_ending: Some(WorkspaceLineEnding::Lf),
            read_only_reason: None,
            truncated: false,
        },
    ));
    add(Fixture::new(
        "MoveWorkspaceEntryRequest",
        MoveWorkspaceEntryRequest {
            target: target(),
            operation_id: s("op-1"),
            expected_checkout_id: s("checkout-1"),
            source_path: s("src/main.rs"),
            destination_path: s("src/lib.rs"),
            expected_source_revision: s("rev-1"),
            expected_kind: WorkspaceEntryKind::File,
        },
    ));
    add(Fixture::new(
        "DeleteWorkspaceEntryRequest",
        DeleteWorkspaceEntryRequest {
            target: target(),
            operation_id: s("op-2"),
            expected_checkout_id: s("checkout-1"),
            path: s("target"),
            expected_source_revision: s("rev-2"),
            expected_kind: WorkspaceEntryKind::Directory,
            recursive: true,
        },
    ));
    add(Fixture::new(
        "WorkspaceMutationOutcome",
        WorkspaceMutationOutcome::Applied {
            operation_id: s("op-1"),
            checkout_id: s("checkout-1"),
            change: file_change(),
            entry: Some(workspace_entry()),
        },
    ));
    add(Fixture::new(
        "WriteWorkspaceFileRequest",
        WriteWorkspaceFileRequest {
            expected_checkout_id: s("checkout-1"),
            target: target(),
            path: s("src/main.rs"),
            text: s("fn main() { run(); }\n"),
            expected_content_hash: s("h-1"),
            encoding: WorkspaceWritableEncoding::Utf8,
            line_ending: WorkspaceWritableLineEnding::Lf,
        },
    ));
    add(Fixture::new(
        "WriteWorkspaceFileOutcome",
        WriteWorkspaceFileOutcome::Written {
            file: WorkspaceFileWriteResult {
                path: s("src/main.rs"),
                content_hash: s("h-2"),
                size: 21,
                modified_at: Some(at()),
            },
        },
    ));
    add(Fixture::new(
        "WatchWorkspaceFilesRequest",
        WatchWorkspaceFilesRequest { target: target() },
    ));
    add(Fixture::new(
        "WorkspaceFileChanges",
        WorkspaceFileChanges {
            sequence: 7,
            resync_required: false,
            changes: vec![file_change()],
        },
    ));

    // Project actions and terminals.
    add(Fixture::new(
        "ListProjectActionsParams",
        ListProjectActionsParams {
            space_id: s("space-1"),
        },
    ));
    add(Fixture::new("ProjectActionsSnapshot", actions_snapshot()));
    add(Fixture::new(
        "UpsertProjectActionParams",
        UpsertProjectActionParams {
            space_id: s("space-1"),
            action_id: None,
            action: action_draft(),
        },
    ));
    add(Fixture::new(
        "DeleteProjectActionParams",
        DeleteProjectActionParams {
            space_id: s("space-1"),
            action_id: s("action-1"),
        },
    ));
    add(Fixture::new(
        "RunProjectActionParams",
        RunProjectActionParams {
            space_id: s("space-1"),
            chat_id: s("chat-1"),
            action_id: s("action-1"),
            cols: 120,
            rows: 32,
        },
    ));
    add(Fixture::new("ProjectActionRun", action_run()));
    add(Fixture::new(
        "OpenTerminalParams",
        OpenTerminalParams {
            chat_id: s("chat-1"),
            cols: 120,
            rows: 32,
            cwd: some("~"),
        },
    ));
    add(Fixture::new("TerminalSession", terminal()));
    add(Fixture::new(
        "SubscribeTerminalParams",
        SubscribeTerminalParams {
            terminal_id: s("term-1"),
            after_seq: Some(10),
        },
    ));
    add(Fixture::new(
        "TerminalEvent.data",
        TerminalEvent::Data {
            seq: 11,
            data: s("JCBjYXJnbyB0ZXN0Cg=="),
        },
    ));
    add(Fixture::new(
        "TerminalEvent.exit",
        TerminalEvent::Exit {
            seq: 12,
            exit_code: 0,
            signal: None,
        },
    ));
    add(Fixture::new(
        "WriteTerminalParams",
        WriteTerminalParams {
            terminal_id: s("term-1"),
            data: s("bHMK"),
        },
    ));
    add(Fixture::new(
        "ResizeTerminalParams",
        ResizeTerminalParams {
            terminal_id: s("term-1"),
            cols: 100,
            rows: 30,
        },
    ));
    add(Fixture::new(
        "TerminalIdParams",
        TerminalIdParams {
            terminal_id: s("term-1"),
        },
    ));

    // Agent accounts, uploads and tool outputs.
    add(Fixture::new(
        "ListAgentAccountsParams",
        ListAgentAccountsParams {
            force_usage: Some(true),
        },
    ));
    add(Fixture::new("AgentAccountsSnapshot", agent_accounts()));
    add(Fixture::new(
        "AgentAccountParams",
        AgentAccountParams {
            harness: HarnessId::Codex,
            account_id: s("account-1"),
        },
    ));
    add(Fixture::new(
        "StartAgentLoginParams",
        StartAgentLoginParams {
            harness: HarnessId::Opencode,
            provider: some("anthropic"),
        },
    ));
    add(Fixture::new(
        "AgentLoginStart",
        AgentLoginStart {
            login_id: s("login-1"),
            url: s("https://auth.example.com/authorize"),
            mode: AgentLoginMode::Browser,
            callback_port: Some(1455),
        },
    ));
    add(Fixture::new(
        "CompleteAgentLoginParams",
        CompleteAgentLoginParams {
            login_id: s("login-1"),
            code: s("code-1"),
        },
    ));
    add(Fixture::new(
        "LoginIdParams",
        LoginIdParams {
            login_id: s("login-1"),
        },
    ));
    add(Fixture::new(
        "AgentLoginPoll",
        AgentLoginPoll {
            status: AgentLoginStatus::Pending,
            message: some("Waiting for the browser"),
            url: None,
            callback_port: None,
        },
    ));
    add(Fixture::new(
        "UploadChunkParams",
        UploadChunkParams {
            upload_id: s("upload-1"),
            data: s("iVBORw0KGgo="),
            seq: Some(0),
        },
    ));
    add(Fixture::new(
        "UploadCommitParams",
        UploadCommitParams {
            upload_id: s("upload-1"),
            file_name: s("shot.png"),
        },
    ));
    add(Fixture::new(
        "UploadCommitReply",
        UploadCommitReply {
            path: s("/home/dev/.uploads/shot.png"),
        },
    ));
    add(Fixture::new(
        "ReadAttachmentChunkParams",
        ReadAttachmentChunkParams {
            path: s("/home/dev/.uploads/shot.png"),
            offset: 0,
        },
    ));
    add(Fixture::new(
        "AttachmentChunk",
        AttachmentChunk {
            name: s("shot.png"),
            mime_type: s("image/png"),
            data: s("iVBORw0KGgo="),
            next_offset: 8,
            done: true,
        },
    ));
    add(Fixture::new(
        "FetchToolBlobParams",
        FetchToolBlobParams {
            blob_ref: s("chat-1/call-1"),
            offset: 0,
            len: Some(262_144),
        },
    ));
    add(Fixture::new(
        "ToolOutputChunk",
        ToolOutputChunk {
            text: s("Finished dev profile"),
            offset: 0,
            len: 20,
            total: 20,
            truncated: false,
            eof: true,
        },
    ));

    // The factory loop's `mcp` methods (FT0 Task 4).
    add(Fixture::new(
        "CreateRunWorktreeParams",
        factory_loop::CreateRunWorktreeParams {
            chat_id: s("chat_01"),
            branch: s("factory/fix-linker"),
            base: some("main"),
        },
    ));
    add(Fixture::new(
        "Worktree",
        Worktree {
            repo_path: s("/home/dev/project"),
            path: s("/home/dev/.loams/worktrees/project/factory-fix-linker"),
            branch: s("factory/fix-linker"),
            name: s("factory-fix-linker"),
            checkout_id: some("ck_01"),
        },
    ));
    add(Fixture::new(
        "GetThreadDiffParams",
        factory_loop::GetThreadDiffParams {
            chat_id: s("chat_01"),
            base_ref: some("main"),
        },
    ));
    add(Fixture::new(
        "PostReviewCommentParams",
        factory_loop::PostReviewCommentParams {
            chat_id: s("chat_01"),
            path: s("src/linker.rs"),
            side: Some(DiffSide::New),
            line: 12,
            end_line: Some(14),
            body: s("This drops the error."),
            diff_checksum: s("sha256:0f1e"),
        },
    ));
    add(Fixture::new(
        "OpenChangeRequestParams",
        factory_loop::OpenChangeRequestParams {
            chat_id: s("chat_01"),
            provider: ChangeRequestProvider::Forgejo,
            base: s("main"),
            title: s("Fix the linker"),
            body: s("Links again."),
            draft: false,
        },
    ));
    add(Fixture::new(
        "OpenChangeRequestReply",
        factory_loop::OpenChangeRequestReply {
            provider: ChangeRequestProvider::Forgejo,
            number: 7,
            url: s("https://forgejo.example.com/dev/project/pulls/7"),
            state: ChangeRequestState::Open,
            repo: s("dev/project"),
            head: s("factory/fix-linker"),
            head_sha: s("0123456789abcdef0123456789abcdef01234567"),
            existed: false,
        },
    ));
    add(Fixture::new(
        "RequestApprovalParams",
        factory_loop::RequestApprovalParams {
            chat_id: s("chat_01"),
            kind: s("merge"),
            summary: s("Merge #7 into main"),
            payload: serde_json::json!({ "number": 7, "base": "main" }),
        },
    ));
    add(Fixture::new(
        "GetApprovalParams",
        factory_loop::GetApprovalParams {
            chat_id: s("chat_01"),
            approval_id: s("appr_01"),
        },
    ));
    add(Fixture::new(
        "TypedApproval",
        factory_loop::TypedApproval {
            id: s("appr_01"),
            chat_id: s("chat_01"),
            kind: s("merge"),
            summary: s("Merge #7 into main"),
            payload_hash: s("sha256:9a8b"),
            state: factory_loop::TypedApprovalState::Approved,
            created_at_ms: MS,
            decided_at_ms: Some(MS + 1_000),
        },
    ));
    add(Fixture::new(
        "MergeChangeRequestParams",
        factory_loop::MergeChangeRequestParams {
            chat_id: s("chat_01"),
            approval_id: s("appr_01"),
            provider: ChangeRequestProvider::Forgejo,
            number: 7,
            head_sha: s("0123456789abcdef0123456789abcdef01234567"),
            strategy: Some(MergeStrategy::Squash),
        },
    ));
    add(Fixture::new(
        "MergeChangeRequestReply",
        factory_loop::MergeChangeRequestReply {
            provider: ChangeRequestProvider::Forgejo,
            number: 7,
            repo: s("dev/project"),
            merged: true,
        },
    ));

    // CI checks for the factory loop (FT0 Task 5).
    add(Fixture::new(
        "GetChecksParams",
        factory_loop::GetChecksParams {
            chat_id: s("chat_01"),
            provider: ChangeRequestProvider::Forgejo,
            sha: some("0123456789abcdef0123456789abcdef01234567"),
        },
    ));
    add(Fixture::new(
        "ChecksReply",
        factory_loop::ChecksReply {
            provider: ChangeRequestProvider::Forgejo,
            repo: s("dev/project"),
            sha: s("0123456789abcdef0123456789abcdef01234567"),
            state: factory_loop::CiState::Failure,
            checks: vec![
                factory_loop::CiCheck {
                    name: s("build"),
                    state: factory_loop::CiState::Success,
                    url: some("https://forgejo.example.com/dev/project/actions/runs/3/jobs/0"),
                    log_id: some("3/0"),
                },
                factory_loop::CiCheck {
                    name: s("test"),
                    state: factory_loop::CiState::Failure,
                    url: some("https://forgejo.example.com/dev/project/actions/runs/3/jobs/1"),
                    log_id: some("3/1"),
                },
            ],
        },
    ));
    add(Fixture::new(
        "GetCheckLogParams",
        factory_loop::GetCheckLogParams {
            chat_id: s("chat_01"),
            provider: ChangeRequestProvider::Forgejo,
            log_id: s("3/1"),
            lines: Some(200),
        },
    ));
    add(Fixture::new(
        "CheckLogReply",
        factory_loop::CheckLogReply {
            text: s("test linker ... FAILED\n"),
            lines: 1,
            truncated: false,
        },
    ));

    // Every variant of the unions a client switches on.
    for (name, call) in tool_calls() {
        add(Fixture::new(&format!("ToolCall.{name}"), call));
    }
    for (name, event) in agent_events() {
        add(Fixture::new(&format!("AgentEvent.{name}"), event));
    }
    out
}

/// MK1 Task 4b's `MarketplaceService`.
fn marketplace_fixtures(add: &mut impl FnMut(Fixture)) {
    use loams_agentd_proto::marketplace::*;
    let official = MarketplaceSource {
        name: s("official"),
        url: s("https://ostrium-labs.github.io/loams-marketplace/index.json"),
        identity: s(
            "https://github.com/ostrium-labs/loams-marketplace/.github/workflows/publish.yml@refs/heads/main",
        ),
        issuer: s("https://token.actions.githubusercontent.com"),
        official: true,
        refreshed_ms: MS,
        error: None,
        listings: 6,
    };
    add(Fixture::new(
        "MarketplaceSourceList",
        MarketplaceSourceList {
            sources: vec![official.clone()],
        },
    ));
    add(Fixture::new(
        "MarketplaceSource",
        MarketplaceSource {
            name: s("example"),
            official: false,
            error: some("HTTP 404 for https://example.org/market/index.json"),
            ..official
        },
    ));
    add(Fixture::new(
        "AddMarketplaceSourceParams",
        AddMarketplaceSourceParams {
            name: s("example"),
            url: s("https://example.org/market/index.json"),
            identity: s(
                "https://github.com/example/market/.github/workflows/publish.yml@refs/heads/main",
            ),
            issuer: None,
        },
    ));
    add(Fixture::new(
        "RemoveMarketplaceSourceParams",
        RemoveMarketplaceSourceParams { name: s("example") },
    ));
    add(Fixture::new(
        "SearchListingsParams",
        SearchListingsParams {
            query: s("github issues"),
            kinds: vec![s("mcp-server"), s("agent")],
            frameworks: vec![s("mastra")],
            languages: vec![s("typescript")],
            min_level: some("core"),
            tiers: vec![ListingTier::FirstParty, ListingTier::Verified],
            source: some("official"),
        },
    ));
    let revocation = ListingRevocation {
        severity: RevocationSeverity::Warn,
        reason: s("An upstream advisory affects some tools."),
        versions: s(">=2.0.0, <2.1.0"),
        advisory: some("https://github.com/advisories/GHSA-0000-0000-0000"),
    };
    let summary = ListingSummary {
        id: s("ostrium/github-mcp-server"),
        source: s("official"),
        kind: s("mcp-server"),
        title: s("GitHub"),
        summary: s("GitHub's official MCP server."),
        tier: ListingTier::FirstParty,
        latest: some("2.0.3"),
        framework: None,
        language: some("go"),
        level: some("core"),
        revocation: Some(revocation.clone()),
    };
    add(Fixture::new(
        "ListingSearchResults",
        ListingSearchResults {
            results: vec![
                summary.clone(),
                ListingSummary {
                    id: s("ostrium/loams-starter"),
                    kind: s("agent"),
                    title: s("Loams starter"),
                    summary: s("A first Mastra agent."),
                    framework: some("mastra"),
                    language: some("typescript"),
                    level: None,
                    revocation: None,
                    ..summary.clone()
                },
            ],
        },
    ));
    add(Fixture::new(
        "ListingRef",
        ListingRef {
            id: s("ostrium/github-mcp-server"),
            version: some("2.0.3"),
            source: some("official"),
        },
    ));
    add(Fixture::new(
        "ListingDetail",
        ListingDetail {
            summary: ListingSummary {
                id: s("ostrium/claude-code"),
                kind: s("harness"),
                title: s("Claude Code"),
                summary: s("Anthropic's coding agent."),
                latest: some("1.0.0"),
                language: None,
                level: None,
                revocation: None,
                ..summary.clone()
            },
            listing: serde_json::json!({
                "id": "ostrium/claude-code",
                "kind": "harness",
                "version": "1.0.0",
                "title": "Claude Code",
                "summary": "Anthropic's coding agent.",
                "licence": "Apache-2.0",
                "pricing": "free",
                "publisher": { "name": "Ostrium Labs", "url": "https://ostriumlabs.org" },
                "source": {
                    "repository": "https://github.com/ostrium-labs/loams",
                    "commit": "0123456789abcdef0123456789abcdef01234567",
                    "path": "crates/loams-agentd-harness"
                },
                "harness": {
                    "auth": ["cli-login", "api-key"],
                    "launch": { "command": "claude", "protocol": "native" },
                    "native_adapter": "claude-code"
                }
            }),
            versions: vec![
                ListingVersion {
                    version: s("0.9.0"),
                    yanked: some("Broken launch line."),
                    revocation: Some(RevocationSeverity::Disable).map(|severity| {
                        ListingRevocation {
                            severity,
                            ..revocation.clone()
                        }
                    }),
                },
                ListingVersion {
                    version: s("1.0.0"),
                    yanked: None,
                    revocation: None,
                },
            ],
        },
    ));
    add(Fixture::new(
        "PlanInstallReply",
        PlanInstallReply {
            plan: InstallPlan {
                digest: s("5f2b1c8e0d3a4b6c7d8e9f00112233445566778899aabbccddeeff0011223344"),
                summary: summary.clone(),
                version: s("2.0.3"),
                place: serde_json::json!({
                    "always_ask": ["merge_pull_request"],
                    "egress": ["api.github.com"]
                }),
                tools: vec![
                    PlanTool {
                        name: s("merge_pull_request"),
                        description: s("Merges a pull request."),
                        risk: s("destructive"),
                        always_asks: true,
                        mcp_server: None,
                    },
                    PlanTool {
                        name: s("get_issue"),
                        description: s("Reads an issue."),
                        risk: s("read"),
                        always_asks: false,
                        mcp_server: some("io.github.github/github-mcp-server"),
                    },
                ],
                egress: vec![s("api.github.com")],
                dependencies: vec![PlanDependency {
                    id: s("ostrium/plane-mcp-server"),
                    version: s(">=0.3.0"),
                    installed: true,
                }],
                secrets: vec![PlanSecret {
                    name: s("token"),
                    description: s("A GitHub personal access token."),
                    required: true,
                }],
                models: serde_json::Value::Null,
                cost: serde_json::Value::Null,
                licence: s("MIT"),
                provenance: serde_json::json!({
                    "type": "github-attestation",
                    "ref": "https://api.github.com/repos/github/github-mcp-server/attestations"
                }),
                source_repository: s("https://github.com/github/github-mcp-server"),
                version_age_days: Some(4),
                cannot: vec![s("Reach any host but api.github.com.")],
            },
        },
    ));
    add(Fixture::new(
        "InstallListingParams",
        InstallListingParams {
            id: s("ostrium/github-mcp-server"),
            version: some("2.0.3"),
            source: some("official"),
            plan_digest: s("5f2b1c8e0d3a4b6c7d8e9f00112233445566778899aabbccddeeff0011223344"),
            config: [(s("org"), s("ostrium-labs"))].into_iter().collect(),
            secrets: [(s("token"), s("ghp_example"))].into_iter().collect(),
        },
    ));
    let installed = InstalledListing {
        id: s("ostrium/github-mcp-server"),
        version: s("2.0.3"),
        source: s("official"),
        kind: s("mcp-server"),
        installed_ms: MS,
        revocation: Some(revocation),
        update: some("2.1.0"),
    };
    add(Fixture::new(
        "InstalledListingReply",
        InstalledListingReply {
            installed: installed.clone(),
        },
    ));
    add(Fixture::new(
        "UninstallListingParams",
        UninstallListingParams {
            id: s("ostrium/github-mcp-server"),
        },
    ));
    add(Fixture::new(
        "InstalledListingList",
        InstalledListingList {
            installed: vec![installed],
        },
    ));
    add(Fixture::new(
        "AvailableUpdateList",
        AvailableUpdateList {
            updates: vec![AvailableUpdate {
                id: s("ostrium/github-mcp-server"),
                from: s("2.0.3"),
                to: s("2.1.0"),
                widens: true,
            }],
        },
    ));
    add(Fixture::new(
        "UpdateListingParams",
        UpdateListingParams {
            id: s("ostrium/github-mcp-server"),
            version: some("2.1.0"),
            plan_digest: some("5f2b1c8e0d3a4b6c7d8e9f00112233445566778899aabbccddeeff0011223344"),
        },
    ));
}

/// SD1 Task 12's run groups (G12–G16).
fn run_group_fixtures(add: &mut impl FnMut(Fixture)) {
    use loams_agentd_proto::groups::*;
    let mut model_options = serde_json::Map::new();
    model_options.insert(s("effort"), serde_json::json!("high"));
    add(Fixture::new(
        "CreateRunGroupParams",
        CreateRunGroupParams {
            space_id: some("space-1"),
            repo_path: s("/home/dev/project"),
            base: s("main"),
            prompt: s("Fix the build"),
            attachments: vec![s("/home/dev/.loams-desktop/uploads/u-1/screenshot.png")],
            variants: vec![
                RunVariant {
                    harness: HarnessId::ClaudeCode,
                    model: some("claude-sonnet"),
                    reasoning: Some(ReasoningLevel::High),
                    model_options,
                },
                RunVariant {
                    harness: HarnessId::Codex,
                    model: None,
                    reasoning: None,
                    model_options: serde_json::Map::new(),
                },
            ],
            check_action_id: some("test"),
            approval_mode: ApprovalMode::Supervised,
        },
    ));
    add(Fixture::new(
        "CreateRunGroupReply",
        CreateRunGroupReply {
            group_id: s("group-1"),
            chat_ids: vec![s("chat-1"), s("chat-2")],
        },
    ));
    add(Fixture::new(
        "WatchRunGroupsParams",
        WatchRunGroupsParams {
            space_id: some("space-1"),
        },
    ));
    add(Fixture::new(
        "RunGroupList",
        RunGroupList {
            groups: vec![RunGroup {
                id: s("group-1"),
                space_id: some("space-1"),
                repo_path: s("/home/dev/project"),
                prompt: s("Fix the build"),
                base: s("main"),
                base_sha: s("4b825dc642cb6eb9a060e54bf8d69288fbee4904"),
                created_at_ms: MS,
                state: RunGroupState::Resolved,
                approval_mode: ApprovalMode::FullAccess,
                check_action_id: some("test"),
                members: vec![
                    RunGroupMember {
                        chat_id: s("chat-1"),
                        harness: HarnessId::ClaudeCode,
                        model: some("claude-sonnet"),
                        status: RunMemberStatus::Done,
                        files_changed: 1,
                        additions: 3,
                        deletions: 1,
                        elapsed_ms: 61_000,
                        started_at_ms: Some(MS),
                        tokens: Some(12_400),
                        check: Some(MemberCheck {
                            state: CheckState::Passed,
                            exit_code: Some(0),
                            terminal_id: some("term-1"),
                            log_tail: vec![s("test result: ok. 12 passed")],
                        }),
                        cwd: s("/home/dev/.loams-desktop/worktrees/project/brave-otter"),
                        branch: s("loams-desktop/brave-otter"),
                        detached: false,
                        archived: false,
                    },
                    RunGroupMember {
                        chat_id: s("chat-2"),
                        harness: HarnessId::Codex,
                        model: None,
                        status: RunMemberStatus::Errored,
                        files_changed: 0,
                        additions: 0,
                        deletions: 0,
                        elapsed_ms: 4_000,
                        started_at_ms: None,
                        tokens: None,
                        check: None,
                        cwd: s("/home/dev/.loams-desktop/worktrees/project/calm-heron"),
                        branch: s("loams-desktop/calm-heron"),
                        detached: false,
                        archived: true,
                    },
                ],
                resolution: Some(RunGroupResolution {
                    action: ResolveAction::Merge,
                    winner_chat_id: s("chat-1"),
                    result: s("merged loams-desktop/brave-otter into main"),
                    sha: some("4b825dc642cb6eb9a060e54bf8d69288fbee4904"),
                    url: None,
                    resolved_at_ms: MS,
                }),
            }],
        },
    ));
    add(Fixture::new(
        "GetRunGroupComparisonParams",
        GetRunGroupComparisonParams {
            group_id: s("group-1"),
            a: s("chat-1"),
            b: some("chat-2"),
            path: some("src/main.rs"),
        },
    ));
    add(Fixture::new(
        "RunGroupComparison",
        RunGroupComparison {
            matrix: vec![ComparisonRow {
                path: s("src/main.rs"),
                per_member: vec![MemberDelta {
                    chat_id: s("chat-1"),
                    additions: 3,
                    deletions: 1,
                }],
            }],
            pair_diff: Some(checkout_diff()),
        },
    ));
    add(Fixture::new(
        "RunGroupCheckParams",
        RunGroupCheckParams {
            group_id: s("group-1"),
            chat_id: some("chat-1"),
        },
    ));
    add(Fixture::new(
        "RunGroupCheckReply",
        RunGroupCheckReply {
            runs: vec![CheckRun {
                chat_id: s("chat-1"),
                terminal_id: s("term-1"),
            }],
        },
    ));
    add(Fixture::new(
        "ResolveRunGroupParams",
        ResolveRunGroupParams {
            group_id: s("group-1"),
            winner_chat_id: s("chat-1"),
            action: ResolveAction::Merge,
            merge: Some(MergeBranchParams {
                repo_path: s("/home/dev/project"),
                source: s("loams-desktop/brave-otter"),
                target: s("main"),
                strategy: MergeStrategy::Squash,
                message: some("Fix the build"),
                delete_source: false,
                remove_worktree: false,
                expected_source_head: s("4b825dc642cb6eb9a060e54bf8d69288fbee4904"),
            }),
            pr: None,
            archive_others: true,
            remove_other_worktrees: false,
            force: false,
            expected_checksum: some("sum-1"),
        },
    ));
    add(Fixture::new(
        "ResolveRunGroupReply",
        ResolveRunGroupReply {
            result: s("merged loams-desktop/brave-otter into main"),
            sha: some("4b825dc642cb6eb9a060e54bf8d69288fbee4904"),
            url: None,
        },
    ));
}
