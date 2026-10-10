// Consistency tokens (design §44 §7.4, D609; runtime contract R4).
//
// A write answers with a `consistency_token`; a read accepts one, so a caller
// that just wrote can read its own write. Threading those by hand is the
// caller's job today. The alternative is a **session store**, which is **off by
// default** (`Options::session_consistency`): when a call opts in, every
// response's token is folded into the session and attached to later reads.
//
// **The token's encoding is not in the protos yet.** §05 §5 defines the
// semantics and API1's write paths carry an opaque `v1:` string; §44 §7.4 says
// it merges by "max offset per stream and partition", which needs the encoding
// parsed. Until that lands, a store keeps the token it was given and reports
// two different tokens meeting as an **error** rather than merging them into a
// wrong one — a silently-wrong consistency token reads stale data, which is
// worse than a failure.

#ifndef LOAMS_CONSISTENCY_HPP
#define LOAMS_CONSISTENCY_HPP

#include <exception>
#include <mutex>
#include <string>

namespace loams {

/// An opaque `consistency_token`. A value type with no interpretation of its
/// own, because the encoding is not specified yet: anything that parsed the
/// `v1:` string to merge two tokens would be inventing the format.
class ConsistencyToken {
 public:
  ConsistencyToken() = default;
  explicit ConsistencyToken(std::string value) : value_(std::move(value)) {}

  /// The token as the API carries it, including any `v1:` prefix.
  const std::string& Value() const noexcept { return value_; }

  /// Whether there is a token at all. An empty string is the same thing, but a
  /// caller reading a response should not have to know that.
  bool Present() const noexcept { return !value_.empty(); }

  /// Identity, so a token can be compared and used as a map key. Two tokens are
  /// equal when their strings are, which is the only equality the format
  /// supports today.
  friend bool operator==(const ConsistencyToken& a, const ConsistencyToken& b) noexcept {
    return a.value_ == b.value_;
  }
  friend bool operator!=(const ConsistencyToken& a, const ConsistencyToken& b) noexcept {
    return !(a == b);
  }

 private:
  std::string value_;
};

/// Raised by `SessionTokenStore::Observe` when a second, **different** token
/// arrives while one is already held.
///
/// It is a `LoamsError` with reason `internal` and a message that says so,
/// because a store that cannot merge is not the caller's mistake in the sense
/// that `internal` normally means — but it is not a Loams RPC failure either,
/// so it carries no server code. The important part is the message: "two
/// different tokens" names the cause and the fix (read your write through one
/// token, or turn the session store off).
class ConsistencyConflict : public std::exception {
 public:
  explicit ConsistencyConflict(std::string held, std::string arriving)
      : message_("loams: the session consistency store holds '" + held +
                 "' and cannot merge the arriving '" + arriving +
                 "': the token encoding is not in the protos yet, and a merged "
                 "token that is wrong reads stale data, so this is reported "
                 "instead") {}
  const char* what() const noexcept override { return message_.c_str(); }

 private:
  std::string message_;
};

/// Folds the tokens a session has seen into the one it attaches to later reads.
///
/// One store per client. Thread-safe: the token is shared mutable state on a
/// client that a C++ program will happily call from several threads, so every
/// access is under the mutex. The critical section is a string compare and an
/// assign, never a network call.
class SessionTokenStore {
 public:
  /// Folds a returned token in. An absent token is ignored: a response that
  /// carries none is saying nothing about consistency, and treating that as
  /// "the token is now empty" would drop a good token.
  ///
  /// Throws `ConsistencyConflict` when `token` differs from the one already
  /// held.
  void Observe(const ConsistencyToken& token);

  /// The token to attach to the next read, or an absent token when the store
  /// holds none.
  ConsistencyToken Merged() const;

  /// Forgets the held token. `Options` calls nothing; this is for a caller that
  /// wants to stop reading its own writes.
  void Clear();

  /// Whether a token is held.
  bool Has() const;

 private:
  mutable std::mutex mutex_;
  ConsistencyToken token_;
};

}  // namespace loams

#endif  // LOAMS_CONSISTENCY_HPP