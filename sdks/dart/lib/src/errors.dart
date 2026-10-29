import 'dart:convert';
import 'dart:typed_data';
import 'envelopes.dart';
import 'wire.dart';

class LoamsException implements Exception {
  final String message;
  final String code;
  final String? reason;
  final String? unknownReason;
  final int httpStatus;
  final List<dynamic> rawDetails;

  LoamsException(
    this.message, {
    this.code = 'unknown',
    this.reason,
    this.unknownReason,
    this.httpStatus = 500,
    this.rawDetails = const [],
  });

  @override
  String toString() => 'LoamsException: $message (code: $code, reason: $reason, httpStatus: $httpStatus)';
}

class ApprovalAlreadyDecidedException extends LoamsException {
  ApprovalAlreadyDecidedException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class ApprovalExpiredException extends LoamsException {
  ApprovalExpiredException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class ApprovalStaleRevisionException extends LoamsException {
  ApprovalStaleRevisionException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class RequesterCannotApproveException extends LoamsException {
  RequesterCannotApproveException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class ReasonRequiredException extends LoamsException {
  ReasonRequiredException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class StepUpRequiredException extends LoamsException {
  StepUpRequiredException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class NotImplementedException extends LoamsException {
  NotImplementedException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class FeatureNotInVariantException extends LoamsException {
  FeatureNotInVariantException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class HouseNotConfiguredException extends LoamsException {
  HouseNotConfiguredException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class HouseSyntaxErrorException extends LoamsException {
  HouseSyntaxErrorException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class HouseUnknownTableException extends LoamsException {
  HouseUnknownTableException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class HouseUnknownDatabaseException extends LoamsException {
  HouseUnknownDatabaseException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class HouseAccessDeniedException extends LoamsException {
  HouseAccessDeniedException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class HouseReadOnlyException extends LoamsException {
  HouseReadOnlyException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class HouseTimeoutException extends LoamsException {
  HouseTimeoutException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class HouseMemoryLimitException extends LoamsException {
  HouseMemoryLimitException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class HouseTooManyQueriesException extends LoamsException {
  HouseTooManyQueriesException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class HouseQuotaExceededException extends LoamsException {
  HouseQuotaExceededException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class HouseDisabledFunctionException extends LoamsException {
  HouseDisabledFunctionException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class HouseCancelledException extends LoamsException {
  HouseCancelledException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class HouseQueryNotFoundException extends LoamsException {
  HouseQueryNotFoundException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class HouseQueryFailedException extends LoamsException {
  HouseQueryFailedException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class ProjectNotFoundException extends LoamsException {
  ProjectNotFoundException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class BranchHasChildrenException extends LoamsException {
  BranchHasChildrenException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class BranchProtectedException extends LoamsException {
  BranchProtectedException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class LsnOutOfRetentionException extends LoamsException {
  LsnOutOfRetentionException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class EndpointExistsForBranchException extends LoamsException {
  EndpointExistsForBranchException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class ComputeStartFailedException extends LoamsException {
  ComputeStartFailedException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class UnsupportedPgVersionException extends LoamsException {
  UnsupportedPgVersionException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class BetaMajorRequiresOptInException extends LoamsException {
  BetaMajorRequiresOptInException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class ComputeMajorMismatchRefusedException extends LoamsException {
  ComputeMajorMismatchRefusedException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class ImportChecksumsEnabledException extends LoamsException {
  ImportChecksumsEnabledException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class QuotaExceededException extends LoamsException {
  QuotaExceededException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class StorageUnavailableException extends LoamsException {
  StorageUnavailableException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class SecretAlreadyIssuedException extends LoamsException {
  SecretAlreadyIssuedException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class RepoNotFoundException extends LoamsException {
  RepoNotFoundException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class RepoAlreadyExistsException extends LoamsException {
  RepoAlreadyExistsException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class RepoDeletingException extends LoamsException {
  RepoDeletingException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class RefStaleException extends LoamsException {
  RefStaleException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class RefProtectedException extends LoamsException {
  RefProtectedException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class OutOfScopeException extends LoamsException {
  OutOfScopeException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class ObjectFormatUnsupportedException extends LoamsException {
  ObjectFormatUnsupportedException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class ForkTooDeepException extends LoamsException {
  ForkTooDeepException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class SequencerUnavailableException extends LoamsException {
  SequencerUnavailableException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class IdempotencyKeyReusedException extends LoamsException {
  IdempotencyKeyReusedException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class WorkspaceNotFoundException extends LoamsException {
  WorkspaceNotFoundException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class WorkspaceFencedException extends LoamsException {
  WorkspaceFencedException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}
class BatchPartiallyAppliedException extends LoamsException {
  BatchPartiallyAppliedException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class WorkspaceReadOnlyException extends LoamsException {
  WorkspaceReadOnlyException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class WorkspaceNameTakenException extends LoamsException {
  WorkspaceNameTakenException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class ObjectMissingException extends LoamsException {
  ObjectMissingException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class SecretDetectedException extends LoamsException {
  SecretDetectedException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class EnvUnsealedException extends LoamsException {
  EnvUnsealedException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class EnvAccessDeniedException extends LoamsException {
  EnvAccessDeniedException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class EnvForeignReferenceException extends LoamsException {
  EnvForeignReferenceException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class MemoryStoreNotFoundException extends LoamsException {
  MemoryStoreNotFoundException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class MemorySpaceNotFoundException extends LoamsException {
  MemorySpaceNotFoundException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class MemorySpaceLimitException extends LoamsException {
  MemorySpaceLimitException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class MemoryManagedObjectException extends LoamsException {
  MemoryManagedObjectException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class MemoryBlockVersionMismatchException extends LoamsException {
  MemoryBlockVersionMismatchException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class MemoryBlockTooLargeException extends LoamsException {
  MemoryBlockTooLargeException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class MemoryCoreBudgetExceededException extends LoamsException {
  MemoryCoreBudgetExceededException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class MemoryContentTooLargeException extends LoamsException {
  MemoryContentTooLargeException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class MemorySecretDetectedException extends LoamsException {
  MemorySecretDetectedException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class MemoryPiiRefusedException extends LoamsException {
  MemoryPiiRefusedException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class MemoryModelNotConfiguredException extends LoamsException {
  MemoryModelNotConfiguredException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class MemoryModelNotLocalException extends LoamsException {
  MemoryModelNotLocalException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class MemoryTokenBudgetExhaustedException extends LoamsException {
  MemoryTokenBudgetExhaustedException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class MemoryExtractionFailedException extends LoamsException {
  MemoryExtractionFailedException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class MemoryTooManySpacesException extends LoamsException {
  MemoryTooManySpacesException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class MemoryCrossSpaceWriteException extends LoamsException {
  MemoryCrossSpaceWriteException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class MemoryNotYetAvailableException extends LoamsException {
  MemoryNotYetAvailableException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class CodeRepoNotIndexedException extends LoamsException {
  CodeRepoNotIndexedException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class CodeIndexBuildingException extends LoamsException {
  CodeIndexBuildingException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class CodeChangeTooLargeException extends LoamsException {
  CodeChangeTooLargeException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class CodeLanguageUnsupportedException extends LoamsException {
  CodeLanguageUnsupportedException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class CodeScipInvalidException extends LoamsException {
  CodeScipInvalidException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class LiveBadCursorException extends LoamsException {
  LiveBadCursorException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class LiveWorkerCrashedException extends LoamsException {
  LiveWorkerCrashedException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class LiveDeployConflictException extends LoamsException {
  LiveDeployConflictException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class LiveSchemaViolationException extends LoamsException {
  LiveSchemaViolationException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}
class FilterConflictException extends LoamsException {
  FilterConflictException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}
class SearchAfterNeedsTiebreakException extends LoamsException {
  SearchAfterNeedsTiebreakException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}
class InvalidConsistencyTokenException extends LoamsException {
  InvalidConsistencyTokenException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}
class NamespaceRequiredException extends LoamsException {
  NamespaceRequiredException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}
class SqlParamCountException extends LoamsException {
  SqlParamCountException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class LiveIndexNotReadyException extends LoamsException {
  LiveIndexNotReadyException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class LiveAppDisabledException extends LoamsException {
  LiveAppDisabledException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class LiveAppExistsException extends LoamsException {
  LiveAppExistsException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class LiveFunctionErrorException extends LoamsException {
  LiveFunctionErrorException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class LiveFunctionTimeoutException extends LoamsException {
  LiveFunctionTimeoutException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}
class ClientDeadlineExceededException extends LoamsException {
  ClientDeadlineExceededException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class LiveFunctionOutOfMemoryException extends LoamsException {
  LiveFunctionOutOfMemoryException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class LiveLimitExceededException extends LoamsException {
  LiveLimitExceededException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}
class ResultTooLargeException extends LoamsException {
  ResultTooLargeException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class LiveAppMismatchException extends LoamsException {
  LiveAppMismatchException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class LiveOriginNotAllowedException extends LoamsException {
  LiveOriginNotAllowedException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class SessionRotateException extends LoamsException {
  SessionRotateException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class SessionGoneException extends LoamsException {
  SessionGoneException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class NodeDrainingException extends LoamsException {
  NodeDrainingException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class RoleRequiredException extends LoamsException {
  RoleRequiredException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class SetupRequiredException extends LoamsException {
  SetupRequiredException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class TokenMissingException extends LoamsException {
  TokenMissingException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class TokenMalformedException extends LoamsException {
  TokenMalformedException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class TokenRevokedException extends LoamsException {
  TokenRevokedException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class TokenNotYetValidException extends LoamsException {
  TokenNotYetValidException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class TokenBadSignatureException extends LoamsException {
  TokenBadSignatureException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class TokenWrongAudienceException extends LoamsException {
  TokenWrongAudienceException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class TokenWrongIssuerException extends LoamsException {
  TokenWrongIssuerException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class TokenUnknownKeyException extends LoamsException {
  TokenUnknownKeyException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class TokenAlgorithmNotAllowedException extends LoamsException {
  TokenAlgorithmNotAllowedException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class ApiKeyInvalidException extends LoamsException {
  ApiKeyInvalidException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class CredentialNotAcceptedException extends LoamsException {
  CredentialNotAcceptedException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class CredentialInQueryStringException extends LoamsException {
  CredentialInQueryStringException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class CsrfTokenInvalidException extends LoamsException {
  CsrfTokenInvalidException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class PrincipalNotAllowedException extends LoamsException {
  PrincipalNotAllowedException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class CorsOriginNotAllowedException extends LoamsException {
  CorsOriginNotAllowedException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class AuthUnavailableException extends LoamsException {
  AuthUnavailableException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class TokenExpiredException extends LoamsException {
  TokenExpiredException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class UnauthenticatedException extends LoamsException {
  UnauthenticatedException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class DecisionProofInvalidException extends LoamsException {
  DecisionProofInvalidException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class InvalidDecisionException extends LoamsException {
  InvalidDecisionException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class PairingExpiredException extends LoamsException {
  PairingExpiredException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class PairingUsedException extends LoamsException {
  PairingUsedException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class DeviceRevokedException extends LoamsException {
  DeviceRevokedException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class PushTargetUnknownException extends LoamsException {
  PushTargetUnknownException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class InvalidArgumentException extends LoamsException {
  InvalidArgumentException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class NotFoundException extends LoamsException {
  NotFoundException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class AlreadyExistsException extends LoamsException {
  AlreadyExistsException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class PermissionDeniedException extends LoamsException {
  PermissionDeniedException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class FailedPreconditionException extends LoamsException {
  FailedPreconditionException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class ResourceExhaustedException extends LoamsException {
  ResourceExhaustedException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class UnavailableException extends LoamsException {
  UnavailableException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class DeadlineExceededException extends LoamsException {
  DeadlineExceededException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});

  /// R12: the call's budget ran out on the client; never retried.
  factory DeadlineExceededException.client() => DeadlineExceededException(
        'the call ran out of time on the client',
        code: 'deadline_exceeded',
        reason: 'client_deadline_exceeded',
        httpStatus: 504,
      );
}

class AbortedException extends LoamsException {
  AbortedException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

class InternalException extends LoamsException {
  InternalException(super.message, {super.code, super.reason, super.unknownReason, super.httpStatus, super.rawDetails});
}

typedef ExceptionConstructor = LoamsException Function(
  String message, {
  String code,
  String? reason,
  String? unknownReason,
  int httpStatus,
  List<dynamic> rawDetails,
});

class ErrorParser {
  static final Map<String, ExceptionConstructor> reasons = {
    'approval_expired': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        ApprovalExpiredException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'approval_already_decided': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        ApprovalAlreadyDecidedException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'approval_stale_revision': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        ApprovalStaleRevisionException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'requester_cannot_approve': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        RequesterCannotApproveException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'decision_proof_invalid': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        DecisionProofInvalidException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'step_up_required': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        StepUpRequiredException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'reason_required': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        ReasonRequiredException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'invalid_decision': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        InvalidDecisionException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'pairing_expired': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        PairingExpiredException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'pairing_used': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        PairingUsedException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'device_revoked': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        DeviceRevokedException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'push_target_unknown': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        PushTargetUnknownException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'not_implemented': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        NotImplementedException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'feature_not_in_variant': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        FeatureNotInVariantException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'house_not_configured': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        HouseNotConfiguredException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'house_syntax_error': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        HouseSyntaxErrorException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'house_unknown_table': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        HouseUnknownTableException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'house_unknown_database': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        HouseUnknownDatabaseException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'house_access_denied': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        HouseAccessDeniedException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'house_read_only': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        HouseReadOnlyException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'house_timeout': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        HouseTimeoutException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'house_memory_limit': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        HouseMemoryLimitException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'house_too_many_queries': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        HouseTooManyQueriesException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'house_quota_exceeded': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        HouseQuotaExceededException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'house_disabled_function': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        HouseDisabledFunctionException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'house_cancelled': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        HouseCancelledException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'house_query_not_found': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        HouseQueryNotFoundException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'house_query_failed': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        HouseQueryFailedException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'project_not_found': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        ProjectNotFoundException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'branch_has_children': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        BranchHasChildrenException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'branch_protected': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        BranchProtectedException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'lsn_out_of_retention': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        LsnOutOfRetentionException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'endpoint_exists_for_branch': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        EndpointExistsForBranchException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'compute_start_failed': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        ComputeStartFailedException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'unsupported_pg_version': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        UnsupportedPgVersionException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'beta_major_requires_opt_in': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        BetaMajorRequiresOptInException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'compute_major_mismatch_refused': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        ComputeMajorMismatchRefusedException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'import_checksums_enabled': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        ImportChecksumsEnabledException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'quota_exceeded': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        QuotaExceededException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'storage_unavailable': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        StorageUnavailableException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'secret_already_issued': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        SecretAlreadyIssuedException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'repo_not_found': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        RepoNotFoundException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'repo_already_exists': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        RepoAlreadyExistsException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'repo_deleting': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        RepoDeletingException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'ref_stale': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        RefStaleException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'ref_protected': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        RefProtectedException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'out_of_scope': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        OutOfScopeException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'object_format_unsupported': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        ObjectFormatUnsupportedException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'fork_too_deep': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        ForkTooDeepException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'sequencer_unavailable': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        SequencerUnavailableException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'idempotency_key_reused': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        IdempotencyKeyReusedException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'workspace_not_found': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        WorkspaceNotFoundException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'workspace_fenced': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        WorkspaceFencedException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'batch_partially_applied': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        BatchPartiallyAppliedException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'workspace_read_only': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        WorkspaceReadOnlyException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'workspace_name_taken': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        WorkspaceNameTakenException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'object_missing': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        ObjectMissingException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'secret_detected': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        SecretDetectedException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'env_unsealed': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        EnvUnsealedException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'env_access_denied': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        EnvAccessDeniedException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'env_foreign_reference': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        EnvForeignReferenceException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'memory_store_not_found': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        MemoryStoreNotFoundException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'memory_space_not_found': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        MemorySpaceNotFoundException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'memory_space_limit': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        MemorySpaceLimitException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'memory_managed_object': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        MemoryManagedObjectException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'memory_block_version_mismatch': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        MemoryBlockVersionMismatchException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'memory_block_too_large': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        MemoryBlockTooLargeException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'memory_core_budget_exceeded': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        MemoryCoreBudgetExceededException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'memory_content_too_large': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        MemoryContentTooLargeException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'memory_secret_detected': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        MemorySecretDetectedException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'memory_pii_refused': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        MemoryPiiRefusedException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'memory_model_not_configured': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        MemoryModelNotConfiguredException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'memory_model_not_local': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        MemoryModelNotLocalException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'memory_token_budget_exhausted': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        MemoryTokenBudgetExhaustedException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'memory_extraction_failed': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        MemoryExtractionFailedException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'memory_too_many_spaces': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        MemoryTooManySpacesException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'memory_cross_space_write': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        MemoryCrossSpaceWriteException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'memory_not_yet_available': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        MemoryNotYetAvailableException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'code_repo_not_indexed': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        CodeRepoNotIndexedException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'code_index_building': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        CodeIndexBuildingException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'code_change_too_large': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        CodeChangeTooLargeException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'code_language_unsupported': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        CodeLanguageUnsupportedException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'code_scip_invalid': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        CodeScipInvalidException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'live_bad_cursor': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        LiveBadCursorException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'live_worker_crashed': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        LiveWorkerCrashedException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'live_deploy_conflict': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        LiveDeployConflictException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'live_schema_violation': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        LiveSchemaViolationException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'filter_conflict': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        FilterConflictException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'search_after_needs_tiebreak': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        SearchAfterNeedsTiebreakException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'invalid_consistency_token': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        InvalidConsistencyTokenException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'namespace_required': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        NamespaceRequiredException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'sql_param_count': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        SqlParamCountException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'live_index_not_ready': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        LiveIndexNotReadyException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'live_app_disabled': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        LiveAppDisabledException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'live_app_exists': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        LiveAppExistsException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'live_function_error': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        LiveFunctionErrorException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'live_function_timeout': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        LiveFunctionTimeoutException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'client_deadline_exceeded': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        ClientDeadlineExceededException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'live_function_out_of_memory': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        LiveFunctionOutOfMemoryException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'live_limit_exceeded': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        LiveLimitExceededException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'result_too_large': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        ResultTooLargeException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'live_app_mismatch': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        LiveAppMismatchException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'live_origin_not_allowed': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        LiveOriginNotAllowedException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'session_rotate': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        SessionRotateException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'session_gone': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        SessionGoneException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'node_draining': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        NodeDrainingException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'role_required': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        RoleRequiredException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'setup_required': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        SetupRequiredException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'token_missing': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        TokenMissingException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'token_malformed': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        TokenMalformedException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'token_revoked': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        TokenRevokedException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'token_not_yet_valid': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        TokenNotYetValidException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'token_bad_signature': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        TokenBadSignatureException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'token_wrong_audience': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        TokenWrongAudienceException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'token_wrong_issuer': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        TokenWrongIssuerException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'token_unknown_key': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        TokenUnknownKeyException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'token_algorithm_not_allowed': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        TokenAlgorithmNotAllowedException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'api_key_invalid': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        ApiKeyInvalidException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'credential_not_accepted': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        CredentialNotAcceptedException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'credential_in_query_string': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        CredentialInQueryStringException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'csrf_token_invalid': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        CsrfTokenInvalidException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'principal_not_allowed': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        PrincipalNotAllowedException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'cors_origin_not_allowed': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        CorsOriginNotAllowedException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'auth_unavailable': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        AuthUnavailableException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'invalid_argument': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        InvalidArgumentException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'not_found': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        NotFoundException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'already_exists': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        AlreadyExistsException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'permission_denied': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        PermissionDeniedException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'token_expired': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        TokenExpiredException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'unauthenticated': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        UnauthenticatedException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'failed_precondition': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        FailedPreconditionException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'resource_exhausted': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        ResourceExhaustedException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'unavailable': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        UnavailableException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'deadline_exceeded': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        DeadlineExceededException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'aborted': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        AbortedException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
    'internal': (msg, {code = 'unknown', reason, unknownReason, httpStatus = 500, rawDetails = const []}) =>
        InternalException(msg, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails),
  };

  static Uint8List decodeBase64Safe(String b64) {
    b64 = b64.trim().replaceAll('-', '+').replaceAll('_', '/');
    final pad = b64.length % 4;
    if (pad > 0) {
      b64 += '=' * (4 - pad);
    }
    return base64Decode(b64);
  }

  static String? decodeErrorInfo(Uint8List binaryPayload) {
    try {
      final bytes = Wire.findBytes(binaryPayload, 1);
      if (bytes != null) {
        return utf8.decode(bytes);
      }
    } catch (_) {}
    return null;
  }

  static String? extractFromStatusDetailsBin(Uint8List bin) {
    try {
      for (final field in Wire.eachField(bin)) {
        if (field.field == 3 && field.wireType == 2) {
          final anyBytes = field.value as Uint8List;
          final anyVal = Wire.findBytes(anyBytes, 2);
          if (anyVal != null) {
            final reason = decodeErrorInfo(anyVal);
            if (reason != null) {
              return reason;
            }
          }
        }
      }
    } catch (_) {}
    return null;
  }

  static LoamsException parse(int httpStatus, dynamic body) {
    String code = 'unknown';
    String message = 'HTTP $httpStatus';
    String? reason;
    String? unknownReason;
    List<dynamic> rawDetails = [];

    if (body is Uint8List) {
      if (body.length >= 5) {
        final frames = Envelopes.split(body);
        for (final frame in frames) {
          if (frame.isTrailer) {
            body = utf8.decode(frame.payload, allowMalformed: true);
            break;
          }
        }
      }
      if (body is Uint8List) {
        body = utf8.decode(body, allowMalformed: true);
      }
    }

    if (body is String) {
      if (body.length >= 5) {
        try {
          final raw = utf8.encode(body);
          final frames = Envelopes.split(raw);
          for (final frame in frames) {
            if (frame.isTrailer) {
              body = utf8.decode(frame.payload, allowMalformed: true);
              break;
            }
          }
        } catch (_) {}
      }

      if (body is String && body.contains('grpc-status-details-bin:')) {
        final match = RegExp(r'grpc-status-details-bin:\s*([^\r\n]+)').firstMatch(body);
        if (match != null) {
          final bin = decodeBase64Safe(match.group(1)!);
          final extracted = extractFromStatusDetailsBin(bin);
          if (extracted != null) {
            reason = extracted;
          }
        }
      }

      try {
        body = jsonDecode(body);
      } catch (_) {}
    }

    if (body is Map) {
      if (body['error'] is Map) {
        body = body['error'];
      }

      if (body['code'] != null) {
        code = body['code'].toString();
      }
      if (body['message'] != null) {
        message = body['message'].toString();
      }

      if (body['details'] is List) {
        rawDetails = body['details'] as List;
        for (final detail in rawDetails) {
          if (detail is Map) {
            final type = detail['@type']?.toString() ?? detail['type']?.toString() ?? '';
            if (type.endsWith('ErrorInfo')) {
              if (detail['reason'] != null) {
                final r = detail['reason'].toString();
                reason = r;
                if (!reasons.containsKey(r)) {
                  unknownReason = r;
                }
              }
            }
            if (detail['value'] is String) {
              final bin = decodeBase64Safe(detail['value'] as String);
              final r = decodeErrorInfo(bin);
              if (r != null) {
                reason = r;
                if (!reasons.containsKey(r)) {
                  unknownReason = r;
                }
              }
            }
            if (detail['debug'] is String) {
              final bin = decodeBase64Safe(detail['debug'] as String);
              final r = decodeErrorInfo(bin);
              if (r != null) {
                reason = r;
                if (!reasons.containsKey(r)) {
                  unknownReason = r;
                }
              }
            }
          } else if (detail is String) {
            final bin = decodeBase64Safe(detail);
            final r = decodeErrorInfo(bin);
            if (r != null) {
              reason = r;
              if (!reasons.containsKey(r)) {
                unknownReason = r;
              }
            }
          }
        }
      }
    }

    if (reason != null && reasons.containsKey(reason)) {
      return reasons[reason]!(message, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails);
    }

    if (code == 'unauthenticated' || httpStatus == 401) {
      return UnauthenticatedException(message, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails);
    }

    // No class for the reason: the code's own class, so `on NotFoundException`
    // catches every `not_found`.
    if (reasons.containsKey(code)) {
      return reasons[code]!(message, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails);
    }

    return LoamsException(message, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails);
  }
}
