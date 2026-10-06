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

    return LoamsException(message, code: code, reason: reason, unknownReason: unknownReason, httpStatus: httpStatus, rawDetails: rawDetails);
  }
}
