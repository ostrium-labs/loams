//! Which role may call what (design §50 §7, plan DD1 Task 7).
//!
//! Role `owner` (the main token) may call every method. Role `mcp` (a scoped
//! run token) may call only the methods the injected MCP shim uses, and only
//! on its own session or on the side chats under it: the shim's tools create
//! side chats, message them and read them back, so the session's tree is
//! what a run may touch. Everything else answers `forbidden_role`, including
//! `ResolveApproval` (Ruling 8) and the daemon's lifecycle methods.
//!
//! The payloads are checked too, so a run cannot start a more privileged one
//! (PR #434 review): a queued run carries no MCP server, no auto-approval, no
//! attachments, worktree or resume, and a sandbox no wider than the session's;
//! a new side chat's sandbox is no wider either, and its working directory
//! lies under the session's, a project's or the worktrees root (where
//! `CreateRunWorktree` puts a factory build thread's checkout, plan FT1 FT0
//! Task 4); a run cannot answer its own session's questions.
//!
//! The factory loop's methods (`CreateRunWorktree`, `GetThreadDiff`,
//! `PostReviewComment`, `OpenChangeRequest`, `RequestApproval`, `GetApproval`,
//! `MergeChangeRequest`) are role `mcp` and name a `chatId`, so they reach only
//! the run's own session tree. `MergeChangeRequest` is refused by the engine
//! without a person's approval; `ResolveApproval` stays role `owner`.
//!
//! Which methods role `mcp` may call is the `role` column of the method
//! table (`loams_agentd_proto::rpc::METHODS`, plan DD1 Task 9).

use std::sync::Arc;

use std::path::{Component, Path};

use async_trait::async_trait;
use loams_agentd_doc::SessionCommandPayload;
use loams_agentd_proto::SandboxLevel;
use loams_agentd_proto::rpc::{FORBIDDEN_ROLE, Role, method_spec};
use loams_agentd_rpc::{Identity, Policy, RpcError, RunScope, methods};
use loams_agentd_sessions::WorkspaceHost;

/// Whether role `mcp` may call `method`: its row in the method table says so.
fn mcp_may_call(method: &str) -> bool {
    method_spec(method).is_some_and(|spec| spec.role == Role::Mcp)
}

/// Methods that act on one session, named by `sessionId` or the fork's `chatId`.
const SESSION_METHODS: &[&str] = &[
    methods::WATCH_DOC_MESSAGES,
    methods::QUEUE_COMMAND,
    methods::QUEUE_MESSAGE,
];

/// The `Mutate` ops role `mcp` may send.
const MCP_MUTATIONS: &[&str] = &["createChat", "renameChat", "setChatArchived"];

/// How far up a chat's parents the session check walks.
const MAX_DEPTH: usize = 8;

/// What the policy needs to know about one chat.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatFacts {
    /// `None`: a top-level chat.
    pub parent: Option<String>,
    /// The chat's configured sandbox, if it has a config.
    pub sandbox: Option<SandboxLevel>,
    /// Its working directory: its own `cwd`, else its project's folder.
    pub cwd: Option<String>,
}

/// The chat tree, as far as the policy needs it.
pub trait ChatTree: Send + Sync + 'static {
    /// `None`: no such chat.
    fn chat(&self, chat_id: &str) -> Option<ChatFacts>;
    /// The folders of the user's projects (spaces).
    fn project_folders(&self) -> Vec<String>;
    /// Where the engine creates worktrees.
    fn worktree_roots(&self) -> Vec<String> {
        Vec::new()
    }
}

impl ChatTree for WorkspaceHost {
    fn chat(&self, chat_id: &str) -> Option<ChatFacts> {
        let chat = match WorkspaceHost::chat(self, chat_id) {
            Ok(chat) => chat?,
            Err(error) => {
                tracing::warn!(%error, "role check could not read the chat tree");
                return None;
            }
        };
        let cwd = chat.cwd.clone().or_else(|| {
            let space = chat.space_id.as_deref()?;
            self.space(space).ok().flatten().map(|space| space.path)
        });
        Some(ChatFacts {
            parent: chat.parent_chat_id,
            sandbox: chat.config.map(|config| config.sandbox),
            cwd,
        })
    }

    fn project_folders(&self) -> Vec<String> {
        self.read_spaces()
            .unwrap_or_default()
            .into_iter()
            .map(|space| space.path)
            .collect()
    }

    fn worktree_roots(&self) -> Vec<String> {
        vec![
            loams_agentd_sessions::repos::default_worktrees_root()
                .to_string_lossy()
                .into_owned(),
        ]
    }
}

fn rank(sandbox: SandboxLevel) -> u8 {
    match sandbox {
        SandboxLevel::ReadOnly => 0,
        SandboxLevel::WorkspaceWrite => 1,
        SandboxLevel::DangerFullAccess => 2,
    }
}

/// `path` is `root` or under it, with no `..` to climb back out.
fn within(path: &str, root: &str) -> bool {
    let path = Path::new(path);
    !path.components().any(|c| matches!(c, Component::ParentDir)) && path.starts_with(root)
}

/// The daemon's [`Policy`].
pub struct DaemonPolicy {
    chats: Arc<dyn ChatTree>,
}

impl std::fmt::Debug for DaemonPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonPolicy").finish_non_exhaustive()
    }
}

impl DaemonPolicy {
    pub fn new(chats: Arc<dyn ChatTree>) -> Self {
        Self { chats }
    }

    /// Whether `target` is the run's session or a chat under it.
    fn in_session(&self, run: &RunScope, target: &str) -> bool {
        let mut chat = target.to_owned();
        for _ in 0..MAX_DEPTH {
            if chat == run.session_id {
                return true;
            }
            match self.chats.chat(&chat).and_then(|facts| facts.parent) {
                Some(parent) => chat = parent,
                None => return false,
            }
        }
        false
    }

    /// The widest sandbox the run's session may hand on: its own (the
    /// workspace-write default without a config).
    fn ceiling(&self, run: &RunScope) -> SandboxLevel {
        self.chats
            .chat(&run.session_id)
            .and_then(|facts| facts.sandbox)
            .unwrap_or(SandboxLevel::WorkspaceWrite)
    }

    /// A new chat's directory: under the session's, or under a project's.
    fn cwd_allowed(&self, run: &RunScope, cwd: &str) -> bool {
        let session = self.chats.chat(&run.session_id).and_then(|facts| facts.cwd);
        session.iter().any(|root| within(cwd, root))
            || self
                .chats
                .project_folders()
                .iter()
                .chain(self.chats.worktree_roots().iter())
                .any(|root| within(cwd, root))
    }

    /// The payload of a `QueueCommand` to `target` (already in the tree).
    fn check_command(
        &self,
        run: &RunScope,
        target: &str,
        params: &serde_json::Value,
    ) -> Result<(), RpcError> {
        let command = params
            .get("command")
            .cloned()
            .and_then(|command| serde_json::from_value::<SessionCommandPayload>(command).ok())
            .ok_or_else(|| forbidden("QueueCommand without a readable command".into()))?;
        match command {
            SessionCommandPayload::Run { request, .. } => {
                if request.mcp.is_some()
                    || request.auto_approve
                    || !request.attachments.is_empty()
                    || request.worktree.is_some()
                    || request.resume.is_some()
                {
                    return Err(forbidden(
                        "a run may not set mcp, autoApprove, attachments, worktree or resume"
                            .into(),
                    ));
                }
                if rank(request.sandbox) > rank(self.ceiling(run)) {
                    return Err(forbidden(
                        "a run's sandbox is wider than the session's".into(),
                    ));
                }
                let target_cwd = self.chats.chat(target).and_then(|facts| facts.cwd);
                let cwd_ok = match &target_cwd {
                    Some(cwd) => request.cwd == *cwd,
                    None => request.cwd == "~",
                };
                if !cwd_ok {
                    return Err(forbidden("a run's cwd is not its chat's".into()));
                }
                Ok(())
            }
            SessionCommandPayload::RespondInput { .. } if target == run.session_id => Err(
                forbidden("a run may not answer its own session's questions".into()),
            ),
            SessionCommandPayload::Steer { .. }
            | SessionCommandPayload::Interrupt {}
            | SessionCommandPayload::RespondInput { .. } => Ok(()),
        }
    }

    /// `createChat`'s config and directory.
    fn check_new_chat(&self, run: &RunScope, params: &serde_json::Value) -> Result<(), RpcError> {
        if let Some(config) = params.get("config").filter(|config| !config.is_null()) {
            let sandbox = config
                .get("sandbox")
                .cloned()
                .and_then(|sandbox| serde_json::from_value::<SandboxLevel>(sandbox).ok())
                .ok_or_else(|| forbidden("a new chat's config needs a sandbox".into()))?;
            if rank(sandbox) > rank(self.ceiling(run)) {
                return Err(forbidden(
                    "a new chat's sandbox is wider than the session's".into(),
                ));
            }
        }
        if let Some(cwd) = params.get("cwd").filter(|cwd| !cwd.is_null()) {
            match cwd.as_str() {
                Some(cwd) if self.cwd_allowed(run, cwd) => {}
                _ => {
                    return Err(forbidden(
                        "a new chat's cwd is outside the session's and the projects' folders"
                            .into(),
                    ));
                }
            }
        }
        Ok(())
    }

    fn authorize_mcp(
        &self,
        run: &RunScope,
        method: &str,
        params: &serde_json::Value,
    ) -> Result<(), RpcError> {
        if !mcp_may_call(method) {
            return Err(forbidden(format!("{method} needs role owner")));
        }
        let outside = || forbidden(format!("{method} on a session outside this run's"));
        if method == methods::MUTATE {
            let op = text(params, "op").unwrap_or_default();
            if !MCP_MUTATIONS.contains(&op) {
                return Err(forbidden(format!("Mutate {op} needs role owner")));
            }
            let chat_id = text(params, "chatId").ok_or_else(outside)?;
            if op == "createChat" {
                // A new side chat under the run's session tree; an existing
                // id must already be in it.
                let parent = text(params, "parentChatId").ok_or_else(outside)?;
                let exists = self.chats.chat(chat_id).is_some();
                if !self.in_session(run, parent) || (exists && !self.in_session(run, chat_id)) {
                    return Err(outside());
                }
                return self.check_new_chat(run, params);
            }
            return if self.in_session(run, chat_id) {
                Ok(())
            } else {
                Err(outside())
            };
        }
        let mut named = None;
        for key in ["sessionId", "chatId"] {
            if let Some(target) = params.get(key) {
                match target.as_str() {
                    Some(target) if self.in_session(run, target) => named = Some(target),
                    _ => return Err(outside()),
                }
            }
        }
        let Some(target) = named else {
            return if SESSION_METHODS.contains(&method) {
                Err(outside())
            } else {
                Ok(())
            };
        };
        match method {
            methods::QUEUE_COMMAND => self.check_command(run, target, params),
            methods::QUEUE_MESSAGE
                if params
                    .get("attachments")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|attachments| !attachments.is_empty()) =>
            {
                Err(forbidden(
                    "a queued message may not carry attachments".into(),
                ))
            }
            _ => Ok(()),
        }
    }
}

#[async_trait]
impl Policy for DaemonPolicy {
    async fn authorize(
        &self,
        identity: &Identity,
        method: &str,
        params: &serde_json::Value,
    ) -> Result<(), RpcError> {
        match (identity.role, identity.run.as_ref()) {
            (Role::Owner, _) => Ok(()),
            (Role::Mcp, Some(run)) => self.authorize_mcp(run, method, params),
            (Role::Mcp, None) => Err(forbidden(format!("{method} needs a run"))),
        }
    }
}

fn text<'p>(params: &'p serde_json::Value, key: &str) -> Option<&'p str> {
    params.get(key).and_then(serde_json::Value::as_str)
}

fn forbidden(detail: String) -> RpcError {
    RpcError::Failed(format!("{FORBIDDEN_ROLE}: {detail}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    /// `main` and `other` are top-level; `side` is under `main`.
    struct Tree(HashMap<&'static str, ChatFacts>);

    impl ChatTree for Tree {
        fn chat(&self, chat_id: &str) -> Option<ChatFacts> {
            self.0.get(chat_id).cloned()
        }
        fn project_folders(&self) -> Vec<String> {
            vec!["/projects/app".into()]
        }

        fn worktree_roots(&self) -> Vec<String> {
            vec!["/worktrees".into()]
        }
    }

    fn facts(parent: Option<&str>, sandbox: Option<SandboxLevel>, cwd: Option<&str>) -> ChatFacts {
        ChatFacts {
            parent: parent.map(str::to_owned),
            sandbox,
            cwd: cwd.map(str::to_owned),
        }
    }

    fn policy() -> DaemonPolicy {
        DaemonPolicy::new(Arc::new(Tree(HashMap::from([
            (
                "main",
                facts(None, Some(SandboxLevel::WorkspaceWrite), Some("/work/main")),
            ),
            ("other", facts(None, None, None)),
            (
                "side",
                facts(
                    Some("main"),
                    Some(SandboxLevel::ReadOnly),
                    Some("/work/main/side"),
                ),
            ),
            ("other-side", facts(Some("other"), None, None)),
        ]))))
    }

    fn mcp(session: &str) -> Identity {
        Identity {
            role: Role::Mcp,
            run: Some(RunScope {
                run_id: "run-1".into(),
                session_id: session.into(),
            }),
        }
    }

    async fn allowed(identity: &Identity, method: &str, params: serde_json::Value) -> bool {
        match policy().authorize(identity, method, &params).await {
            Ok(()) => true,
            Err(error) => {
                assert!(error.to_string().starts_with(FORBIDDEN_ROLE), "{error}");
                false
            }
        }
    }

    #[tokio::test]
    async fn owner_may_call_anything() {
        for method in ["ResolveApproval", "Shutdown", "Mutate", "AnythingAtAll"] {
            assert!(allowed(&Identity::owner(), method, json!({"op": "deleteChat"})).await);
        }
    }

    /// SD1 Task 9: an agent can never commit, push, merge, open a change
    /// request or discard through the MCP shim (§53 §8.1).
    #[tokio::test]
    async fn git_writes_not_in_mcp_role() {
        let run = mcp("main");
        for method in [
            methods::LIST_WORKTREES,
            methods::COMMIT_WORKTREE,
            methods::PUSH_BRANCH,
            methods::CREATE_CHANGE_REQUEST,
            methods::MERGE_BRANCH,
            methods::DISCARD_PATHS,
            methods::DISCARD_WORKING_TREE,
            methods::DELETE_WORKTREE,
        ] {
            assert!(
                !allowed(&run, method, json!({"chatId": "main", "cwd": "/work/main"})).await,
                "{method}"
            );
            assert!(
                allowed(&Identity::owner(), method, json!({})).await,
                "{method}"
            );
        }
    }

    /// SD1 Task 11: an agent can never read, write or post review comments,
    /// queue a review turn or search contents through the MCP shim.
    #[tokio::test]
    async fn review_and_search_not_in_mcp_role() {
        let run = mcp("main");
        for method in [
            methods::WATCH_REVIEW_COMMENTS,
            methods::UPSERT_REVIEW_COMMENT,
            methods::DELETE_REVIEW_COMMENT,
            methods::RESOLVE_REVIEW_COMMENT,
            methods::SEND_REVIEW_TO_THREAD,
            methods::POST_REVIEW_TO_CHANGE_REQUEST,
            methods::SEARCH_WORKSPACE_CONTENT,
        ] {
            assert!(
                !allowed(&run, method, json!({"chatId": "main", "checkoutId": "c"})).await,
                "{method}"
            );
            assert!(
                allowed(&Identity::owner(), method, json!({})).await,
                "{method}"
            );
        }
    }

    /// MK1 Task 4b: an agent can never search, plan, install, update or
    /// uninstall marketplace listings, or change the sources, through the
    /// MCP shim: installing is the person's consent (D1466).
    #[tokio::test]
    async fn marketplace_not_in_mcp_role() {
        let run = mcp("main");
        for method in [
            methods::LIST_MARKETPLACE_SOURCES,
            methods::ADD_MARKETPLACE_SOURCE,
            methods::REMOVE_MARKETPLACE_SOURCE,
            methods::SEARCH_LISTINGS,
            methods::GET_LISTING,
            methods::PLAN_INSTALL,
            methods::INSTALL_LISTING,
            methods::UNINSTALL_LISTING,
            methods::LIST_INSTALLED_LISTINGS,
            methods::CHECK_LISTING_UPDATES,
            methods::UPDATE_LISTING,
        ] {
            assert!(
                !allowed(&run, method, json!({"chatId": "main", "id": "a/b"})).await,
                "{method}"
            );
            assert!(
                allowed(&Identity::owner(), method, json!({})).await,
                "{method}"
            );
        }
    }

    /// SD1 Task 12: an agent can never create, check or resolve a run group
    /// through the MCP shim; a resolve merges or pushes (§53 §8.1).
    #[tokio::test]
    async fn run_groups_not_in_mcp_role() {
        let run = mcp("main");
        for method in [
            methods::CREATE_RUN_GROUP,
            methods::WATCH_RUN_GROUPS,
            methods::GET_RUN_GROUP_COMPARISON,
            methods::RUN_GROUP_CHECK,
            methods::RESOLVE_RUN_GROUP,
        ] {
            assert!(
                !allowed(&run, method, json!({"chatId": "main", "groupId": "g"})).await,
                "{method}"
            );
            assert!(
                allowed(&Identity::owner(), method, json!({})).await,
                "{method}"
            );
        }
    }

    /// SD1 Task 13 (DD1 Ruling 8): an agent can never answer an approval,
    /// switch its thread to full access, or watch what waits for the person,
    /// not even for its own session; only the owner can.
    #[tokio::test]
    async fn mcp_role_cannot_resolve() {
        let run = mcp("main");
        for (method, params) in [
            (
                methods::RESOLVE_APPROVAL,
                json!({"callId": "call-1", "chatId": "main", "decision": "always"}),
            ),
            (
                methods::SET_THREAD_APPROVAL_MODE,
                json!({"chatId": "main", "mode": "full-access"}),
            ),
            (methods::WATCH_APPROVALS, json!({"chatId": "main"})),
        ] {
            assert!(!allowed(&run, method, params.clone()).await, "{method}");
            assert!(
                allowed(&Identity::owner(), method, params).await,
                "{method}"
            );
            let spec = loams_agentd_proto::rpc::method_spec(method).expect("in the table");
            assert_eq!(spec.role, Role::Owner, "{method} must never be open to mcp");
            assert!(spec.renderer, "{method} is a Studio method");
        }
    }

    #[tokio::test]
    async fn mcp_may_call_only_the_shims_methods() {
        let run = mcp("main");
        for method in [
            "ResolveApproval",
            methods::SHUTDOWN,
            methods::WRITE_WORKSPACE_FILE,
            methods::OPEN_TERMINAL,
            methods::INSTALL_HARNESS,
            methods::FETCH_TOOL_BLOB,
        ] {
            assert!(
                !allowed(&run, method, json!({"chatId": "main"})).await,
                "{method}"
            );
        }
        for method in [
            methods::LIST_HARNESSES,
            methods::WATCH_CHATS,
            methods::LOCAL_DEVICE,
        ] {
            assert!(allowed(&run, method, json!({})).await, "{method}");
        }
    }

    #[tokio::test]
    async fn mcp_is_bound_to_its_session_tree() {
        let run = mcp("main");
        for method in SESSION_METHODS {
            assert!(
                allowed(
                    &run,
                    method,
                    json!({"chatId": "main", "command": {"kind": "interrupt"}})
                )
                .await
            );
            assert!(
                allowed(
                    &run,
                    method,
                    json!({"chatId": "side", "command": {"kind": "interrupt"}})
                )
                .await
            );
            assert!(
                allowed(
                    &run,
                    method,
                    json!({"sessionId": "side", "command": {"kind": "interrupt"}})
                )
                .await
            );
            assert!(
                !allowed(
                    &run,
                    method,
                    json!({"chatId": "other", "command": {"kind": "interrupt"}})
                )
                .await
            );
            assert!(
                !allowed(
                    &run,
                    method,
                    json!({"chatId": "other-side", "command": {"kind": "interrupt"}})
                )
                .await
            );
            assert!(
                !allowed(
                    &run,
                    method,
                    json!({"chatId": "missing", "command": {"kind": "interrupt"}})
                )
                .await
            );
            assert!(
                !allowed(
                    &run,
                    method,
                    json!({"chatId": 7, "command": {"kind": "interrupt"}})
                )
                .await
            );
            assert!(!allowed(&run, method, json!({"command": {"kind": "interrupt"}})).await);
            // Both names must agree with the session.
            assert!(
                !allowed(
                    &run,
                    method,
                    json!({"sessionId": "main", "chatId": "other", "command": {"kind": "interrupt"}})
                )
                .await
            );
        }
        // A side chat's run reaches only itself.
        let side = mcp("side");
        assert!(
            allowed(
                &side,
                methods::QUEUE_MESSAGE,
                json!({"chatId": "side", "command": {"kind": "interrupt"}})
            )
            .await
        );
        assert!(
            !allowed(
                &side,
                methods::QUEUE_MESSAGE,
                json!({"chatId": "main", "command": {"kind": "interrupt"}})
            )
            .await
        );
    }

    #[tokio::test]
    async fn mcp_mutations_stay_in_the_tree() {
        let run = mcp("main");
        let create = |chat: &str, parent: Option<&str>| {
            let mut params = json!({"op": "createChat", "chatId": chat});
            if let Some(parent) = parent {
                params["parentChatId"] = json!(parent);
            }
            params
        };
        assert!(allowed(&run, methods::MUTATE, create("new", Some("main"))).await);
        assert!(!allowed(&run, methods::MUTATE, create("new", None)).await);
        assert!(!allowed(&run, methods::MUTATE, create("new", Some("other"))).await);
        // An existing id outside the tree cannot be re-created under it.
        assert!(!allowed(&run, methods::MUTATE, create("other", Some("main"))).await);
        for op in ["renameChat", "setChatArchived"] {
            assert!(allowed(&run, methods::MUTATE, json!({"op": op, "chatId": "side"})).await);
            assert!(!allowed(&run, methods::MUTATE, json!({"op": op, "chatId": "other"})).await);
        }
        for op in [
            "deleteChat",
            "deleteSpace",
            "createSpace",
            "setChatCwd",
            "renameDevice",
        ] {
            assert!(!allowed(&run, methods::MUTATE, json!({"op": op, "chatId": "side"})).await);
        }
    }

    fn run_command(cwd: &str, extra: serde_json::Value) -> serde_json::Value {
        let mut request = json!({
            "prompt": "go", "cwd": cwd, "sandbox": "workspace-write",
            "autoApprove": false, "attachments": [],
        });
        for (key, value) in extra.as_object().unwrap() {
            request[key] = value.clone();
        }
        json!({ "chatId": "side", "command": { "kind": "run", "request": request, "messageId": "m" } })
    }

    #[tokio::test]
    async fn mcp_cannot_queue_a_more_privileged_run() {
        let run = mcp("main");
        let queue = |params| async { allowed(&run, methods::QUEUE_COMMAND, params).await };
        // What the shim sends: the chat's own cwd, its sandbox, nothing else.
        let plain = run_command("/work/main/side", json!({ "sandbox": "read-only" }));
        assert!(queue(plain).await);
        for extra in [
            json!({ "autoApprove": true }),
            json!({ "mcp": { "name": "x", "command": "/bin/sh" } }),
            json!({ "attachments": ["/etc/passwd"] }),
            json!({ "worktree": { "name": "w" } }),
            json!({ "resume": "abc" }),
            json!({ "sandbox": "danger-full-access" }),
        ] {
            assert!(
                !queue(run_command("/work/main/side", extra.clone())).await,
                "{extra}"
            );
        }
        assert!(!queue(run_command("/", json!({}))).await, "another cwd");
        // Answering a side chat's question is the shim's job; its own, not.
        let answer = |chat: &str| json!({ "chatId": chat, "command": { "kind": "respondInput", "requestId": "q", "answers": [] } });
        assert!(queue(answer("side")).await);
        assert!(!queue(answer("main")).await);
        assert!(queue(json!({ "chatId": "side", "command": { "kind": "interrupt" } })).await);
        assert!(!queue(json!({ "chatId": "side", "command": "nonsense" })).await);
        assert!(
            !allowed(
                &run,
                methods::QUEUE_MESSAGE,
                json!({ "chatId": "side", "text": "hi", "attachments": ["pending://x"] })
            )
            .await
        );
    }

    #[tokio::test]
    async fn mcp_new_chats_stay_within_the_sessions_reach() {
        let run = mcp("main");
        let create = |extra: serde_json::Value| {
            let mut params = json!({ "op": "createChat", "chatId": "new", "parentChatId": "main" });
            for (key, value) in extra.as_object().unwrap() {
                params[key] = value.clone();
            }
            params
        };
        let config = |sandbox: &str| json!({ "config": { "harness": "mock", "sandbox": sandbox } });
        assert!(allowed(&run, methods::MUTATE, create(config("workspace-write"))).await);
        assert!(allowed(&run, methods::MUTATE, create(config("read-only"))).await);
        assert!(!allowed(&run, methods::MUTATE, create(config("danger-full-access"))).await);
        for cwd in ["/work/main", "/work/main/sub", "/projects/app/x"] {
            assert!(
                allowed(&run, methods::MUTATE, create(json!({ "cwd": cwd }))).await,
                "{cwd}"
            );
        }
        for cwd in [
            "/",
            "/work",
            "/work/main/../../etc",
            "/projects/application",
        ] {
            assert!(
                !allowed(&run, methods::MUTATE, create(json!({ "cwd": cwd }))).await,
                "{cwd}"
            );
        }
    }

    /// Plan FT1, FT0 Task 4: the factory loop's tools reach the run's own
    /// session tree only, and no run can settle an approval it asked for.
    #[tokio::test]
    async fn request_approval_cannot_be_settled_by_agent() {
        let run = mcp("main");
        let ask = |chat: &str| json!({ "chatId": chat, "kind": "merge", "summary": "Merge #7", "payload": {} });
        assert!(allowed(&run, methods::REQUEST_APPROVAL, ask("main")).await);
        assert!(allowed(&run, methods::REQUEST_APPROVAL, ask("side")).await);
        assert!(!allowed(&run, methods::REQUEST_APPROVAL, ask("other")).await);
        for settle in [
            json!({ "callId": "fa-1", "decision": "once" }),
            json!({ "callId": "fa-1", "decision": "once", "chatId": "main" }),
        ] {
            assert!(!allowed(&run, methods::RESOLVE_APPROVAL, settle).await);
        }
        assert!(
            !allowed(
                &run,
                methods::SET_THREAD_APPROVAL_MODE,
                json!({ "chatId": "main", "mode": "full-access" })
            )
            .await
        );
    }

    #[tokio::test]
    async fn factory_tools_stay_in_the_session_tree() {
        let run = mcp("main");
        for method in [
            methods::CREATE_RUN_WORKTREE,
            methods::GET_THREAD_DIFF,
            methods::POST_REVIEW_COMMENT,
            methods::OPEN_CHANGE_REQUEST,
            methods::GET_APPROVAL,
            methods::MERGE_CHANGE_REQUEST,
            methods::GET_CHECKS,
            methods::GET_CHECK_LOG,
        ] {
            assert!(
                allowed(&run, method, json!({ "chatId": "side" })).await,
                "{method}"
            );
            assert!(
                !allowed(&run, method, json!({ "chatId": "other" })).await,
                "{method}"
            );
            assert!(
                !allowed(&run, method, json!({ "chatId": "other-side" })).await,
                "{method}"
            );
        }
    }

    /// A build thread's chat may live in a worktree the engine created.
    #[tokio::test]
    async fn new_chat_may_run_in_a_worktree() {
        let run = mcp("main");
        let create = |cwd: &str| json!({ "op": "createChat", "chatId": "build", "parentChatId": "main", "cwd": cwd });
        assert!(allowed(&run, methods::MUTATE, create("/worktrees/app/factory-r1-1")).await);
        assert!(!allowed(&run, methods::MUTATE, create("/worktrees/../etc")).await);
        assert!(!allowed(&run, methods::MUTATE, create("/worktreesx/app")).await);
    }
}
