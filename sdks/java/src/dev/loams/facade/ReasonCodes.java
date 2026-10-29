package dev.loams.facade;

import java.util.Arrays;
import java.util.Collections;
import java.util.EnumMap;
import java.util.List;
import java.util.Map;
import java.util.Optional;

/**
 * The reason registry as the runtime sees it: every reason, the Connect code each one is
 * raised under, and the lookup the error mapper needs.
 *
 * <p>The codes come from {@code docs/api/reasons.md} and are the same in every SDK, so a
 * caller that wants the coarse class has it without parsing anything.
 *
 * <p>Provenance is the Q604 hand-written-facade fallback; see {@link Reason}.
 */
public final class ReasonCodes {

    private ReasonCodes() {}

    private static final Map<Reason, String> CODES = codes();

    private static Map<Reason, String> codes() {
        Map<Reason, String> map = new EnumMap<>(Reason.class);
        map.put(Reason.APPROVAL_EXPIRED, "failed_precondition");
        map.put(Reason.APPROVAL_ALREADY_DECIDED, "failed_precondition");
        map.put(Reason.APPROVAL_STALE_REVISION, "failed_precondition");
        map.put(Reason.REQUESTER_CANNOT_APPROVE, "permission_denied");
        map.put(Reason.DECISION_PROOF_INVALID, "permission_denied");
        map.put(Reason.STEP_UP_REQUIRED, "unauthenticated");
        map.put(Reason.REASON_REQUIRED, "invalid_argument");
        map.put(Reason.INVALID_DECISION, "invalid_argument");
        map.put(Reason.PAIRING_EXPIRED, "failed_precondition");
        map.put(Reason.PAIRING_USED, "failed_precondition");
        map.put(Reason.DEVICE_REVOKED, "unauthenticated");
        map.put(Reason.PUSH_TARGET_UNKNOWN, "not_found");
        map.put(Reason.NOT_IMPLEMENTED, "unimplemented");
        map.put(Reason.FEATURE_NOT_IN_VARIANT, "unimplemented");
        map.put(Reason.HOUSE_NOT_CONFIGURED, "unimplemented");
        map.put(Reason.HOUSE_SYNTAX_ERROR, "invalid_argument");
        map.put(Reason.HOUSE_UNKNOWN_TABLE, "not_found");
        map.put(Reason.HOUSE_UNKNOWN_DATABASE, "not_found");
        map.put(Reason.HOUSE_ACCESS_DENIED, "permission_denied");
        map.put(Reason.HOUSE_READ_ONLY, "failed_precondition");
        map.put(Reason.HOUSE_TIMEOUT, "deadline_exceeded");
        map.put(Reason.HOUSE_MEMORY_LIMIT, "resource_exhausted");
        map.put(Reason.HOUSE_TOO_MANY_QUERIES, "resource_exhausted");
        map.put(Reason.HOUSE_QUOTA_EXCEEDED, "resource_exhausted");
        map.put(Reason.HOUSE_DISABLED_FUNCTION, "permission_denied");
        map.put(Reason.HOUSE_CANCELLED, "canceled");
        map.put(Reason.HOUSE_QUERY_NOT_FOUND, "not_found");
        map.put(Reason.HOUSE_QUERY_FAILED, "unknown");
        map.put(Reason.PROJECT_NOT_FOUND, "not_found");
        map.put(Reason.BRANCH_HAS_CHILDREN, "failed_precondition");
        map.put(Reason.BRANCH_PROTECTED, "failed_precondition");
        map.put(Reason.BETA_MAJOR_REQUIRES_OPT_IN, "failed_precondition");
        map.put(Reason.COMPUTE_MAJOR_MISMATCH_REFUSED, "failed_precondition");
        map.put(Reason.IMPORT_CHECKSUMS_ENABLED, "failed_precondition");
        map.put(Reason.LSN_OUT_OF_RETENTION, "failed_precondition");
        map.put(Reason.ENDPOINT_EXISTS_FOR_BRANCH, "already_exists");
        map.put(Reason.COMPUTE_START_FAILED, "unavailable");
        map.put(Reason.QUOTA_EXCEEDED, "resource_exhausted");
        map.put(Reason.STORAGE_UNAVAILABLE, "unavailable");
        map.put(Reason.SECRET_ALREADY_ISSUED, "failed_precondition");
        map.put(Reason.REPO_NOT_FOUND, "not_found");
        map.put(Reason.REPO_ALREADY_EXISTS, "already_exists");
        map.put(Reason.REPO_DELETING, "failed_precondition");
        map.put(Reason.REF_STALE, "aborted");
        map.put(Reason.REF_PROTECTED, "failed_precondition");
        map.put(Reason.OUT_OF_SCOPE, "permission_denied");
        map.put(Reason.OBJECT_FORMAT_UNSUPPORTED, "unimplemented");
        map.put(Reason.FORK_TOO_DEEP, "failed_precondition");
        map.put(Reason.SEQUENCER_UNAVAILABLE, "unavailable");
        map.put(Reason.IDEMPOTENCY_KEY_REUSED, "failed_precondition");
        map.put(Reason.WORKSPACE_NOT_FOUND, "not_found");
        map.put(Reason.WORKSPACE_FENCED, "aborted");
        map.put(Reason.BATCH_PARTIALLY_APPLIED, "aborted");
        map.put(Reason.WORKSPACE_READ_ONLY, "failed_precondition");
        map.put(Reason.WORKSPACE_NAME_TAKEN, "already_exists");
        map.put(Reason.OBJECT_MISSING, "failed_precondition");
        map.put(Reason.SECRET_DETECTED, "failed_precondition");
        map.put(Reason.ENV_UNSEALED, "invalid_argument");
        map.put(Reason.ENV_ACCESS_DENIED, "permission_denied");
        map.put(Reason.ENV_FOREIGN_REFERENCE, "permission_denied");
        map.put(Reason.MEMORY_STORE_NOT_FOUND, "not_found");
        map.put(Reason.MEMORY_SPACE_NOT_FOUND, "not_found");
        map.put(Reason.MEMORY_SPACE_LIMIT, "resource_exhausted");
        map.put(Reason.MEMORY_MANAGED_OBJECT, "failed_precondition");
        map.put(Reason.MEMORY_BLOCK_VERSION_MISMATCH, "aborted");
        map.put(Reason.MEMORY_BLOCK_TOO_LARGE, "invalid_argument");
        map.put(Reason.MEMORY_CORE_BUDGET_EXCEEDED, "resource_exhausted");
        map.put(Reason.MEMORY_CONTENT_TOO_LARGE, "invalid_argument");
        map.put(Reason.MEMORY_SECRET_DETECTED, "failed_precondition");
        map.put(Reason.MEMORY_PII_REFUSED, "failed_precondition");
        map.put(Reason.MEMORY_MODEL_NOT_CONFIGURED, "failed_precondition");
        map.put(Reason.MEMORY_MODEL_NOT_LOCAL, "failed_precondition");
        map.put(Reason.MEMORY_TOKEN_BUDGET_EXHAUSTED, "resource_exhausted");
        map.put(Reason.MEMORY_EXTRACTION_FAILED, "failed_precondition");
        map.put(Reason.MEMORY_TOO_MANY_SPACES, "invalid_argument");
        map.put(Reason.MEMORY_CROSS_SPACE_WRITE, "invalid_argument");
        map.put(Reason.MEMORY_NOT_YET_AVAILABLE, "unimplemented");
        map.put(Reason.CODE_REPO_NOT_INDEXED, "failed_precondition");
        map.put(Reason.CODE_INDEX_BUILDING, "unavailable");
        map.put(Reason.CODE_CHANGE_TOO_LARGE, "invalid_argument");
        map.put(Reason.CODE_LANGUAGE_UNSUPPORTED, "failed_precondition");
        map.put(Reason.CODE_SCIP_INVALID, "invalid_argument");
        map.put(Reason.LIVE_BAD_CURSOR, "invalid_argument");
        map.put(Reason.LIVE_WORKER_CRASHED, "internal");
        map.put(Reason.LIVE_DEPLOY_CONFLICT, "failed_precondition");
        map.put(Reason.LIVE_SCHEMA_VIOLATION, "invalid_argument");
        map.put(Reason.FILTER_CONFLICT, "invalid_argument");
        map.put(Reason.SEARCH_AFTER_NEEDS_TIEBREAK, "invalid_argument");
        map.put(Reason.INVALID_CONSISTENCY_TOKEN, "invalid_argument");
        map.put(Reason.NAMESPACE_REQUIRED, "invalid_argument");
        map.put(Reason.SQL_PARAM_COUNT, "invalid_argument");
        map.put(Reason.UNSUPPORTED_PG_VERSION, "invalid_argument");
        map.put(Reason.LIVE_INDEX_NOT_READY, "failed_precondition");
        map.put(Reason.LIVE_APP_DISABLED, "failed_precondition");
        map.put(Reason.LIVE_APP_EXISTS, "failed_precondition");
        map.put(Reason.LIVE_FUNCTION_ERROR, "unknown");
        map.put(Reason.LIVE_FUNCTION_TIMEOUT, "deadline_exceeded");
        map.put(Reason.CLIENT_DEADLINE_EXCEEDED, "deadline_exceeded");
        map.put(Reason.LIVE_FUNCTION_OUT_OF_MEMORY, "resource_exhausted");
        map.put(Reason.LIVE_LIMIT_EXCEEDED, "resource_exhausted");
        map.put(Reason.RESULT_TOO_LARGE, "resource_exhausted");
        map.put(Reason.LIVE_APP_MISMATCH, "permission_denied");
        map.put(Reason.LIVE_ORIGIN_NOT_ALLOWED, "permission_denied");
        map.put(Reason.SESSION_ROTATE, "unavailable");
        map.put(Reason.SESSION_GONE, "not_found");
        map.put(Reason.NODE_DRAINING, "unavailable");
        map.put(Reason.ROLE_REQUIRED, "permission_denied");
        map.put(Reason.SETUP_REQUIRED, "failed_precondition");
        map.put(Reason.TOKEN_MISSING, "unauthenticated");
        map.put(Reason.TOKEN_MALFORMED, "unauthenticated");
        map.put(Reason.TOKEN_REVOKED, "unauthenticated");
        map.put(Reason.TOKEN_NOT_YET_VALID, "unauthenticated");
        map.put(Reason.TOKEN_BAD_SIGNATURE, "unauthenticated");
        map.put(Reason.TOKEN_WRONG_AUDIENCE, "unauthenticated");
        map.put(Reason.TOKEN_WRONG_ISSUER, "unauthenticated");
        map.put(Reason.TOKEN_UNKNOWN_KEY, "unauthenticated");
        map.put(Reason.TOKEN_ALGORITHM_NOT_ALLOWED, "unauthenticated");
        map.put(Reason.API_KEY_INVALID, "unauthenticated");
        map.put(Reason.CREDENTIAL_NOT_ACCEPTED, "unauthenticated");
        map.put(Reason.CREDENTIAL_IN_QUERY_STRING, "unauthenticated");
        map.put(Reason.CSRF_TOKEN_INVALID, "permission_denied");
        map.put(Reason.PRINCIPAL_NOT_ALLOWED, "permission_denied");
        map.put(Reason.CORS_ORIGIN_NOT_ALLOWED, "permission_denied");
        map.put(Reason.AUTH_UNAVAILABLE, "unavailable");
        map.put(Reason.INVALID_ARGUMENT, "invalid_argument");
        map.put(Reason.NOT_FOUND, "not_found");
        map.put(Reason.ALREADY_EXISTS, "already_exists");
        map.put(Reason.PERMISSION_DENIED, "permission_denied");
        map.put(Reason.TOKEN_EXPIRED, "unauthenticated");
        map.put(Reason.UNAUTHENTICATED, "unauthenticated");
        map.put(Reason.FAILED_PRECONDITION, "failed_precondition");
        map.put(Reason.RESOURCE_EXHAUSTED, "resource_exhausted");
        map.put(Reason.UNAVAILABLE, "unavailable");
        map.put(Reason.DEADLINE_EXCEEDED, "deadline_exceeded");
        map.put(Reason.ABORTED, "aborted");
        map.put(Reason.INTERNAL, "internal");
        return Collections.unmodifiableMap(map);
    }

    /**
     * The Connect code a reason is raised under, or {@code null} for a reason this registry
     * does not have — in which case the wire's own code is the classification, and losing it
     * would be worse than not knowing the reason.
     */
    public static String of(Reason reason) {
        return reason == null ? null : CODES.get(reason);
    }

    /** The reason registry's wire strings, in registry order. */
    public static List<String> wireValues() {
        return Arrays.stream(Reason.all()).map(Reason::wire).toList();
    }

    /**
     * Every reason the registry has, or a reason it does not have, which is the registry's
     * whole vocabulary in one call. The error-reason test iterates it rather than a
     * hand-written list, so a reason added to {@link Reason} is covered the day it lands.
     */
    public static Optional<Reason> lookup(String wire) {
        return Optional.ofNullable(Reason.fromWire(wire));
    }
}