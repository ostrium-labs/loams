// Idempotency keys (design §44 §7.4, D610; runtime contract R3).
//
// A mutating call that carries an `idempotency_key` field is given one **per
// logical call**, before the first attempt, and **the same key goes out on
// every retry**. A key regenerated per attempt turns one write into two, which
// is the exact failure the key exists to prevent.
//
// The decision to key is read from the message's **descriptor**, not from the
// object a caller happened to build. That matters for proto3 `optional`: a
// caller who leaves `idempotency_key` out sends no key at all, so the mutation
// is not retryable and the SDK would never know — unless it asks the schema.
// And it must not confuse a request that declares the field with one that does
// not, or it would invent a field the schema does not know.

#ifndef LOAMS_IDEMPOTENCY_HPP
#define LOAMS_IDEMPOTENCY_HPP

#include <memory>
#include <optional>
#include <string>
#include <string_view>

#include <google/protobuf/message.h>

namespace loams {

/// The generated C++ field name of `idempotency_key`.
inline constexpr const char* kIdempotencyKeyField = "idempotency_key";

/// A fresh UUIDv7 as the canonical lowercase hyphenated string.
///
/// An idempotency key has to be unique across every client that has ever talked
/// to an instance **and** sort by creation time, because a key that sorts is
/// one an operator can correlate in a log. Layout: 48 bits of Unix
/// milliseconds, 4 bits of version (7), 12 bits of a counter within the
/// millisecond, 2 bits of variant, 62 random bits.
///
/// The counter is seeded randomly and incremented under a mutex, which is what
/// makes two UUIDs minted in the same millisecond on the same thread distinct
/// without a dependency. `UuidV7` throws `LoamsError` (`internal`) if the
/// platform's entropy source fails: a predictable idempotency key **is** a
/// duplicate key, so there is nothing to fall back to.
std::string UuidV7();

/// The Unix milliseconds a UUIDv7 encodes, and `std::nullopt` for anything
/// else. The timestamp is the first **twelve** hex digits, not eight: 48 bits,
/// and milliseconds since the epoch use 41 of them. Reading eight digits returns
/// a number around 2^25, which is January 1970.
std::optional<long long> UuidV7Millis(std::string_view value);

/// A request the runtime has decided to key, and whether it made that decision.
struct KeyedRequest {
  /// The message to send: a copy with the key set, or the original when
  /// nothing was set. Never null when `request` was not null.
  std::unique_ptr<google::protobuf::Message> request;
  /// Whether the request carries a key the retry policy may rely on.
  bool keyed = false;
};

/// Decides a mutating call's idempotency key, once per logical call.
///
/// `declared` says whether the request's **schema** declares the field, which
/// is what `loams::MethodBinding::idempotency_field` carries. A message
/// without the field is left exactly as the caller wrote it.
///
/// `supplied` is the caller's own key, if any: making a retry yours rather than
/// the SDK's is sometimes the right call, because the key is what your storage
/// dedupes on.
///
/// The returned message is a **copy**. Mutating the caller's message in place
/// would be visible to a caller who reuses it, and would race a concurrent
/// `SerializeToString` on it — so the copy is not an optimisation here, it is
/// what makes a second attempt safe.
KeyedRequest ApplyIdempotencyKey(const google::protobuf::Message& request, const std::string& supplied,
                                  bool declared);

/// Whether a proto message declares `idempotency_key`. It exists so a caller —
/// and the conformance suite — can ask the **schema** rather than the object,
/// which is the point of R3.
bool DeclaresIdempotencyKey(const google::protobuf::Message& message);

/// The key a request currently carries, and whether the field is present.
/// Present-and-empty is different from absent: proto3 `optional` means exactly
/// that, and a caller who wrote an empty key means "no key", which the runtime
/// honours by minting one.
std::optional<std::string> ReadIdempotencyKey(const google::protobuf::Message& request);

}  // namespace loams

#endif  // LOAMS_IDEMPOTENCY_HPP