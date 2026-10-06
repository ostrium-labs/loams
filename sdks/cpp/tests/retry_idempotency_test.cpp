// `cpp_retry_reuses_idempotency_key`.
//
// Runtime contract R2 and R3, and the pair is one test because the failure they
// share is the same one:
//
//   - R3: a mutating call that carries an `idempotency_key` is given one **per
//     logical call**, before the first attempt, and **the same key goes out on
//     every retry**. A key regenerated per attempt turns one write into two,
//     which is the exact failure the key exists to prevent.
//   - R2: a call's retry class comes from the generated bindings, not from a
//     guess, and a mutation does not retry at all unless it carries a key.
//
// So the assertions are: the same key went out N times, a read retried without
// ever minting one, a mutation with no key did **not** retry at all, and a mutation
// with a key did. Plus the backoff numbers, which are M1.6 Ruling 5's and the same
// in every SDK: base 100 ms, doubling, capped at 2 s, 3 retries, full jitter.
//
// **Why a stub and not a fixture.** The manifest says so in its own words for R2:
// "**No fixture, and none is possible from either server today.** A retryable
// `unavailable` needs a dependency to be down"; and `faults.json` records that
// `mock_injects_retryable_errors` is "not built". A suite that claimed to pin R2
// against the corpus would be pinning a stub and calling it a fixture.

#include "support.hpp"

#include <algorithm>
#include <chrono>
#include <map>
#include <set>
#include <thread>

namespace {

using namespace loams;
using namespace loams_test;

/// A Connect `unavailable` body with a `RetryInfo`-shaped metadata entry, which is
/// what a server under backpressure sends.
std::string UnavailableBody(const std::string& message) {
  return std::string("{\"code\":\"unavailable\",\"message\":\"") + message +
         "\",\"details\":[{\"type\":\"loams.errors.v1.ErrorInfo\",\"value\":\"" +
         Base64Encode(std::string()) + "\"}]}";
}

/// How many times a request carried a given idempotency key.
std::map<std::string, int> CountKeys(const std::vector<HttpRequest>& requests) {
  std::map<std::string, int> counts;
  for (const HttpRequest& request : requests) {
    for (const auto& header : request.headers) {
      if (header.first != "loams-idempotency-key" && header.first != "idempotency-key") {
        continue;
      }
      counts[header.second] += 1;
    }
  }
  return counts;
}

/// The idempotency key a request's **body** carries, which is where the contract
/// puts it: the key is a field of the message, not a header, and a test that read
/// a header would pass an SDK that never set one.
std::optional<std::string> KeyInBody(const HttpRequest& request,
                                     const std::function<std::optional<std::string>(const std::string&)>& read) {
  const std::optional<std::vector<EnvelopeFrame>> frames = DecodeEnvelopeFrames(request.body);
  if (frames.has_value() && !frames->empty()) {
    return read(frames->front().payload);
  }
  return read(request.body);
}

Options OptionsFor(ScriptedTransport* transport, int max_retries = kDefaultMaxRetries) {
  Options options;
  options.endpoint = "http://127.0.0.1:1";
  // Non-owning, so the stack object outlives the client.
  options.transport = std::shared_ptr<HttpTransport>(transport, [](HttpTransport*) {});
  options.content_type = ContentType::kProto;
  options.max_retries = max_retries;
  return options;
}

}  // namespace

int main() {
  // --- R3, the headline: one key per logical call, reused on every retry ---------
  {
    ScriptedTransport transport;
    // Four `unavailable`s then a success: the call exhausts its budget's first
    // three retries and then fails, which is the shape that shows the key went out
    // on every attempt.
    for (int at = 0; at < 4; ++at) {
      ScriptedTransport::Answer answer;
      answer.status = 503;
      answer.content_type = "application/json";
      answer.body = UnavailableBody("the dependency is down");
      transport.AddAnswer(answer);
    }

    Options options = OptionsFor(&transport);
    std::shared_ptr<Loams> loams = MakeLoams(std::move(options));

    bool threw = false;
    try {
      // `DecideApproval` declares `idempotency_key`, so it is `kManual` and only
      // retryable once the SDK has minted one.
      approvals::v1::DecideApprovalRequest request;
      request.set_approval_id("apr_1");
      approvals::v1::DecideApprovalResponse response;
      loams->Approvals()->DecideApproval(&request, &response);
    } catch (const UnavailableError& error) {
      threw = true;
      LOAMS_CHECK_EQ(static_cast<int>(error.CodeValue()), static_cast<int>(Code::kUnavailable), "the code");
    }
    LOAMS_CHECK(threw, "four unavailable answers and no error");

    // One attempt plus three retries is four requests.
    const std::vector<HttpRequest> requests = transport.Requests();
    LOAMS_CHECK_EQ(requests.size(), std::size_t{4}, "an attempt plus 3 retries is four requests");

    // The same key on all four. `CountKeys` counts the header, which this SDK does
    // **not** send for the key — so this is asserted to be zero, and the real
    // assertion is the one below, on the body.
    LOAMS_CHECK(CountKeys(requests).empty(), "the key travels in the message, not in a header");

    std::vector<std::string> keys;
    for (const HttpRequest& request : requests) {
      const std::optional<std::string> key = KeyInBody(
          request, [](const std::string& bytes) {
            approvals::v1::DecideApprovalRequest parsed;
            if (!parsed.ParseFromString(bytes)) {
              return std::optional<std::string>();
            }
            // `idempotency_key` is a plain proto3 `string`, not an `optional`, so
            // presence is "not empty" rather than a `has_` accessor.
            if (parsed.idempotency_key().empty()) {
              return std::optional<std::string>();
            }
            return std::optional<std::string>(parsed.idempotency_key());
          });
      if (key.has_value()) {
        keys.push_back(*key);
      }
    }
    LOAMS_REQUIRE(!keys.empty(), "no attempt carried an idempotency key at all");
    const std::set<std::string> distinct(keys.begin(), keys.end());
    LOAMS_CHECK_EQ(distinct.size(), std::size_t{1},
                   "the same key must go out on every retry; saw " + std::to_string(distinct.size()) +
                       " distinct keys over " + std::to_string(keys.size()) + " attempts");
    LOAMS_CHECK_EQ(keys.size(), std::size_t{4}, "every attempt carried the key");

    // And it is a UUIDv7, minted before the first attempt.
    const std::string& key = keys.front();
    LOAMS_CHECK_EQ(key.size(), std::size_t{36}, "an idempotency key should be a UUID, got \"" + key + "\"");
    const std::optional<long long> millis = UuidV7Millis(key);
    LOAMS_CHECK(millis.has_value(), "\"" + key + "\" is not a UUIDv7");
    if (millis.has_value()) {
      const long long now = std::chrono::duration_cast<std::chrono::milliseconds>(
                                std::chrono::system_clock::now().time_since_epoch())
                                .count();
      // Within an hour either way: a minted key should be now, and a key read out
      // of the wrong 48 bits lands in 1970.
      LOAMS_CHECK(millis.value() > now - 3600 * 1000 && millis.value() < now + 3600 * 1000,
                  "the UUIDv7's timestamp is not near now: " + std::to_string(millis.value()));
    }
  }

  // --- R3: a caller's own key is honoured, and is not overwritten -----------------
  {
    ScriptedTransport transport;
    ScriptedTransport::Answer failing;
    failing.status = 503;
    failing.content_type = "application/json";
    failing.body = UnavailableBody("the dependency is down");
    transport.AddAnswer(failing);
    transport.AddAnswer(failing);

    Options options = OptionsFor(&transport, /*max_retries=*/1);
    std::shared_ptr<Loams> loams = MakeLoams(std::move(options));

    try {
      approvals::v1::DecideApprovalRequest request;
      request.set_idempotency_key("conformance-my-own-key");
      approvals::v1::DecideApprovalResponse response;
      loams->Approvals()->DecideApproval(&request, &response);
    } catch (const std::exception&) {
    }
    const std::vector<HttpRequest> requests = transport.Requests();
    LOAMS_REQUIRE(!requests.empty(), "no request was sent");
    const std::optional<std::string> sent = KeyInBody(
        requests.front(), [](const std::string& bytes) {
          approvals::v1::DecideApprovalRequest parsed;
          if (!parsed.ParseFromString(bytes)) return std::optional<std::string>();
          return std::optional<std::string>(parsed.idempotency_key());
        });
    LOAMS_CHECK(sent.has_value() && *sent == "conformance-my-own-key",
                "the SDK overwrote the caller's own key: " + sent.value_or("<none>"));
  }

  // --- R2: a read retries on its own, and never mints a key ----------------------
  {
    ScriptedTransport transport;
    for (int at = 0; at < 2; ++at) {
      ScriptedTransport::Answer failing;
      failing.status = 503;
      failing.content_type = "application/json";
      failing.body = UnavailableBody("the dependency is down");
      transport.AddAnswer(failing);
    }
    // The third attempt succeeds, so the call returns normally after two retries.
    ScriptedTransport::Answer success;
    success.status = 200;
    success.content_type = "application/proto";
    std::string body;
    instance::v1::GetInstanceResponse response;
    response.set_name("Loams");
    response.add_api_versions("loams.instance.v1");
    if (!response.SerializeToString(&body)) {
      Record(__FILE__, __LINE__, "the scripted GetInstance response would not serialise");
    }
    success.body = body;
    transport.AddAnswer(success);

    Options options = OptionsFor(&transport);
    std::shared_ptr<Loams> loams = MakeLoams(std::move(options));

    bool failed = false;
    try {
      instance::v1::GetInstanceResponse got;
      loams->Instance()->GetInstance(&got);
      LOAMS_CHECK_EQ(got.name(), std::string("Loams"), "the successful attempt's answer");
    } catch (const std::exception& error) {
      failed = true;
      Record(__FILE__, __LINE__, std::string("a read should have succeeded on its third attempt: ") + error.what());
    }
    LOAMS_CHECK(!failed, "a read (NO_SIDE_EFFECTS) did not retry through two unavailables");
    LOAMS_CHECK_EQ(transport.Requests().size(), std::size_t{3}, "two retries then a success is three requests");

    // `GetInstanceRequest` declares no `idempotency_key`, so a read must never
    // have one — R3's "A request with no such field is left alone".
    for (const HttpRequest& request : transport.Requests()) {
      LOAMS_CHECK_EQ(request.body.size(), std::size_t{0}, "an empty request should serialise to no bytes");
    }
  }

  // --- R2: a mutation with no key does **not** retry -----------------------------
  {
    ScriptedTransport transport;
    for (int at = 0; at < 4; ++at) {
      ScriptedTransport::Answer failing;
      failing.status = 503;
      failing.content_type = "application/json";
      failing.body = UnavailableBody("the dependency is down");
      transport.AddAnswer(failing);
    }
    Options options = OptionsFor(&transport);
    std::shared_ptr<Loams> loams = MakeLoams(std::move(options));

    bool threw = false;
    try {
      // `DeployRequest` is a mutation that declares **no** `idempotency_key`, so
      // R3 leaves the request alone and R2's class stays `kManual`: no retry at
      // all. Retrying it would turn one deploy into two.
      live::v1::DeployRequest request;
      live::v1::DeployResponse response;
      loams->Tables()->Deploy(&request, &response);
    } catch (const std::exception&) {
      threw = true;
    }
    LOAMS_CHECK(threw, "Deploy answered, but every attempt was unavailable");
    LOAMS_CHECK_EQ(transport.Requests().size(), std::size_t{1},
                   "a mutation with no idempotency key must not be retried at all");
  }

  // --- R2: `max_retries = 0` disables retries -------------------------------------
  {
    ScriptedTransport transport;
    for (int at = 0; at < 4; ++at) {
      ScriptedTransport::Answer failing;
      failing.status = 503;
      failing.content_type = "application/json";
      failing.body = UnavailableBody("the dependency is down");
      transport.AddAnswer(failing);
    }
    Options options = OptionsFor(&transport, /*max_retries=*/0);
    std::shared_ptr<Loams> loams = MakeLoams(std::move(options));
    try {
      instance::v1::GetInstanceResponse got;
      loams->Instance()->GetInstance(&got);
    } catch (const std::exception&) {
    }
    LOAMS_CHECK_EQ(transport.Requests().size(), std::size_t{1}, "max_retries=0 sends one attempt and no retries");
  }

  // --- R2: the backoff numbers ---------------------------------------------------
  {
    LOAMS_CHECK_EQ(kBaseDelayMs, 100, "M1.6 Ruling 5's base");
    LOAMS_CHECK_EQ(kMaxDelayMs, 2000, "M1.6 Ruling 5's cap");
    LOAMS_CHECK_EQ(kDefaultMaxRetries, 3, "M1.6 Ruling 5's retries");
    LOAMS_CHECK_EQ(kMaxServerDelayMs, 30000, "the ceiling on a server-sent RetryInfo.retry_delay");

    // The retryable set is exactly D610's three codes. `aborted` is deliberately
    // absent: a concurrent write won, and retrying immediately is how two clients
    // fight over the same row.
    LOAMS_CHECK(IsRetryableCode(Code::kUnavailable), "unavailable is retryable");
    LOAMS_CHECK(IsRetryableCode(Code::kDeadlineExceeded), "deadline_exceeded is retryable");
    LOAMS_CHECK(IsRetryableCode(Code::kResourceExhausted), "resource_exhausted is retryable");
    for (const Code code : {Code::kInvalidArgument, Code::kNotFound, Code::kAlreadyExists, Code::kPermissionDenied,
                            Code::kUnauthenticated, Code::kFailedPrecondition, Code::kAborted, Code::kInternal,
                            Code::kUnimplemented, Code::kCanceled, Code::kOutOfRange, Code::kDataLoss,
                            Code::kUnknown, Code::kOk}) {
      LOAMS_CHECK(!IsRetryableCode(code), std::string("code ") + std::string(ToString(code)) + " is not retryable");
    }

    // Full jitter: uniform over `[0, min(cap, base << attempt)]`. The bound is what
    // is asserted — the distribution is not, because a test that asserted on it
    // would be flaky, which design §44's task rules forbid rather than retry.
    for (const int attempt : {0, 1, 2, 3, 10, 64, 1000}) {
      const int ceiling = attempt < 16 ? std::min<int>(kMaxDelayMs, kBaseDelayMs << attempt) : kMaxDelayMs;
      for (int at = 0; at < 200; ++at) {
        const int delay = BackoffMs(attempt, 0);
        LOAMS_CHECK(delay >= 0 && delay <= ceiling,
                    "backoff for attempt " + std::to_string(attempt) + " was " + std::to_string(delay) +
                        ", want [0, " + std::to_string(ceiling) + "]");
      }
    }
    // A shift past the sign bit must not produce a negative delay: `100 << 64` is
    // undefined in C++ and a caller passing 64 would otherwise get one.
    for (int at = 0; at < 200; ++at) {
      LOAMS_CHECK(BackoffMs(64, 0) >= 0, "a large attempt number must not produce a negative delay");
    }
    // A server-sent delay replaces the computed backoff, up to 30 s.
    LOAMS_CHECK_EQ(BackoffMs(0, 500), 500, "a server delay replaces the computed backoff");
    LOAMS_CHECK_EQ(BackoffMs(0, 60000), kMaxServerDelayMs, "a server delay is capped at 30 s");
    LOAMS_CHECK_EQ(BackoffMs(0, 0) >= 0, true, "no server delay falls back to the computed backoff");

    // And the jitter is actually varied, not a constant: two SDKs retrying at the
    // same instant is how a recovering node gets knocked over again.
    bool varied = false;
    int first = BackoffMs(3, 0);
    for (int at = 0; at < 50; ++at) {
      const int delay = BackoffMs(3, 0);
      if (delay != first) {
        varied = true;
        break;
      }
      first = delay;
    }
    LOAMS_CHECK(varied, "full jitter produced the same delay 50 times running");
  }

  // --- R3: the key is decided from the schema, not the object --------------------
  {
    // `DecideApprovalRequest` declares `idempotency_key`; `DeployRequest` does not.
    LOAMS_CHECK(DeclaresIdempotencyKey(approvals::v1::DecideApprovalRequest()), "DecideApprovalRequest declares the field");
    LOAMS_CHECK(DeclaresIdempotencyKey(live::v1::MutateRequest()), "MutateRequest declares the field");
    LOAMS_CHECK(!DeclaresIdempotencyKey(live::v1::DeployRequest()), "DeployRequest does not declare the field");
    LOAMS_CHECK(!DeclaresIdempotencyKey(instance::v1::GetInstanceRequest()), "GetInstanceRequest does not declare it");

    // A message with the field and none written gets one; a message without the
    // field is left **exactly** as the caller wrote it.
    approvals::v1::DecideApprovalRequest request;
    const KeyedRequest keyed = ApplyIdempotencyKey(request, "", /*declared=*/true);
    LOAMS_CHECK(keyed.keyed, "a request declaring the field is keyed");
    LOAMS_REQUIRE(keyed.request != nullptr, "the keyed request should be a copy");
    LOAMS_CHECK(keyed.request->GetReflection()->GetString(*keyed.request,
                                                          keyed.request->GetDescriptor()->FindFieldByName(
                                                              "idempotency_key"))
                    .empty() == false,
                "the copy should carry a key");
    // And the **caller's** message is untouched: mutating it in place would be
    // visible to a caller who reuses it, and would race a concurrent serialise.
    LOAMS_CHECK_EQ(request.idempotency_key(), std::string(),
                   "the caller's request was mutated in place");

    live::v1::DeployRequest deploy;
    const KeyedRequest unkeyed = ApplyIdempotencyKey(deploy, "", /*declared=*/false);
    LOAMS_CHECK(!unkeyed.keyed, "a request without the field is not keyed");
    LOAMS_CHECK(unkeyed.request == nullptr, "a request without the field is left as the caller wrote it");

    // A supplied key wins over a minted one.
    approvals::v1::DecideApprovalRequest with_key;
    const KeyedRequest supplied = ApplyIdempotencyKey(with_key, "my-key", /*declared=*/true);
    LOAMS_CHECK(supplied.keyed, "a supplied key still counts as keyed");
    if (supplied.request != nullptr) {
      LOAMS_CHECK_EQ(supplied.request->GetReflection()->GetString(
                        *supplied.request, supplied.request->GetDescriptor()->FindFieldByName("idempotency_key")),
                    std::string("my-key"), "the supplied key should be used");
    }
  }

  // --- UUIDv7 ---------------------------------------------------------------------
  {
    std::set<std::string> seen;
    for (int at = 0; at < 5000; ++at) {
      seen.insert(UuidV7());
    }
    LOAMS_CHECK_EQ(seen.size(), std::size_t{5000}, "5000 UUIDv7s must all differ");
    // Two minted in the same millisecond must still differ, which is what the
    // counter is for.
    const std::string one = UuidV7();
    const std::string two = UuidV7();
    LOAMS_CHECK(one != two, "two UUIDv7s minted back to back must differ");
    // The layout: version 7 and variant 10.
    for (const std::string& value : {one, two}) {
      LOAMS_CHECK_EQ(value[14], '7', "the version nibble");
      const char variant = value[19];
      LOAMS_CHECK(variant == '8' || variant == '9' || variant == 'a' || variant == 'b', "the variant nibble");
      LOAMS_CHECK_EQ(value[8], '-', "the hyphenation");
      LOAMS_CHECK_EQ(value.size(), std::size_t{36}, "the canonical length");
    }
    // And the reader: 48 bits is **twelve** hex digits. Reading eight would land in
    // 1970.
    const std::optional<long long> millis = UuidV7Millis(one);
    LOAMS_CHECK(millis.has_value(), "a minted UUIDv7 should read back");
    if (millis.has_value()) {
      const long long now = std::chrono::duration_cast<std::chrono::milliseconds>(
                                std::chrono::system_clock::now().time_since_epoch())
                                .count();
      LOAMS_CHECK(std::llabs(millis.value() - now) < 5000, "the UUIDv7's timestamp is not near now");
    }
    LOAMS_CHECK(!UuidV7Millis("not-a-uuid").has_value(), "a non-UUID has no timestamp");
    LOAMS_CHECK(!UuidV7Millis("00000000-0000-4000-8000-000000000000").has_value(),
                "a UUIDv4 is not a UUIDv7");
  }

  return Finish("cpp_retry_reuses_idempotency_key");
}