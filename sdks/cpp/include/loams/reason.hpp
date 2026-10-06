// The `ErrorInfo.reason` registry, as a type (design §44 §7.4, D611; runtime
// contract R8).
//
// `reason` is the stable branch a caller switches on. It is **generated from
// the registry page** `docs/api/reasons.md`, so it is a closed enum here: a
// reason the registry has lost stops compiling, and a reason a *newer* server
// returns is not silently turned into "some other reason" (see `error.hpp`'s
// `LoamsError::UnknownReason`).
//
// Three things are deliberately distinct and are not conflated:
//
//   - a reason from a **newer** server, which this enum does not have. It is
//     surfaced as text in `UnknownReason` and flagged, never dropped: losing it
//     would leave a caller unable to tell "not supported here" from "not
//     supported at all".
//   - a failure from **below the API** — a socket, a refused connection, a
//     timeout — which carries no `reason` at all.
//   - a mapped `LoamsError`, which is returned unchanged if mapped twice.
//
// The enum values here are generated from the registry table in the order that
// page lists them, and `scripts/docs/check-decision-ids.sh`'s sibling,
// `crates/loams`'s `reasons_are_snake_case_and_unique`, is the authority: the
// page changes first, then this file.

#ifndef LOAMS_REASON_HPP
#define LOAMS_REASON_HPP

#include <optional>
#include <ostream>
#include <string>
#include <string_view>
#include <vector>

namespace loams {

/// Every reason `docs/api/reasons.md` registers. `kUnknown` is not a reason:
/// it is what a failure from below the API carries, and `Reason()` on an error
/// with no `ErrorInfo` returns it.
enum class Reason {
  kUnknown = 0,
  // The approval causes.
  kApprovalExpired,
  kApprovalAlreadyDecided,
  kApprovalStaleRevision,
  kRequesterCannotApprove,
  kDecisionProofInvalid,
  kStepUpRequired,
  kReasonRequired,
  kInvalidDecision,
  // The pairing grant (MT).
  kPairingExpired,
  kPairingUsed,
  // The device and push-target causes.
  kDeviceRevoked,
  kPushTargetUnknown,
  // The service-level causes.
  kNotImplemented,
  kFeatureNotInVariant,
  // The generic code-to-class rows.
  kInvalidArgument,
  kNotFound,
  kAlreadyExists,
  kPermissionDenied,
  kTokenExpired,
  kUnauthenticated,
  kFailedPrecondition,
  kResourceExhausted,
  kUnavailable,
  kDeadlineExceeded,
  kAborted,
  kInternal,
};

/// The registry spelling of a reason, its `snake_case` string. `kUnknown` maps
/// to the empty string rather than to `"unknown"`, because `"unknown"` is not
/// a reason the registry has and a caller comparing strings must not be offered
/// a value that could also come off the wire.
std::string_view ToString(Reason reason);

/// The reason a registry string names, or `std::nullopt` for a string this
/// SDK's registry does not have — a reason from a newer server. `std::nullopt`
/// rather than `kUnknown` because the two are different facts: `kUnknown` is
/// "there was no reason", `std::nullopt` is "there was a reason and I do not
/// know it".
std::optional<Reason> ReasonFromString(std::string_view name);

/// The registry spelling, for a log or a test's failure message. `ToString`
/// returning a `string_view` and this returning an `ostream&` are the same fact
/// said twice; a caller should not have to know that to print a reason.
std::ostream& operator<<(std::ostream& out, Reason reason);

/// Every reason except `kUnknown`, in registry order. The list an exhaustive
/// switch is checked against, and what `cpp_error_reason_mapping` walks.
const std::vector<Reason>& AllReasons();

/// The reasons D611's error hierarchy names a class for, and which
/// `feature_not_in_variant` and `token_expired` are two of. R8 requires the
/// three stay distinct: a *newer-server* reason, a failure from *below the API*,
/// and a *mapped* error. Keeping this predicate out of the mapping is what
/// stops the second and third collapsing into the first.
bool IsRegistryReason(Reason reason);

}  // namespace loams

#endif  // LOAMS_REASON_HPP