// Token sources, as `token_source.hpp` documents them.

#include "loams/token_source.hpp"

#include <cstdlib>

namespace loams {
namespace {

/// An API key: a bearer that never expires, so `Refresh` is the no-op R1
/// describes for "a source that cannot refresh".
class ApiKeySource final : public TokenSource {
 public:
  explicit ApiKeySource(std::string key) : key_(std::move(key)) {}

  std::string Token() override { return key_; }
  bool Refresh() override { return false; }
  // An API key cannot be refreshed, so the retry policy must know that **before**
  // spending an attempt: re-sending the same key would fail identically, and a
  // retry that cannot differ is not a retry.
  bool CanRefresh() const override { return false; }

 private:
  std::string key_;
};

/// A bearer the caller manages. Like the API key it never refreshes, because the
/// caller owns its lifetime.
class StaticSource final : public TokenSource {
 public:
  explicit StaticSource(std::string token) : token_(std::move(token)) {}

  std::string Token() override { return token_; }
  bool Refresh() override { return false; }
  bool CanRefresh() const override { return false; }

 private:
  std::string token_;
};

/// Reads the environment on every call, so a process that receives its
/// credentials after the client is built still authenticates.
class EnvSource final : public TokenSource {
 public:
  std::string Token() override {
    for (const std::string& name : EnvTokenNames()) {
      const char* const value = std::getenv(name.c_str());
      if (value != nullptr && value[0] != '\0') {
        return value;
      }
    }
    return std::string();
  }
  bool Refresh() override { return false; }
  bool CanRefresh() const override { return false; }
};

}  // namespace

const std::vector<std::string>& EnvTokenNames() {
  // Named after design §44 §7.4. `LOAMS_API_KEY` first: a machine credential
  // beats a person's token when both are in the environment, because the machine
  // credential is the one the process was given deliberately.
  static const std::vector<std::string>* const names =
      new std::vector<std::string>{"LOAMS_API_KEY", "LOAMS_TOKEN"};
  return *names;
}

std::shared_ptr<TokenSource> ApiKey(std::string key) { return std::make_shared<ApiKeySource>(std::move(key)); }

std::shared_ptr<TokenSource> StaticToken(std::string token) {
  return std::make_shared<StaticSource>(std::move(token));
}

std::shared_ptr<TokenSource> EnvToken() { return std::make_shared<EnvSource>(); }

RefreshingTokenSource::RefreshingTokenSource(std::function<std::string()> fetch)
    : fetch_(std::move(fetch)) {}

std::string RefreshingTokenSource::Token() {
  {
    std::lock_guard<std::mutex> const guard(mutex_);
    if (!cached_.empty()) {
      return cached_;
    }
  }
  // The cache is empty. A source that sent no credential at all would get
  // `unauthenticated` back, which the call path treats as "the token expired" and
  // retries — with still no credential — so the first `Token` fetches.
  Refresh();
  std::lock_guard<std::mutex> const guard(mutex_);
  return cached_;
}

bool RefreshingTokenSource::Refresh() {
  std::unique_lock<std::mutex> lock(mutex_);
  if (in_flight_) {
    // Another thread is already fetching. Wait for **its** result rather than
    // starting a second exchange: a burst of `401`s must cost one exchange, not
    // one per in-flight call. That is the difference between a credential
    // rotation and a self-inflicted denial of service.
    done_.wait(lock, [this] { return !in_flight_; });
    return !failed_;
  }
  in_flight_ = true;
  lock.unlock();

  // The fetch runs outside the lock: it is a network call, and holding a mutex
  // across one would serialise every reader behind it.
  std::string fetched;
  std::exception_ptr failure;
  try {
    fetched = fetch_ ? fetch_() : std::string();
  } catch (...) {
    failure = std::current_exception();
  }

  lock.lock();
  if (!failure && !fetched.empty()) {
    cached_ = std::move(fetched);
    failed_ = false;
  } else {
    // A failed fetch leaves the previous token in place rather than clearing it:
    // a stale token is still worth sending on the next call, and clearing it
    // would turn one failed refresh into every subsequent call failing too.
    failed_ = true;
  }
  in_flight_ = false;
  lock.unlock();
  done_.notify_all();

  if (failure) {
    std::rethrow_exception(failure);
  }
  return true;
}

std::string RefreshingTokenSource::Cached() const {
  std::lock_guard<std::mutex> const guard(mutex_);
  return cached_;
}

}  // namespace loams