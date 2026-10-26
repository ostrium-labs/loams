//! studio.proto: the package's shared enums and `StudioService`'s messages.

use loams_agentd_proto as rs;
use loams_agentd_proto::daemon as rs_daemon;

use crate::proto::loams::agentd::v1 as pb;

wire_enum!(rs::HarnessId => pb::HarnessId {
    ClaudeCode => HARNESS_ID_CLAUDE_CODE,
    Codex => HARNESS_ID_CODEX,
    Devin => HARNESS_ID_DEVIN,
    Grok => HARNESS_ID_GROK,
    Hermes => HARNESS_ID_HERMES,
    Pi => HARNESS_ID_PI,
    Opencode => HARNESS_ID_OPENCODE,
    Antigravity => HARNESS_ID_ANTIGRAVITY,
    LoamsBot => HARNESS_ID_LOAMS_BOT,
    Goose => HARNESS_ID_GOOSE,
    Mock => HARNESS_ID_MOCK,
    Marketplace => HARNESS_ID_MARKETPLACE,
    Unsupported => HARNESS_ID_UNSUPPORTED,
});

wire_enum!(rs::ReasoningLevel => pb::ReasoningLevel {
    Minimal => REASONING_LEVEL_MINIMAL,
    Low => REASONING_LEVEL_LOW,
    Medium => REASONING_LEVEL_MEDIUM,
    High => REASONING_LEVEL_HIGH,
    XHigh => REASONING_LEVEL_XHIGH,
    Max => REASONING_LEVEL_MAX,
    Ultra => REASONING_LEVEL_ULTRA,
    Ultracode => REASONING_LEVEL_ULTRACODE,
    Ultrathink => REASONING_LEVEL_ULTRATHINK,
});

wire_enum!(rs::SandboxLevel => pb::SandboxLevel {
    ReadOnly => SANDBOX_LEVEL_READ_ONLY,
    WorkspaceWrite => SANDBOX_LEVEL_WORKSPACE_WRITE,
    DangerFullAccess => SANDBOX_LEVEL_DANGER_FULL_ACCESS,
});

wire_enum!(rs_daemon::DaemonMode => pb::DaemonMode {
    Service => DAEMON_MODE_SERVICE,
    Child => DAEMON_MODE_CHILD,
});

wire_enum!(rs::WorkspaceScope => pb::WorkspaceScope {
    Local => WORKSPACE_SCOPE_LOCAL,
});

wire_struct!(rs::TitleSettings => pb::TitleSettings {
    harness => harness,
    model => model,
});
