package dev.loams.facade;

/**
 * The stable, machine-readable cause of a failed RPC (design §44 §7.4, D611).
 *
 * <p>It is generated from {@code docs/api/reasons.md} in a renderer that does not
 * exist yet, so this is the <b>Q604 hand-written-facade fallback</b> that design §44
 * §7.3 permits ("If the plugin proves too costly for a language, that language falls back
 * to a hand-written facade checked by the same conformance suite"). {@code
 * sdks/go/gen/facade/reason.go} is the same fallback in Go. See {@code README.md}.
 *
 * <p>It is an {@code enum}, not a string, which is the whole point: a caller switching on
 * it is exhaustive over the registry this SDK was built against, and a reason the registry
 * has lost <em>stops compiling</em> rather than silently becoming a default arm.
 *
 * <p>{@link #fromWire(String)} is how a value off the wire becomes one of these, and it
 * returns {@code null} for a reason this registry does not have. That case is a server
 * newer than this SDK, and the runtime surfaces it rather than dropping it — see R8 in
 * {@code docs/sdk/runtime-contract.md}.
 */
public enum Reason {
    APPROVAL_EXPIRED("approval_expired"),
    APPROVAL_ALREADY_DECIDED("approval_already_decided"),
    APPROVAL_STALE_REVISION("approval_stale_revision"),
    REQUESTER_CANNOT_APPROVE("requester_cannot_approve"),
    DECISION_PROOF_INVALID("decision_proof_invalid"),
    STEP_UP_REQUIRED("step_up_required"),
    REASON_REQUIRED("reason_required"),
    INVALID_DECISION("invalid_decision"),
    PAIRING_EXPIRED("pairing_expired"),
    PAIRING_USED("pairing_used"),
    DEVICE_REVOKED("device_revoked"),
    PUSH_TARGET_UNKNOWN("push_target_unknown"),
    NOT_IMPLEMENTED("not_implemented"),
    FEATURE_NOT_IN_VARIANT("feature_not_in_variant"),
    HOUSE_NOT_CONFIGURED("house_not_configured"),
    HOUSE_SYNTAX_ERROR("house_syntax_error"),
    HOUSE_UNKNOWN_TABLE("house_unknown_table"),
    HOUSE_UNKNOWN_DATABASE("house_unknown_database"),
    HOUSE_ACCESS_DENIED("house_access_denied"),
    HOUSE_READ_ONLY("house_read_only"),
    HOUSE_TIMEOUT("house_timeout"),
    HOUSE_MEMORY_LIMIT("house_memory_limit"),
    HOUSE_TOO_MANY_QUERIES("house_too_many_queries"),
    HOUSE_QUOTA_EXCEEDED("house_quota_exceeded"),
    HOUSE_DISABLED_FUNCTION("house_disabled_function"),
    HOUSE_CANCELLED("house_cancelled"),
    HOUSE_QUERY_NOT_FOUND("house_query_not_found"),
    HOUSE_QUERY_FAILED("house_query_failed"),
    PROJECT_NOT_FOUND("project_not_found"),
    BRANCH_HAS_CHILDREN("branch_has_children"),
    BRANCH_PROTECTED("branch_protected"),
    LSN_OUT_OF_RETENTION("lsn_out_of_retention"),
    ENDPOINT_EXISTS_FOR_BRANCH("endpoint_exists_for_branch"),
    COMPUTE_START_FAILED("compute_start_failed"),
    UNSUPPORTED_PG_VERSION("unsupported_pg_version"),
    BETA_MAJOR_REQUIRES_OPT_IN("beta_major_requires_opt_in"),
    COMPUTE_MAJOR_MISMATCH_REFUSED("compute_major_mismatch_refused"),
    IMPORT_CHECKSUMS_ENABLED("import_checksums_enabled"),
    QUOTA_EXCEEDED("quota_exceeded"),
    STORAGE_UNAVAILABLE("storage_unavailable"),
    SECRET_ALREADY_ISSUED("secret_already_issued"),
    REPO_NOT_FOUND("repo_not_found"),
    REPO_ALREADY_EXISTS("repo_already_exists"),
    REPO_DELETING("repo_deleting"),
    REF_STALE("ref_stale"),
    REF_PROTECTED("ref_protected"),
    OUT_OF_SCOPE("out_of_scope"),
    OBJECT_FORMAT_UNSUPPORTED("object_format_unsupported"),
    FORK_TOO_DEEP("fork_too_deep"),
    SEQUENCER_UNAVAILABLE("sequencer_unavailable"),
    IDEMPOTENCY_KEY_REUSED("idempotency_key_reused"),
    WORKSPACE_NOT_FOUND("workspace_not_found"),
    WORKSPACE_FENCED("workspace_fenced"),
    BATCH_PARTIALLY_APPLIED("batch_partially_applied"),
    WORKSPACE_READ_ONLY("workspace_read_only"),
    WORKSPACE_NAME_TAKEN("workspace_name_taken"),
    OBJECT_MISSING("object_missing"),
    SECRET_DETECTED("secret_detected"),
    ENV_UNSEALED("env_unsealed"),
    ENV_ACCESS_DENIED("env_access_denied"),
    ENV_FOREIGN_REFERENCE("env_foreign_reference"),
    MEMORY_STORE_NOT_FOUND("memory_store_not_found"),
    MEMORY_SPACE_NOT_FOUND("memory_space_not_found"),
    MEMORY_SPACE_LIMIT("memory_space_limit"),
    MEMORY_MANAGED_OBJECT("memory_managed_object"),
    MEMORY_BLOCK_VERSION_MISMATCH("memory_block_version_mismatch"),
    MEMORY_BLOCK_TOO_LARGE("memory_block_too_large"),
    MEMORY_CORE_BUDGET_EXCEEDED("memory_core_budget_exceeded"),
    MEMORY_CONTENT_TOO_LARGE("memory_content_too_large"),
    MEMORY_SECRET_DETECTED("memory_secret_detected"),
    MEMORY_PII_REFUSED("memory_pii_refused"),
    MEMORY_MODEL_NOT_CONFIGURED("memory_model_not_configured"),
    MEMORY_MODEL_NOT_LOCAL("memory_model_not_local"),
    MEMORY_TOKEN_BUDGET_EXHAUSTED("memory_token_budget_exhausted"),
    MEMORY_EXTRACTION_FAILED("memory_extraction_failed"),
    MEMORY_TOO_MANY_SPACES("memory_too_many_spaces"),
    MEMORY_CROSS_SPACE_WRITE("memory_cross_space_write"),
    MEMORY_NOT_YET_AVAILABLE("memory_not_yet_available"),
    CODE_REPO_NOT_INDEXED("code_repo_not_indexed"),
    CODE_INDEX_BUILDING("code_index_building"),
    CODE_CHANGE_TOO_LARGE("code_change_too_large"),
    CODE_LANGUAGE_UNSUPPORTED("code_language_unsupported"),
    CODE_SCIP_INVALID("code_scip_invalid"),
    LIVE_BAD_CURSOR("live_bad_cursor"),
    LIVE_WORKER_CRASHED("live_worker_crashed"),
    LIVE_DEPLOY_CONFLICT("live_deploy_conflict"),
    LIVE_SCHEMA_VIOLATION("live_schema_violation"),
    FILTER_CONFLICT("filter_conflict"),
    SEARCH_AFTER_NEEDS_TIEBREAK("search_after_needs_tiebreak"),
    INVALID_CONSISTENCY_TOKEN("invalid_consistency_token"),
    NAMESPACE_REQUIRED("namespace_required"),
    SQL_PARAM_COUNT("sql_param_count"),
    LIVE_INDEX_NOT_READY("live_index_not_ready"),
    LIVE_APP_DISABLED("live_app_disabled"),
    LIVE_APP_EXISTS("live_app_exists"),
    LIVE_FUNCTION_ERROR("live_function_error"),
    LIVE_FUNCTION_TIMEOUT("live_function_timeout"),
    CLIENT_DEADLINE_EXCEEDED("client_deadline_exceeded"),
    LIVE_FUNCTION_OUT_OF_MEMORY("live_function_out_of_memory"),
    LIVE_LIMIT_EXCEEDED("live_limit_exceeded"),
    RESULT_TOO_LARGE("result_too_large"),
    LIVE_APP_MISMATCH("live_app_mismatch"),
    LIVE_ORIGIN_NOT_ALLOWED("live_origin_not_allowed"),
    SESSION_ROTATE("session_rotate"),
    SESSION_GONE("session_gone"),
    NODE_DRAINING("node_draining"),
    ROLE_REQUIRED("role_required"),
    SETUP_REQUIRED("setup_required"),
    TOKEN_MISSING("token_missing"),
    TOKEN_MALFORMED("token_malformed"),
    TOKEN_REVOKED("token_revoked"),
    TOKEN_NOT_YET_VALID("token_not_yet_valid"),
    TOKEN_BAD_SIGNATURE("token_bad_signature"),
    TOKEN_WRONG_AUDIENCE("token_wrong_audience"),
    TOKEN_WRONG_ISSUER("token_wrong_issuer"),
    TOKEN_UNKNOWN_KEY("token_unknown_key"),
    TOKEN_ALGORITHM_NOT_ALLOWED("token_algorithm_not_allowed"),
    API_KEY_INVALID("api_key_invalid"),
    CREDENTIAL_NOT_ACCEPTED("credential_not_accepted"),
    CREDENTIAL_IN_QUERY_STRING("credential_in_query_string"),
    CSRF_TOKEN_INVALID("csrf_token_invalid"),
    PRINCIPAL_NOT_ALLOWED("principal_not_allowed"),
    CORS_ORIGIN_NOT_ALLOWED("cors_origin_not_allowed"),
    AUTH_UNAVAILABLE("auth_unavailable"),
    INVALID_ARGUMENT("invalid_argument"),
    NOT_FOUND("not_found"),
    ALREADY_EXISTS("already_exists"),
    PERMISSION_DENIED("permission_denied"),
    TOKEN_EXPIRED("token_expired"),
    UNAUTHENTICATED("unauthenticated"),
    FAILED_PRECONDITION("failed_precondition"),
    RESOURCE_EXHAUSTED("resource_exhausted"),
    UNAVAILABLE("unavailable"),
    DEADLINE_EXCEEDED("deadline_exceeded"),
    ABORTED("aborted"),
    INTERNAL("internal");

    private final String wire;

    Reason(String wire) {
        this.wire = wire;
    }

    /** The snake_case value on the wire, and in {@code docs/api/reasons.md}. */
    public String wire() {
        return wire;
    }

    @Override
    public String toString() {
        return wire;
    }

    /**
     * The reason a {@code reason} string names, or {@code null} when this SDK's registry
     * does not have it.
     *
     * <p>A {@code null} return means <em>the server is newer than this SDK</em>. It is not
     * an error and it is not the same as "no reason at all": a failure from below the API
     * carries no reason because no service answered, while this one carries a reason that
     * simply postdates the registry. Conflating them would leave a caller unable to tell
     * "not supported here" from "not supported at all" (R8).
     */
    public static Reason fromWire(String value) {
        if (value == null || value.isEmpty()) {
            return null;
        }
        for (Reason reason : VALUES) {
            if (reason.wire.equals(value)) {
                return reason;
            }
        }
        return null;
    }

    /** Every reason in the registry, in {@code docs/api/reasons.md}'s order. */
    public static Reason[] all() {
        return VALUES.clone();
    }

    private static final Reason[] VALUES = values();
}