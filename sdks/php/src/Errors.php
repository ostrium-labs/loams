<?php

declare(strict_types=1);

namespace Loams;

class LoamsException extends \RuntimeException
{
    public readonly string $errorCode;
    public readonly ?string $reason;
    public readonly ?string $unknownReason;
    public readonly int $httpStatus;
    public readonly array $rawDetails;

    public function __construct(
        string $message,
        string $errorCode = 'unknown',
        ?string $reason = null,
        ?string $unknownReason = null,
        int $httpStatus = 500,
        array $rawDetails = [],
        ?\Throwable $previous = null
    ) {
        parent::__construct($message, 0, $previous);
        $this->errorCode = $errorCode;
        $this->reason = $reason;
        $this->unknownReason = $unknownReason;
        $this->httpStatus = $httpStatus;
        $this->rawDetails = $rawDetails;
    }

    public function __get(string $name): mixed
    {
        if ($name === 'code') {
            return $this->errorCode;
        }
        return null;
    }
}

class ApprovalAlreadyDecidedException extends LoamsException {}
class ApprovalExpiredException extends LoamsException {}
class ApprovalStaleRevisionException extends LoamsException {}
class RequesterCannotApproveException extends LoamsException {}
class ReasonRequiredException extends LoamsException {}
class StepUpRequiredException extends LoamsException {}
class NotImplementedException extends LoamsException {}
class FeatureNotInVariantException extends LoamsException {}
class HouseNotConfiguredException extends LoamsException {}
class HouseSyntaxErrorException extends LoamsException {}
class HouseUnknownTableException extends LoamsException {}
class HouseUnknownDatabaseException extends LoamsException {}
class HouseAccessDeniedException extends LoamsException {}
class HouseReadOnlyException extends LoamsException {}
class HouseTimeoutException extends LoamsException {}
class HouseMemoryLimitException extends LoamsException {}
class HouseTooManyQueriesException extends LoamsException {}
class HouseQuotaExceededException extends LoamsException {}
class HouseDisabledFunctionException extends LoamsException {}
class HouseCancelledException extends LoamsException {}
class HouseQueryNotFoundException extends LoamsException {}
class HouseQueryFailedException extends LoamsException {}
class ProjectNotFoundException extends LoamsException {}
class BranchHasChildrenException extends LoamsException {}
class BranchProtectedException extends LoamsException {}
class LsnOutOfRetentionException extends LoamsException {}
class EndpointExistsForBranchException extends LoamsException {}
class ComputeStartFailedException extends LoamsException {}
class QuotaExceededException extends LoamsException {}
class StorageUnavailableException extends LoamsException {}
class SecretAlreadyIssuedException extends LoamsException {}
class RepoNotFoundException extends LoamsException {}
class RepoAlreadyExistsException extends LoamsException {}
class RepoDeletingException extends LoamsException {}
class RefStaleException extends LoamsException {}
class RefProtectedException extends LoamsException {}
class OutOfScopeException extends LoamsException {}
class ObjectFormatUnsupportedException extends LoamsException {}
class ForkTooDeepException extends LoamsException {}
class SequencerUnavailableException extends LoamsException {}
class IdempotencyKeyReusedException extends LoamsException {}
class WorkspaceNotFoundException extends LoamsException {}
class WorkspaceFencedException extends LoamsException {}
class BatchPartiallyAppliedException extends LoamsException {}
class WorkspaceReadOnlyException extends LoamsException {}
class WorkspaceNameTakenException extends LoamsException {}
class ObjectMissingException extends LoamsException {}
class SecretDetectedException extends LoamsException {}
class EnvUnsealedException extends LoamsException {}
class EnvAccessDeniedException extends LoamsException {}
class EnvForeignReferenceException extends LoamsException {}
class MemoryStoreNotFoundException extends LoamsException {}
class MemorySpaceNotFoundException extends LoamsException {}
class MemorySpaceLimitException extends LoamsException {}
class MemoryManagedObjectException extends LoamsException {}
class MemoryBlockVersionMismatchException extends LoamsException {}
class MemoryBlockTooLargeException extends LoamsException {}
class MemoryCoreBudgetExceededException extends LoamsException {}
class MemoryContentTooLargeException extends LoamsException {}
class MemorySecretDetectedException extends LoamsException {}
class MemoryPiiRefusedException extends LoamsException {}
class MemoryModelNotConfiguredException extends LoamsException {}
class MemoryModelNotLocalException extends LoamsException {}
class MemoryTokenBudgetExhaustedException extends LoamsException {}
class MemoryExtractionFailedException extends LoamsException {}
class MemoryTooManySpacesException extends LoamsException {}
class MemoryCrossSpaceWriteException extends LoamsException {}
class MemoryNotYetAvailableException extends LoamsException {}
class CodeRepoNotIndexedException extends LoamsException {}
class CodeIndexBuildingException extends LoamsException {}
class CodeChangeTooLargeException extends LoamsException {}
class CodeLanguageUnsupportedException extends LoamsException {}
class CodeScipInvalidException extends LoamsException {}
class LiveBadCursorException extends LoamsException {}
class LiveWorkerCrashedException extends LoamsException {}
class LiveDeployConflictException extends LoamsException {}
class LiveSchemaViolationException extends LoamsException {}
class FilterConflictException extends LoamsException {}
class SearchAfterNeedsTiebreakException extends LoamsException {}
class InvalidConsistencyTokenException extends LoamsException {}
class NamespaceRequiredException extends LoamsException {}
class SqlParamCountException extends LoamsException {}
class LiveIndexNotReadyException extends LoamsException {}
class LiveAppDisabledException extends LoamsException {}
class LiveAppExistsException extends LoamsException {}
class LiveFunctionErrorException extends LoamsException {}
class LiveFunctionTimeoutException extends LoamsException {}
class ClientDeadlineExceededException extends LoamsException {}
class LiveFunctionOutOfMemoryException extends LoamsException {}
class LiveLimitExceededException extends LoamsException {}
class ResultTooLargeException extends LoamsException {}
class LiveAppMismatchException extends LoamsException {}
class LiveOriginNotAllowedException extends LoamsException {}
class SessionRotateException extends LoamsException {}
class SessionGoneException extends LoamsException {}
class NodeDrainingException extends LoamsException {}
class RoleRequiredException extends LoamsException {}
class SetupRequiredException extends LoamsException {}
class TokenMissingException extends LoamsException {}
class TokenMalformedException extends LoamsException {}
class TokenRevokedException extends LoamsException {}
class TokenNotYetValidException extends LoamsException {}
class TokenBadSignatureException extends LoamsException {}
class TokenWrongAudienceException extends LoamsException {}
class TokenWrongIssuerException extends LoamsException {}
class TokenUnknownKeyException extends LoamsException {}
class TokenAlgorithmNotAllowedException extends LoamsException {}
class ApiKeyInvalidException extends LoamsException {}
class CredentialNotAcceptedException extends LoamsException {}
class CredentialInQueryStringException extends LoamsException {}
class CsrfTokenInvalidException extends LoamsException {}
class PrincipalNotAllowedException extends LoamsException {}
class CorsOriginNotAllowedException extends LoamsException {}
class AuthUnavailableException extends LoamsException {}
class TokenExpiredException extends LoamsException {}
class UnauthenticatedException extends LoamsException {}
class DecisionProofInvalidException extends LoamsException {}
class InvalidDecisionException extends LoamsException {}
class PairingExpiredException extends LoamsException {}
class PairingUsedException extends LoamsException {}
class DeviceRevokedException extends LoamsException {}
class PushTargetUnknownException extends LoamsException {}
class InvalidArgumentException extends LoamsException
{
    /** Refused on the client, before any network call. */
    public static function client(string $message, ?string $reason = null): self
    {
        return new self($message, 'invalid_argument', $reason, null, 400);
    }
}
class NotFoundException extends LoamsException {}
class AlreadyExistsException extends LoamsException {}
class PermissionDeniedException extends LoamsException {}
class FailedPreconditionException extends LoamsException {}
class ResourceExhaustedException extends LoamsException {}
class UnavailableException extends LoamsException {}
class DeadlineExceededException extends LoamsException
{
    /** R12: the call's budget ran out on the client; never retried. */
    public static function client(): self
    {
        return new self('the call ran out of time on the client', 'deadline_exceeded', 'client_deadline_exceeded', null, 504);
    }
}
class AbortedException extends LoamsException {}
class InternalException extends LoamsException {}

class ErrorParser
{
    public const REASONS = [
        'approval_expired' => ApprovalExpiredException::class,
        'approval_already_decided' => ApprovalAlreadyDecidedException::class,
        'approval_stale_revision' => ApprovalStaleRevisionException::class,
        'requester_cannot_approve' => RequesterCannotApproveException::class,
        'decision_proof_invalid' => DecisionProofInvalidException::class,
        'step_up_required' => StepUpRequiredException::class,
        'reason_required' => ReasonRequiredException::class,
        'invalid_decision' => InvalidDecisionException::class,
        'pairing_expired' => PairingExpiredException::class,
        'pairing_used' => PairingUsedException::class,
        'device_revoked' => DeviceRevokedException::class,
        'push_target_unknown' => PushTargetUnknownException::class,
        'not_implemented' => NotImplementedException::class,
        'feature_not_in_variant' => FeatureNotInVariantException::class,
        'house_not_configured' => HouseNotConfiguredException::class,
        'house_syntax_error' => HouseSyntaxErrorException::class,
        'house_unknown_table' => HouseUnknownTableException::class,
        'house_unknown_database' => HouseUnknownDatabaseException::class,
        'house_access_denied' => HouseAccessDeniedException::class,
        'house_read_only' => HouseReadOnlyException::class,
        'house_timeout' => HouseTimeoutException::class,
        'house_memory_limit' => HouseMemoryLimitException::class,
        'house_too_many_queries' => HouseTooManyQueriesException::class,
        'house_quota_exceeded' => HouseQuotaExceededException::class,
        'house_disabled_function' => HouseDisabledFunctionException::class,
        'house_cancelled' => HouseCancelledException::class,
        'house_query_not_found' => HouseQueryNotFoundException::class,
        'house_query_failed' => HouseQueryFailedException::class,
        'project_not_found' => ProjectNotFoundException::class,
        'branch_has_children' => BranchHasChildrenException::class,
        'branch_protected' => BranchProtectedException::class,
        'lsn_out_of_retention' => LsnOutOfRetentionException::class,
        'endpoint_exists_for_branch' => EndpointExistsForBranchException::class,
        'compute_start_failed' => ComputeStartFailedException::class,
        'quota_exceeded' => QuotaExceededException::class,
        'storage_unavailable' => StorageUnavailableException::class,
        'secret_already_issued' => SecretAlreadyIssuedException::class,
        'repo_not_found' => RepoNotFoundException::class,
        'repo_already_exists' => RepoAlreadyExistsException::class,
        'repo_deleting' => RepoDeletingException::class,
        'ref_stale' => RefStaleException::class,
        'ref_protected' => RefProtectedException::class,
        'out_of_scope' => OutOfScopeException::class,
        'object_format_unsupported' => ObjectFormatUnsupportedException::class,
        'fork_too_deep' => ForkTooDeepException::class,
        'sequencer_unavailable' => SequencerUnavailableException::class,
        'idempotency_key_reused' => IdempotencyKeyReusedException::class,
        'workspace_not_found' => WorkspaceNotFoundException::class,
        'workspace_fenced' => WorkspaceFencedException::class,
        'batch_partially_applied' => BatchPartiallyAppliedException::class,
        'workspace_read_only' => WorkspaceReadOnlyException::class,
        'workspace_name_taken' => WorkspaceNameTakenException::class,
        'object_missing' => ObjectMissingException::class,
        'secret_detected' => SecretDetectedException::class,
        'env_unsealed' => EnvUnsealedException::class,
        'env_access_denied' => EnvAccessDeniedException::class,
        'env_foreign_reference' => EnvForeignReferenceException::class,
        'memory_store_not_found' => MemoryStoreNotFoundException::class,
        'memory_space_not_found' => MemorySpaceNotFoundException::class,
        'memory_space_limit' => MemorySpaceLimitException::class,
        'memory_managed_object' => MemoryManagedObjectException::class,
        'memory_block_version_mismatch' => MemoryBlockVersionMismatchException::class,
        'memory_block_too_large' => MemoryBlockTooLargeException::class,
        'memory_core_budget_exceeded' => MemoryCoreBudgetExceededException::class,
        'memory_content_too_large' => MemoryContentTooLargeException::class,
        'memory_secret_detected' => MemorySecretDetectedException::class,
        'memory_pii_refused' => MemoryPiiRefusedException::class,
        'memory_model_not_configured' => MemoryModelNotConfiguredException::class,
        'memory_model_not_local' => MemoryModelNotLocalException::class,
        'memory_token_budget_exhausted' => MemoryTokenBudgetExhaustedException::class,
        'memory_extraction_failed' => MemoryExtractionFailedException::class,
        'memory_too_many_spaces' => MemoryTooManySpacesException::class,
        'memory_cross_space_write' => MemoryCrossSpaceWriteException::class,
        'memory_not_yet_available' => MemoryNotYetAvailableException::class,
        'code_repo_not_indexed' => CodeRepoNotIndexedException::class,
        'code_index_building' => CodeIndexBuildingException::class,
        'code_change_too_large' => CodeChangeTooLargeException::class,
        'code_language_unsupported' => CodeLanguageUnsupportedException::class,
        'code_scip_invalid' => CodeScipInvalidException::class,
        'live_bad_cursor' => LiveBadCursorException::class,
        'live_worker_crashed' => LiveWorkerCrashedException::class,
        'live_deploy_conflict' => LiveDeployConflictException::class,
        'live_schema_violation' => LiveSchemaViolationException::class,
        'filter_conflict' => FilterConflictException::class,
        'search_after_needs_tiebreak' => SearchAfterNeedsTiebreakException::class,
        'invalid_consistency_token' => InvalidConsistencyTokenException::class,
        'namespace_required' => NamespaceRequiredException::class,
        'sql_param_count' => SqlParamCountException::class,
        'live_index_not_ready' => LiveIndexNotReadyException::class,
        'live_app_disabled' => LiveAppDisabledException::class,
        'live_app_exists' => LiveAppExistsException::class,
        'live_function_error' => LiveFunctionErrorException::class,
        'live_function_timeout' => LiveFunctionTimeoutException::class,
        'client_deadline_exceeded' => ClientDeadlineExceededException::class,
        'live_function_out_of_memory' => LiveFunctionOutOfMemoryException::class,
        'live_limit_exceeded' => LiveLimitExceededException::class,
        'result_too_large' => ResultTooLargeException::class,
        'live_app_mismatch' => LiveAppMismatchException::class,
        'live_origin_not_allowed' => LiveOriginNotAllowedException::class,
        'session_rotate' => SessionRotateException::class,
        'session_gone' => SessionGoneException::class,
        'node_draining' => NodeDrainingException::class,
        'role_required' => RoleRequiredException::class,
        'setup_required' => SetupRequiredException::class,
        'token_missing' => TokenMissingException::class,
        'token_malformed' => TokenMalformedException::class,
        'token_revoked' => TokenRevokedException::class,
        'token_not_yet_valid' => TokenNotYetValidException::class,
        'token_bad_signature' => TokenBadSignatureException::class,
        'token_wrong_audience' => TokenWrongAudienceException::class,
        'token_wrong_issuer' => TokenWrongIssuerException::class,
        'token_unknown_key' => TokenUnknownKeyException::class,
        'token_algorithm_not_allowed' => TokenAlgorithmNotAllowedException::class,
        'api_key_invalid' => ApiKeyInvalidException::class,
        'credential_not_accepted' => CredentialNotAcceptedException::class,
        'credential_in_query_string' => CredentialInQueryStringException::class,
        'csrf_token_invalid' => CsrfTokenInvalidException::class,
        'principal_not_allowed' => PrincipalNotAllowedException::class,
        'cors_origin_not_allowed' => CorsOriginNotAllowedException::class,
        'auth_unavailable' => AuthUnavailableException::class,
        'invalid_argument' => InvalidArgumentException::class,
        'not_found' => NotFoundException::class,
        'already_exists' => AlreadyExistsException::class,
        'permission_denied' => PermissionDeniedException::class,
        'token_expired' => TokenExpiredException::class,
        'unauthenticated' => UnauthenticatedException::class,
        'failed_precondition' => FailedPreconditionException::class,
        'resource_exhausted' => ResourceExhaustedException::class,
        'unavailable' => UnavailableException::class,
        'deadline_exceeded' => DeadlineExceededException::class,
        'aborted' => AbortedException::class,
        'internal' => InternalException::class,
    ];

    public static function decodeBase64Safe(string $b64): string
    {
        $b64 = trim($b64);
        $b64 = strtr($b64, '-_', '+/');
        $pad = strlen($b64) % 4;
        if ($pad > 0) {
            $b64 .= str_repeat('=', 4 - $pad);
        }
        $decoded = base64_decode($b64, true);
        return $decoded !== false ? $decoded : '';
    }

    public static function decodeErrorInfo(string $binaryPayload): ?string
    {
        try {
            return Wire::findBytes($binaryPayload, 1);
        } catch (\Throwable) {
            return null;
        }
    }

    public static function extractFromStatusDetailsBin(string $bin): ?string
    {
        try {
            foreach (Wire::eachField($bin) as $field) {
                if ($field['field'] === 3 && $field['wireType'] === 2) {
                    $anyBytes = (string) $field['value'];
                    $anyVal = Wire::findBytes($anyBytes, 2);
                    if ($anyVal !== null) {
                        $reason = self::decodeErrorInfo($anyVal);
                        if ($reason !== null) {
                            return $reason;
                        }
                    }
                }
            }
        } catch (\Throwable) {
        }
        return null;
    }

    /**
     * @param array<string, mixed>|string $body
     */
    public static function parse(int $httpStatus, array|string $body): LoamsException
    {
        $code = 'unknown';
        $message = "HTTP {$httpStatus}";
        $reason = null;
        $unknownReason = null;
        $rawDetails = [];

        // Check if body is framed gRPC-Web / Connect
        if (is_string($body)) {
            if (strlen($body) >= 5) {
                $frames = Envelopes::split($body);
                foreach ($frames as $frame) {
                    if ($frame->isTrailer()) {
                        $body = $frame->payload;
                        break;
                    }
                }
            }

            if (str_contains($body, 'grpc-status-details-bin:')) {
                if (preg_match('/grpc-status-details-bin:\s*([^\r\n]+)/', $body, $m)) {
                    $bin = self::decodeBase64Safe($m[1]);
                    $extracted = self::extractFromStatusDetailsBin($bin);
                    if ($extracted !== null) {
                        $reason = $extracted;
                    }
                }
            }

            $parsed = json_decode($body, true);
            if (is_array($parsed)) {
                $body = $parsed;
            }
        }

        if (is_array($body)) {
            if (isset($body['error']) && is_array($body['error'])) {
                $body = $body['error'];
            }
            if (isset($body['code'])) {
                $code = (string) $body['code'];
            }
            if (isset($body['message'])) {
                $message = (string) $body['message'];
            }
            if (isset($body['details']) && is_array($body['details'])) {
                $rawDetails = $body['details'];
                foreach ($body['details'] as $detail) {
                    if (is_array($detail)) {
                        $type = $detail['@type'] ?? $detail['type'] ?? '';
                        if (str_ends_with($type, 'ErrorInfo')) {
                            if (isset($detail['reason'])) {
                                $reasonCandidate = (string) $detail['reason'];
                                $reason = $reasonCandidate;
                                if (!isset(self::REASONS[$reasonCandidate])) {
                                    $unknownReason = $reasonCandidate;
                                }
                            }
                        }
                        if (isset($detail['value']) && is_string($detail['value'])) {
                            $bin = self::decodeBase64Safe($detail['value']);
                            $extractedReason = self::decodeErrorInfo($bin);
                            if ($extractedReason !== null) {
                                $reason = $extractedReason;
                                if (!isset(self::REASONS[$extractedReason])) {
                                    $unknownReason = $extractedReason;
                                }
                            }
                        }
                        if (isset($detail['debug']) && is_string($detail['debug'])) {
                            $bin = self::decodeBase64Safe($detail['debug']);
                            $extractedReason = self::decodeErrorInfo($bin);
                            if ($extractedReason !== null) {
                                $reason = $extractedReason;
                                if (!isset(self::REASONS[$extractedReason])) {
                                    $unknownReason = $extractedReason;
                                }
                            }
                        }
                    } elseif (is_string($detail)) {
                        $bin = self::decodeBase64Safe($detail);
                        $extractedReason = self::decodeErrorInfo($bin);
                        if ($extractedReason !== null) {
                            $reason = $extractedReason;
                            if (!isset(self::REASONS[$extractedReason])) {
                                $unknownReason = $extractedReason;
                            }
                        }
                    }
                }
            }
        }

        if ($reason !== null && isset(self::REASONS[$reason])) {
            $class = self::REASONS[$reason];
            return new $class($message, $code, $reason, $unknownReason, $httpStatus, $rawDetails);
        }

        if ($code === 'unauthenticated' || $httpStatus === 401) {
            return new UnauthenticatedException($message, $code, $reason, $unknownReason, $httpStatus, $rawDetails);
        }

        $byCode = [
            'invalid_argument' => InvalidArgumentException::class,
            'not_found' => NotFoundException::class,
            'already_exists' => AlreadyExistsException::class,
            'permission_denied' => PermissionDeniedException::class,
            'failed_precondition' => FailedPreconditionException::class,
            'resource_exhausted' => ResourceExhaustedException::class,
            'unavailable' => UnavailableException::class,
            'deadline_exceeded' => DeadlineExceededException::class,
            'aborted' => AbortedException::class,
            'internal' => InternalException::class,
        ];
        if (isset($byCode[$code])) {
            $class = $byCode[$code];
            return new $class($message, $code, $reason, $unknownReason, $httpStatus, $rawDetails);
        }

        return new LoamsException($message, $code, $reason, $unknownReason, $httpStatus, $rawDetails);
    }
}
