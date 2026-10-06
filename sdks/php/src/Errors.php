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
class TokenExpiredException extends LoamsException {}
class UnauthenticatedException extends LoamsException {}
class DecisionProofInvalidException extends LoamsException {}
class InvalidDecisionException extends LoamsException {}
class PairingExpiredException extends LoamsException {}
class PairingUsedException extends LoamsException {}
class DeviceRevokedException extends LoamsException {}
class PushTargetUnknownException extends LoamsException {}
class InvalidArgumentException extends LoamsException {}
class NotFoundException extends LoamsException {}
class AlreadyExistsException extends LoamsException {}
class PermissionDeniedException extends LoamsException {}
class FailedPreconditionException extends LoamsException {}
class ResourceExhaustedException extends LoamsException {}
class UnavailableException extends LoamsException {}
class DeadlineExceededException extends LoamsException {}
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

        return new LoamsException($message, $code, $reason, $unknownReason, $httpStatus, $rawDetails);
    }
}
