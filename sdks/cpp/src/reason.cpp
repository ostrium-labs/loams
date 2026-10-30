// The reason registry, transcribed from `docs/api/reasons.md`.
//
// **Transcribed, not generated.** Design §44 §7.3 has `protoc-gen-loams-facade`
// read the registry page and render the reason table per language; that renderer
// has no C++ template (see `facade.hpp`'s header comment), so this is the
// hand-written equivalent and it says so rather than pretending.
//
// The authority is the page. `cpp_error_reason_mapping` walks this table and
// asserts that it has all fifty-two registry reasons, that every spelling is
// `snake_case`, and that the three distinct cases of R8 stay distinct — so a
// reason added to the page without being added here fails a test rather than
// being dropped.

#include "loams/reason.hpp"

#include <algorithm>
#include <cctype>
#include <ostream>

namespace loams {
namespace {

/// One row of the registry page, in the page's order.
struct Row {
  Reason reason;
  const char* name;
};

/// The registry, in `docs/api/reasons.md`'s order. The order is the page's
/// because it is the order a reader of either has to keep in their head.
constexpr Row kRows[] = {
    {Reason::kApprovalExpired, "approval_expired"},
    {Reason::kApprovalAlreadyDecided, "approval_already_decided"},
    {Reason::kApprovalStaleRevision, "approval_stale_revision"},
    {Reason::kRequesterCannotApprove, "requester_cannot_approve"},
    {Reason::kDecisionProofInvalid, "decision_proof_invalid"},
    {Reason::kStepUpRequired, "step_up_required"},
    {Reason::kReasonRequired, "reason_required"},
    {Reason::kInvalidDecision, "invalid_decision"},
    {Reason::kPairingExpired, "pairing_expired"},
    {Reason::kPairingUsed, "pairing_used"},
    {Reason::kDeviceRevoked, "device_revoked"},
    {Reason::kPushTargetUnknown, "push_target_unknown"},
    {Reason::kNotImplemented, "not_implemented"},
    {Reason::kFeatureNotInVariant, "feature_not_in_variant"},
    {Reason::kHouseNotConfigured, "house_not_configured"},
    {Reason::kHouseSyntaxError, "house_syntax_error"},
    {Reason::kHouseUnknownTable, "house_unknown_table"},
    {Reason::kHouseUnknownDatabase, "house_unknown_database"},
    {Reason::kHouseAccessDenied, "house_access_denied"},
    {Reason::kHouseReadOnly, "house_read_only"},
    {Reason::kHouseTimeout, "house_timeout"},
    {Reason::kHouseMemoryLimit, "house_memory_limit"},
    {Reason::kHouseTooManyQueries, "house_too_many_queries"},
    {Reason::kHouseQuotaExceeded, "house_quota_exceeded"},
    {Reason::kHouseDisabledFunction, "house_disabled_function"},
    {Reason::kHouseCancelled, "house_cancelled"},
    {Reason::kHouseQueryNotFound, "house_query_not_found"},
    {Reason::kHouseQueryFailed, "house_query_failed"},
    {Reason::kProjectNotFound, "project_not_found"},
    {Reason::kBranchHasChildren, "branch_has_children"},
    {Reason::kBranchProtected, "branch_protected"},
    {Reason::kLsnOutOfRetention, "lsn_out_of_retention"},
    {Reason::kEndpointExistsForBranch, "endpoint_exists_for_branch"},
    {Reason::kComputeStartFailed, "compute_start_failed"},
    {Reason::kUnsupportedPgVersion, "unsupported_pg_version"},
    {Reason::kBetaMajorRequiresOptIn, "beta_major_requires_opt_in"},
    {Reason::kComputeMajorMismatchRefused, "compute_major_mismatch_refused"},
    {Reason::kImportChecksumsEnabled, "import_checksums_enabled"},
    {Reason::kQuotaExceeded, "quota_exceeded"},
    {Reason::kStorageUnavailable, "storage_unavailable"},
    {Reason::kSecretAlreadyIssued, "secret_already_issued"},
    {Reason::kRepoNotFound, "repo_not_found"},
    {Reason::kRepoAlreadyExists, "repo_already_exists"},
    {Reason::kRepoDeleting, "repo_deleting"},
    {Reason::kRefStale, "ref_stale"},
    {Reason::kRefProtected, "ref_protected"},
    {Reason::kOutOfScope, "out_of_scope"},
    {Reason::kObjectFormatUnsupported, "object_format_unsupported"},
    {Reason::kForkTooDeep, "fork_too_deep"},
    {Reason::kSequencerUnavailable, "sequencer_unavailable"},
    {Reason::kIdempotencyKeyReused, "idempotency_key_reused"},
    {Reason::kWorkspaceNotFound, "workspace_not_found"},
    {Reason::kWorkspaceFenced, "workspace_fenced"},
    {Reason::kBatchPartiallyApplied, "batch_partially_applied"},
    {Reason::kWorkspaceReadOnly, "workspace_read_only"},
    {Reason::kWorkspaceNameTaken, "workspace_name_taken"},
    {Reason::kObjectMissing, "object_missing"},
    {Reason::kSecretDetected, "secret_detected"},
    {Reason::kEnvUnsealed, "env_unsealed"},
    {Reason::kEnvAccessDenied, "env_access_denied"},
    {Reason::kEnvForeignReference, "env_foreign_reference"},
    {Reason::kMemoryStoreNotFound, "memory_store_not_found"},
    {Reason::kMemorySpaceNotFound, "memory_space_not_found"},
    {Reason::kMemorySpaceLimit, "memory_space_limit"},
    {Reason::kMemoryManagedObject, "memory_managed_object"},
    {Reason::kMemoryBlockVersionMismatch, "memory_block_version_mismatch"},
    {Reason::kMemoryBlockTooLarge, "memory_block_too_large"},
    {Reason::kMemoryCoreBudgetExceeded, "memory_core_budget_exceeded"},
    {Reason::kMemoryContentTooLarge, "memory_content_too_large"},
    {Reason::kMemorySecretDetected, "memory_secret_detected"},
    {Reason::kMemoryPiiRefused, "memory_pii_refused"},
    {Reason::kMemoryModelNotConfigured, "memory_model_not_configured"},
    {Reason::kMemoryModelNotLocal, "memory_model_not_local"},
    {Reason::kMemoryTokenBudgetExhausted, "memory_token_budget_exhausted"},
    {Reason::kMemoryExtractionFailed, "memory_extraction_failed"},
    {Reason::kMemoryTooManySpaces, "memory_too_many_spaces"},
    {Reason::kMemoryCrossSpaceWrite, "memory_cross_space_write"},
    {Reason::kMemoryNotYetAvailable, "memory_not_yet_available"},
    {Reason::kCodeRepoNotIndexed, "code_repo_not_indexed"},
    {Reason::kCodeIndexBuilding, "code_index_building"},
    {Reason::kCodeChangeTooLarge, "code_change_too_large"},
    {Reason::kCodeLanguageUnsupported, "code_language_unsupported"},
    {Reason::kCodeScipInvalid, "code_scip_invalid"},
    {Reason::kLiveBadCursor, "live_bad_cursor"},
    {Reason::kLiveWorkerCrashed, "live_worker_crashed"},
    {Reason::kLiveDeployConflict, "live_deploy_conflict"},
    {Reason::kLiveSchemaViolation, "live_schema_violation"},
    {Reason::kFilterConflict, "filter_conflict"},
    {Reason::kSearchAfterNeedsTiebreak, "search_after_needs_tiebreak"},
    {Reason::kInvalidConsistencyToken, "invalid_consistency_token"},
    {Reason::kNamespaceRequired, "namespace_required"},
    {Reason::kSqlParamCount, "sql_param_count"},
    {Reason::kLiveIndexNotReady, "live_index_not_ready"},
    {Reason::kLiveAppDisabled, "live_app_disabled"},
    {Reason::kLiveAppExists, "live_app_exists"},
    {Reason::kLiveFunctionError, "live_function_error"},
    {Reason::kLiveFunctionTimeout, "live_function_timeout"},
    {Reason::kClientDeadlineExceeded, "client_deadline_exceeded"},
    {Reason::kLiveFunctionOutOfMemory, "live_function_out_of_memory"},
    {Reason::kLiveLimitExceeded, "live_limit_exceeded"},
    {Reason::kResultTooLarge, "result_too_large"},
    {Reason::kLiveAppMismatch, "live_app_mismatch"},
    {Reason::kLiveOriginNotAllowed, "live_origin_not_allowed"},
    {Reason::kSessionRotate, "session_rotate"},
    {Reason::kSessionGone, "session_gone"},
    {Reason::kNodeDraining, "node_draining"},
    {Reason::kRoleRequired, "role_required"},
    {Reason::kSetupRequired, "setup_required"},
    {Reason::kTokenMissing, "token_missing"},
    {Reason::kTokenMalformed, "token_malformed"},
    {Reason::kTokenRevoked, "token_revoked"},
    {Reason::kTokenNotYetValid, "token_not_yet_valid"},
    {Reason::kTokenBadSignature, "token_bad_signature"},
    {Reason::kTokenWrongAudience, "token_wrong_audience"},
    {Reason::kTokenWrongIssuer, "token_wrong_issuer"},
    {Reason::kTokenUnknownKey, "token_unknown_key"},
    {Reason::kTokenAlgorithmNotAllowed, "token_algorithm_not_allowed"},
    {Reason::kApiKeyInvalid, "api_key_invalid"},
    {Reason::kCredentialNotAccepted, "credential_not_accepted"},
    {Reason::kCredentialInQueryString, "credential_in_query_string"},
    {Reason::kCsrfTokenInvalid, "csrf_token_invalid"},
    {Reason::kPrincipalNotAllowed, "principal_not_allowed"},
    {Reason::kCorsOriginNotAllowed, "cors_origin_not_allowed"},
    {Reason::kAuthUnavailable, "auth_unavailable"},
    {Reason::kInvalidArgument, "invalid_argument"},
    {Reason::kNotFound, "not_found"},
    {Reason::kAlreadyExists, "already_exists"},
    {Reason::kPermissionDenied, "permission_denied"},
    {Reason::kTokenExpired, "token_expired"},
    {Reason::kUnauthenticated, "unauthenticated"},
    {Reason::kFailedPrecondition, "failed_precondition"},
    {Reason::kResourceExhausted, "resource_exhausted"},
    {Reason::kUnavailable, "unavailable"},
    {Reason::kDeadlineExceeded, "deadline_exceeded"},
    {Reason::kAborted, "aborted"},
    {Reason::kInternal, "internal"},
};

}  // namespace

std::string_view ToString(Reason reason) {
  for (const Row& row : kRows) {
    if (row.reason == reason) {
      return row.name;
    }
  }
  // `kUnknown` maps to the empty string, not to `"unknown"`: `"unknown"` is not
  // a reason the registry has, and a caller comparing strings must not be
  // offered a value that could also arrive from a newer server.
  return reason == Reason::kUnknown ? std::string_view() : std::string_view("<unknown reason>");
}

std::optional<Reason> ReasonFromString(std::string_view name) {
  for (const Row& row : kRows) {
    if (name == row.name) {
      return row.reason;
    }
  }
  return std::nullopt;
}

std::ostream& operator<<(std::ostream& out, Reason reason) { return out << ToString(reason); }

const std::vector<Reason>& AllReasons() {
  static const std::vector<Reason>* const reasons = [] {
    auto* const built = new std::vector<Reason>();
    built->reserve(std::size(kRows));
    for (const Row& row : kRows) {
      built->push_back(row.reason);
    }
    return built;
  }();
  return *reasons;
}

bool IsRegistryReason(Reason reason) { return ReasonFromString(ToString(reason)).has_value(); }

}  // namespace loams