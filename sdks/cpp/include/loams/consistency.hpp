// Consistency tokens (design §44 §7.4, D609; runtime contract R4 as amended by
// SDKP §2.1).
//
// A write answers with a `token` (a graph statement with a `consistency_token`)
// and a read answers with a `read_token`; a read accepts one as
// `consistency.at_least`, so a caller that just wrote can read its own write.
//
// The encoding is `crates/loams-collection/src/token.rs`'s:
// `v1:s<stream>/p<partition>@<offset>,…`, every number canonical decimal.
// Two tokens merge by keeping the largest offset per `(stream, partition)`.
//
// The **session store** is off by default (`Options::session_consistency`).
// With it on, the app modules merge every token an answer carries into the
// session and send `consistency.at_least` on every read that leaves
// `consistency` unset. One token is kept per proto package: a graph's tokens
// name the graph's own stream and a graph read refuses a token of another
// stream.

#ifndef LOAMS_CONSISTENCY_HPP
#define LOAMS_CONSISTENCY_HPP

#include <cstdint>
#include <exception>
#include <map>
#include <mutex>
#include <optional>
#include <string>
#include <vector>

namespace loams {

/// A parsed consistency token. Default-constructed it is **absent**; `v1:` is a
/// present token with no items.
class ConsistencyToken {
 public:
  /// One `s<stream>/p<partition>@<offset>`.
  struct Item {
    std::uint64_t stream = 0;
    std::uint32_t partition = 0;
    std::uint64_t offset = 0;
    friend bool operator==(const Item& a, const Item& b) noexcept {
      return a.stream == b.stream && a.partition == b.partition && a.offset == b.offset;
    }
  };

  ConsistencyToken() = default;

  /// Parses `text`, keeping its order. Throws `loams::InvalidArgumentError`
  /// (client-side, no network call) when it is not a `v1:` token.
  static ConsistencyToken Parse(const std::string& text);

  /// Whether `text` parses.
  static bool Valid(const std::string& text);

  /// The token's text (`v1:` and the items), or empty when absent.
  const std::string& Value() const noexcept { return text_; }
  std::string ToString() const { return text_; }

  /// Whether there is a token at all.
  bool Present() const noexcept { return present_; }

  /// Whether the token holds no item (`v1:`).
  bool Empty() const noexcept { return items_.empty(); }

  /// The items, in the order the token spells them.
  const std::vector<Item>& Items() const noexcept { return items_; }

  /// Sorted by stream then partition, one item per pair (the largest offset).
  ConsistencyToken Normalized() const;

  /// A new token holding the larger offset of the two for every
  /// `(stream, partition)`, normalised. An absent side is the identity.
  ConsistencyToken Merge(const ConsistencyToken& other) const;

  /// The offset this token holds for one partition.
  std::optional<std::uint64_t> Offset(std::uint64_t stream, std::uint32_t partition) const;

  friend bool operator==(const ConsistencyToken& a, const ConsistencyToken& b) noexcept {
    return a.present_ == b.present_ && a.items_ == b.items_;
  }
  friend bool operator!=(const ConsistencyToken& a, const ConsistencyToken& b) noexcept { return !(a == b); }

 private:
  explicit ConsistencyToken(std::vector<Item> items);

  bool present_ = false;
  std::vector<Item> items_;
  std::string text_;
};

/// **Deprecated, never thrown.** The session store once refused to merge two
/// different tokens and raised this; now that the `v1:` encoding is parsed (R4
/// as amended), it merges them. Kept because SDKs are additive-only (design §54
/// §13.2), so code that catches it still compiles.
class ConsistencyConflict : public std::exception {
 public:
  explicit ConsistencyConflict(std::string held, std::string arriving)
      : message_("loams: the session consistency store holds '" + held + "' and cannot merge the arriving '" +
                 arriving + "'") {}
  const char* what() const noexcept override { return message_.c_str(); }

 private:
  std::string message_;
};

/// The session store (R4): one merged token per scope (a proto package).
/// Thread-safe.
class SessionTokenStore {
 public:
  /// The scope collection reads and writes share.
  static constexpr const char* kCollections = "loams.collection.v1";

  /// Merges `token` into `scope`. An absent token is ignored rather than
  /// clearing the one already held.
  void Observe(const ConsistencyToken& token, const std::string& scope = kCollections);

  /// Merges `text` when it parses; a token that does not parse is ignored, so
  /// the store never holds a token it cannot merge.
  void ObserveText(const std::string& text, const std::string& scope = kCollections);

  /// The merged token for `scope`, absent when none was observed.
  ConsistencyToken Merged(const std::string& scope = kCollections) const;

  void Clear();
  bool Has(const std::string& scope = kCollections) const;

 private:
  mutable std::mutex mutex_;
  std::map<std::string, ConsistencyToken> tokens_;
};

}  // namespace loams

#endif  // LOAMS_CONSISTENCY_HPP
