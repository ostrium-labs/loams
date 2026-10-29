package dev.loams

/**
 * The `ErrorInfo.reason` registry (design §44 §7.4, D611; runtime contract R8).
 *
 * A failed RPC carries a Connect code and one `loams.errors.v1.ErrorInfo` in its
 * details. The **code** gives the class and the **`reason`** is the branch: a stable
 * `snake_case` string, registered in `docs/api/reasons.md` and never renamed within
 * an API major version. A caller branches on [Reason], never on the message, which
 * may change.
 *
 * Three states are deliberately distinct rather than collapsed (R8):
 *
 *  - [Reason.NONE] — the failure came from **below the API** (a socket, a timeout, a
 *    cancelled call) or the server sent no `ErrorInfo`. It carries no reason, which
 *    is not the same thing as any reason in the registry;
 *  - a reason off the registry, from a server newer than this SDK: it is surfaced as
 *    the **text** in [LoamsException.unknownReason] rather than dropped, because
 *    dropping it leaves a caller unable to tell "not supported here" from "not
 *    supported at all";
 *  - a reason in the registry, which is [LoamsException.reason].
 *
 * # Provenance
 *
 * Hand-written, and it is the same hand-written answer `sdks/go` and `sdks/java`
 * give: design §44 §7.3's **Q604 fallback**, which permits a hand-written facade
 * checked by the same conformance suite where the generator is too costly for a
 * language. **A registry cannot be derived** the way a facade can: the wire names are
 * `snake_case` strings with no proto to read them from, and the reason a registry
 * can be written out and a binding table cannot is that the registry is the
 * *authority* — `docs/api/reasons.md` — rather than a copy of something else.
 * `kotlin_the_reason_registry_matches_the_registry_page` reads that page and fails on
 * a drift, so this enum cannot quietly fall behind it. **Do not add a reason here
 * without adding its row to that page first.**
 */
enum class Reason {
    /**
     * The failure carried no reason: it came from below the API, or the server sent
     * an error with no `ErrorInfo`.
     *
     * Distinct from every registry reason, and R8 requires it to stay distinct —
     * conflating it with [TOKEN_EXPIRED] would make a client refresh a request that
     * has no credential to refresh.
     */
    NONE,

    /** The approval was still pending and is no longer. */
    APPROVAL_EXPIRED,

    /** The approval was already decided, by this or another caller. */
    APPROVAL_ALREADY_DECIDED,

    /** The decision named a revision the server is not on. */
    APPROVAL_STALE_REVISION,

    /** An agent requested the operation, so its requester cannot approve it. */
    REQUESTER_CANNOT_APPROVE,

    /** The decision proof did not verify against the paired device key. */
    DECISION_PROOF_INVALID,

    /**
     * The credential is valid but too old for this decision. R1's refresh trigger;
     * distinct from [UNAUTHENTICATED].
     */
    STEP_UP_REQUIRED,

    /** Rejecting, or approving something destructive, needs a reason. */
    REASON_REQUIRED,

    /** The decision is not one the policy allows. */
    INVALID_DECISION,

    /** The pairing grant is past its expiry. */
    PAIRING_EXPIRED,

    /** The pairing grant was already redeemed; it is single-use. */
    PAIRING_USED,

    /** The device the token is bound to has been revoked. */
    DEVICE_REVOKED,

    /** The push target the request named is not registered. */
    PUSH_TARGET_UNKNOWN,

    /** A stub handler whose service has not landed yet. */
    NOT_IMPLEMENTED,

    /**
     * A catalogue package whose engine is not in this build variant (design §44 §4,
     * D600). The variant is in `metadata.variant`.
     */
    FEATURE_NOT_IN_VARIANT,

    /** `house_not_configured` (docs/api/reasons.md). */
    HOUSE_NOT_CONFIGURED,

    /** `house_syntax_error` (docs/api/reasons.md). */
    HOUSE_SYNTAX_ERROR,

    /** `house_unknown_table` (docs/api/reasons.md). */
    HOUSE_UNKNOWN_TABLE,

    /** `house_unknown_database` (docs/api/reasons.md). */
    HOUSE_UNKNOWN_DATABASE,

    /** `house_access_denied` (docs/api/reasons.md). */
    HOUSE_ACCESS_DENIED,

    /** `house_read_only` (docs/api/reasons.md). */
    HOUSE_READ_ONLY,

    /** `house_timeout` (docs/api/reasons.md). */
    HOUSE_TIMEOUT,

    /** `house_memory_limit` (docs/api/reasons.md). */
    HOUSE_MEMORY_LIMIT,

    /** `house_too_many_queries` (docs/api/reasons.md). */
    HOUSE_TOO_MANY_QUERIES,

    /** `house_quota_exceeded` (docs/api/reasons.md). */
    HOUSE_QUOTA_EXCEEDED,

    /** `house_disabled_function` (docs/api/reasons.md). */
    HOUSE_DISABLED_FUNCTION,

    /** `house_cancelled` (docs/api/reasons.md). */
    HOUSE_CANCELLED,

    /** `house_query_not_found` (docs/api/reasons.md). */
    HOUSE_QUERY_NOT_FOUND,

    /** `house_query_failed` (docs/api/reasons.md). */
    HOUSE_QUERY_FAILED,

    /** `project_not_found` (docs/api/reasons.md). */
    PROJECT_NOT_FOUND,

    /** `branch_has_children` (docs/api/reasons.md). */
    BRANCH_HAS_CHILDREN,

    /** `branch_protected` (docs/api/reasons.md). */
    BRANCH_PROTECTED,

    /** `lsn_out_of_retention` (docs/api/reasons.md). */
    LSN_OUT_OF_RETENTION,

    /** `endpoint_exists_for_branch` (docs/api/reasons.md). */
    ENDPOINT_EXISTS_FOR_BRANCH,

    /** `compute_start_failed` (docs/api/reasons.md). */
    COMPUTE_START_FAILED,

    /** `unsupported_pg_version` (docs/api/reasons.md). */
    UNSUPPORTED_PG_VERSION,

    /** `beta_major_requires_opt_in` (docs/api/reasons.md). */
    BETA_MAJOR_REQUIRES_OPT_IN,

    /** `compute_major_mismatch_refused` (docs/api/reasons.md). */
    COMPUTE_MAJOR_MISMATCH_REFUSED,

    /** `import_checksums_enabled` (docs/api/reasons.md). */
    IMPORT_CHECKSUMS_ENABLED,

    /** `quota_exceeded` (docs/api/reasons.md). */
    QUOTA_EXCEEDED,

    /** `storage_unavailable` (docs/api/reasons.md). */
    STORAGE_UNAVAILABLE,

    /** `secret_already_issued` (docs/api/reasons.md). */
    SECRET_ALREADY_ISSUED,

    /** `repo_not_found` (docs/api/reasons.md). */
    REPO_NOT_FOUND,

    /** `repo_already_exists` (docs/api/reasons.md). */
    REPO_ALREADY_EXISTS,

    /** `repo_deleting` (docs/api/reasons.md). */
    REPO_DELETING,

    /** `ref_stale` (docs/api/reasons.md). */
    REF_STALE,

    /** `ref_protected` (docs/api/reasons.md). */
    REF_PROTECTED,

    /** `out_of_scope` (docs/api/reasons.md). */
    OUT_OF_SCOPE,

    /** `object_format_unsupported` (docs/api/reasons.md). */
    OBJECT_FORMAT_UNSUPPORTED,

    /** `fork_too_deep` (docs/api/reasons.md). */
    FORK_TOO_DEEP,

    /** `sequencer_unavailable` (docs/api/reasons.md). */
    SEQUENCER_UNAVAILABLE,

    /** `idempotency_key_reused` (docs/api/reasons.md). */
    IDEMPOTENCY_KEY_REUSED,

    /** `workspace_not_found` (docs/api/reasons.md). */
    WORKSPACE_NOT_FOUND,

    /** `workspace_fenced` (docs/api/reasons.md). */
    WORKSPACE_FENCED,
    /** `batch_partially_applied` (docs/api/reasons.md). */
    BATCH_PARTIALLY_APPLIED,

    /** `workspace_read_only` (docs/api/reasons.md). */
    WORKSPACE_READ_ONLY,

    /** `workspace_name_taken` (docs/api/reasons.md). */
    WORKSPACE_NAME_TAKEN,

    /** `object_missing` (docs/api/reasons.md). */
    OBJECT_MISSING,

    /** `secret_detected` (docs/api/reasons.md). */
    SECRET_DETECTED,

    /** `env_unsealed` (docs/api/reasons.md). */
    ENV_UNSEALED,

    /** `env_access_denied` (docs/api/reasons.md). */
    ENV_ACCESS_DENIED,

    /** `env_foreign_reference` (docs/api/reasons.md). */
    ENV_FOREIGN_REFERENCE,

    /** `memory_store_not_found` (docs/api/reasons.md). */
    MEMORY_STORE_NOT_FOUND,

    /** `memory_space_not_found` (docs/api/reasons.md). */
    MEMORY_SPACE_NOT_FOUND,

    /** `memory_space_limit` (docs/api/reasons.md). */
    MEMORY_SPACE_LIMIT,

    /** `memory_managed_object` (docs/api/reasons.md). */
    MEMORY_MANAGED_OBJECT,

    /** `memory_block_version_mismatch` (docs/api/reasons.md). */
    MEMORY_BLOCK_VERSION_MISMATCH,

    /** `memory_block_too_large` (docs/api/reasons.md). */
    MEMORY_BLOCK_TOO_LARGE,

    /** `memory_core_budget_exceeded` (docs/api/reasons.md). */
    MEMORY_CORE_BUDGET_EXCEEDED,

    /** `memory_content_too_large` (docs/api/reasons.md). */
    MEMORY_CONTENT_TOO_LARGE,

    /** `memory_secret_detected` (docs/api/reasons.md). */
    MEMORY_SECRET_DETECTED,

    /** `memory_pii_refused` (docs/api/reasons.md). */
    MEMORY_PII_REFUSED,

    /** `memory_model_not_configured` (docs/api/reasons.md). */
    MEMORY_MODEL_NOT_CONFIGURED,

    /** `memory_model_not_local` (docs/api/reasons.md). */
    MEMORY_MODEL_NOT_LOCAL,

    /** `memory_token_budget_exhausted` (docs/api/reasons.md). */
    MEMORY_TOKEN_BUDGET_EXHAUSTED,

    /** `memory_extraction_failed` (docs/api/reasons.md). */
    MEMORY_EXTRACTION_FAILED,

    /** `memory_too_many_spaces` (docs/api/reasons.md). */
    MEMORY_TOO_MANY_SPACES,

    /** `memory_cross_space_write` (docs/api/reasons.md). */
    MEMORY_CROSS_SPACE_WRITE,

    /** `memory_not_yet_available` (docs/api/reasons.md). */
    MEMORY_NOT_YET_AVAILABLE,

    /** `code_repo_not_indexed` (docs/api/reasons.md). */
    CODE_REPO_NOT_INDEXED,

    /** `code_index_building` (docs/api/reasons.md). */
    CODE_INDEX_BUILDING,

    /** `code_change_too_large` (docs/api/reasons.md). */
    CODE_CHANGE_TOO_LARGE,

    /** `code_language_unsupported` (docs/api/reasons.md). */
    CODE_LANGUAGE_UNSUPPORTED,

    /** `code_scip_invalid` (docs/api/reasons.md). */
    CODE_SCIP_INVALID,

    /** `live_bad_cursor` (docs/api/reasons.md). */
    LIVE_BAD_CURSOR,

    /** `live_worker_crashed` (docs/api/reasons.md). */
    LIVE_WORKER_CRASHED,

    /** `live_deploy_conflict` (docs/api/reasons.md). */
    LIVE_DEPLOY_CONFLICT,

    /** `live_schema_violation` (docs/api/reasons.md). */
    LIVE_SCHEMA_VIOLATION,
    /** `filter_conflict` (docs/api/reasons.md). */
    FILTER_CONFLICT,
    /** `search_after_needs_tiebreak` (docs/api/reasons.md). */
    SEARCH_AFTER_NEEDS_TIEBREAK,
    /** `invalid_consistency_token` (docs/api/reasons.md). */
    INVALID_CONSISTENCY_TOKEN,
    /** `namespace_required` (docs/api/reasons.md). */
    NAMESPACE_REQUIRED,
    /** `sql_param_count` (docs/api/reasons.md). */
    SQL_PARAM_COUNT,

    /** `live_index_not_ready` (docs/api/reasons.md). */
    LIVE_INDEX_NOT_READY,

    /** `live_app_disabled` (docs/api/reasons.md). */
    LIVE_APP_DISABLED,

    /** `live_app_exists` (docs/api/reasons.md). */
    LIVE_APP_EXISTS,

    /** `live_function_error` (docs/api/reasons.md). */
    LIVE_FUNCTION_ERROR,

    /** `live_function_timeout` (docs/api/reasons.md). */
    LIVE_FUNCTION_TIMEOUT,
    /** `client_deadline_exceeded` (docs/api/reasons.md). */
    CLIENT_DEADLINE_EXCEEDED,

    /** `live_function_out_of_memory` (docs/api/reasons.md). */
    LIVE_FUNCTION_OUT_OF_MEMORY,

    /** `live_limit_exceeded` (docs/api/reasons.md). */
    LIVE_LIMIT_EXCEEDED,
    /** `result_too_large` (docs/api/reasons.md). */
    RESULT_TOO_LARGE,

    /** `live_app_mismatch` (docs/api/reasons.md). */
    LIVE_APP_MISMATCH,

    /** `live_origin_not_allowed` (docs/api/reasons.md). */
    LIVE_ORIGIN_NOT_ALLOWED,

    /** `session_rotate` (docs/api/reasons.md). */
    SESSION_ROTATE,
    /** `session_gone` (docs/api/reasons.md). */
    SESSION_GONE,
    /** `node_draining` (docs/api/reasons.md). */
    NODE_DRAINING,

    /** The caller's org role is below the one the admin RPC needs, or the caller is not in the org. */
    ROLE_REQUIRED,

    /** First-run setup has not created the org yet. */
    SETUP_REQUIRED,

    /** The call carried no credential. */
    TOKEN_MISSING,

    /** The credential is not an API key, a session token or a JWT, or more than one Authorization header was sent. */
    TOKEN_MALFORMED,

    /** The API key was revoked, or the console session was signed out or its person left the org. */
    TOKEN_REVOKED,

    /** The OIDC token's nbf is in the future. */
    TOKEN_NOT_YET_VALID,

    /** The OIDC token's signature does not verify. */
    TOKEN_BAD_SIGNATURE,

    /** The OIDC token is for another audience. */
    TOKEN_WRONG_AUDIENCE,

    /** The OIDC token is from an issuer this engine does not trust. */
    TOKEN_WRONG_ISSUER,

    /** The OIDC token is signed with a key its issuer's JWKS does not have. */
    TOKEN_UNKNOWN_KEY,

    /** The OIDC token is signed with an algorithm other than RS256 or EdDSA. */
    TOKEN_ALGORITHM_NOT_ALLOWED,

    /** No key file or metastore record admits the API key. */
    API_KEY_INVALID,

    /** This engine's auth mode does not take this kind of credential. */
    CREDENTIAL_NOT_ACCEPTED,

    /** A credential was sent in the query string. */
    CREDENTIAL_IN_QUERY_STRING,

    /** A cookie-authenticated call that is not a GET lacked the session's CSRF token. */
    CSRF_TOKEN_INVALID,

    /** A valid OIDC or tailnet identity that is not on the engine's allowlist. */
    PRINCIPAL_NOT_ALLOWED,

    /** A browser call from an origin the engine's CORS allowlist does not have. */
    CORS_ORIGIN_NOT_ALLOWED,

    /** The credential cannot be checked now: the issuer's JWKS or the principal store is unreachable. */
    AUTH_UNAVAILABLE,

    /** A malformed field or an unparseable value. */
    INVALID_ARGUMENT,

    /** The named resource does not exist. */
    NOT_FOUND,

    /** The named resource already exists. */
    ALREADY_EXISTS,

    /** The caller's role may not make this call. */
    PERMISSION_DENIED,

    /**
     * A rejected access token, which R1 refreshes once and retries once. A second
     * expiry reaches the caller as a [TokenExpiredException].
     */
    TOKEN_EXPIRED,

    /** No credential, or one that cannot be used. */
    UNAUTHENTICATED,

    /** The call's preconditions do not hold. */
    FAILED_PRECONDITION,

    /** A backpressure or quota refusal. */
    RESOURCE_EXHAUSTED,

    /** A dependency is down, or this node cannot serve the read. */
    UNAVAILABLE,

    /** The caller's deadline passed. */
    DEADLINE_EXCEEDED,

    /** A concurrent write won; the caller retries. */
    ABORTED,

    /** A bug in the server. The message and the request id go to the log. */
    INTERNAL,
}

/**
 * The registry itself: the wire name of every [Reason], the Connect code each is
 * raised under, and the reverse lookup.
 *
 * Three lookups, and they answer different questions. [fromWire] answers "what did
 * the server say" and returns null for a reason this SDK does not have, which the
 * runtime surfaces as `unknownReason` rather than dropping. [name] answers "what do I
 * write in a log" and returns the empty string for [Reason.NONE], because a failure
 * with no reason has no reason to print. [codeOf] answers "what class is it", which
 * is what the retry policy and the error hierarchy branch on.
 */
object ReasonRegistry {
    /**
     * The registry in the registry's own order, which is the order of the table in
     * `docs/api/reasons.md`: the AP0-specific causes first, then the generic
     * code-to-class rows.
     *
     * Kept as one ordered list rather than derived from a map, because "the
     * registry's order" is a property [all] promises and a hash map does not have
     * one.
     */
    private val IN_REGISTRY_ORDER: List<Reason> = listOf(
        Reason.APPROVAL_EXPIRED,
        Reason.APPROVAL_ALREADY_DECIDED,
        Reason.APPROVAL_STALE_REVISION,
        Reason.REQUESTER_CANNOT_APPROVE,
        Reason.DECISION_PROOF_INVALID,
        Reason.STEP_UP_REQUIRED,
        Reason.REASON_REQUIRED,
        Reason.INVALID_DECISION,
        Reason.PAIRING_EXPIRED,
        Reason.PAIRING_USED,
        Reason.DEVICE_REVOKED,
        Reason.PUSH_TARGET_UNKNOWN,
        Reason.NOT_IMPLEMENTED,
        Reason.FEATURE_NOT_IN_VARIANT,
        Reason.HOUSE_NOT_CONFIGURED,
        Reason.HOUSE_SYNTAX_ERROR,
        Reason.HOUSE_UNKNOWN_TABLE,
        Reason.HOUSE_UNKNOWN_DATABASE,
        Reason.HOUSE_ACCESS_DENIED,
        Reason.HOUSE_READ_ONLY,
        Reason.HOUSE_TIMEOUT,
        Reason.HOUSE_MEMORY_LIMIT,
        Reason.HOUSE_TOO_MANY_QUERIES,
        Reason.HOUSE_QUOTA_EXCEEDED,
        Reason.HOUSE_DISABLED_FUNCTION,
        Reason.HOUSE_CANCELLED,
        Reason.HOUSE_QUERY_NOT_FOUND,
        Reason.HOUSE_QUERY_FAILED,
        Reason.PROJECT_NOT_FOUND,
        Reason.BRANCH_HAS_CHILDREN,
        Reason.BRANCH_PROTECTED,
        Reason.LSN_OUT_OF_RETENTION,
        Reason.ENDPOINT_EXISTS_FOR_BRANCH,
        Reason.COMPUTE_START_FAILED,
        Reason.UNSUPPORTED_PG_VERSION,
        Reason.BETA_MAJOR_REQUIRES_OPT_IN,
        Reason.COMPUTE_MAJOR_MISMATCH_REFUSED,
        Reason.IMPORT_CHECKSUMS_ENABLED,
        Reason.QUOTA_EXCEEDED,
        Reason.STORAGE_UNAVAILABLE,
        Reason.SECRET_ALREADY_ISSUED,
        Reason.REPO_NOT_FOUND,
        Reason.REPO_ALREADY_EXISTS,
        Reason.REPO_DELETING,
        Reason.REF_STALE,
        Reason.REF_PROTECTED,
        Reason.OUT_OF_SCOPE,
        Reason.OBJECT_FORMAT_UNSUPPORTED,
        Reason.FORK_TOO_DEEP,
        Reason.SEQUENCER_UNAVAILABLE,
        Reason.IDEMPOTENCY_KEY_REUSED,
        Reason.WORKSPACE_NOT_FOUND,
        Reason.WORKSPACE_FENCED,
        Reason.BATCH_PARTIALLY_APPLIED,
        Reason.WORKSPACE_READ_ONLY,
        Reason.WORKSPACE_NAME_TAKEN,
        Reason.OBJECT_MISSING,
        Reason.SECRET_DETECTED,
        Reason.ENV_UNSEALED,
        Reason.ENV_ACCESS_DENIED,
        Reason.ENV_FOREIGN_REFERENCE,
        Reason.MEMORY_STORE_NOT_FOUND,
        Reason.MEMORY_SPACE_NOT_FOUND,
        Reason.MEMORY_SPACE_LIMIT,
        Reason.MEMORY_MANAGED_OBJECT,
        Reason.MEMORY_BLOCK_VERSION_MISMATCH,
        Reason.MEMORY_BLOCK_TOO_LARGE,
        Reason.MEMORY_CORE_BUDGET_EXCEEDED,
        Reason.MEMORY_CONTENT_TOO_LARGE,
        Reason.MEMORY_SECRET_DETECTED,
        Reason.MEMORY_PII_REFUSED,
        Reason.MEMORY_MODEL_NOT_CONFIGURED,
        Reason.MEMORY_MODEL_NOT_LOCAL,
        Reason.MEMORY_TOKEN_BUDGET_EXHAUSTED,
        Reason.MEMORY_EXTRACTION_FAILED,
        Reason.MEMORY_TOO_MANY_SPACES,
        Reason.MEMORY_CROSS_SPACE_WRITE,
        Reason.MEMORY_NOT_YET_AVAILABLE,
        Reason.CODE_REPO_NOT_INDEXED,
        Reason.CODE_INDEX_BUILDING,
        Reason.CODE_CHANGE_TOO_LARGE,
        Reason.CODE_LANGUAGE_UNSUPPORTED,
        Reason.CODE_SCIP_INVALID,
        Reason.LIVE_BAD_CURSOR,
        Reason.LIVE_WORKER_CRASHED,
        Reason.LIVE_DEPLOY_CONFLICT,
        Reason.LIVE_SCHEMA_VIOLATION,
        Reason.FILTER_CONFLICT,
        Reason.SEARCH_AFTER_NEEDS_TIEBREAK,
        Reason.INVALID_CONSISTENCY_TOKEN,
        Reason.NAMESPACE_REQUIRED,
        Reason.SQL_PARAM_COUNT,
        Reason.LIVE_INDEX_NOT_READY,
        Reason.LIVE_APP_DISABLED,
        Reason.LIVE_APP_EXISTS,
        Reason.LIVE_FUNCTION_ERROR,
        Reason.LIVE_FUNCTION_TIMEOUT,
        Reason.CLIENT_DEADLINE_EXCEEDED,
        Reason.LIVE_FUNCTION_OUT_OF_MEMORY,
        Reason.LIVE_LIMIT_EXCEEDED,
        Reason.RESULT_TOO_LARGE,
        Reason.LIVE_APP_MISMATCH,
        Reason.LIVE_ORIGIN_NOT_ALLOWED,
        Reason.SESSION_ROTATE,
        Reason.SESSION_GONE,
        Reason.NODE_DRAINING,
        Reason.ROLE_REQUIRED,
        Reason.SETUP_REQUIRED,
        Reason.TOKEN_MISSING,
        Reason.TOKEN_MALFORMED,
        Reason.TOKEN_REVOKED,
        Reason.TOKEN_NOT_YET_VALID,
        Reason.TOKEN_BAD_SIGNATURE,
        Reason.TOKEN_WRONG_AUDIENCE,
        Reason.TOKEN_WRONG_ISSUER,
        Reason.TOKEN_UNKNOWN_KEY,
        Reason.TOKEN_ALGORITHM_NOT_ALLOWED,
        Reason.API_KEY_INVALID,
        Reason.CREDENTIAL_NOT_ACCEPTED,
        Reason.CREDENTIAL_IN_QUERY_STRING,
        Reason.CSRF_TOKEN_INVALID,
        Reason.PRINCIPAL_NOT_ALLOWED,
        Reason.CORS_ORIGIN_NOT_ALLOWED,
        Reason.AUTH_UNAVAILABLE,
        Reason.INVALID_ARGUMENT,
        Reason.NOT_FOUND,
        Reason.ALREADY_EXISTS,
        Reason.PERMISSION_DENIED,
        Reason.TOKEN_EXPIRED,
        Reason.UNAUTHENTICATED,
        Reason.FAILED_PRECONDITION,
        Reason.RESOURCE_EXHAUSTED,
        Reason.UNAVAILABLE,
        Reason.DEADLINE_EXCEEDED,
        Reason.ABORTED,
        Reason.INTERNAL,
    )

    private val NAMES: Map<Reason, String> = mapOf(
        Reason.APPROVAL_EXPIRED to "approval_expired",
        Reason.APPROVAL_ALREADY_DECIDED to "approval_already_decided",
        Reason.APPROVAL_STALE_REVISION to "approval_stale_revision",
        Reason.REQUESTER_CANNOT_APPROVE to "requester_cannot_approve",
        Reason.DECISION_PROOF_INVALID to "decision_proof_invalid",
        Reason.STEP_UP_REQUIRED to "step_up_required",
        Reason.REASON_REQUIRED to "reason_required",
        Reason.INVALID_DECISION to "invalid_decision",
        Reason.PAIRING_EXPIRED to "pairing_expired",
        Reason.PAIRING_USED to "pairing_used",
        Reason.DEVICE_REVOKED to "device_revoked",
        Reason.PUSH_TARGET_UNKNOWN to "push_target_unknown",
        Reason.NOT_IMPLEMENTED to "not_implemented",
        Reason.FEATURE_NOT_IN_VARIANT to "feature_not_in_variant",
        Reason.HOUSE_NOT_CONFIGURED to "house_not_configured",
        Reason.HOUSE_SYNTAX_ERROR to "house_syntax_error",
        Reason.HOUSE_UNKNOWN_TABLE to "house_unknown_table",
        Reason.HOUSE_UNKNOWN_DATABASE to "house_unknown_database",
        Reason.HOUSE_ACCESS_DENIED to "house_access_denied",
        Reason.HOUSE_READ_ONLY to "house_read_only",
        Reason.HOUSE_TIMEOUT to "house_timeout",
        Reason.HOUSE_MEMORY_LIMIT to "house_memory_limit",
        Reason.HOUSE_TOO_MANY_QUERIES to "house_too_many_queries",
        Reason.HOUSE_QUOTA_EXCEEDED to "house_quota_exceeded",
        Reason.HOUSE_DISABLED_FUNCTION to "house_disabled_function",
        Reason.HOUSE_CANCELLED to "house_cancelled",
        Reason.HOUSE_QUERY_NOT_FOUND to "house_query_not_found",
        Reason.HOUSE_QUERY_FAILED to "house_query_failed",
        Reason.PROJECT_NOT_FOUND to "project_not_found",
        Reason.BRANCH_HAS_CHILDREN to "branch_has_children",
        Reason.BRANCH_PROTECTED to "branch_protected",
        Reason.LSN_OUT_OF_RETENTION to "lsn_out_of_retention",
        Reason.ENDPOINT_EXISTS_FOR_BRANCH to "endpoint_exists_for_branch",
        Reason.COMPUTE_START_FAILED to "compute_start_failed",
        Reason.UNSUPPORTED_PG_VERSION to "unsupported_pg_version",
        Reason.BETA_MAJOR_REQUIRES_OPT_IN to "beta_major_requires_opt_in",
        Reason.COMPUTE_MAJOR_MISMATCH_REFUSED to "compute_major_mismatch_refused",
        Reason.IMPORT_CHECKSUMS_ENABLED to "import_checksums_enabled",
        Reason.QUOTA_EXCEEDED to "quota_exceeded",
        Reason.STORAGE_UNAVAILABLE to "storage_unavailable",
        Reason.SECRET_ALREADY_ISSUED to "secret_already_issued",
        Reason.REPO_NOT_FOUND to "repo_not_found",
        Reason.REPO_ALREADY_EXISTS to "repo_already_exists",
        Reason.REPO_DELETING to "repo_deleting",
        Reason.REF_STALE to "ref_stale",
        Reason.REF_PROTECTED to "ref_protected",
        Reason.OUT_OF_SCOPE to "out_of_scope",
        Reason.OBJECT_FORMAT_UNSUPPORTED to "object_format_unsupported",
        Reason.FORK_TOO_DEEP to "fork_too_deep",
        Reason.SEQUENCER_UNAVAILABLE to "sequencer_unavailable",
        Reason.IDEMPOTENCY_KEY_REUSED to "idempotency_key_reused",
        Reason.WORKSPACE_NOT_FOUND to "workspace_not_found",
        Reason.WORKSPACE_FENCED to "workspace_fenced",
        Reason.BATCH_PARTIALLY_APPLIED to "batch_partially_applied",
        Reason.WORKSPACE_READ_ONLY to "workspace_read_only",
        Reason.WORKSPACE_NAME_TAKEN to "workspace_name_taken",
        Reason.OBJECT_MISSING to "object_missing",
        Reason.SECRET_DETECTED to "secret_detected",
        Reason.ENV_UNSEALED to "env_unsealed",
        Reason.ENV_ACCESS_DENIED to "env_access_denied",
        Reason.ENV_FOREIGN_REFERENCE to "env_foreign_reference",
        Reason.MEMORY_STORE_NOT_FOUND to "memory_store_not_found",
        Reason.MEMORY_SPACE_NOT_FOUND to "memory_space_not_found",
        Reason.MEMORY_SPACE_LIMIT to "memory_space_limit",
        Reason.MEMORY_MANAGED_OBJECT to "memory_managed_object",
        Reason.MEMORY_BLOCK_VERSION_MISMATCH to "memory_block_version_mismatch",
        Reason.MEMORY_BLOCK_TOO_LARGE to "memory_block_too_large",
        Reason.MEMORY_CORE_BUDGET_EXCEEDED to "memory_core_budget_exceeded",
        Reason.MEMORY_CONTENT_TOO_LARGE to "memory_content_too_large",
        Reason.MEMORY_SECRET_DETECTED to "memory_secret_detected",
        Reason.MEMORY_PII_REFUSED to "memory_pii_refused",
        Reason.MEMORY_MODEL_NOT_CONFIGURED to "memory_model_not_configured",
        Reason.MEMORY_MODEL_NOT_LOCAL to "memory_model_not_local",
        Reason.MEMORY_TOKEN_BUDGET_EXHAUSTED to "memory_token_budget_exhausted",
        Reason.MEMORY_EXTRACTION_FAILED to "memory_extraction_failed",
        Reason.MEMORY_TOO_MANY_SPACES to "memory_too_many_spaces",
        Reason.MEMORY_CROSS_SPACE_WRITE to "memory_cross_space_write",
        Reason.MEMORY_NOT_YET_AVAILABLE to "memory_not_yet_available",
        Reason.CODE_REPO_NOT_INDEXED to "code_repo_not_indexed",
        Reason.CODE_INDEX_BUILDING to "code_index_building",
        Reason.CODE_CHANGE_TOO_LARGE to "code_change_too_large",
        Reason.CODE_LANGUAGE_UNSUPPORTED to "code_language_unsupported",
        Reason.CODE_SCIP_INVALID to "code_scip_invalid",
        Reason.LIVE_BAD_CURSOR to "live_bad_cursor",
        Reason.LIVE_WORKER_CRASHED to "live_worker_crashed",
        Reason.LIVE_DEPLOY_CONFLICT to "live_deploy_conflict",
        Reason.LIVE_SCHEMA_VIOLATION to "live_schema_violation",
        Reason.FILTER_CONFLICT to "filter_conflict",
        Reason.SEARCH_AFTER_NEEDS_TIEBREAK to "search_after_needs_tiebreak",
        Reason.INVALID_CONSISTENCY_TOKEN to "invalid_consistency_token",
        Reason.NAMESPACE_REQUIRED to "namespace_required",
        Reason.SQL_PARAM_COUNT to "sql_param_count",
        Reason.LIVE_INDEX_NOT_READY to "live_index_not_ready",
        Reason.LIVE_APP_DISABLED to "live_app_disabled",
        Reason.LIVE_APP_EXISTS to "live_app_exists",
        Reason.LIVE_FUNCTION_ERROR to "live_function_error",
        Reason.LIVE_FUNCTION_TIMEOUT to "live_function_timeout",
        Reason.CLIENT_DEADLINE_EXCEEDED to "client_deadline_exceeded",
        Reason.LIVE_FUNCTION_OUT_OF_MEMORY to "live_function_out_of_memory",
        Reason.LIVE_LIMIT_EXCEEDED to "live_limit_exceeded",
        Reason.RESULT_TOO_LARGE to "result_too_large",
        Reason.LIVE_APP_MISMATCH to "live_app_mismatch",
        Reason.LIVE_ORIGIN_NOT_ALLOWED to "live_origin_not_allowed",
        Reason.SESSION_ROTATE to "session_rotate",
        Reason.SESSION_GONE to "session_gone",
        Reason.NODE_DRAINING to "node_draining",
        Reason.ROLE_REQUIRED to "role_required",
        Reason.SETUP_REQUIRED to "setup_required",
        Reason.TOKEN_MISSING to "token_missing",
        Reason.TOKEN_MALFORMED to "token_malformed",
        Reason.TOKEN_REVOKED to "token_revoked",
        Reason.TOKEN_NOT_YET_VALID to "token_not_yet_valid",
        Reason.TOKEN_BAD_SIGNATURE to "token_bad_signature",
        Reason.TOKEN_WRONG_AUDIENCE to "token_wrong_audience",
        Reason.TOKEN_WRONG_ISSUER to "token_wrong_issuer",
        Reason.TOKEN_UNKNOWN_KEY to "token_unknown_key",
        Reason.TOKEN_ALGORITHM_NOT_ALLOWED to "token_algorithm_not_allowed",
        Reason.API_KEY_INVALID to "api_key_invalid",
        Reason.CREDENTIAL_NOT_ACCEPTED to "credential_not_accepted",
        Reason.CREDENTIAL_IN_QUERY_STRING to "credential_in_query_string",
        Reason.CSRF_TOKEN_INVALID to "csrf_token_invalid",
        Reason.PRINCIPAL_NOT_ALLOWED to "principal_not_allowed",
        Reason.CORS_ORIGIN_NOT_ALLOWED to "cors_origin_not_allowed",
        Reason.AUTH_UNAVAILABLE to "auth_unavailable",
        Reason.INVALID_ARGUMENT to "invalid_argument",
        Reason.NOT_FOUND to "not_found",
        Reason.ALREADY_EXISTS to "already_exists",
        Reason.PERMISSION_DENIED to "permission_denied",
        Reason.TOKEN_EXPIRED to "token_expired",
        Reason.UNAUTHENTICATED to "unauthenticated",
        Reason.FAILED_PRECONDITION to "failed_precondition",
        Reason.RESOURCE_EXHAUSTED to "resource_exhausted",
        Reason.UNAVAILABLE to "unavailable",
        Reason.DEADLINE_EXCEEDED to "deadline_exceeded",
        Reason.ABORTED to "aborted",
        Reason.INTERNAL to "internal",
    )

    private val BY_WIRE: Map<String, Reason> = NAMES.entries.associate { (reason, wire) -> wire to reason }

    /**
     * The rows above `invalid_argument` in `docs/api/reasons.md` are AP0's specific
     * causes and do not map to the code named in their own tail; the rows below it are
     * the generic code-to-class rows and map to the code their name carries.
     */
    private val CODES: Map<Reason, Code> = mapOf(
        Reason.APPROVAL_EXPIRED to Code.FAILED_PRECONDITION,
        Reason.APPROVAL_ALREADY_DECIDED to Code.FAILED_PRECONDITION,
        Reason.APPROVAL_STALE_REVISION to Code.FAILED_PRECONDITION,
        Reason.REQUESTER_CANNOT_APPROVE to Code.PERMISSION_DENIED,
        Reason.DECISION_PROOF_INVALID to Code.PERMISSION_DENIED,
        Reason.STEP_UP_REQUIRED to Code.UNAUTHENTICATED,
        Reason.REASON_REQUIRED to Code.INVALID_ARGUMENT,
        Reason.INVALID_DECISION to Code.INVALID_ARGUMENT,
        Reason.PAIRING_EXPIRED to Code.FAILED_PRECONDITION,
        Reason.PAIRING_USED to Code.FAILED_PRECONDITION,
        Reason.DEVICE_REVOKED to Code.UNAUTHENTICATED,
        Reason.PUSH_TARGET_UNKNOWN to Code.NOT_FOUND,
        Reason.NOT_IMPLEMENTED to Code.UNIMPLEMENTED,
        Reason.FEATURE_NOT_IN_VARIANT to Code.UNIMPLEMENTED,
        Reason.HOUSE_NOT_CONFIGURED to Code.UNIMPLEMENTED,
        Reason.HOUSE_SYNTAX_ERROR to Code.INVALID_ARGUMENT,
        Reason.HOUSE_UNKNOWN_TABLE to Code.NOT_FOUND,
        Reason.HOUSE_UNKNOWN_DATABASE to Code.NOT_FOUND,
        Reason.HOUSE_ACCESS_DENIED to Code.PERMISSION_DENIED,
        Reason.HOUSE_READ_ONLY to Code.FAILED_PRECONDITION,
        Reason.HOUSE_TIMEOUT to Code.DEADLINE_EXCEEDED,
        Reason.HOUSE_MEMORY_LIMIT to Code.RESOURCE_EXHAUSTED,
        Reason.HOUSE_TOO_MANY_QUERIES to Code.RESOURCE_EXHAUSTED,
        Reason.HOUSE_QUOTA_EXCEEDED to Code.RESOURCE_EXHAUSTED,
        Reason.HOUSE_DISABLED_FUNCTION to Code.PERMISSION_DENIED,
        Reason.HOUSE_CANCELLED to Code.CANCELLED,
        Reason.HOUSE_QUERY_NOT_FOUND to Code.NOT_FOUND,
        Reason.HOUSE_QUERY_FAILED to Code.UNKNOWN,
        Reason.PROJECT_NOT_FOUND to Code.NOT_FOUND,
        Reason.BRANCH_HAS_CHILDREN to Code.FAILED_PRECONDITION,
        Reason.BRANCH_PROTECTED to Code.FAILED_PRECONDITION,
        Reason.BETA_MAJOR_REQUIRES_OPT_IN to Code.FAILED_PRECONDITION,
        Reason.COMPUTE_MAJOR_MISMATCH_REFUSED to Code.FAILED_PRECONDITION,
        Reason.IMPORT_CHECKSUMS_ENABLED to Code.FAILED_PRECONDITION,
        Reason.LSN_OUT_OF_RETENTION to Code.FAILED_PRECONDITION,
        Reason.ENDPOINT_EXISTS_FOR_BRANCH to Code.ALREADY_EXISTS,
        Reason.COMPUTE_START_FAILED to Code.UNAVAILABLE,
        Reason.QUOTA_EXCEEDED to Code.RESOURCE_EXHAUSTED,
        Reason.STORAGE_UNAVAILABLE to Code.UNAVAILABLE,
        Reason.SECRET_ALREADY_ISSUED to Code.FAILED_PRECONDITION,
        Reason.REPO_NOT_FOUND to Code.NOT_FOUND,
        Reason.REPO_ALREADY_EXISTS to Code.ALREADY_EXISTS,
        Reason.REPO_DELETING to Code.FAILED_PRECONDITION,
        Reason.REF_STALE to Code.ABORTED,
        Reason.REF_PROTECTED to Code.FAILED_PRECONDITION,
        Reason.OUT_OF_SCOPE to Code.PERMISSION_DENIED,
        Reason.OBJECT_FORMAT_UNSUPPORTED to Code.UNIMPLEMENTED,
        Reason.FORK_TOO_DEEP to Code.FAILED_PRECONDITION,
        Reason.SEQUENCER_UNAVAILABLE to Code.UNAVAILABLE,
        Reason.IDEMPOTENCY_KEY_REUSED to Code.FAILED_PRECONDITION,
        Reason.WORKSPACE_NOT_FOUND to Code.NOT_FOUND,
        Reason.WORKSPACE_FENCED to Code.ABORTED,
        Reason.BATCH_PARTIALLY_APPLIED to Code.ABORTED,
        Reason.WORKSPACE_READ_ONLY to Code.FAILED_PRECONDITION,
        Reason.WORKSPACE_NAME_TAKEN to Code.ALREADY_EXISTS,
        Reason.OBJECT_MISSING to Code.FAILED_PRECONDITION,
        Reason.SECRET_DETECTED to Code.FAILED_PRECONDITION,
        Reason.ENV_UNSEALED to Code.INVALID_ARGUMENT,
        Reason.ENV_ACCESS_DENIED to Code.PERMISSION_DENIED,
        Reason.ENV_FOREIGN_REFERENCE to Code.PERMISSION_DENIED,
        Reason.MEMORY_STORE_NOT_FOUND to Code.NOT_FOUND,
        Reason.MEMORY_SPACE_NOT_FOUND to Code.NOT_FOUND,
        Reason.MEMORY_SPACE_LIMIT to Code.RESOURCE_EXHAUSTED,
        Reason.MEMORY_MANAGED_OBJECT to Code.FAILED_PRECONDITION,
        Reason.MEMORY_BLOCK_VERSION_MISMATCH to Code.ABORTED,
        Reason.MEMORY_BLOCK_TOO_LARGE to Code.INVALID_ARGUMENT,
        Reason.MEMORY_CORE_BUDGET_EXCEEDED to Code.RESOURCE_EXHAUSTED,
        Reason.MEMORY_CONTENT_TOO_LARGE to Code.INVALID_ARGUMENT,
        Reason.MEMORY_SECRET_DETECTED to Code.FAILED_PRECONDITION,
        Reason.MEMORY_PII_REFUSED to Code.FAILED_PRECONDITION,
        Reason.MEMORY_MODEL_NOT_CONFIGURED to Code.FAILED_PRECONDITION,
        Reason.MEMORY_MODEL_NOT_LOCAL to Code.FAILED_PRECONDITION,
        Reason.MEMORY_TOKEN_BUDGET_EXHAUSTED to Code.RESOURCE_EXHAUSTED,
        Reason.MEMORY_EXTRACTION_FAILED to Code.FAILED_PRECONDITION,
        Reason.MEMORY_TOO_MANY_SPACES to Code.INVALID_ARGUMENT,
        Reason.MEMORY_CROSS_SPACE_WRITE to Code.INVALID_ARGUMENT,
        Reason.MEMORY_NOT_YET_AVAILABLE to Code.UNIMPLEMENTED,
        Reason.CODE_REPO_NOT_INDEXED to Code.FAILED_PRECONDITION,
        Reason.CODE_INDEX_BUILDING to Code.UNAVAILABLE,
        Reason.CODE_CHANGE_TOO_LARGE to Code.INVALID_ARGUMENT,
        Reason.CODE_LANGUAGE_UNSUPPORTED to Code.FAILED_PRECONDITION,
        Reason.CODE_SCIP_INVALID to Code.INVALID_ARGUMENT,
        Reason.LIVE_BAD_CURSOR to Code.INVALID_ARGUMENT,
        Reason.LIVE_WORKER_CRASHED to Code.INTERNAL,
        Reason.LIVE_DEPLOY_CONFLICT to Code.FAILED_PRECONDITION,
        Reason.LIVE_SCHEMA_VIOLATION to Code.INVALID_ARGUMENT,
        Reason.FILTER_CONFLICT to Code.INVALID_ARGUMENT,
        Reason.SEARCH_AFTER_NEEDS_TIEBREAK to Code.INVALID_ARGUMENT,
        Reason.INVALID_CONSISTENCY_TOKEN to Code.INVALID_ARGUMENT,
        Reason.NAMESPACE_REQUIRED to Code.INVALID_ARGUMENT,
        Reason.SQL_PARAM_COUNT to Code.INVALID_ARGUMENT,
        Reason.UNSUPPORTED_PG_VERSION to Code.INVALID_ARGUMENT,
        Reason.LIVE_INDEX_NOT_READY to Code.FAILED_PRECONDITION,
        Reason.LIVE_APP_DISABLED to Code.FAILED_PRECONDITION,
        Reason.LIVE_APP_EXISTS to Code.FAILED_PRECONDITION,
        Reason.LIVE_FUNCTION_ERROR to Code.UNKNOWN,
        Reason.LIVE_FUNCTION_TIMEOUT to Code.DEADLINE_EXCEEDED,
        Reason.CLIENT_DEADLINE_EXCEEDED to Code.DEADLINE_EXCEEDED,
        Reason.LIVE_FUNCTION_OUT_OF_MEMORY to Code.RESOURCE_EXHAUSTED,
        Reason.LIVE_LIMIT_EXCEEDED to Code.RESOURCE_EXHAUSTED,
        Reason.RESULT_TOO_LARGE to Code.RESOURCE_EXHAUSTED,
        Reason.LIVE_APP_MISMATCH to Code.PERMISSION_DENIED,
        Reason.LIVE_ORIGIN_NOT_ALLOWED to Code.PERMISSION_DENIED,
        Reason.SESSION_ROTATE to Code.UNAVAILABLE,
        Reason.SESSION_GONE to Code.NOT_FOUND,
        Reason.NODE_DRAINING to Code.UNAVAILABLE,
        Reason.ROLE_REQUIRED to Code.PERMISSION_DENIED,
        Reason.SETUP_REQUIRED to Code.FAILED_PRECONDITION,
        Reason.TOKEN_MISSING to Code.UNAUTHENTICATED,
        Reason.TOKEN_MALFORMED to Code.UNAUTHENTICATED,
        Reason.TOKEN_REVOKED to Code.UNAUTHENTICATED,
        Reason.TOKEN_NOT_YET_VALID to Code.UNAUTHENTICATED,
        Reason.TOKEN_BAD_SIGNATURE to Code.UNAUTHENTICATED,
        Reason.TOKEN_WRONG_AUDIENCE to Code.UNAUTHENTICATED,
        Reason.TOKEN_WRONG_ISSUER to Code.UNAUTHENTICATED,
        Reason.TOKEN_UNKNOWN_KEY to Code.UNAUTHENTICATED,
        Reason.TOKEN_ALGORITHM_NOT_ALLOWED to Code.UNAUTHENTICATED,
        Reason.API_KEY_INVALID to Code.UNAUTHENTICATED,
        Reason.CREDENTIAL_NOT_ACCEPTED to Code.UNAUTHENTICATED,
        Reason.CREDENTIAL_IN_QUERY_STRING to Code.UNAUTHENTICATED,
        Reason.CSRF_TOKEN_INVALID to Code.PERMISSION_DENIED,
        Reason.PRINCIPAL_NOT_ALLOWED to Code.PERMISSION_DENIED,
        Reason.CORS_ORIGIN_NOT_ALLOWED to Code.PERMISSION_DENIED,
        Reason.AUTH_UNAVAILABLE to Code.UNAVAILABLE,
        Reason.INVALID_ARGUMENT to Code.INVALID_ARGUMENT,
        Reason.NOT_FOUND to Code.NOT_FOUND,
        Reason.ALREADY_EXISTS to Code.ALREADY_EXISTS,
        Reason.PERMISSION_DENIED to Code.PERMISSION_DENIED,
        Reason.TOKEN_EXPIRED to Code.UNAUTHENTICATED,
        Reason.UNAUTHENTICATED to Code.UNAUTHENTICATED,
        Reason.FAILED_PRECONDITION to Code.FAILED_PRECONDITION,
        Reason.RESOURCE_EXHAUSTED to Code.RESOURCE_EXHAUSTED,
        Reason.UNAVAILABLE to Code.UNAVAILABLE,
        Reason.DEADLINE_EXCEEDED to Code.DEADLINE_EXCEEDED,
        Reason.ABORTED to Code.ABORTED,
        Reason.INTERNAL to Code.INTERNAL,
    )

    /** Every reason in the registry, in the registry's order. */
    val all: List<Reason> get() = IN_REGISTRY_ORDER

    /** Every registry reason's wire name, in the registry's order. */
    val allNames: List<String> get() = IN_REGISTRY_ORDER.map { name(it) }

    /** The stable `snake_case` wire name, or the empty string for [Reason.NONE]. */
    fun name(reason: Reason): String = NAMES[reason] ?: ""

    /**
     * The reason a wire string names, or null for one this SDK's registry does not
     * have — which means the server is newer, and the caller must be told rather than
     * handed a default.
     */
    fun fromWire(wire: String): Reason? = BY_WIRE[wire]

    /** Whether a wire string is in this SDK's registry. */
    fun isKnown(wire: String): Boolean = BY_WIRE.containsKey(wire)

    /** The Connect code a reason is raised under, from the same registry. */
    fun codeOf(reason: Reason): Code = CODES[reason] ?: Code.UNKNOWN
}