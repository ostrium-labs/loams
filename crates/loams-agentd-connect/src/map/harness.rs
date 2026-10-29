//! harness.proto: harnesses, models, skills, updates and agent accounts.

use loams_agentd_proto as rs;
use loams_agentd_proto::invocation as rs_invocation;

use crate::proto::loams::agentd::v1 as pb;

wire_enum!(rs::SteeringMode => pb::SteeringMode {
    StepBoundary => STEERING_MODE_STEP_BOUNDARY,
    TurnBoundary => STEERING_MODE_TURN_BOUNDARY,
});

wire_struct!(rs::HarnessDescriptor => pb::HarnessDescriptor {
    id => id,
    name => name,
    supports_steering => supports_steering,
    steering_mode => steering_mode,
    reasoning_levels => reasoning_levels,
    installed => installed,
    can_install => can_install,
    enabled => enabled,
    marketplace_listing => marketplace_listing,
});

wire_struct!(rs::Model => pb::Model {
    id => id,
    label => label,
    description => description,
    reasoning_levels => reasoning_levels,
    options => options,
});

wire_struct!(rs::ModelOption => pb::ModelOption {
    id => id,
    label => label,
    choices => choices,
    default_choice => default_choice,
});

wire_struct!(rs::ModelOptionChoice => pb::ModelOptionChoice {
    id => id,
    label => label,
});

wire_struct!(rs_invocation::Skill => pb::Skill {
    name => name,
    path => path,
    description => description,
    enabled => enabled,
    command => command as opt_msg,
});

wire_struct!(rs_invocation::SkillCommand => pb::SkillCommand {
    name => name,
    harness => harness,
});

wire_struct!(rs::SlashCommand => pb::SlashCommand {
    name => name,
    description => description,
    input_hint => input_hint,
});

wire_enum!(rs::HarnessInstallSource => pb::HarnessInstallSource {
    Npm => HARNESS_INSTALL_SOURCE_NPM,
    Homebrew => HARNESS_INSTALL_SOURCE_HOMEBREW,
    Cargo => HARNESS_INSTALL_SOURCE_CARGO,
    Vendor => HARNESS_INSTALL_SOURCE_VENDOR,
    ManagedByLoamsDesktop => HARNESS_INSTALL_SOURCE_MANAGED_BY_LOAMS_DESKTOP,
    Unknown => HARNESS_INSTALL_SOURCE_UNKNOWN,
});

wire_enum!(rs::HarnessUpdatePhase => pb::HarnessUpdatePhase {
    Dormant => HARNESS_UPDATE_PHASE_DORMANT,
    Checking => HARNESS_UPDATE_PHASE_CHECKING,
    Current => HARNESS_UPDATE_PHASE_CURRENT,
    Available => HARNESS_UPDATE_PHASE_AVAILABLE,
    WaitingForIdle => HARNESS_UPDATE_PHASE_WAITING_FOR_IDLE,
    Preparing => HARNESS_UPDATE_PHASE_PREPARING,
    Downloading => HARNESS_UPDATE_PHASE_DOWNLOADING,
    Installing => HARNESS_UPDATE_PHASE_INSTALLING,
    Verifying => HARNESS_UPDATE_PHASE_VERIFYING,
    Updated => HARNESS_UPDATE_PHASE_UPDATED,
    ManualActionRequired => HARNESS_UPDATE_PHASE_MANUAL_ACTION_REQUIRED,
    Failed => HARNESS_UPDATE_PHASE_FAILED,
});

wire_struct!(rs::HarnessUpdateProgress => pb::HarnessUpdateProgress {
    completed_bytes => completed_bytes,
    total_bytes => total_bytes,
    message => message,
});

wire_struct!(rs::HarnessUpdateFailure => pb::HarnessUpdateFailure {
    message => message,
    retryable => retryable,
});

wire_struct!(rs::HarnessUpdateStatus => pb::HarnessUpdateStatus {
    harness => harness,
    installed_version => installed_version,
    latest_version => latest_version,
    channel => channel,
    source => source,
    phase => phase,
    progress => progress as opt_msg,
    checked_at => checked_at_ms,
    error => error as opt_msg,
    can_apply => can_apply,
    manual_command => manual_command,
});

wire_enum!(rs::AgentAuthKind => pb::AgentAuthKind {
    Oauth => AGENT_AUTH_KIND_OAUTH,
    ApiKey => AGENT_AUTH_KIND_API_KEY,
});

wire_struct!(rs::AgentUsageWindow => pb::AgentUsageWindow {
    label => label,
    used_fraction => used_fraction,
    resets_at => resets_at_ms,
});

wire_struct!(rs::AgentAccount => pb::AgentAccount {
    id => id,
    harness => harness,
    email => email,
    plan_label => plan_label,
    active => active,
    usage_windows => usage_windows,
    usage_fetched_at => usage_fetched_at_ms,
    usage_error => usage_error,
    display_name => display_name,
    organization => organization,
    auth_kind => auth_kind,
    switchable => switchable,
    saved_at => saved_at_ms,
    provider => provider,
});

wire_struct!(rs::AgentAccountWarning => pb::AgentAccountWarning {
    harness => harness,
    message => message,
});

wire_struct!(rs::AgentAccountsSnapshot => pb::AgentAccountsSnapshot {
    accounts => accounts,
    warnings => warnings,
});

wire_enum!(rs::AgentLoginMode => pb::AgentLoginMode {
    PasteCode => AGENT_LOGIN_MODE_PASTE_CODE,
    Browser => AGENT_LOGIN_MODE_BROWSER,
});

wire_struct!(rs::AgentLoginStart => pb::AgentLoginStart {
    login_id => login_id,
    url => url,
    mode => mode,
    callback_port => callback_port,
});

wire_enum!(rs::AgentLoginStatus => pb::AgentLoginStatus {
    Pending => AGENT_LOGIN_STATUS_PENDING,
    Done => AGENT_LOGIN_STATUS_DONE,
    Error => AGENT_LOGIN_STATUS_ERROR,
});

wire_struct!(rs::AgentLoginPoll => pb::AgentLoginPoll {
    status => status,
    message => message,
    url => url,
    callback_port => callback_port,
});

// Requests whose message is theirs alone.

wire_struct!(rs::ListModelsParams => pb::ListModelsRequest {
    harness => harness,
    force => force,
});

wire_struct!(rs::SetHarnessEnabledParams => pb::SetHarnessEnabledRequest {
    harness => harness,
    enabled => enabled,
});

wire_struct!(rs::HarnessUpdateParams => pb::CheckHarnessUpdatesRequest {
    harness => harness,
});

wire_struct!(rs::DismissHarnessUpdateParams => pb::DismissHarnessUpdateRequest {
    harness => harness,
    version => version,
});

wire_struct!(rs::ListAgentAccountsParams => pb::ListAgentAccountsRequest {
    force_usage => force_usage,
});

wire_struct!(rs::StartAgentLoginParams => pb::StartAgentLoginRequest {
    harness => harness,
    provider => provider,
});

wire_struct!(rs::CompleteAgentLoginParams => pb::CompleteAgentLoginRequest {
    login_id => login_id,
    code => code,
});
