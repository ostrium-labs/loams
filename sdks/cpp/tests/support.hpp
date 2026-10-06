// The test support the six conformance tests share: assertions, a fixture
// endpoint, and the recorded corpus.
//
// Nothing here is part of the SDK. It is in `tests/` rather than `src/` so a
// consumer of the library cannot link it, and the assertions are here rather than
// pulled from a framework so the SDK has **no** test-framework dependency: a C++
// SDK whose build drags in gtest is a heavier install for a consumer, and design
// §44 §10.1's "one pipeline, thirteen languages" does not need a fourteenth
// thing to keep in step.

#ifndef LOAMS_TESTS_SUPPORT_HPP
#define LOAMS_TESTS_SUPPORT_HPP

#include <cstdio>
#include <exception>
#include <cstdlib>
#include <exception>
#include <iostream>
#include <optional>
#include <sstream>
#include <string>
#include <vector>

#include <google/protobuf/struct.pb.h>
#include <google/protobuf/util/json_util.h>

#include "loams/loams.hpp"

namespace loams_test {

// The SDK's types are used unqualified throughout this header: `HttpTransport`,
// `ByteReader` and `Options` are named often enough that spelling the namespace
// on each is noise, and a test helper that reads `loams::HttpTransport` on one
// line and `HttpTransport` on the next is worse.
using namespace loams;  // NOLINT(build/namespaces)

/// Thrown by a failed assertion, and by anything in the harness that gives up.
///
/// **Derives from `std::exception`**, and that is load-bearing twice over: a
/// driver's `catch (const std::exception&)` around a per-fixture step has to
/// catch it, or the first fixture that fails aborts the whole run and the report
/// names one failure instead of all of them; and `what()` has to exist for the
/// message to survive.
struct Failed : public std::exception {
  explicit Failed(std::string text) : message(std::move(text)) {}
  const char* what() const noexcept override { return message.c_str(); }

  std::string message;
};

/// Records a failure and keeps going. A test that stops at the first failed
/// assertion makes the second one wait for another run, which is how a five-line
/// fix becomes a five-round fix.
void Record(const std::string& file, int line, const std::string& text);

/// Aborts the current test with `text`.
[[noreturn]] void Abort(const std::string& file, int line, const std::string& text);

#define LOAMS_CHECK(condition, message)                                            \
  do {                                                                             \
    if (!(condition)) {                                                            \
      ::loams_test::Record(__FILE__, __LINE__, std::string(message) + " [" #condition "]"); \
    }                                                                              \
  } while (false)

#define LOAMS_REQUIRE(condition, message)                                          \
  do {                                                                             \
    if (!(condition)) {                                                            \
      ::loams_test::Abort(__FILE__, __LINE__, std::string(message) + " [" #condition "]"); \
    }                                                                              \
  } while (false)

// **Copies, not references.** `const auto&` binds a reference to the temporary a
// caller wrote inline — `LOAMS_CHECK_EQ(x.reason(), Reason::kNotFound, ...)` — and
// `-Wdangling-reference` is right that it may dangle; `auto&&` is no better,
// because an rvalue reference to a temporary is the same lifetime problem one
// compiler check earlier. A copy of an enum or a short string in a test costs
// nothing and is the only spelling that is never wrong.
#define LOAMS_CHECK_EQ(actual, expected, message)                                                            \
  do {                                                                                                        \
    const auto loams_actual = (actual);                                                                       \
    const auto loams_expected = (expected);                                                                   \
    if (!(loams_actual == loams_expected)) {                                                                  \
      std::ostringstream loams_stream;                                                                        \
      loams_stream << message << " [" #actual "]: got " << loams_actual << ", want " << loams_expected;         \
      ::loams_test::Record(__FILE__, __LINE__, loams_stream.str());                                           \
    }                                                                                                         \
  } while (false)

#define LOAMS_CHECK_THROWS(statement, ExceptionType, message)                                    \
  do {                                                                                            \
    bool loams_threw = false;                                                                     \
    try {                                                                                          \
      statement;                                                                                   \
    } catch (const ExceptionType&) {                                                               \
      loams_threw = true;                                                                          \
    } catch (const std::exception& loams_other) {                                                  \
      ::loams_test::Record(__FILE__, __LINE__,                                                    \
                           std::string(message) + ": threw " + loams_other.what() +                \
                               ", want " #ExceptionType);                                          \
      loams_threw = true;                                                                          \
    }                                                                                               \
    if (!loams_threw) {                                                                             \
      ::loams_test::Record(__FILE__, __LINE__, std::string(message) + ": did not throw " #ExceptionType); \
    }                                                                                               \
  } while (false)

/// The root of the repository, from `LOAMS_FIXTURES_DIR`, which the CMakeLists
/// sets per test. Absolute because ctest runs in the build directory.
std::string FixturesDir();
/// `sdks/conformance`, the fixture server and its scripts.
std::string ConformanceDir();
/// `sdks/cpp`, so a test can read `../fixtures` the way `fixture-server.mjs` does.
std::string SdkDir();

/// What one recorded step has to hold.
///
/// Read from the recording rather than written down anywhere else, so a fixture
/// that moves cannot leave the suite asserting the old thing. A scenario carries
/// one per step: the cursor belongs to the step that handed it out, not to the
/// fixture.
struct Expect {
  long status = 200;
  /// The gRPC status a `200` carries in its trailers instead of an HTTP status.
  std::optional<int> grpc_status;
  /// Whether the recording declares a `reason` at all. `null` is a **fact** — the
  /// server sent no `ErrorInfo` — so it has to be distinguishable from the key
  /// being absent, which is why this is a flag beside the value rather than the
  /// value alone.
  bool has_reason = false;
  std::optional<std::string> reason;
  std::optional<std::string> state;
  std::optional<std::string> revision;
  std::vector<std::string> api_versions;
  /// The step whose answer this one is recorded as byte-identical to.
  std::optional<int> identical_to_step;
  std::optional<int> frames;
  std::vector<std::string> frame_kinds;
  std::optional<std::string> cursor;
  std::optional<bool> snapshot_reset;
  bool truncated = false;
};

/// A recorded fixture, as `sdks/fixtures/recorded/**.json` carries it.
struct RecordedStep {
  std::string method;
  std::string path;
  std::string content_type;
  std::string body;
  /// The `loams-fixture-step` header the recording was made with, empty for step 0.
  std::string step;
  long status = 0;
  std::string response_content_type;
  std::string response_body;
  /// The frames a streamed recording carries, as raw envelope bytes, one entry
  /// per frame. Empty for a unary recording.
  std::vector<std::string> frames;
  /// The trailers a streamed gRPC-Web recording carries.
  std::string trailers;
  /// True when the recording is a **prefix** of a stream that never ended. Stated
  /// by the recorder rather than inferred, because accepting any end-of-body after
  /// a stream would turn the truncation into a loophole.
  bool truncated = false;
  /// What the step has to hold.
  Expect expect;
};

/// One recorded fixture and its steps.
struct Recorded {
  std::string name;
  std::string about;
  std::vector<RecordedStep> steps;
  /// The fixture-level `expect` block. A scenario's per-step blocks live on the
  /// steps, which is where an assertion about them belongs.
  Expect expect;
};

/// Reads every recorded fixture, `recorded/*.json` and `recorded/apps-mock/*.json`.
std::vector<Recorded> ReadCorpus();

/// Parses one JSON document into a `Struct`, or nothing when it does not parse.
///
/// A `Struct` and not a hand-written JSON reader: the SDK and this harness both
/// need one, and protobuf's is the one already linked.
inline std::optional<google::protobuf::Struct> ParseJson(const std::string& text) {
  google::protobuf::Struct parsed;
  if (!google::protobuf::util::JsonStringToMessage(text, &parsed).ok()) {
    return std::nullopt;
  }
  return parsed;
}

/// A field of a parsed JSON object, or null when it is absent.
///
/// One helper rather than `Struct::fields().find("x")` at every call site, because
/// `google::protobuf::Map::find` is a template on the key type and a string
/// **literal** does not deduce against `std::string`.
inline const google::protobuf::Value* Field(const google::protobuf::Struct& object, const std::string& name) {
  const auto& fields = object.fields();
  const auto found = fields.find(name);
  if (found == fields.end()) {
    return nullptr;
  }
  return &found->second;
}

/// A field's string value, or the empty string when it is absent or not a string.
inline std::string StringAt(const google::protobuf::Struct& object, const std::string& name) {
  const google::protobuf::Value* const value = Field(object, name);
  return value == nullptr ? std::string() : value->string_value();
}

/// An endpoint the suite can talk to.
///
/// Three ways to get one, in the order they are tried — the same order
/// `sdks/go/fixture_server_test.go` uses, so the thirteen suites answer the same
/// questions about the same bytes:
///
///   1. `LOAMS_TEST_ENDPOINT` — a live `loams dev`. This short-circuits
///      everything else, which is how `sdks/conformance/run-test.sh` runs the same
///      suite in CI against a recording and on a developer machine against the
///      real thing.
///   2. `node sdks/conformance/fixture-server.mjs` — the shared server. Preferred
///      when Node is on PATH, because then this suite really does run against the
///      same server the other twelve do.
///   3. An in-process replay of `sdks/fixtures/recorded` with the same matching
///      rules, for a machine with no Node. A C++ test should not need a
///      JavaScript runtime to run, and the corpus is the part that matters.
class Endpoint {
 public:
  Endpoint();
  ~Endpoint();
  Endpoint(const Endpoint&) = delete;
  Endpoint& operator=(const Endpoint&) = delete;

  /// Whether this is a real `loams dev` rather than a replay. Only changes what a
  /// skip is allowed to claim.
  bool Live() const { return live_; }
  /// How the endpoint was obtained, for the report.
  std::string How() const { return how_; }

  /// The transport to build the client with, or null for tiers 1 and 2 where a
  /// URL is enough.
  ///
  /// Tier 3 is the in-process replay, which is a **lookup** rather than a socket:
  /// that is the whole reason the tier exists — a C++ test should not need a
  /// JavaScript runtime to run, and the recorded bytes are identical either way.
  HttpTransport* Transport() const { return transport_; }

  /// The replay's resolved URL. `replay://…` is not dialable, so a test that
  /// reads it must use `Transport()`.
  const std::string& Url() const { return url_; }

 private:
  void StartNode();
  void StartReplay();
  void Stop();

  std::string url_;
  std::string how_;
  bool live_ = false;
  /// The replay transport, tier 3 only. Owned by the process-wide replay the
  /// endpoint shares, so it outlives this object.
  HttpTransport* transport_ = nullptr;
  /// The `node` child, when one was started. Reaped in the destructor: a fixture
  /// server outliving the run would collide with the next one on the port.
  long child_ = -1;
  /// Where that child's stdout went. A **file**, not a pipe: a pipe closed after
  /// the URL has been read takes the server down with EPIPE on its next log line.
  std::string log_path_;
};

/// Reads a whole file, or throws. A suite that cannot see the corpus is not a
/// suite, and a silent empty string would be a corpus that looks empty.
std::string ReadFile(const std::string& path);

/// Joins with a separator, because `std::accumulate` with a string literal is a
/// mouthful and the alternative — four lines of loop in every caller — is worse.
std::string JoinWith(const std::vector<std::string>& parts, const std::string& separator);

/// A `ByteReader` over a fixed body, for a scripted stream.
class StringReader final : public ByteReader {
 public:
  StringReader(long status, std::vector<std::pair<std::string, std::string>> headers, std::string body)
      : status_(status), headers_(std::move(headers)), body_(std::move(body)) {}

  bool Next(std::string* into, bool* at_eof, std::exception_ptr* error) override {
    *error = nullptr;
    if (body_.empty()) {
      *at_eof = true;
      return false;
    }
    // Handed over in pieces, so a stream's reader is exercised the way a real one
    // is: a frame split across two reads must still be assembled.
    const std::size_t piece = std::min<std::size_t>(body_.size(), 7);
    into->append(body_.substr(0, piece));
    body_.erase(0, piece);
    return true;
  }

  long Status() const override { return status_; }
  const std::vector<std::pair<std::string, std::string>>& Headers() const override { return headers_; }
  void Close() override { closed_ = true; }
  bool Closed() const { return closed_; }

 private:
  long status_;
  std::vector<std::pair<std::string, std::string>> headers_;
  std::string body_;
  bool closed_ = false;
};

/// A transport that answers from a script, for the clauses the corpus cannot
/// reach: a retryable `unavailable` on demand (R2), a token expiry (R1), a
/// paged call (R6), and a stream that breaks mid-answer (R7).
///
/// Scripted rather than recorded, because these are exactly the cases no healthy
/// server produces on demand — which is why `faults.json` exists and why several
/// of them are pinned against a stub in every language.
class ScriptedTransport : public HttpTransport {
 public:
  /// One scripted answer.
  struct Answer {
    /// The status to answer with. 501 is Connect's `unimplemented`; gRPC-Web
    /// answers 200 and puts the code in the trailers.
    long status = 501;
    std::string content_type = "application/json";
    std::string body;
    /// gRPC-Web trailers, sent as headers for a unary answer.
    std::vector<std::pair<std::string, std::string>> trailers;
  };

  /// Every request the client sent, in order. The idempotency-key test reads it.
  std::vector<HttpRequest> Requests() const { return requests_; }
  void AddAnswer(Answer answer) { answers_.push_back(std::move(answer)); }
  void Clear() {
    requests_.clear();
    answers_.clear();
  }

  HttpResponse Send(const HttpRequest& request) override;
  std::unique_ptr<ByteReader> Open(const HttpRequest& request) override;

 private:
  Answer Next();
  std::vector<Answer> answers_;
  std::vector<HttpRequest> requests_;
};

/// Writes `sdks/fixtures/results/cpp.json` in the format
/// `sdks/conformance/required.mjs` reads: `{tests, ran, skipped, transport}`.
///
/// `transport` is the field `maySkip` keys off, so it is written from the run
/// rather than left out: a report with no transport cannot express why a skip is
/// legal, which means every skip is illegal, which is the correct default and
/// still not the same as saying so.
///
/// A suite that writes none is reported as unverified coverage, which is the
/// right answer for a suite that ran nothing and the wrong one for a suite that
/// ran everything — so the fixture test writes it whether it passed or not.
void WriteResults(const std::vector<std::string>& tests, const std::vector<std::string>& ran,
                  const std::vector<std::pair<std::string, std::string>>& skipped, const std::string& transport);

/// Prints the summary and returns the process exit code. `0` when nothing failed.
int Finish(const std::string& name);

/// How many failures this test has recorded.
int Failures();

}  // namespace loams_test

#endif  // LOAMS_TESTS_SUPPORT_HPP