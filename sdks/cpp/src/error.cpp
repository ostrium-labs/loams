// The typed error hierarchy, as `error.hpp` documents it.

#include "loams/error.hpp"

#include <array>
#include <utility>

#include "loams/reason.hpp"

namespace loams {
namespace {

struct CodeRow {
  Code code;
  const char* name;
};

/// The Connect codes, in the order §44 §7.4's hierarchy lists them. The wire
/// spellings are Connect's own (`unimplemented`, `resource_exhausted`, ...), the
/// same strings that appear in a JSON error body's `code` field and in a
/// `grpc-status` name.
constexpr CodeRow kCodes[] = {
    {Code::kOk, "ok"},
    {Code::kCanceled, "canceled"},
    {Code::kUnknown, "unknown"},
    {Code::kInvalidArgument, "invalid_argument"},
    {Code::kDeadlineExceeded, "deadline_exceeded"},
    {Code::kNotFound, "not_found"},
    {Code::kAlreadyExists, "already_exists"},
    {Code::kPermissionDenied, "permission_denied"},
    {Code::kResourceExhausted, "resource_exhausted"},
    {Code::kFailedPrecondition, "failed_precondition"},
    {Code::kAborted, "aborted"},
    {Code::kOutOfRange, "out_of_range"},
    {Code::kUnimplemented, "unimplemented"},
    {Code::kInternal, "internal"},
    {Code::kUnavailable, "unavailable"},
    {Code::kDataLoss, "data_loss"},
    {Code::kUnauthenticated, "unauthenticated"},
};

/// Builds the `what()` string. Kept in one place because every subclass inherits
/// the same formatting and a caller reads the message in a log, not in a `catch`.
std::string Describe(Code code, const std::string& rpc, Reason reason, const std::string& unknown_reason,
                     const std::string& message, const std::string& request_id) {
  std::string out;
  if (!rpc.empty()) {
    out += rpc;
    out += ": ";
  }
  out += ToString(code);
  const std::string reason_text = reason == Reason::kUnknown ? unknown_reason : std::string(ToString(reason));
  if (!reason_text.empty()) {
    out += " (";
    out += reason_text;
    out += ")";
  }
  if (!message.empty()) {
    out += ": ";
    out += message;
  }
  if (!request_id.empty()) {
    out += " [request_id=";
    out += request_id;
    out += "]";
  }
  return out;
}

}  // namespace

std::string_view ToString(Code code) {
  for (const CodeRow& row : kCodes) {
    if (row.code == code) {
      return row.name;
    }
  }
  return "<unknown code>";
}

std::optional<Code> CodeFromString(std::string_view name) {
  for (const CodeRow& row : kCodes) {
    if (name == row.name) {
      return row.code;
    }
  }
  // Absent rather than `kUnknown`: a string that does not parse means the server
  // is newer than this SDK, which is a fact the caller should see, not a reason
  // to lose the string. `LoamsError` keeps the raw text in `what()`.
  return std::nullopt;
}

Code CodeFromHttpStatus(long status) {
  // Only ever a fallback, for a Connect unary failure whose body did not carry a
  // code. Connect's own mapping, as its spec states it.
  switch (status) {
    case 400:
      return Code::kInternal;
    case 401:
      return Code::kUnauthenticated;
    case 403:
      return Code::kPermissionDenied;
    case 404:
      return Code::kNotFound;
    case 429:
      return Code::kUnavailable;
    case 502:
    case 503:
    case 504:
      return Code::kUnavailable;
    default:
      return Code::kUnknown;
  }
}

LoamsError::LoamsError(Code code, std::string rpc, Reason reason, std::string unknown_reason,
                       std::map<std::string, std::string> metadata, std::string hint, std::string message,
                       std::string request_id)
    : code_(code),
      reason_(reason),
      unknown_reason_(std::move(unknown_reason)),
      metadata_(std::move(metadata)),
      hint_(std::move(hint)),
      rpc_(std::move(rpc)),
      request_id_(std::move(request_id)),
      what_(Describe(code, rpc_, reason_, unknown_reason_, message, request_id_)) {}

FeatureNotInVariantError::FeatureNotInVariantError(std::string rpc,
                                                   std::map<std::string, std::string> metadata,
                                                   std::string hint, std::string message,
                                                   std::string request_id)
    : UnimplementedError(Code::kUnimplemented, std::move(rpc), Reason::kFeatureNotInVariant, std::string(),
                         std::move(metadata), std::move(hint), std::move(message), std::move(request_id)) {
  // Read out of the metadata rather than parsed out of the message: the variant
  // is structured context, and the message is for a person.
  // Read through the public accessor: the member is private, and a subclass
  // reaching into its base's storage is how a base's layout starts to matter.
  const std::map<std::string, std::string>& held = Metadata();
  const auto found = held.find("variant");
  if (found != held.end()) {
    variant_ = found->second;
  }
}

TransportError::TransportError(std::string rpc, std::string message)
    : LoamsError(Code::kUnknown, std::move(rpc), Reason::kUnknown, std::string(), {},
                 std::string(), std::move(message), std::string()) {}

void ThrowMapped(Code code, std::string rpc, Reason reason, std::string unknown_reason,
                 std::map<std::string, std::string> metadata, std::string hint, std::string message,
                 std::string request_id) {
  std::rethrow_exception(
      MakeLoamsError(code, std::move(rpc), reason, std::move(unknown_reason), std::move(metadata),
                     std::move(hint), std::move(message), std::move(request_id)));
}

std::exception_ptr MakeLoamsError(Code code, std::string rpc, Reason reason, std::string unknown_reason,
                                  std::map<std::string, std::string> metadata, std::string hint,
                                  std::string message, std::string request_id) {
  // `feature_not_in_variant` and `token_expired` are checked **before** the
  // code-to-class switch, because both are subclasses: a `switch` that fell
  // through to `UnimplementedError` would lose the variant and blur R5's
  // "a dedicated error type" into R1's "a second expiry is reported".
  if (reason == Reason::kFeatureNotInVariant || unknown_reason == "feature_not_in_variant") {
    return std::make_exception_ptr(FeatureNotInVariantError(std::move(rpc), std::move(metadata), std::move(hint),
                                                            std::move(message), std::move(request_id)));
  }

  // One `LoamsError` built once and copied into each subclass: every subclass
  // takes its arguments by value, so rebuilding the base per `case` would be a
  // dozen copies of the same string work, and a future field added to the base
  // would have to be threaded through twelve call sites.
  const LoamsError base(code, rpc, reason, unknown_reason, metadata, hint, message, request_id);

  switch (code) {
    case Code::kInvalidArgument:
      return std::make_exception_ptr(InvalidArgumentError(base));
    case Code::kNotFound:
      return std::make_exception_ptr(NotFoundError(base));
    case Code::kAlreadyExists:
      return std::make_exception_ptr(AlreadyExistsError(base));
    case Code::kPermissionDenied:
      return std::make_exception_ptr(PermissionDeniedError(base));
    case Code::kUnauthenticated:
      if (reason == Reason::kTokenExpired || unknown_reason == "token_expired") {
        return std::make_exception_ptr(TokenExpiredError(base));
      }
      return std::make_exception_ptr(UnauthenticatedError(base));
    case Code::kFailedPrecondition:
      return std::make_exception_ptr(FailedPreconditionError(base));
    case Code::kResourceExhausted:
      return std::make_exception_ptr(ResourceExhaustedError(base));
    case Code::kUnavailable:
      return std::make_exception_ptr(UnavailableError(base));
    case Code::kDeadlineExceeded:
      return std::make_exception_ptr(DeadlineExceededError(base));
    case Code::kAborted:
      return std::make_exception_ptr(AbortedError(base));
    case Code::kInternal:
      return std::make_exception_ptr(InternalError(base));
    case Code::kUnimplemented:
      return std::make_exception_ptr(UnimplementedError(base));
    default:
      // A code D611 does not name (`canceled`, `out_of_range`, `data_loss`) is the
      // base type itself.
      return std::make_exception_ptr(base);
  }
}

void ThrowInternal(std::string rpc, std::string message) {
  throw InternalError(Code::kInternal, std::move(rpc), Reason::kInternal, std::string(), {}, std::string(),
                      std::move(message), std::string());
}

Reason ReasonOf(const std::exception& error) {
  if (const auto* const loams = dynamic_cast<const LoamsError*>(&error)) {
    return loams->ReasonValue();
  }
  return Reason::kUnknown;
}

Code CodeOf(const std::exception& error) {
  if (const auto* const loams = dynamic_cast<const LoamsError*>(&error)) {
    return loams->CodeValue();
  }
  return Code::kUnknown;
}

bool IsLoamsError(const std::exception& error) { return dynamic_cast<const LoamsError*>(&error) != nullptr; }

bool IsTransportError(const std::exception& error) {
  return dynamic_cast<const TransportError*>(&error) != nullptr;
}

}  // namespace loams