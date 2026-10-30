package facade

import "sort"

// Reason is a stable, machine-readable cause of a failed RPC
// (design §44 §7.4, D611). It is generated from `docs/api/reasons.md`, so a
// caller switching on it is exhaustive over the registry this SDK was built
// against, and a reason the registry has lost stops compiling.
//
// It is a distinct string type rather than `string` on purpose: that is what
// makes `errors.As`-free exhaustiveness possible, and it is why the wire value
// has to go through `KnownReason` before it becomes a `Reason`.
type Reason string

// Every reason in the registry. Transcribed from `docs/api/reasons.md`; the test
// `TestReasonRegistryMatchesDocs` reads that page and fails on a drift, so this
// list cannot silently fall behind.
const (
	ReasonApprovalExpired             Reason = "approval_expired"
	ReasonApprovalAlreadyDecided      Reason = "approval_already_decided"
	ReasonApprovalStaleRevision       Reason = "approval_stale_revision"
	ReasonRequesterCannotApprove      Reason = "requester_cannot_approve"
	ReasonDecisionProofInvalid        Reason = "decision_proof_invalid"
	ReasonStepUpRequired              Reason = "step_up_required"
	ReasonReasonRequired              Reason = "reason_required"
	ReasonInvalidDecision             Reason = "invalid_decision"
	ReasonPairingExpired              Reason = "pairing_expired"
	ReasonPairingUsed                 Reason = "pairing_used"
	ReasonDeviceRevoked               Reason = "device_revoked"
	ReasonPushTargetUnknown           Reason = "push_target_unknown"
	ReasonNotImplemented              Reason = "not_implemented"
	ReasonFeatureNotInVariant         Reason = "feature_not_in_variant"
	ReasonHouseNotConfigured          Reason = "house_not_configured"
	ReasonHouseSyntaxError            Reason = "house_syntax_error"
	ReasonHouseUnknownTable           Reason = "house_unknown_table"
	ReasonHouseUnknownDatabase        Reason = "house_unknown_database"
	ReasonHouseAccessDenied           Reason = "house_access_denied"
	ReasonHouseReadOnly               Reason = "house_read_only"
	ReasonHouseTimeout                Reason = "house_timeout"
	ReasonHouseMemoryLimit            Reason = "house_memory_limit"
	ReasonHouseTooManyQueries         Reason = "house_too_many_queries"
	ReasonHouseQuotaExceeded          Reason = "house_quota_exceeded"
	ReasonHouseDisabledFunction       Reason = "house_disabled_function"
	ReasonHouseCancelled              Reason = "house_cancelled"
	ReasonHouseQueryNotFound          Reason = "house_query_not_found"
	ReasonHouseQueryFailed            Reason = "house_query_failed"
	ReasonProjectNotFound             Reason = "project_not_found"
	ReasonBranchHasChildren           Reason = "branch_has_children"
	ReasonBranchProtected             Reason = "branch_protected"
	ReasonLsnOutOfRetention           Reason = "lsn_out_of_retention"
	ReasonEndpointExistsForBranch     Reason = "endpoint_exists_for_branch"
	ReasonComputeStartFailed          Reason = "compute_start_failed"
	ReasonUnsupportedPgVersion        Reason = "unsupported_pg_version"
	ReasonBetaMajorRequiresOptIn      Reason = "beta_major_requires_opt_in"
	ReasonComputeMajorMismatchRefused Reason = "compute_major_mismatch_refused"
	ReasonImportChecksumsEnabled      Reason = "import_checksums_enabled"
	ReasonQuotaExceeded               Reason = "quota_exceeded"
	ReasonStorageUnavailable          Reason = "storage_unavailable"
	ReasonSecretAlreadyIssued         Reason = "secret_already_issued"
	ReasonRepoNotFound                Reason = "repo_not_found"
	ReasonRepoAlreadyExists           Reason = "repo_already_exists"
	ReasonRepoDeleting                Reason = "repo_deleting"
	ReasonRefStale                    Reason = "ref_stale"
	ReasonRefProtected                Reason = "ref_protected"
	ReasonOutOfScope                  Reason = "out_of_scope"
	ReasonObjectFormatUnsupported     Reason = "object_format_unsupported"
	ReasonForkTooDeep                 Reason = "fork_too_deep"
	ReasonSequencerUnavailable        Reason = "sequencer_unavailable"
	ReasonIdempotencyKeyReused        Reason = "idempotency_key_reused"
	ReasonWorkspaceNotFound           Reason = "workspace_not_found"
	ReasonWorkspaceFenced             Reason = "workspace_fenced"
	ReasonBatchPartiallyApplied       Reason = "batch_partially_applied"
	ReasonWorkspaceReadOnly           Reason = "workspace_read_only"
	ReasonWorkspaceNameTaken          Reason = "workspace_name_taken"
	ReasonObjectMissing               Reason = "object_missing"
	ReasonSecretDetected              Reason = "secret_detected"
	ReasonEnvUnsealed                 Reason = "env_unsealed"
	ReasonEnvAccessDenied             Reason = "env_access_denied"
	ReasonEnvForeignReference         Reason = "env_foreign_reference"
	ReasonMemoryStoreNotFound         Reason = "memory_store_not_found"
	ReasonMemorySpaceNotFound         Reason = "memory_space_not_found"
	ReasonMemorySpaceLimit            Reason = "memory_space_limit"
	ReasonMemoryManagedObject         Reason = "memory_managed_object"
	ReasonMemoryBlockVersionMismatch  Reason = "memory_block_version_mismatch"
	ReasonMemoryBlockTooLarge         Reason = "memory_block_too_large"
	ReasonMemoryCoreBudgetExceeded    Reason = "memory_core_budget_exceeded"
	ReasonMemoryContentTooLarge       Reason = "memory_content_too_large"
	ReasonMemorySecretDetected        Reason = "memory_secret_detected"
	ReasonMemoryPiiRefused            Reason = "memory_pii_refused"
	ReasonMemoryModelNotConfigured    Reason = "memory_model_not_configured"
	ReasonMemoryModelNotLocal         Reason = "memory_model_not_local"
	ReasonMemoryTokenBudgetExhausted  Reason = "memory_token_budget_exhausted"
	ReasonMemoryExtractionFailed      Reason = "memory_extraction_failed"
	ReasonMemoryTooManySpaces         Reason = "memory_too_many_spaces"
	ReasonMemoryCrossSpaceWrite       Reason = "memory_cross_space_write"
	ReasonMemoryNotYetAvailable       Reason = "memory_not_yet_available"
	ReasonCodeRepoNotIndexed          Reason = "code_repo_not_indexed"
	ReasonCodeIndexBuilding           Reason = "code_index_building"
	ReasonCodeChangeTooLarge          Reason = "code_change_too_large"
	ReasonCodeLanguageUnsupported     Reason = "code_language_unsupported"
	ReasonCodeScipInvalid             Reason = "code_scip_invalid"
	ReasonLiveBadCursor               Reason = "live_bad_cursor"
	ReasonLiveWorkerCrashed           Reason = "live_worker_crashed"
	ReasonLiveDeployConflict          Reason = "live_deploy_conflict"
	ReasonLiveSchemaViolation         Reason = "live_schema_violation"
	ReasonFilterConflict              Reason = "filter_conflict"
	ReasonSearchAfterNeedsTiebreak    Reason = "search_after_needs_tiebreak"
	ReasonInvalidConsistencyToken     Reason = "invalid_consistency_token"
	ReasonNamespaceRequired           Reason = "namespace_required"
	ReasonSqlParamCount               Reason = "sql_param_count"
	ReasonLiveIndexNotReady           Reason = "live_index_not_ready"
	ReasonLiveAppDisabled             Reason = "live_app_disabled"
	ReasonLiveAppExists               Reason = "live_app_exists"
	ReasonLiveFunctionError           Reason = "live_function_error"
	ReasonLiveFunctionTimeout         Reason = "live_function_timeout"
	ReasonClientDeadlineExceeded      Reason = "client_deadline_exceeded"
	ReasonLiveFunctionOutOfMemory     Reason = "live_function_out_of_memory"
	ReasonLiveLimitExceeded           Reason = "live_limit_exceeded"
	ReasonResultTooLarge              Reason = "result_too_large"
	ReasonLiveAppMismatch             Reason = "live_app_mismatch"
	ReasonLiveOriginNotAllowed        Reason = "live_origin_not_allowed"
	ReasonSessionRotate               Reason = "session_rotate"
	ReasonSessionGone                 Reason = "session_gone"
	ReasonNodeDraining                Reason = "node_draining"
	ReasonRoleRequired                Reason = "role_required"
	ReasonSetupRequired               Reason = "setup_required"
	ReasonTokenMissing                Reason = "token_missing"
	ReasonTokenMalformed              Reason = "token_malformed"
	ReasonTokenRevoked                Reason = "token_revoked"
	ReasonTokenNotYetValid            Reason = "token_not_yet_valid"
	ReasonTokenBadSignature           Reason = "token_bad_signature"
	ReasonTokenWrongAudience          Reason = "token_wrong_audience"
	ReasonTokenWrongIssuer            Reason = "token_wrong_issuer"
	ReasonTokenUnknownKey             Reason = "token_unknown_key"
	ReasonTokenAlgorithmNotAllowed    Reason = "token_algorithm_not_allowed"
	ReasonApiKeyInvalid               Reason = "api_key_invalid"
	ReasonCredentialNotAccepted       Reason = "credential_not_accepted"
	ReasonCredentialInQueryString     Reason = "credential_in_query_string"
	ReasonCsrfTokenInvalid            Reason = "csrf_token_invalid"
	ReasonPrincipalNotAllowed         Reason = "principal_not_allowed"
	ReasonCorsOriginNotAllowed        Reason = "cors_origin_not_allowed"
	ReasonAuthUnavailable             Reason = "auth_unavailable"
	ReasonJoinTokenUnknown            Reason = "join_token_unknown"
	ReasonJoinTokenSpent              Reason = "join_token_spent"
	ReasonJoinTokenExpired            Reason = "join_token_expired"
	ReasonIdentityNotGranted          Reason = "identity_not_granted"
	ReasonIdentityRevoked             Reason = "identity_revoked"
	ReasonCaNotInitialized            Reason = "ca_not_initialized"
	ReasonInvalidArgument             Reason = "invalid_argument"
	ReasonNotFound                    Reason = "not_found"
	ReasonAlreadyExists               Reason = "already_exists"
	ReasonPermissionDenied            Reason = "permission_denied"
	ReasonTokenExpired                Reason = "token_expired"
	ReasonUnauthenticated             Reason = "unauthenticated"
	ReasonFailedPrecondition          Reason = "failed_precondition"
	ReasonResourceExhausted           Reason = "resource_exhausted"
	ReasonUnavailable                 Reason = "unavailable"
	ReasonDeadlineExceeded            Reason = "deadline_exceeded"
	ReasonAborted                     Reason = "aborted"
	ReasonInternal                    Reason = "internal"
)

// FeatureNotInVariant is the reason a package the build variant does not carry
// answers with (design §44 §4, D600). Every RPC of such a package refuses with
// it and names the variant in `metadata.variant`; the runtime turns that refusal
// into `loams.FeatureNotInVariantError`, so a caller can branch on the type —
// or on this reason — without having to call and read the message.
const FeatureNotInVariant = ReasonFeatureNotInVariant

// Reasons is every reason in the registry, in the registry's order.
var Reasons = []Reason{
	ReasonApprovalExpired,
	ReasonApprovalAlreadyDecided,
	ReasonApprovalStaleRevision,
	ReasonRequesterCannotApprove,
	ReasonDecisionProofInvalid,
	ReasonStepUpRequired,
	ReasonReasonRequired,
	ReasonInvalidDecision,
	ReasonPairingExpired,
	ReasonPairingUsed,
	ReasonDeviceRevoked,
	ReasonPushTargetUnknown,
	ReasonNotImplemented,
	ReasonFeatureNotInVariant,
	ReasonHouseNotConfigured,
	ReasonHouseSyntaxError,
	ReasonHouseUnknownTable,
	ReasonHouseUnknownDatabase,
	ReasonHouseAccessDenied,
	ReasonHouseReadOnly,
	ReasonHouseTimeout,
	ReasonHouseMemoryLimit,
	ReasonHouseTooManyQueries,
	ReasonHouseQuotaExceeded,
	ReasonHouseDisabledFunction,
	ReasonHouseCancelled,
	ReasonHouseQueryNotFound,
	ReasonHouseQueryFailed,
	ReasonProjectNotFound,
	ReasonBranchHasChildren,
	ReasonBranchProtected,
	ReasonLsnOutOfRetention,
	ReasonEndpointExistsForBranch,
	ReasonComputeStartFailed,
	ReasonUnsupportedPgVersion,
	ReasonBetaMajorRequiresOptIn,
	ReasonComputeMajorMismatchRefused,
	ReasonImportChecksumsEnabled,
	ReasonQuotaExceeded,
	ReasonStorageUnavailable,
	ReasonSecretAlreadyIssued,
	ReasonRepoNotFound,
	ReasonRepoAlreadyExists,
	ReasonRepoDeleting,
	ReasonRefStale,
	ReasonRefProtected,
	ReasonOutOfScope,
	ReasonObjectFormatUnsupported,
	ReasonForkTooDeep,
	ReasonSequencerUnavailable,
	ReasonIdempotencyKeyReused,
	ReasonWorkspaceNotFound,
	ReasonWorkspaceFenced,
	ReasonBatchPartiallyApplied,
	ReasonWorkspaceReadOnly,
	ReasonWorkspaceNameTaken,
	ReasonObjectMissing,
	ReasonSecretDetected,
	ReasonEnvUnsealed,
	ReasonEnvAccessDenied,
	ReasonEnvForeignReference,
	ReasonMemoryStoreNotFound,
	ReasonMemorySpaceNotFound,
	ReasonMemorySpaceLimit,
	ReasonMemoryManagedObject,
	ReasonMemoryBlockVersionMismatch,
	ReasonMemoryBlockTooLarge,
	ReasonMemoryCoreBudgetExceeded,
	ReasonMemoryContentTooLarge,
	ReasonMemorySecretDetected,
	ReasonMemoryPiiRefused,
	ReasonMemoryModelNotConfigured,
	ReasonMemoryModelNotLocal,
	ReasonMemoryTokenBudgetExhausted,
	ReasonMemoryExtractionFailed,
	ReasonMemoryTooManySpaces,
	ReasonMemoryCrossSpaceWrite,
	ReasonMemoryNotYetAvailable,
	ReasonCodeRepoNotIndexed,
	ReasonCodeIndexBuilding,
	ReasonCodeChangeTooLarge,
	ReasonCodeLanguageUnsupported,
	ReasonCodeScipInvalid,
	ReasonLiveBadCursor,
	ReasonLiveWorkerCrashed,
	ReasonLiveDeployConflict,
	ReasonLiveSchemaViolation,
	ReasonFilterConflict,
	ReasonSearchAfterNeedsTiebreak,
	ReasonInvalidConsistencyToken,
	ReasonNamespaceRequired,
	ReasonSqlParamCount,
	ReasonLiveIndexNotReady,
	ReasonLiveAppDisabled,
	ReasonLiveAppExists,
	ReasonLiveFunctionError,
	ReasonLiveFunctionTimeout,
	ReasonClientDeadlineExceeded,
	ReasonLiveFunctionOutOfMemory,
	ReasonLiveLimitExceeded,
	ReasonResultTooLarge,
	ReasonLiveAppMismatch,
	ReasonLiveOriginNotAllowed,
	ReasonSessionRotate,
	ReasonSessionGone,
	ReasonNodeDraining,
	ReasonRoleRequired,
	ReasonSetupRequired,
	ReasonTokenMissing,
	ReasonTokenMalformed,
	ReasonTokenRevoked,
	ReasonTokenNotYetValid,
	ReasonTokenBadSignature,
	ReasonTokenWrongAudience,
	ReasonTokenWrongIssuer,
	ReasonTokenUnknownKey,
	ReasonTokenAlgorithmNotAllowed,
	ReasonApiKeyInvalid,
	ReasonCredentialNotAccepted,
	ReasonCredentialInQueryString,
	ReasonCsrfTokenInvalid,
	ReasonPrincipalNotAllowed,
	ReasonCorsOriginNotAllowed,
	ReasonAuthUnavailable,
	ReasonJoinTokenUnknown,
	ReasonJoinTokenSpent,
	ReasonJoinTokenExpired,
	ReasonIdentityNotGranted,
	ReasonIdentityRevoked,
	ReasonCaNotInitialized,
	ReasonInvalidArgument,
	ReasonNotFound,
	ReasonAlreadyExists,
	ReasonPermissionDenied,
	ReasonTokenExpired,
	ReasonUnauthenticated,
	ReasonFailedPrecondition,
	ReasonResourceExhausted,
	ReasonUnavailable,
	ReasonDeadlineExceeded,
	ReasonAborted,
	ReasonInternal,
}

// ReasonCodes is the Connect code each reason is raised under, from the same
// registry. It is a `connect.Code`, which is the SDK's `loams.Code`, so a
// caller that wants the coarse class has it without string parsing.
var ReasonCodes = map[Reason]string{
	ReasonApprovalExpired:             "failed_precondition",
	ReasonApprovalAlreadyDecided:      "failed_precondition",
	ReasonApprovalStaleRevision:       "failed_precondition",
	ReasonRequesterCannotApprove:      "permission_denied",
	ReasonDecisionProofInvalid:        "permission_denied",
	ReasonStepUpRequired:              "unauthenticated",
	ReasonReasonRequired:              "invalid_argument",
	ReasonInvalidDecision:             "invalid_argument",
	ReasonPairingExpired:              "failed_precondition",
	ReasonPairingUsed:                 "failed_precondition",
	ReasonDeviceRevoked:               "unauthenticated",
	ReasonPushTargetUnknown:           "not_found",
	ReasonNotImplemented:              "unimplemented",
	ReasonFeatureNotInVariant:         "unimplemented",
	ReasonHouseNotConfigured:          "unimplemented",
	ReasonHouseSyntaxError:            "invalid_argument",
	ReasonHouseUnknownTable:           "not_found",
	ReasonHouseUnknownDatabase:        "not_found",
	ReasonHouseAccessDenied:           "permission_denied",
	ReasonHouseReadOnly:               "failed_precondition",
	ReasonHouseTimeout:                "deadline_exceeded",
	ReasonHouseMemoryLimit:            "resource_exhausted",
	ReasonHouseTooManyQueries:         "resource_exhausted",
	ReasonHouseQuotaExceeded:          "resource_exhausted",
	ReasonHouseDisabledFunction:       "permission_denied",
	ReasonHouseCancelled:              "canceled",
	ReasonHouseQueryNotFound:          "not_found",
	ReasonHouseQueryFailed:            "unknown",
	ReasonProjectNotFound:             "not_found",
	ReasonBranchHasChildren:           "failed_precondition",
	ReasonBranchProtected:             "failed_precondition",
	ReasonLsnOutOfRetention:           "failed_precondition",
	ReasonEndpointExistsForBranch:     "already_exists",
	ReasonComputeStartFailed:          "unavailable",
	ReasonUnsupportedPgVersion:        "invalid_argument",
	ReasonBetaMajorRequiresOptIn:      "failed_precondition",
	ReasonComputeMajorMismatchRefused: "failed_precondition",
	ReasonImportChecksumsEnabled:      "failed_precondition",
	ReasonQuotaExceeded:               "resource_exhausted",
	ReasonStorageUnavailable:          "unavailable",
	ReasonSecretAlreadyIssued:         "failed_precondition",
	ReasonRepoNotFound:                "not_found",
	ReasonRepoAlreadyExists:           "already_exists",
	ReasonRepoDeleting:                "failed_precondition",
	ReasonRefStale:                    "aborted",
	ReasonRefProtected:                "failed_precondition",
	ReasonOutOfScope:                  "permission_denied",
	ReasonObjectFormatUnsupported:     "unimplemented",
	ReasonForkTooDeep:                 "failed_precondition",
	ReasonSequencerUnavailable:        "unavailable",
	ReasonIdempotencyKeyReused:        "failed_precondition",
	ReasonWorkspaceNotFound:           "not_found",
	ReasonWorkspaceFenced:             "aborted",
	ReasonBatchPartiallyApplied:       "aborted",
	ReasonWorkspaceReadOnly:           "failed_precondition",
	ReasonWorkspaceNameTaken:          "already_exists",
	ReasonObjectMissing:               "failed_precondition",
	ReasonSecretDetected:              "failed_precondition",
	ReasonEnvUnsealed:                 "invalid_argument",
	ReasonEnvAccessDenied:             "permission_denied",
	ReasonEnvForeignReference:         "permission_denied",
	ReasonMemoryStoreNotFound:         "not_found",
	ReasonMemorySpaceNotFound:         "not_found",
	ReasonMemorySpaceLimit:            "resource_exhausted",
	ReasonMemoryManagedObject:         "failed_precondition",
	ReasonMemoryBlockVersionMismatch:  "aborted",
	ReasonMemoryBlockTooLarge:         "invalid_argument",
	ReasonMemoryCoreBudgetExceeded:    "resource_exhausted",
	ReasonMemoryContentTooLarge:       "invalid_argument",
	ReasonMemorySecretDetected:        "failed_precondition",
	ReasonMemoryPiiRefused:            "failed_precondition",
	ReasonMemoryModelNotConfigured:    "failed_precondition",
	ReasonMemoryModelNotLocal:         "failed_precondition",
	ReasonMemoryTokenBudgetExhausted:  "resource_exhausted",
	ReasonMemoryExtractionFailed:      "failed_precondition",
	ReasonMemoryTooManySpaces:         "invalid_argument",
	ReasonMemoryCrossSpaceWrite:       "invalid_argument",
	ReasonMemoryNotYetAvailable:       "unimplemented",
	ReasonCodeRepoNotIndexed:          "failed_precondition",
	ReasonCodeIndexBuilding:           "unavailable",
	ReasonCodeChangeTooLarge:          "invalid_argument",
	ReasonCodeLanguageUnsupported:     "failed_precondition",
	ReasonCodeScipInvalid:             "invalid_argument",
	ReasonLiveBadCursor:               "invalid_argument",
	ReasonLiveWorkerCrashed:           "internal",
	ReasonLiveDeployConflict:          "failed_precondition",
	ReasonLiveSchemaViolation:         "invalid_argument",
	ReasonFilterConflict:              "invalid_argument",
	ReasonSearchAfterNeedsTiebreak:    "invalid_argument",
	ReasonInvalidConsistencyToken:     "invalid_argument",
	ReasonNamespaceRequired:           "invalid_argument",
	ReasonSqlParamCount:               "invalid_argument",
	ReasonLiveIndexNotReady:           "failed_precondition",
	ReasonLiveAppDisabled:             "failed_precondition",
	ReasonLiveAppExists:               "failed_precondition",
	ReasonLiveFunctionError:           "unknown",
	ReasonLiveFunctionTimeout:         "deadline_exceeded",
	ReasonClientDeadlineExceeded:      "deadline_exceeded",
	ReasonLiveFunctionOutOfMemory:     "resource_exhausted",
	ReasonLiveLimitExceeded:           "resource_exhausted",
	ReasonResultTooLarge:              "resource_exhausted",
	ReasonLiveAppMismatch:             "permission_denied",
	ReasonLiveOriginNotAllowed:        "permission_denied",
	ReasonSessionRotate:               "unavailable",
	ReasonSessionGone:                 "not_found",
	ReasonNodeDraining:                "unavailable",
	ReasonRoleRequired:                "permission_denied",
	ReasonSetupRequired:               "failed_precondition",
	ReasonTokenMissing:                "unauthenticated",
	ReasonTokenMalformed:              "unauthenticated",
	ReasonTokenRevoked:                "unauthenticated",
	ReasonTokenNotYetValid:            "unauthenticated",
	ReasonTokenBadSignature:           "unauthenticated",
	ReasonTokenWrongAudience:          "unauthenticated",
	ReasonTokenWrongIssuer:            "unauthenticated",
	ReasonTokenUnknownKey:             "unauthenticated",
	ReasonTokenAlgorithmNotAllowed:    "unauthenticated",
	ReasonApiKeyInvalid:               "unauthenticated",
	ReasonCredentialNotAccepted:       "unauthenticated",
	ReasonCredentialInQueryString:     "unauthenticated",
	ReasonCsrfTokenInvalid:            "permission_denied",
	ReasonPrincipalNotAllowed:         "permission_denied",
	ReasonCorsOriginNotAllowed:        "permission_denied",
	ReasonAuthUnavailable:             "unavailable",
	ReasonJoinTokenUnknown:            "unauthenticated",
	ReasonJoinTokenSpent:              "unauthenticated",
	ReasonJoinTokenExpired:            "unauthenticated",
	ReasonIdentityNotGranted:          "permission_denied",
	ReasonIdentityRevoked:             "permission_denied",
	ReasonCaNotInitialized:            "failed_precondition",
	ReasonInvalidArgument:             "invalid_argument",
	ReasonNotFound:                    "not_found",
	ReasonAlreadyExists:               "already_exists",
	ReasonPermissionDenied:            "permission_denied",
	ReasonTokenExpired:                "unauthenticated",
	ReasonUnauthenticated:             "unauthenticated",
	ReasonFailedPrecondition:          "failed_precondition",
	ReasonResourceExhausted:           "resource_exhausted",
	ReasonUnavailable:                 "unavailable",
	ReasonDeadlineExceeded:            "deadline_exceeded",
	ReasonAborted:                     "aborted",
	ReasonInternal:                    "internal",
}

var reasonSet = func() map[Reason]struct{} {
	set := make(map[Reason]struct{}, len(Reasons))
	for _, reason := range Reasons {
		set[reason] = struct{}{}
	}
	return set
}()

// KnownReason turns a wire string into a Reason. The second result is false for
// a reason this SDK's registry does not have — a server newer than this SDK —
// which the runtime surfaces rather than drops (design §44 §7.4, R8: losing it
// would leave a caller unable to tell "not supported here" from "not supported
// at all").
func KnownReason(value string) (Reason, bool) {
	reason := Reason(value)
	_, known := reasonSet[reason]
	return reason, known
}

// IsKnownReason whether a reason is in this SDK's registry.
func IsKnownReason(reason Reason) bool {
	_, known := reasonSet[reason]
	return known
}

// Binding finds the binding a module and call name identify. `call` is the Go
// method name (`GetInstance`), which is what the module methods are named after.
func Binding(module, call string) (CallBinding, bool) {
	for _, entry := range Modules {
		if entry.Name != module {
			continue
		}
		for _, candidate := range entry.Calls {
			if candidate.Name == call || candidate.ProtoName == call {
				return candidate, true
			}
		}
	}
	return CallBinding{}, false
}

// Module finds a module binding by name.
func Module(name string) (ModuleBinding, bool) {
	for _, entry := range Modules {
		if entry.Name == name {
			return entry, true
		}
	}
	return ModuleBinding{}, false
}

// PackageOf answers which module owns a proto package. `loams.live` and
// `loams.tables` are two names for one package, so the first module in
// registration order that claims it wins; use `Modules` when the whole list of
// facade names matters.
func PackageOf(pkg string) (string, bool) {
	for _, entry := range Modules {
		if entry.Package == pkg {
			return entry.Name, true
		}
	}
	return "", false
}

// ModulesForPackage is every module name that is a facade for one proto
// package. It is what `loams.System().Guard` consults, because a guard on
// "live" must also cover "tables" and a refusal on either means the engine is
// absent.
func ModulesForPackage(pkg string) []string {
	var names []string
	for _, entry := range Modules {
		if entry.Package == pkg {
			names = append(names, entry.Name)
		}
	}
	sort.Strings(names)
	return names
}
