// The test support: assertions, the corpus reader, the endpoint, the scripted
// transport and the results report.

#include "support.hpp"

#include <dirent.h>
#include <cstring>
#include <thread>
#include <signal.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#include <algorithm>
#include <array>
#include <cstring>
#include <fstream>
#include <sstream>

#include <google/protobuf/struct.pb.h>
#include <google/protobuf/util/json_util.h>

namespace loams_test {
namespace {

int g_failures = 0;

/// Reads a whole file. Throws `Failed` when it cannot: a suite that cannot see
/// the corpus is not a suite.
std::string ReadFileImpl(const std::string& path) {
  std::ifstream file(path, std::ios::binary);
  if (!file) {
    throw Failed{"cannot read " + path};
  }
  std::ostringstream out;
  out << file.rdbuf();
  return out.str();
}

std::vector<std::string> ListDir(const std::string& path) {
  std::vector<std::string> names;
  // `std::filesystem` would be tidier, and it is not available in libstdc++ 8
  // without a link flag. The build here is clang 22 against a modern libstdc++,
  // but `opendir` is portable to every toolchain this SDK claims and needs no
  // link option.
  DIR* const dir = ::opendir(path.c_str());
  if (dir == nullptr) {
    return names;
  }
  while (const dirent* const entry = ::readdir(dir)) {
    const std::string name = entry->d_name;
    if (name == "." || name == "..") {
      continue;
    }
    names.push_back(name);
  }
  ::closedir(dir);
  std::sort(names.begin(), names.end());
  return names;
}

std::string Join(const std::string& left, const std::string& right) {
  if (left.empty()) {
    return right;
  }
  if (left.back() == '/') {
    return left + right;
  }
  return left + "/" + right;
}



/// The `request` or `response` object of a recorded step, or null when it is
/// absent. A step missing either side cannot be replayed, and saying so beats
/// sending a request with no body and wondering why the answer is 400.
const google::protobuf::Struct* Side(const google::protobuf::Struct& step, const char* name) {
  const google::protobuf::Value* const value = Field(step, std::string(name));
  if (value == nullptr || !value->has_struct_value()) {
    return nullptr;
  }
  return &value->struct_value();
}

std::string BodyOf(const google::protobuf::Struct& side) {
  const google::protobuf::Value* const body = Field(side, "body");
  if (body != nullptr && body->has_string_value()) {
    return body->string_value();
  }
  if (body != nullptr && body->has_struct_value()) {
    // A Connect **unary** answer is filed as a structured JSON object, not as the
    // string a binary one is. Reading it with `string_value()` yields the empty
    // string, which makes every refusal look like a body carrying no code and so
    // sends the assertion to the HTTP status — and `400` is `invalid_argument` for
    // the three `failed_precondition` approvals as well as for the genuine
    // `invalid_argument` one.
    std::string json;
    if (google::protobuf::util::MessageToJsonString(body->struct_value(), &json).ok()) {
      return json;
    }
  }
  const google::protobuf::Value* const encoded = Field(side, "bodyBase64");
  if (encoded != nullptr && encoded->has_string_value()) {
    const std::optional<std::string> decoded = loams::Base64Decode(encoded->string_value());
    if (decoded.has_value()) {
      return *decoded;
    }
  }
  return std::string();
}

std::string HeaderAt(const google::protobuf::Struct& object, const char* name) {
  const google::protobuf::Value* const headers = Field(object, "headers");
  if (headers == nullptr || !headers->has_struct_value()) {
    return std::string();
  }
  const google::protobuf::Value* const value = Field(headers->struct_value(), std::string(name));
  if (value == nullptr) {
    return std::string();
  }
  return value->string_value();
}

/// Reads one `expect` block. `into->has_reason` is set whenever the block
/// carries a `reason` key at all, including `null`: "the server sent no
/// `ErrorInfo`" is a recorded fact the suite has to be able to assert, and it
/// cannot be told apart from an absent key if both read as `std::nullopt`.
void ReadExpect(const google::protobuf::Struct& block, Expect* into) {
  if (const google::protobuf::Value* const status = Field(block, "status")) {
    into->status = static_cast<long>(status->number_value());
  }
  if (const google::protobuf::Value* const grpc_status = Field(block, "grpcStatus")) {
    into->grpc_status = static_cast<int>(grpc_status->number_value());
  }
  if (const google::protobuf::Value* const reason = Field(block, "reason")) {
    into->has_reason = true;
    if (reason->has_string_value() && !reason->string_value().empty()) {
      into->reason = reason->string_value();
    }
  }
  if (const google::protobuf::Value* const state = Field(block, "state")) {
    into->state = state->string_value();
  }
  if (const google::protobuf::Value* const revision = Field(block, "revision")) {
    into->revision = revision->string_value();
  }
  if (const google::protobuf::Value* const versions = Field(block, "apiVersions");
      versions != nullptr && versions->has_list_value()) {
    for (const google::protobuf::Value& entry : versions->list_value().values()) {
      into->api_versions.push_back(entry.string_value());
    }
  }
  if (const google::protobuf::Value* const same = Field(block, "identicalToStep")) {
    into->identical_to_step = static_cast<int>(same->number_value());
  }
  if (const google::protobuf::Value* const frames = Field(block, "frames")) {
    into->frames = static_cast<int>(frames->number_value());
  }
  if (const google::protobuf::Value* const kinds = Field(block, "frameKinds");
      kinds != nullptr && kinds->has_list_value()) {
    for (const google::protobuf::Value& entry : kinds->list_value().values()) {
      into->frame_kinds.push_back(entry.string_value());
    }
  }
  if (const google::protobuf::Value* const cursor = Field(block, "cursor");
      cursor != nullptr && cursor->has_string_value()) {
    into->cursor = cursor->string_value();
  }
  if (const google::protobuf::Value* const reset = Field(block, "snapshotReset")) {
    into->snapshot_reset = reset->bool_value();
  }
  if (const google::protobuf::Value* const truncated = Field(block, "truncated")) {
    into->truncated = truncated->bool_value();
  }
}

/// Reads a whole file. Throws `Failed` when it cannot.
std::string ReadFileImpl(const std::string& path);

std::string JoinWithImpl(const std::vector<std::string>& parts, const std::string& separator) {
  std::string out;
  for (std::size_t at = 0; at < parts.size(); ++at) {
    if (at != 0) {
      out += separator;
    }
    out += parts[at];
  }
  return out;
}

}  // namespace

std::string ReadFile(const std::string& path) { return ReadFileImpl(path); }
std::string JoinWith(const std::vector<std::string>& parts, const std::string& separator) {
  return JoinWithImpl(parts, separator);
}

void Record(const std::string& file, int line, const std::string& text) {
  ++g_failures;
  // Flushed per line: `LOAMS_REQUIRE` aborts by throwing, and an exception that
  // reaches `std::terminate` skips every buffer flush — so an unflushed message
  // means the *first* failure of an aborted test is the one nobody reads.
  std::cout << "  FAIL " << file << ":" << line << ": " << text << std::endl;
}

void Abort(const std::string& file, int line, const std::string& text) {
  Record(file, line, text);
  throw Failed{text};
}

int Failures() { return g_failures; }

std::string FixturesDir() {
  const char* const from_env = std::getenv("LOAMS_FIXTURES_DIR");
  // Defaulted to `../fixtures` relative to the source directory, which is where
  // `go test` and `swift test` also look. ctest runs in the build directory, so
  // the CMakeLists passes the absolute path.
  return from_env != nullptr ? from_env : Join(SdkDir(), "../fixtures");
}

std::string ConformanceDir() {
  const char* const from_env = std::getenv("LOAMS_CONFORMANCE_DIR");
  return from_env != nullptr ? from_env : Join(SdkDir(), "../conformance");
}

std::string SdkDir() {
  // `__FILE__` is `tests/support.cpp` relative to the build, so the source
  // directory is two levels up from the directory this file was compiled from.
  // Resolved once and cached, because it is on every corpus read.
  static const std::string* const dir = new std::string([] {
    std::string here = __FILE__;
    const std::size_t last = here.find_last_of('/');
    if (last != std::string::npos) {
      here = here.substr(0, last);
    }
    const std::size_t up = here.find_last_of('/');
    if (up != std::string::npos) {
      here = here.substr(0, up);
    }
    // The compiled path may be absolute (a build outside the source tree), in
    // which case it already ends in `sdks/cpp`.
    if (here.rfind("sdks", here.size() - 4) != std::string::npos && here.size() >= 4) {
      return here;
    }
    return here;
  }());
  return *dir;
}

std::vector<Recorded> ReadCorpus() {
  std::vector<Recorded> corpus;
  const std::string root = Join(FixturesDir(), "recorded");
  std::array<const char*, 2> const directories{"", "apps-mock"};
  for (const char* const directory : directories) {
    for (const std::string& name : ListDir(Join(root, directory))) {
      if (name.size() < 6 || name.compare(name.size() - 5, 5, ".json") != 0) {
        continue;
      }
      const std::string path = Join(Join(root, directory), name);
      const std::optional<google::protobuf::Struct> parsed = ParseJson(ReadFile(path));
      if (!parsed.has_value()) {
        continue;
      }
      Recorded entry;
      entry.name = StringAt(*parsed, "name");
      entry.about = StringAt(*parsed, "about");
      const google::protobuf::Value* const steps = Field(*parsed, "steps");
      if (steps != nullptr && steps->has_list_value()) {
        for (const google::protobuf::Value& value : steps->list_value().values()) {
          if (!value.has_struct_value()) {
            continue;
          }
          const google::protobuf::Struct& step = value.struct_value();
          const google::protobuf::Struct* const request = Side(step, "request");
          const google::protobuf::Struct* const response = Side(step, "response");
          if (request == nullptr || response == nullptr) {
            continue;
          }
          RecordedStep built;
          built.method = StringAt(*request, "method");
          built.path = StringAt(*request, "path");
          built.content_type = HeaderAt(*request, "content-type");
          built.step = HeaderAt(*request, "loams-fixture-step");
          built.body = BodyOf(*request);
          const google::protobuf::Value* const status = Field(*response, "status");
          built.status = status == nullptr ? 0 : static_cast<long>(status->number_value());
          built.response_content_type = HeaderAt(*response, "content-type");
          built.response_body = BodyOf(*response);
          const google::protobuf::Value* const frames = Field(*response, "frames");
          if (frames != nullptr && frames->has_list_value()) {
            for (const google::protobuf::Value& frame : frames->list_value().values()) {
              if (!frame.has_struct_value()) {
                continue;
              }
              const std::string payload = StringAt(frame.struct_value(), "payload");
              const std::optional<std::string> decoded = loams::Base64Decode(payload);
              const google::protobuf::Value* const flags = Field(frame.struct_value(), "flags");
              const auto flag = flags == nullptr ? std::uint8_t{0}
                                                : static_cast<std::uint8_t>(flags->number_value());
              built.frames.push_back(
                  loams::EncodeEnvelopeFrame(flag, decoded.has_value() ? *decoded : std::string()));
            }
          }
          // A scenario carries its `expect` **per step**: the resumed stream's
          // expected cursor belongs to step 2, not to the fixture, and reading only
          // the top-level block would leave every scenario's expectations empty.
          if (const google::protobuf::Struct* const expect = Side(step, "expect")) {
            ReadExpect(*expect, &built.expect);
          }
          if (const google::protobuf::Value* const truncated = Field(*response, "truncated");
              truncated != nullptr) {
            built.truncated = truncated->bool_value();
            built.expect.truncated = built.truncated;
          }
          entry.steps.push_back(std::move(built));
        }
      } else if (const google::protobuf::Struct* const request = Side(*parsed, "request")) {
        // A single-request recording has the request at the top level, with no
        // `steps` array.
        const google::protobuf::Struct* const response = Side(*parsed, "response");
        if (response != nullptr) {
          RecordedStep built;
          built.method = StringAt(*request, "method");
          built.path = StringAt(*request, "path");
          built.content_type = HeaderAt(*request, "content-type");
          built.body = BodyOf(*request);
          const google::protobuf::Value* const status = Field(*response, "status");
          built.status = status == nullptr ? 0 : static_cast<long>(status->number_value());
          built.response_content_type = HeaderAt(*response, "content-type");
          built.response_body = BodyOf(*response);
          // The top-level `expect` is the step's: a single-request fixture has one
          // step, and every assertion reads `expect` off the step, so leaving it
          // on the fixture would make every single-request fixture look like it
          // expected a `200` and carried no reason.
          if (const google::protobuf::Struct* const expect = Side(*parsed, "expect")) {
            ReadExpect(*expect, &built.expect);
          }
          if (const google::protobuf::Value* const truncated = Field(*response, "truncated");
              truncated != nullptr) {
            built.truncated = truncated->bool_value();
            built.expect.truncated = built.truncated;
          }
          entry.steps.push_back(std::move(built));
        }
      }
      if (const google::protobuf::Struct* const expect = Side(*parsed, "expect")) {
        ReadExpect(*expect, &entry.expect);
      }
      if (const google::protobuf::Struct* const response = Side(*parsed, "response")) {
        if (const google::protobuf::Value* const truncated = Field(*response, "truncated");
            truncated != nullptr) {
          entry.expect.truncated = truncated->bool_value();
        }
      }
      corpus.push_back(std::move(entry));
    }
  }
  return corpus;
}

// --- Endpoint --------------------------------------------------------------------------------------

Endpoint::Endpoint() {
  const char* const live = std::getenv("LOAMS_TEST_ENDPOINT");
  if (live != nullptr && live[0] != '\0') {
    std::string trimmed = live;
    while (!trimmed.empty() && trimmed.back() == '/') {
      trimmed.pop_back();
    }
    url_ = trimmed;
    live_ = true;
    how_ = "LOAMS_TEST_ENDPOINT";
    return;
  }
  StartNode();
}

void Endpoint::StartNode() {
  // `node` is on PATH on every machine that develops this repository and on every
  // CI runner. When it is not, the in-process replay below runs instead: a C++
  // test should not need a JavaScript runtime.
  const std::string script = Join(ConformanceDir(), "fixture-server.mjs");
  const std::string fixtures = FixturesDir();

  // The server's stdout goes to a **file**, not to a pipe this process reads once.
  //
  // A pipe that is closed after the URL has been read takes the server down: the
  // next `console.log` on a closed pipe is EPIPE, and Node exits on it. That is not
  // a hypothetical — it hung the suite on the first multi-step fixture, because
  // the server died after the fourth request and every later call waited for a
  // timeout. A file cannot be closed from under the child.
  char log_path[] = "/tmp/loams-fixture-server-XXXXXX";
  const int log_fd = ::mkstemp(log_path);
  if (log_fd < 0) {
    StartReplay();
    return;
  }
  const pid_t child = ::fork();
  if (child < 0) {
    ::close(log_fd);
    ::unlink(log_path);
    StartReplay();
    return;
  }
  if (child == 0) {
    ::dup2(log_fd, STDOUT_FILENO);
    // stderr is left alone so a fixture server that fails says why on the test's
    // own output rather than into a file nobody reads.
    ::close(log_fd);
    ::execlp("node", "node", script.c_str(), "--fixtures", fixtures.c_str(), "--port", "0",
             static_cast<char*>(nullptr));
    // `execlp` only returns on failure, and a port-0 fixture server exiting 127 is
    // how the parent finds out.
    ::_exit(127);
  }
  ::close(log_fd);
  child_ = child;
  log_path_ = log_path;

  // The server prints `{"url":"http://127.0.0.1:PORT"}` once it is listening.
  // Poll the file until that line appears, with a deadline, because a server that
  // never prints would otherwise hang the test forever.
  const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(30);
  std::string text;
  bool got = false;
  while (std::chrono::steady_clock::now() < deadline) {
    std::ifstream log(log_path, std::ios::binary);
    if (log) {
      std::ostringstream out;
      out << log.rdbuf();
      text = out.str();
    }
    std::size_t at = 0;
    while (at < text.size()) {
      const std::size_t end = text.find('\n', at);
      const std::string candidate =
          text.substr(at, end == std::string::npos ? std::string::npos : end - at);
      const std::optional<google::protobuf::Struct> parsed = ParseJson(candidate);
      if (parsed.has_value() && !StringAt(*parsed, "url").empty()) {
        url_ = StringAt(*parsed, "url");
        got = true;
        break;
      }
      if (end == std::string::npos) {
        break;
      }
      at = end + 1;
    }
    if (got) {
      break;
    }
    std::this_thread::sleep_for(std::chrono::milliseconds(50));
  }
  if (!got) {
    Stop();
    StartReplay();
    return;
  }
  how_ = "node sdks/conformance/fixture-server.mjs";
}

void Endpoint::Stop() {
  if (child_ < 0) {
    return;
  }
  // SIGTERM first, then a reap: a fixture server that outlives the run would
  // collide with the next one on the port.
  ::kill(static_cast<pid_t>(child_), SIGTERM);
  int status = 0;
  ::waitpid(static_cast<pid_t>(child_), &status, 0);
  child_ = -1;
  if (!log_path_.empty()) {
    ::unlink(log_path_.c_str());
    log_path_.clear();
  }
}

Endpoint::~Endpoint() { Stop(); }

namespace {

/// The in-process replay, for a machine with no Node.
///
/// Same matching rules as `fixture-server.mjs`: keyed on method, path and content
/// **family**, with the recorded request body checked, and a **404 naming the
/// gap** for anything unrecorded rather than a silent success. A suite that passed
/// because everything answered 200 would be worse than no suite.
class ReplayTransport final : public HttpTransport {
 public:
  explicit ReplayTransport(const std::vector<Recorded>& corpus) {
    for (const Recorded& entry : corpus) {
      for (const RecordedStep& step : entry.steps) {
        const std::string key = Key(step.method, step.path, step.content_type);
        scripts_[key].push_back(Script{entry.name, step});
      }
    }
    // The `loams dev` half is the default where a key overlaps, which is what
    // `fixture-server.mjs` does and what every suite resolved before the app-mock
    // scenarios existed.
    for (const auto& entry : scripts_) {
      const auto found =
          std::find_if(entry.second.begin(), entry.second.end(), [](const Script& script) {
            return script.name.rfind("instance_", 0) == 0 || script.name.rfind("live_", 0) == 0;
          });
      if (found != entry.second.end()) {
        defaults_[entry.first] = static_cast<std::size_t>(found - entry.second.begin());
      }
    }
  }

  HttpResponse Send(const HttpRequest& request) override {
    const Answer answer = Look(request);
    HttpResponse response;
    response.status = answer.status;
    response.headers = answer.headers;
    response.body = answer.body;
    return response;
  }

  std::unique_ptr<ByteReader> Open(const HttpRequest& request) override {
    const Answer answer = Look(request);
    return std::make_unique<StringReader>(answer.status, answer.headers, answer.body);
  }

 private:
  struct Script {
    std::string name;
    RecordedStep step;
  };

  struct Answer {
    long status = 0;
    std::vector<std::pair<std::string, std::string>> headers;
    std::string body;
  };

  static std::string Key(const std::string& method, const std::string& path, const std::string& content_type) {
    return method + " " + path + " " + loams::ContentFamily(content_type);
  }

  static std::string ContentTypeOf(const HttpRequest& request) {
    for (const auto& header : request.headers) {
      if (header.first == "content-type" || header.first == "Content-Type") {
        return header.second;
      }
    }
    return std::string();
  }

  static std::string HeaderOf(const HttpRequest& request, const char* name) {
    for (const auto& header : request.headers) {
      std::string lowered = header.first;
      for (char& character : lowered) {
        if (character >= 'A' && character <= 'Z') character = static_cast<char>(character - 'A' + 'a');
      }
      if (lowered == name) {
        return header.second;
      }
    }
    return std::string();
  }

  Answer Look(const HttpRequest& request) const {
    Answer answer;
    // The path the SDK built is `endpoint + /pkg.Service/Method`; only the path
    // part keys the lookup, so the endpoint can be any URL.
    std::string path = request.url;
    const std::size_t scheme = path.find("://");
    if (scheme != std::string::npos) {
      const std::size_t slash = path.find('/', scheme + 3);
      path = slash == std::string::npos ? std::string("/") : path.substr(slash);
    }
    const std::size_t query = path.find('?');
    if (query != std::string::npos) {
      path = path.substr(0, query);
    }

    const std::string key = Key(request.method, path, ContentTypeOf(request));
    const auto found = scripts_.find(key);
    if (found == scripts_.end()) {
      // Loudly. A suite that passed because everything answered 200 would be worse
      // than no suite.
      answer.status = 404;
      answer.headers.emplace_back("content-type", "application/json");
      answer.body = "{\"error\":\"no recorded fixture for " + key + "\"}";
      return answer;
    }

    const std::string name = HeaderOf(request, "loams-fixture-name");
    const Script* chosen = nullptr;
    const std::vector<Script>& candidates = found->second;
    if (!name.empty()) {
      for (const Script& script : candidates) {
        if (script.name == name) {
          chosen = &script;
        }
      }
      if (chosen == nullptr) {
        answer.status = 404;
        answer.headers.emplace_back("content-type", "application/json");
        answer.body = "{\"error\":\"no fixture named " + name + " for " + key + "\"}";
        return answer;
      }
    } else {
      const auto by_default = defaults_.find(key);
      if (by_default != defaults_.end()) {
        chosen = &candidates[by_default->second];
      } else if (candidates.size() == 1) {
        chosen = &candidates.front();
      } else {
        // Six recorded scenarios are `DecideApproval` over Connect JSON; a silent
        // pick would hand the suite the bytes of a fixture it did not ask for.
        answer.status = 409;
        answer.headers.emplace_back("content-type", "application/json");
        answer.body = "{\"error\":\"" + std::to_string(candidates.size()) + " fixtures answer " + key +
                      "; say which with loams-fixture-name\"}";
        return answer;
      }
    }

    const std::string step_header = HeaderOf(request, "loams-fixture-step");
    const RecordedStep& step = chosen->step;
    // The request is checked as well as the response: a replay that ignored what
    // the client sent would pass an SDK that frames a gRPC-Web message wrongly or
    // posts the wrong payload, which is the class of bug a recorded corpus exists
    // to catch.
    if (request.body != step.body) {
      answer.status = 400;
      answer.headers.emplace_back("content-type", "application/json");
      answer.body = "{\"error\":\"the request does not match the recorded one for " + chosen->name +
                    " step " + step_header + "\",\"expected\":\"" + loams::Base64Encode(step.body) +
                    "\",\"sent\":\"" + loams::Base64Encode(request.body) + "\"}";
      return answer;
    }

    answer.status = step.status;
    answer.headers.emplace_back("content-type", step.response_content_type);
    if (!step.response_content_type.empty()) {
      answer.headers.emplace_back("loams-fixture-name", chosen->name);
    }
    // A recorded stream is written as its frames, so the reader observes them
    // arriving one at a time — the only way a cursor or a heartbeat is observable
    // at all.
    answer.body.clear();
    for (const std::string& frame : step.frames) {
      answer.body += frame;
    }
    if (step.frames.empty()) {
      answer.body = step.response_body;
    }
    return answer;
  }

  std::map<std::string, std::vector<Script>> scripts_;
  std::map<std::string, std::size_t> defaults_;
};

/// One replay per process: the recordings are read once and the table is
/// immutable, so sharing it costs nothing and a second read of twenty files per
/// test is pure waste.
std::unique_ptr<ReplayTransport> g_replay_transport;

}  // namespace

void Endpoint::StartReplay() {
  if (!g_replay_transport) {
    g_replay_transport = std::make_unique<ReplayTransport>(ReadCorpus());
  }
  // The in-process replay is a **lookup**, not a socket: the SDK is given this
  // transport and it answers from the same table `fixture-server.mjs` serves.
  // That is the whole reason the tier exists — a C++ test should not need a
  // JavaScript runtime to run, and the recorded bytes are identical either way.
  how_ = "in-process replay of sdks/fixtures/recorded";
  url_ = "replay://sdks/fixtures/recorded";
  live_ = false;
  transport_ = g_replay_transport.get();
}

// --- ScriptedTransport -----------------------------------------------------------------------------

HttpResponse ScriptedTransport::Send(const HttpRequest& request) {
  requests_.push_back(request);
  if (answers_.empty()) {
    // Running out of script is a failure **below the API** — a connection that went
    // away — and it is thrown rather than answered with a status, because a status
    // would make it a Loams failure with a reason, and R8 keeps those two apart.
    throw TransportError("", "loams_test: the scripted transport has no answer left");
  }
  const Answer answer = Next();
  HttpResponse response;
  response.status = answer.status;
  response.headers.emplace_back("content-type", answer.content_type);
  for (const auto& trailer : answer.trailers) {
    response.headers.push_back(trailer);
  }
  response.body = answer.body;
  return response;
}

std::unique_ptr<ByteReader> ScriptedTransport::Open(const HttpRequest& request) {
  requests_.push_back(request);
  if (answers_.empty()) {
    // The header list is built first rather than braced in the call: a braced
    // initialiser list inside a template argument pack does not deduce, and the
    // error that comes back says nothing about which argument it was.
    std::vector<std::pair<std::string, std::string>> headers{{"content-type", "application/json"}};
    return std::make_unique<StringReader>(599, headers,
                                          R"({"error":"the scripted transport has no answer left"})");
  }
  const Answer answer = Next();
  std::vector<std::pair<std::string, std::string>> headers{{"content-type", answer.content_type}};
  for (const auto& trailer : answer.trailers) {
    headers.push_back(trailer);
  }
  return std::make_unique<StringReader>(answer.status, headers, answer.body);
}

ScriptedTransport::Answer ScriptedTransport::Next() {
  Answer answer = answers_.front();
  answers_.erase(answers_.begin());
  return answer;
}

// --- Reporting -------------------------------------------------------------------------------------

void WriteResults(const std::vector<std::string>& tests, const std::vector<std::string>& ran,
                  const std::vector<std::pair<std::string, std::string>>& skipped,
                  const std::string& transport) {
  const std::string results = Join(FixturesDir(), "results");
  ::mkdir(results.c_str(), 0755);
  std::ostringstream json;
  json << "{\n";
  json << "  \"tests\": [\n";
  // Sorted, one per line: `required.mjs` checks the names and ignores the order,
  // so sorted is for the reader's diff and for two runs of the same corpus
  // producing the same file.
  std::vector<std::string> names = tests;
  std::sort(names.begin(), names.end());
  for (std::size_t at = 0; at < names.size(); ++at) {
    json << "    \"" << names[at] << "\"" << (at + 1 == names.size() ? "\n" : ",\n");
  }
  json << "  ],\n";
  json << "  \"ran\": [\n";
  std::vector<std::string> sorted = ran;
  std::sort(sorted.begin(), sorted.end());
  for (std::size_t at = 0; at < sorted.size(); ++at) {
    json << "    \"" << sorted[at] << "\"" << (at + 1 == sorted.size() ? "\n" : ",\n");
  }
  json << "  ],\n";
  json << "  \"skipped\": [\n";
  for (std::size_t at = 0; at < skipped.size(); ++at) {
    json << "    { \"fixture\": \"" << skipped[at].first << "\", \"reason\": \"" << skipped[at].second
         << "\" }" << (at + 1 == skipped.size() ? "\n" : ",\n");
  }
  json << "  ],\n";
  json << "  \"transport\": \"" << transport << "\"\n";
  json << "}\n";
  std::ofstream file(Join(results, "cpp.json"), std::ios::binary | std::ios::trunc);
  if (!file) {
    Record(__FILE__, __LINE__, "cannot write " + Join(results, "cpp.json"));
    return;
  }
  file << json.str();
}

int Finish(const std::string& name) {
  if (g_failures == 0) {
    std::cout << name << ": ok\n";
    return 0;
  }
  std::cout << name << ": " << g_failures << " failure(s)\n";
  return 1;
}

}  // namespace loams_test