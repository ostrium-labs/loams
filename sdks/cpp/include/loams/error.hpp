// The typed error hierarchy of design §44 §7.4, decision D611; runtime
// contract R8.
//
// A failed RPC carries a Connect code and one `loams.errors.v1.ErrorInfo` in
// its details. The **code** gives the class (a taxonomy that does not change
// within a major version) and the **`reason`** is the stable branch. The
// message is for a person and may change; nothing in an SDK branches on it.
//
// In C++ the branch is a type:
//
//     try {
//       client.Instance().GetInstance(request, &response);
//     } catch (const loams::NotFoundError& error) {
//       // the named resource does not exist; error.Reason() is a Reason
//     } catch (const loams::FeatureNotInVariantError& error) {
//       // not in this build variant; error.Variant() names which
//     }
//
// and for a reason rather than a class, `catch (const loams::LoamsError& e)`
// and then `switch (e.Reason())`, which is exhaustive over `Reason` — a
// `default:` is not required, and adding a reason to the registry is a
// compile error in every caller that switches on it.
//
// Every failure the SDK raises is a `std::exception`, so ordinary `catch
// (const std::exception&)` still works for a caller who does not care which
// layer it came from.

#ifndef LOAMS_ERROR_HPP
#define LOAMS_ERROR_HPP

#include <exception>
#include <map>
#include <memory>
#include <ostream>
#include <string>
#include <string_view>

#include "loams/reason.hpp"

namespace loams {

/// The Connect code, the canonical classification of a failure. It does not
/// change within a major version.
enum class Code {
  kOk = 0,
  kCanceled,
  kUnknown,
  kInvalidArgument,
  kDeadlineExceeded,
  kNotFound,
  kAlreadyExists,
  kPermissionDenied,
  kResourceExhausted,
  kFailedPrecondition,
  kAborted,
  kOutOfRange,
  kUnimplemented,
  kInternal,
  kUnavailable,
  kDataLoss,
  kUnauthenticated,
};

/// The wire spelling of a code, as Connect writes it in a JSON error body.
std::string_view ToString(Code code);

/// The code a wire string names, or `std::nullopt` for a string this SDK does
/// not know. A string that does not parse is **not** silently `kUnknown`: it is
/// returned as an unknown reason on the error, so a newer server's code cannot
/// be mistaken for `unknown`.
std::optional<Code> CodeFromString(std::string_view name);

/// The wire spelling, for a log or a test's failure message. See the same operator
/// on `Reason` for why both exist.
std::ostream& operator<<(std::ostream& out, Code code);

/// The code an HTTP status maps to, for a Connect unary failure where the body
/// carried no code. Connect answers a failed RPC with 501 for
/// `unimplemented` and 400-family statuses for the rest; the mapping is only a
/// fallback for a body that did not parse, never the primary source.
Code CodeFromHttpStatus(long status);

/// What every Loams failure carries. The concrete types below are its
/// subclasses; a failure whose code D611 does not name (`canceled`,
/// `out_of_range`, `data_loss`) is a `LoamsError` itself.
class LoamsError : public std::exception {
 public:
  LoamsError(Code code, std::string rpc, Reason reason, std::string unknown_reason,
             std::map<std::string, std::string> metadata, std::string hint,
             std::string message, std::string request_id);

  /// The Connect code, the canonical classification.
  Code CodeValue() const noexcept { return code_; }
  /// The stable cause, when the server sent an `ErrorInfo`. `kUnknown` means
  /// the failure came from below the API — a socket, a timeout, a cancelled
  /// call — not from a Loams service.
  Reason ReasonValue() const noexcept { return reason_; }
  /// A reason off the wire that this SDK's registry does not have, meaning the
  /// server is newer than the SDK. Surfaced rather than dropped (R8). Empty
  /// unless `ReasonValue() == Reason::kUnknown` with a reason string present.
  const std::string& UnknownReason() const noexcept { return unknown_reason_; }
  /// Structured context the server sent, for example `{"variant": "standard"}`.
  /// Never secrets.
  const std::map<std::string, std::string>& Metadata() const noexcept { return metadata_; }
  /// A short next step in the caller's locale, when the server sent one.
  const std::string& Hint() const noexcept { return hint_; }
  /// The RPC that failed, as `package.Service/Method`.
  const std::string& Rpc() const noexcept { return rpc_; }
  /// The server's request id, when it sent one: the thing to quote in a bug
  /// report.
  const std::string& RequestId() const noexcept { return request_id_; }

  const char* what() const noexcept override { return what_.c_str(); }

 private:
  Code code_;
  Reason reason_;
  std::string unknown_reason_;
  std::map<std::string, std::string> metadata_;
  std::string hint_;
  std::string rpc_;
  std::string request_id_;
  std::string what_;
};

// One subclass per code D611 names, so the branch is a type. None adds a field
// of its own, because the branch is what a caller wants and the reason is on
// the base — which is why each takes a `LoamsError` **by value** and copies it:
// the mapping builds the base once and hands a copy to whichever subclass the
// code names, so adding a field to the base is one edit rather than thirteen.
class InvalidArgumentError : public LoamsError {
 public:
  /// Copies the base: the mapping builds one and hands a copy to whichever
  /// subclass the code names.
  explicit InvalidArgumentError(const LoamsError& base) : LoamsError(base) {}
  using LoamsError::LoamsError;
};
class NotFoundError : public LoamsError {
 public:
  /// Copies the base: the mapping builds one and hands a copy to whichever
  /// subclass the code names.
  explicit NotFoundError(const LoamsError& base) : LoamsError(base) {}
  using LoamsError::LoamsError;
};
class AlreadyExistsError : public LoamsError {
 public:
  /// Copies the base: the mapping builds one and hands a copy to whichever
  /// subclass the code names.
  explicit AlreadyExistsError(const LoamsError& base) : LoamsError(base) {}
  using LoamsError::LoamsError;
};
class PermissionDeniedError : public LoamsError {
 public:
  /// Copies the base: the mapping builds one and hands a copy to whichever
  /// subclass the code names.
  explicit PermissionDeniedError(const LoamsError& base) : LoamsError(base) {}
  using LoamsError::LoamsError;
};
class UnauthenticatedError : public LoamsError {
 public:
  /// Copies the base: the mapping builds one and hands a copy to whichever
  /// subclass the code names.
  explicit UnauthenticatedError(const LoamsError& base) : LoamsError(base) {}
  using LoamsError::LoamsError;
};
class FailedPreconditionError : public LoamsError {
 public:
  /// Copies the base: the mapping builds one and hands a copy to whichever
  /// subclass the code names.
  explicit FailedPreconditionError(const LoamsError& base) : LoamsError(base) {}
  using LoamsError::LoamsError;
};
class ResourceExhaustedError : public LoamsError {
 public:
  /// Copies the base: the mapping builds one and hands a copy to whichever
  /// subclass the code names.
  explicit ResourceExhaustedError(const LoamsError& base) : LoamsError(base) {}
  using LoamsError::LoamsError;
};
class UnavailableError : public LoamsError {
 public:
  /// Copies the base: the mapping builds one and hands a copy to whichever
  /// subclass the code names.
  explicit UnavailableError(const LoamsError& base) : LoamsError(base) {}
  using LoamsError::LoamsError;
};
class DeadlineExceededError : public LoamsError {
 public:
  /// Copies the base: the mapping builds one and hands a copy to whichever
  /// subclass the code names.
  explicit DeadlineExceededError(const LoamsError& base) : LoamsError(base) {}
  using LoamsError::LoamsError;
};
class AbortedError : public LoamsError {
 public:
  /// Copies the base: the mapping builds one and hands a copy to whichever
  /// subclass the code names.
  explicit AbortedError(const LoamsError& base) : LoamsError(base) {}
  using LoamsError::LoamsError;
};
class InternalError : public LoamsError {
 public:
  /// Copies the base: the mapping builds one and hands a copy to whichever
  /// subclass the code names.
  explicit InternalError(const LoamsError& base) : LoamsError(base) {}
  using LoamsError::LoamsError;
};
class UnimplementedError : public LoamsError {
 public:
  /// Copies the base: the mapping builds one and hands a copy to whichever
  /// subclass the code names.
  explicit UnimplementedError(const LoamsError& base) : LoamsError(base) {}
  using LoamsError::LoamsError;
};

/// A package this build variant does not carry (design §44 §4, D600; R5).
///
/// The server answers `unimplemented` with `reason = feature_not_in_variant`
/// and names the variant in `metadata.variant`. A caller usually never gets
/// here: `loams::System::Guard` feature-detects from `GetInstance.services[]`
/// before calling, so an unavailable module raises **this same type** from the
/// guard with no request spent. One `catch (const FeatureNotInVariantError&)`
/// therefore covers "the guard said no" and "the server refused", which is the
/// whole point of the guard raising the same type.
class FeatureNotInVariantError : public UnimplementedError {
 public:
  FeatureNotInVariantError(std::string rpc, std::map<std::string, std::string> metadata, std::string hint,
                           std::string message, std::string request_id);

  /// The build variant that was asked for, from `metadata.variant`. Empty if
  /// the server did not send one — which is a server bug, not a reason to
  /// guess, so it is reported as empty rather than defaulted to a name.
  const std::string& Variant() const noexcept { return variant_; }

 private:
  std::string variant_;
};

/// A token the server rejected as expired: `unauthenticated` with reason
/// `token_expired`. The runtime refreshes once and retries once (D608, R1); a
/// second expiry reaches the caller as this type.
class TokenExpiredError : public UnauthenticatedError {
 public:
  explicit TokenExpiredError(const LoamsError& base) : UnauthenticatedError(base) {}
  using UnauthenticatedError::UnauthenticatedError;
};

/// A failure from **below the API**: a refused connection, a DNS failure, a
/// timeout, a body that did not parse. It carries no `reason`, because nothing
/// at this layer knows one — which is the third of the three cases R8 keeps
/// distinct, and the reason `Reason()` returning `kUnknown` on a
/// `TransportError` is a fact and not a gap.
class TransportError : public LoamsError {
 public:
  TransportError(std::string rpc, std::string message);
};

/// Builds the exception a failure maps to, as an `exception_ptr`, without
/// throwing it.
///
/// The stream reader needs an `exception_ptr` rather than a throw, and a second
/// construction path would be a second answer to "what type is this".
///
/// **`std::exception_ptr`, not `std::unique_ptr<LoamsError>`.** A caller that
/// wrote `std::make_exception_ptr(*MakeLoamsError(...))` would **slice** the
/// derived object down to its base: a `feature_not_in_variant` refusal would
/// arrive as a plain `LoamsError` and R5's "one `catch` covers the guard and the
/// refusal" would be true for unary calls and false for streams. Building the
/// `exception_ptr` from the derived type in the first place is the only spelling
/// that cannot slice.
std::exception_ptr MakeLoamsError(Code code, std::string rpc, Reason reason, std::string unknown_reason,
                                  std::map<std::string, std::string> metadata, std::string hint,
                                  std::string message, std::string request_id);

/// Builds the type a code names, and throws it.
///
/// `unknown_reason` is the raw registry string when the registry string is one
/// this SDK does not have; `reason` is `kUnknown` in that case. Mapping is
/// idempotent in the sense R8 requires: a caller that catches a `LoamsError`
/// and re-raises it through here gets the same type back, which is what
/// "returned unchanged if mapped twice" means in a language without an
/// `errors.As` chain.
[[noreturn]] void ThrowMapped(Code code, std::string rpc, Reason reason, std::string unknown_reason,
                              std::map<std::string, std::string> metadata, std::string hint,
                              std::string message, std::string request_id);

/// The SDK's own failures — a binding that does not resolve, a request whose
/// shape it cannot read. They are `internal` because they are bugs in this
/// package, not in the caller or the server.
[[noreturn]] void ThrowInternal(std::string rpc, std::string message);

/// The reason a caught exception carries, or `Reason::kUnknown` when it is not
/// a `LoamsError` — which includes any `std::exception` a caller threw. The
/// C++ spelling of a duck-typed `ReasonOf`, so a caller can write one
/// `catch (const std::exception&)` and still branch.
Reason ReasonOf(const std::exception& error);

/// The code a caught exception carries, or `Code::kUnknown`.
Code CodeOf(const std::exception& error);

/// Whether an exception is one of this SDK's typed failures. `TransportError`
/// included: a failure below the API is still a typed failure, and what makes it
/// the second of R8's three cases is that it carries **no reason**, not that it
/// sits outside the hierarchy — a caller who wanted "did the network break" would
/// have nothing to catch if it did.
bool IsLoamsError(const std::exception& error);

/// Whether a failure came from **below the API** — a socket, a timeout, a body
/// that did not parse. That is R8's second case, and it is the one that carries no
/// `reason` at all.
bool IsTransportError(const std::exception& error);

}  // namespace loams

#endif  // LOAMS_ERROR_HPP