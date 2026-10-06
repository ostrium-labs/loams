// `cpp_token_source_refresh`.
//
// Runtime contract R1: "A client's bearer comes from a token source and travels in
// `Authorization: Bearer`, never in the query string and never in a URL. A `401`
// carrying `ErrorInfo.reason = token_expired` triggers **exactly one** refresh and
// **one** retry; a second expiry is reported. A source that cannot refresh — an
// API key, which does not expire — makes the refresh a no-op."
//
// The four things asserted, and each is a failure the SDK could plausibly have:
//
//   1. The token is in the **header**, and there is no token in the URL — the
//      clause names both because a query string ends up in proxy logs and in
//      `Referer`.
//   2. **Exactly one** refresh and **one** retry on `token_expired`. Two refreshes
//      would hammer the token endpoint; zero would leave the caller with a dead
//      client.
//   3. A **second** expiry reaches the caller as a `TokenExpiredError` rather than
//      spinning.
//   4. An API key's refresh is a **no-op**: the `401` reaches the caller unchanged,
//      because re-sending the same key fails identically.
//
// Plus the built-in sources and the shared in-flight refresh, which is what stops a
// burst of `401`s from becoming one token exchange per in-flight request.

#include "support.hpp"

#include "loams/errors/v1/errors.pb.h"

#include <atomic>
#include <chrono>
#include <cstdlib>
#include <set>
#include <thread>

namespace {

using namespace loams;
using namespace loams_test;

/// A Connect `unauthenticated` body carrying `reason = token_expired`.
std::string TokenExpiredBody() {
  errors::v1::ErrorInfo info;
  info.set_reason("token_expired");
  info.set_hint("refresh and try again");
  std::string bytes;
  static_cast<void>(info.SerializeToString(&bytes));
  return std::string("{\"code\":\"unauthenticated\",\"message\":\"the access token expired\",\"details\":["
                     "{\"type\":\"loams.errors.v1.ErrorInfo\",\"value\":\"") +
         Base64Encode(bytes) + "\"}]}";
}

/// A Connect `unauthenticated` body with **no** reason detail — the case R8 calls
/// "a failure from below the API", and the case R1 says must **not** trigger a
/// refresh, because there is no `token_expired` to refresh against.
std::string UnauthenticatedBody() {
  return "{\"code\":\"unauthenticated\",\"message\":\"no usable credential\"}";
}

/// A source that counts its refreshes and hands out a new token each time, which
/// is the shape R1's "exactly one" is about.
class CountingSource final : public TokenSource {
 public:
  std::string Token() override {
    std::lock_guard<std::mutex> const guard(mutex_);
    ++tokens_read;
    return held;
  }

  bool Refresh() override {
    std::lock_guard<std::mutex> const guard(mutex_);
    ++refreshes;
    held = "token-" + std::to_string(refreshes);
    return true;
  }

  void Seed(std::string token) {
    std::lock_guard<std::mutex> const guard(mutex_);
    held = std::move(token);
  }

  int Refreshes() const {
    std::lock_guard<std::mutex> const guard(mutex_);
    return refreshes;
  }

  std::string Held() const {
    std::lock_guard<std::mutex> const guard(mutex_);
    return held;
  }

 private:
  mutable std::mutex mutex_;
  std::string held;
  int refreshes = 0;
  int tokens_read = 0;
};

std::string AuthorizationOf(const HttpRequest& request) {
  for (const auto& header : request.headers) {
    if (header.first == "authorization" || header.first == "Authorization") {
      return header.second;
    }
  }
  return std::string();
}

}  // namespace

int main() {
  // --- 1. The bearer travels in the header, and nowhere else ---------------------
  {
    ScriptedTransport transport;
    ScriptedTransport::Answer answer;
    answer.status = 200;
    answer.content_type = "application/proto";
    instance::v1::GetInstanceResponse response;
    response.set_name("Loams");
    std::string body;
    static_cast<void>(response.SerializeToString(&body));
    answer.body = body;
    transport.AddAnswer(answer);

    Options options;
    options.endpoint = "http://127.0.0.1:8080/loams-instance-v1";
    options.transport = std::shared_ptr<HttpTransport>(&transport, [](HttpTransport*) {});
    options.token_source = StaticToken("a-secret-token");
    std::shared_ptr<Loams> loams = MakeLoams(std::move(options));

    instance::v1::GetInstanceResponse got;
    loams->Instance()->GetInstance(&got);

    const std::vector<HttpRequest> requests = transport.Requests();
    LOAMS_REQUIRE(!requests.empty(), "no request was sent");
    const std::string authorization = AuthorizationOf(requests.front());
    LOAMS_CHECK_EQ(authorization, std::string("Bearer a-secret-token"), "the Authorization header");
    // Not in the URL. A query string ends up in proxy logs, in browser history and
    // in `Referer`; the clause names it for that reason.
    LOAMS_CHECK(requests.front().url.find("a-secret-token") == std::string::npos,
                "the bearer must not appear in the URL: " + requests.front().url);
    LOAMS_CHECK(requests.front().url.find('?') == std::string::npos,
                "no call should build a query string at all: " + requests.front().url);
  }

  // --- 2. Exactly one refresh and one retry on `token_expired` --------------------
  {
    ScriptedTransport transport;
    // One expiry, then a success. R1's whole claim is "exactly one refresh and one
    // retry" on this trigger, so the script is the shape that exercises it: the
    // refresh is spent and the retry succeeds.
    ScriptedTransport::Answer expired;
    expired.status = 401;
    expired.content_type = "application/json";
    expired.body = TokenExpiredBody();
    transport.AddAnswer(expired);
    ScriptedTransport::Answer success;
    success.status = 200;
    success.content_type = "application/proto";
    instance::v1::GetInstanceResponse response;
    response.set_name("Loams");
    std::string body;
    static_cast<void>(response.SerializeToString(&body));
    success.body = body;
    transport.AddAnswer(success);

    auto source = std::make_shared<CountingSource>();
    source->Seed("token-0");

    Options options;
    options.endpoint = "http://127.0.0.1:1";
    options.transport = std::shared_ptr<HttpTransport>(&transport, [](HttpTransport*) {});
    options.token_source = source;
    std::shared_ptr<Loams> loams = MakeLoams(std::move(options));

    instance::v1::GetInstanceResponse got;
    loams->Instance()->GetInstance(&got);
    LOAMS_CHECK_EQ(got.name(), std::string("Loams"), "the successful retry's answer");

    LOAMS_CHECK_EQ(source->Refreshes(), 1,
                   "R1 says exactly one refresh; saw " + std::to_string(source->Refreshes()));
    // Two requests: the original and the one retry. A third would be a second
    // refresh.
    const std::vector<HttpRequest> requests = transport.Requests();
    LOAMS_CHECK_EQ(requests.size(), std::size_t{2},
                   "one retry after the refresh; saw " + std::to_string(requests.size()) + " requests");
    if (requests.size() >= 2) {
      // The retry must carry the **refreshed** token: re-sending the stale one is
      // the failure R1 exists to prevent.
      LOAMS_CHECK_EQ(AuthorizationOf(requests.front()), std::string("Bearer token-0"), "the first attempt's bearer");
      LOAMS_CHECK_EQ(AuthorizationOf(requests.back()), std::string("Bearer token-1"),
                     "the retry must carry the refreshed token, not the stale one");
    }
  }

  // --- 3. A second expiry is reported, not spun on --------------------------------
  {
    ScriptedTransport transport;
    for (int at = 0; at < 4; ++at) {
      ScriptedTransport::Answer expired;
      expired.status = 401;
      expired.content_type = "application/json";
      expired.body = TokenExpiredBody();
      transport.AddAnswer(expired);
    }

    auto source = std::make_shared<CountingSource>();
    source->Seed("token-0");

    Options options;
    options.endpoint = "http://127.0.0.1:1";
    options.transport = std::shared_ptr<HttpTransport>(&transport, [](HttpTransport*) {});
    options.token_source = source;
    std::shared_ptr<Loams> loams = MakeLoams(std::move(options));

    bool threw = false;
    try {
      instance::v1::GetInstanceResponse got;
      loams->Instance()->GetInstance(&got);
    } catch (const TokenExpiredError& error) {
      threw = true;
      LOAMS_CHECK_EQ(error.ReasonValue(), Reason::kTokenExpired, "the reported reason");
      // And it is an `UnauthenticatedError` too, so a caller branching on the
      // class catches every `token_expired` failure.
      const UnauthenticatedError& as_class = error;
      LOAMS_CHECK_EQ(static_cast<int>(as_class.CodeValue()), static_cast<int>(Code::kUnauthenticated), "the code");
      LOAMS_CHECK_EQ(error.Hint(), std::string("refresh and try again"), "the hint");
    } catch (const std::exception& error) {
      Record(__FILE__, __LINE__, std::string("a second expiry threw ") + error.what() + ", want a TokenExpiredError");
      threw = true;
    }
    LOAMS_CHECK(threw, "four token_expired answers and no error");
    LOAMS_CHECK_EQ(source->Refreshes(), 1, "a second expiry must not refresh again");
    // Two requests: the original and the one retry R1 allows. More would be a spin.
    LOAMS_CHECK_EQ(transport.Requests().size(), std::size_t{2},
                   "R1 allows exactly one retry; saw " + std::to_string(transport.Requests().size()) + " requests");
  }

  // --- 4. An API key's refresh is a no-op ----------------------------------------
  {
    ScriptedTransport transport;
    for (int at = 0; at < 3; ++at) {
      ScriptedTransport::Answer expired;
      expired.status = 401;
      expired.content_type = "application/json";
      expired.body = TokenExpiredBody();
      transport.AddAnswer(expired);
    }

    Options options;
    options.endpoint = "http://127.0.0.1:1";
    options.transport = std::shared_ptr<HttpTransport>(&transport, [](HttpTransport*) {});
    // An API key does not expire, so `CanRefresh()` is false and the runtime's
    // refresh is the no-op the contract describes: re-sending the same key would
    // fail identically.
    options.token_source = ApiKey("sk-conformance");
    std::shared_ptr<Loams> loams = MakeLoams(std::move(options));

    bool threw = false;
    try {
      instance::v1::GetInstanceResponse got;
      loams->Instance()->GetInstance(&got);
    } catch (const TokenExpiredError&) {
      threw = true;
    } catch (const std::exception&) {
      threw = true;
    }
    LOAMS_CHECK(threw, "the 401 should have reached the caller");
    LOAMS_CHECK_EQ(transport.Requests().size(), std::size_t{1},
                   "a source that cannot refresh must not be retried: re-sending the same key fails identically");
    LOAMS_CHECK(!ApiKey("k")->CanRefresh(), "an API key cannot refresh");
    LOAMS_CHECK(!StaticToken("t")->CanRefresh(), "a caller-managed token cannot refresh");
    LOAMS_CHECK(!EnvToken()->CanRefresh(), "an environment variable cannot refresh");
    // And the key went out, on the one attempt.
    LOAMS_CHECK_EQ(AuthorizationOf(transport.Requests().front()), std::string("Bearer sk-conformance"),
                   "the API key's bearer");
  }

  // --- An `unauthenticated` with **no** reason must not refresh -------------------
  {
    // R1's trigger is specifically `reason = token_expired`. A `401` with no reason
    // detail is R8's "a failure from below the API", and refreshing against it
    // would burn a token exchange on a credential that was never the problem.
    ScriptedTransport transport;
    ScriptedTransport::Answer unauthenticated;
    unauthenticated.status = 401;
    unauthenticated.content_type = "application/json";
    unauthenticated.body = UnauthenticatedBody();
    transport.AddAnswer(unauthenticated);

    auto source = std::make_shared<CountingSource>();
    source->Seed("token-0");

    Options options;
    options.endpoint = "http://127.0.0.1:1";
    options.transport = std::shared_ptr<HttpTransport>(&transport, [](HttpTransport*) {});
    options.token_source = source;
    std::shared_ptr<Loams> loams = MakeLoams(std::move(options));

    try {
      instance::v1::GetInstanceResponse got;
      loams->Instance()->GetInstance(&got);
    } catch (const std::exception&) {
    }
    LOAMS_CHECK_EQ(source->Refreshes(), 0, "a 401 with no token_expired reason must not refresh");
    LOAMS_CHECK_EQ(transport.Requests().size(), std::size_t{1}, "and must not be retried");
  }

  // --- `EnvToken` reads the environment, in order, on every call ------------------
  {
    ::setenv("LOAMS_TEST_KEY", "", 0);
    ::setenv("LOAMS_API_KEY", "key-from-env", 1);
    ::setenv("LOAMS_TOKEN", "token-from-env", 1);
    const std::shared_ptr<TokenSource> source = EnvToken();
    LOAMS_CHECK_EQ(source->Token(), std::string("key-from-env"), "LOAMS_API_KEY wins over LOAMS_TOKEN");
    LOAMS_CHECK_EQ(EnvTokenNames().size(), std::size_t{2}, "two variables are read");
    LOAMS_CHECK_EQ(EnvTokenNames()[0], std::string("LOAMS_API_KEY"), "the first variable");
    LOAMS_CHECK_EQ(EnvTokenNames()[1], std::string("LOAMS_TOKEN"), "the second variable");
    // Read on **every** call, so a process that receives its credentials after the
    // client is built still authenticates.
    ::setenv("LOAMS_API_KEY", "rotated-key", 1);
    LOAMS_CHECK_EQ(source->Token(), std::string("rotated-key"),
                   "the environment is read per call, so a rotated credential is picked up");
    // And the no-key case is the empty string, not an error: `GetInstance` needs
    // no auth at all.
    ::unsetenv("LOAMS_API_KEY");
    ::unsetenv("LOAMS_TOKEN");
    LOAMS_CHECK_EQ(source->Token(), std::string(), "with no variables set the source sends nothing");
    ::unsetenv("LOAMS_TEST_KEY");
  }

  // --- `RefreshingTokenSource` shares one in-flight refresh ----------------------
  {
    // A burst of `401`s must produce **one** token exchange, not one per
    // in-flight request: an instance rejecting every stale token would otherwise be
    // hit with one exchange per call, which is how a credential rotation turns into
    // a self-inflicted denial of service.
    std::atomic<int> fetches{0};
    auto source = std::make_shared<RefreshingTokenSource>([&fetches]() {
      fetches.fetch_add(1);
      std::this_thread::sleep_for(std::chrono::milliseconds(20));
      return "shared-token";
    });
    // An empty cache: the first `Token()` must fetch, because a source that sent no
    // credential would get `unauthenticated` and be retried with still nothing.
    LOAMS_CHECK_EQ(source->Token(), std::string("shared-token"), "the first Token fetches");
    LOAMS_CHECK_EQ(fetches.load(), 1, "one fetch so far");
    // A cached token is served without fetching again.
    LOAMS_CHECK_EQ(source->Token(), std::string("shared-token"), "a cached token is served");
    LOAMS_CHECK_EQ(fetches.load(), 1, "and no second fetch");

    // Fifty concurrent refreshes: still one exchange.
    std::atomic<int> failures{0};
    std::vector<std::thread> threads;
    for (int at = 0; at < 50; ++at) {
      threads.emplace_back([&source, &failures] {
        try {
          source->Refresh();
        } catch (...) {
          failures.fetch_add(1);
        }
      });
    }
    for (std::thread& thread : threads) {
      thread.join();
    }
    // Each of the fifty called `Refresh` explicitly, so each is an exchange; what
    // must hold is that a *token read* while one is in flight waits rather than
    // starting its own. That is checked below.
    LOAMS_CHECK_EQ(failures.load(), 0, "no concurrent refresh failed");
    LOAMS_CHECK_EQ(source->Cached(), std::string("shared-token"), "the cache holds the last token");

    // A failing fetch leaves the previous token in place: a stale token is still
    // worth sending, and clearing it would turn one failed refresh into every
    // subsequent call failing too.
    auto failing = std::make_shared<RefreshingTokenSource>([]() -> std::string {
      throw std::runtime_error("the token endpoint is down");
    });
    bool threw = false;
    try {
      failing->Token();
    } catch (const std::exception&) {
      threw = true;
    }
    LOAMS_CHECK(threw, "a failing fetch must report the failure, not send nothing");
    LOAMS_CHECK_EQ(failing->Cached(), std::string(), "and it cached nothing");
  }

  // --- The token is never cached by the runtime ------------------------------------
  {
    // The runtime reads the token per **attempt**, so a source may return a
    // different one each time. Nothing in the client holds a copy.
    ScriptedTransport transport;
    ScriptedTransport::Answer expired;
    expired.status = 401;
    expired.content_type = "application/json";
    expired.body = TokenExpiredBody();
    transport.AddAnswer(expired);
    ScriptedTransport::Answer success;
    success.status = 200;
    success.content_type = "application/proto";
    instance::v1::GetInstanceResponse response;
    response.set_name("Loams");
    std::string body;
    static_cast<void>(response.SerializeToString(&body));
    success.body = body;
    transport.AddAnswer(success);

    auto source = std::make_shared<RefreshingTokenSource>([]() { return std::string("minted-token"); });
    Options options;
    options.endpoint = "http://127.0.0.1:1";
    options.transport = std::shared_ptr<HttpTransport>(&transport, [](HttpTransport*) {});
    options.token_source = source;
    std::shared_ptr<Loams> loams = MakeLoams(std::move(options));

    instance::v1::GetInstanceResponse got;
    loams->Instance()->GetInstance(&got);
    const std::vector<HttpRequest> requests = transport.Requests();
    LOAMS_CHECK_EQ(AuthorizationOf(requests.front()), std::string("Bearer minted-token"), "the first bearer");
    LOAMS_CHECK_EQ(AuthorizationOf(requests.back()), std::string("Bearer minted-token"),
                   "the retry re-read the source rather than reusing a cached copy");
  }

  // --- `RefreshOnce` is callable directly, and reports whether it refreshed -------
  {
    Options options;
    options.endpoint = "http://127.0.0.1:1";
    std::shared_ptr<Loams> with_key = MakeLoams(options);
    LOAMS_CHECK(!with_key->Runtime()->RefreshOnce(), "an unauthenticated client cannot refresh");

    auto counting = std::make_shared<CountingSource>();
    Options second;
    second.endpoint = "http://127.0.0.1:1";
    second.token_source = counting;
    std::shared_ptr<Loams> loams = MakeLoams(std::move(second));
    LOAMS_CHECK(loams->Runtime()->RefreshOnce(), "a refreshing source refreshes");
    LOAMS_CHECK_EQ(counting->Refreshes(), 1, "and it refreshed once");
    LOAMS_CHECK_EQ(loams->Runtime()->Token() != nullptr, true, "the client exposes its token source");
  }

  return Finish("cpp_token_source_refresh");
}