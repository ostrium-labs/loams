// One call, and the binding table the facade dispatches through (design §44
// §7.3, D606; runtime contract R2, R3, R8).
//
// A `MethodBinding` is what the generated facade would carry: the RPC's name, its
// retry class, whether its request declares an idempotency key, whether it is
// server-streaming, and which two fields its pagination uses. **Every** of those
// comes from the proto's annotations or its schema rather than from a guess, and
// a caller can read the table to see what the SDK believes about an RPC.
//
// `loams::Client::Invoke` is the whole call path — auth, R3's key, R2's retry,
// R1's one refresh, R8's mapping — in one function. The facade methods and the
// conformance suite both sit on it, which is what makes "the suite exercises
// the public surface" true rather than a claim.

#ifndef LOAMS_CALL_HPP
#define LOAMS_CALL_HPP

#include <chrono>
#include <map>
#include <memory>
#include <optional>
#include <string>
#include <utility>
#include <vector>

#include <google/protobuf/message.h>

#include "loams/error.hpp"
#include "loams/retry.hpp"
#include "loams/wire.hpp"

namespace loams {

/// What the generated facade knows about one RPC.
struct MethodBinding {
  /// `package.Service/Method`, which is what a failure is reported against and
  /// what `Rpc()` on the error carries.
  std::string rpc;
  /// The HTTP path, `/package.Service/Method`. Derived from `rpc` when empty.
  std::string path;
  /// The retry class the proto's idempotency level gives it.
  RetryClass retry_class = RetryClass::kManual;
  /// The generated C++ name of the request's `idempotency_key` field, or empty
  /// when the request declares none. A message without the field is left alone
  /// (R3).
  std::string idempotency_field;
  /// Server-streaming, and therefore not retryable by the unary policy.
  bool server_streaming = false;
  /// AIP-158 pagination: `items_field` and `next_page_token_field`, both empty
  /// when the RPC is not paged. One function serves every paged RPC because the
  /// binding names the two fields (R6).
  std::string items_field;
  std::string next_page_token_field;

  /// The HTTP path for this binding.
  std::string HttpPath() const { return path.empty() ? "/" + rpc : path; }
  /// Whether the RPC is paged.
  bool Paged() const { return !items_field.empty() && !next_page_token_field.empty(); }
};

/// One unary attempt's worth of request detail. `Client::Invoke` takes one of
/// these plus a callback that performs a single attempt, so the retry loop, the
/// idempotency key and the one-refresh rule live in exactly one place.
struct CallPlan {
  /// The binding, for the error's `Rpc()` and the retry class.
  MethodBinding binding;
  /// The encoding to send.
  ContentType content_type = ContentType::kProto;
  /// The request body, already marshalled. On a retry this is re-marshalled
  /// from the **keyed** request, which is what makes R3's "the same key goes
  /// out on every retry" true rather than incidental.
  std::string body;
  /// The idempotency key minted for this logical call, empty when the RPC takes
  /// none. Minted once, before the first attempt, and reused.
  std::string idempotency_key;
  /// Whether this request carries a key, which is what turns a `kManual` class
  /// into a retryable one.
  bool keyed = false;
  /// Headers the caller wants on every attempt.
  std::vector<std::pair<std::string, std::string>> headers;
  /// Overrides `Options::max_retries` when set.
  std::optional<int> max_retries;
  /// Overrides `Options::timeout` when set.
  std::optional<std::chrono::milliseconds> timeout;
};

/// The `google.rpc.Status` a Connect failure decodes into, exposed because
/// `cpp_error_reason_mapping` walks all twenty-six registry reasons through it
/// and a test that rebuilt the message by hand would be testing its own encoding
/// rather than the decoder.
struct DecodedFailure {
  /// The code.
  Code code = Code::kUnknown;
  /// The wire's code string, recognised or not.
  std::string code_name;
  /// The server's message.
  std::string message;
  /// The registry reason, when one was carried and this SDK has it.
  Reason reason = Reason::kUnknown;
  /// The reason string as it came off the wire, always, so a newer server's
  /// reason is not lost.
  std::string reason_name;
  /// `ErrorInfo.metadata`.
  std::map<std::string, std::string> metadata;
  /// `ErrorInfo.hint`.
  std::string hint;
  /// Whether an `ErrorInfo` was present at all.
  bool has_error_info = false;
};

}  // namespace loams

#endif  // LOAMS_CALL_HPP