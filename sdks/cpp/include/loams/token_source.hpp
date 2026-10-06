// Token sources (design §44 §7.4, D608; runtime contract R1).
//
// A `TokenSource` returns a bearer and can be asked for a new one. Tokens travel
// in `Authorization: Bearer` and **never** in a URL: a query string ends up in
// proxy logs, in browser history and in `Referer`. A `401` carrying
// `reason = token_expired` triggers **exactly one** refresh and **one** retry; a
// second expiry is reported as a `loams::TokenExpiredError`. That logic lives in
// the call path (`loams::Client`), so a source stays a source.
//
// A source that cannot refresh — an API key, which does not expire — says so by
// returning `false` from `Refresh()`, and the runtime's refresh is then the
// no-op the contract describes.

#ifndef LOAMS_TOKEN_SOURCE_HPP
#define LOAMS_TOKEN_SOURCE_HPP

#include <chrono>
#include <condition_variable>
#include <functional>
#include <memory>
#include <mutex>
#include <string>
#include <vector>

namespace loams {

/// Where a call's bearer comes from.
class TokenSource {
 public:
  virtual ~TokenSource() = default;

  /// The bearer to send, or the empty string to send no credential at all.
  ///
  /// Called once per attempt, so a source may return a different token each
  /// time. May throw: a source that fetches over the network has to be able to
  /// report that it could not, and a client that sent an empty bearer instead
  /// would get `unauthenticated` back and could not tell why.
  virtual std::string Token() = 0;

  /// Fetches a new token after the server reported the current one expired.
  ///
  /// Returns `false` for a source whose token does not expire, which makes R1's
  /// refresh a no-op and the `401` reach the caller unchanged — the right answer
  /// for an API key, because re-sending the same key would fail identically.
  virtual bool Refresh() = 0;

  /// Whether this source can refresh. Separate from `Refresh()`'s return so a
  /// caller can ask the question without performing the refresh, which is what
  /// the retry policy wants: it must know whether the **next** attempt can
  /// possibly differ.
  virtual bool CanRefresh() const { return true; }
};

/// A Loams API key. The key does not expire, so `Refresh()` is a no-op and
/// `CanRefresh()` is false: this is the shape R1 describes for "a source that
/// cannot refresh".
std::shared_ptr<TokenSource> ApiKey(std::string key);

/// A bearer that is already valid, for a caller that manages its own. Like the
/// API key it never refreshes, because the caller owns its lifetime.
std::shared_ptr<TokenSource> StaticToken(std::string token);

/// Reads `LOAMS_API_KEY`, then `LOAMS_TOKEN`, then nothing.
///
/// The environment is read on **every** call rather than once at construction,
/// so a process that receives its credentials after the client is built — a
/// sidecar, a test — still authenticates. `Refresh()` is a no-op: an environment
/// variable is not something the SDK can renew.
std::shared_ptr<TokenSource> EnvToken();

/// The variables `EnvToken` reads, in order. Exposed so a caller can assert on
/// the order rather than on the string.
const std::vector<std::string>& EnvTokenNames();

/// A source that caches a token and calls a fetch function when it is asked to
/// refresh. This is the shape every refreshing source has, including the RFC
/// 8693 OIDC exchange that design §44 §7.4 describes and which the instance does
/// not serve yet.
///
/// One in-flight refresh is **shared** by concurrent callers, so a burst of
/// `401`s produces one token exchange rather than one per request. That is not a
/// micro-optimisation: an instance rejecting every token because it is stale
/// would otherwise be hit with one exchange per in-flight call, which is how a
/// credential rotation turns into a self-inflicted denial of service.
class RefreshingTokenSource : public TokenSource {
 public:
  /// `fetch` is called with no arguments and returns the new token. It may
  /// throw, and the exception reaches every caller waiting on that refresh.
  explicit RefreshingTokenSource(std::function<std::string()> fetch);

  std::string Token() override;
  bool Refresh() override;
  bool CanRefresh() const override { return true; }

  /// The cached token, or the empty string when none has been fetched. Exposed
  /// for a test asserting that a refresh happened exactly once.
  std::string Cached() const;

 private:
  std::function<std::string()> fetch_;
  mutable std::mutex mutex_;
  std::condition_variable done_;
  std::string cached_;
  bool in_flight_ = false;
  bool failed_ = false;
};

}  // namespace loams

#endif  // LOAMS_TOKEN_SOURCE_HPP