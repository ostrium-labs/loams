// The app-building API without a server (SDKP Part A): the token and builder
// corpora, the runtime additions R11–R14 and the collection handle.
//
// One executable, one ctest test per clause: `app_test <name>` runs `<name>`,
// and the CMakeLists registers each as `cpp_<name>`, the names
// `check-languages.mjs` looks for.

#include <algorithm>
#include <cmath>
#include <set>
#include <sys/stat.h>
#include <cstdio>
#include <fstream>
#include <functional>
#include <map>
#include <sstream>
#include <string>
#include <vector>

#include <dirent.h>

#include <google/protobuf/struct.pb.h>
#include <google/protobuf/util/json_util.h>
#include <google/protobuf/util/message_differencer.h>

#include "support.hpp"

namespace {

using namespace loams_test;  // NOLINT(build/namespaces)
using google::protobuf::util::MessageDifferencer;

std::string ReadFile(const std::string& path) {
  std::ifstream in(path);
  std::stringstream text;
  text << in.rdbuf();
  return text.str();
}

bool Exists(const std::string& path) { return std::ifstream(path).good(); }

google::protobuf::Value Parse(const std::string& json) {
  google::protobuf::Value out;
  if (!google::protobuf::util::JsonStringToMessage(json, &out).ok()) {
    throw Failed("not JSON: " + json.substr(0, 80));
  }
  return out;
}

const google::protobuf::Value& At(const google::protobuf::Value& object, const char* key) {
  static const google::protobuf::Value kNull;
  const auto& fields = object.struct_value().fields();
  const auto found = fields.find(key);
  return found == fields.end() ? kNull : found->second;
}

bool Has(const google::protobuf::Value& object, const char* key) {
  return object.has_struct_value() && object.struct_value().fields().count(key) > 0;
}

std::string Str(const google::protobuf::Value& value) { return value.string_value(); }

std::uint32_t U32(const google::protobuf::Value& value) { return static_cast<std::uint32_t>(value.number_value()); }

std::vector<std::string> Strings(const google::protobuf::Value& value) {
  std::vector<std::string> out;
  for (const auto& item : value.list_value().values()) {
    out.push_back(item.string_value());
  }
  return out;
}

std::vector<float> Floats(const google::protobuf::Value& value) {
  std::vector<float> out;
  for (const auto& item : value.list_value().values()) {
    out.push_back(static_cast<float>(item.number_value()));
  }
  return out;
}

/// A JSON client: answers are proto3 JSON, so a test can script them in text.
struct Harness {
  std::shared_ptr<ScriptedTransport> transport = std::make_shared<ScriptedTransport>();
  std::shared_ptr<Loams> loams;

  explicit Harness(const std::function<void(Options&)>& configure = {}) {
    Options options;
    options.endpoint = "http://127.0.0.1:4400";
    options.transport = transport;
    options.content_type = ContentType::kJson;
    if (configure) {
      configure(options);
    }
    loams = MakeLoams(std::move(options));
  }

  void Ok(const std::string& json = "{}") {
    ScriptedTransport::Answer answer;
    answer.status = 200;
    answer.body = json;
    transport->AddAnswer(answer);
  }

  void Fail(long status, const std::string& code, const std::string& message) {
    ScriptedTransport::Answer answer;
    answer.status = status;
    answer.body = "{\"code\":\"" + code + "\",\"message\":\"" + message + "\"}";
    transport->AddAnswer(answer);
  }

  google::protobuf::Value Sent(std::size_t index) const { return Parse(transport->Requests().at(index).body); }

  std::string Header(std::size_t index, const std::string& name) const {
    const std::vector<HttpRequest> requests = transport->Requests();
    for (const auto& header : requests.at(index).headers) {
      if (header.first == name) {
        return header.second;
      }
    }
    return std::string();
  }

  std::string Method(std::size_t index) const {
    const std::string url = transport->Requests().at(index).url;
    return url.substr(url.rfind('/') + 1);
  }
};

// --- R4 ------------------------------------------------------------------------------------------

void ConsistencyTokenMerge() {
  LOAMS_CHECK_EQ(ConsistencyToken::Parse("v1:s7/p3@918274").Value(), std::string("v1:s7/p3@918274"), "one item");
  LOAMS_CHECK_EQ(ConsistencyToken::Parse("v1:s2/p0@1,s1/p5@3").Value(), std::string("v1:s2/p0@1,s1/p5@3"),
                 "parse keeps the order");
  const ConsistencyToken merged =
      ConsistencyToken::Parse("v1:s7/p3@10,s1/p0@5").Merge(ConsistencyToken::Parse("v1:s7/p3@4,s7/p1@2,s1/p0@9"));
  LOAMS_CHECK_EQ(merged.Value(), std::string("v1:s1/p0@9,s7/p1@2,s7/p3@10"), "merge keeps the max");
  LOAMS_CHECK(merged.Offset(7, 3) == std::optional<std::uint64_t>(10), "offset");
  LOAMS_CHECK(!merged.Offset(1, 1).has_value(), "no offset");

  const std::string corpus = FixturesDir() + "/tokens.json";
  if (!Exists(corpus)) {
    std::cout << "no token corpus at " << corpus << "; the inline cases ran\n";
    return;
  }
  const google::protobuf::Value data = Parse(ReadFile(corpus));
  for (const auto& c : At(data, "parse").list_value().values()) {
    const ConsistencyToken token = ConsistencyToken::Parse(Str(At(c, "text")));
    LOAMS_CHECK_EQ(token.Value(), Str(At(c, "text")), Str(At(c, "name")));
    const auto& items = At(c, "items").list_value();
    LOAMS_CHECK_EQ(static_cast<int>(token.Items().size()), items.values_size(), Str(At(c, "name")));
    for (int i = 0; i < items.values_size() && static_cast<std::size_t>(i) < token.Items().size(); ++i) {
      const auto& item = items.values(i).list_value();
      const auto& got = token.Items()[static_cast<std::size_t>(i)];
      LOAMS_CHECK_EQ(std::to_string(got.stream), item.values(0).string_value(), Str(At(c, "name")) + " stream");
      LOAMS_CHECK_EQ(got.partition, static_cast<std::uint32_t>(item.values(1).number_value()),
                     Str(At(c, "name")) + " partition");
      LOAMS_CHECK_EQ(std::to_string(got.offset), item.values(2).string_value(), Str(At(c, "name")) + " offset");
    }
  }
  for (const auto& c : At(data, "merge").list_value().values()) {
    LOAMS_CHECK_EQ(ConsistencyToken::Parse(Str(At(c, "a"))).Merge(ConsistencyToken::Parse(Str(At(c, "b")))).Value(),
                   Str(At(c, "expect")), Str(At(c, "name")));
  }
  for (const auto& bad : At(data, "malformed").list_value().values()) {
    LOAMS_CHECK_THROWS(ConsistencyToken::Parse(bad.string_value()), InvalidArgumentError,
                       "malformed \"" + bad.string_value() + "\"");
  }
}

void SessionReadYourWrites() {
  Harness h([](Options& o) {
    o.default_namespace = "app";
    o.session_consistency = true;
  });
  h.Ok(R"({"token":"v1:s1/p0@5"})");
  h.Ok(R"({"readToken":"v1:s1/p0@5,s1/p1@3"})");
  h.Ok();
  h.Ok();
  h.loams->Documents()->Upsert("notes", {Document::FromJson(1, R"({"title":"a"})")});
  h.loams->Documents()->Get("notes", {1});
  LOAMS_CHECK_EQ(Str(At(At(h.Sent(1), "consistency"), "atLeast")), std::string("v1:s1/p0@5"), "reads its write");
  h.loams->Documents()->Get("notes", {1});
  LOAMS_CHECK_EQ(Str(At(At(h.Sent(2), "consistency"), "atLeast")), std::string("v1:s1/p0@5,s1/p1@3"),
                 "the read token merges in");
  ReadOptions eventual;
  eventual.consistency = "eventual";
  h.loams->Documents()->Get("notes", {1}, eventual);
  LOAMS_CHECK_EQ(Str(At(At(h.Sent(3), "consistency"), "freshness")), std::string("FRESHNESS_EVENTUAL"),
                 "an explicit consistency is the caller's");
  LOAMS_CHECK(!Has(At(h.Sent(3), "consistency"), "atLeast"), "and is not overwritten");

  // SDKP-17: a graph read never takes the session's token.
  h.Ok();
  GraphQueryOptions read_only;
  read_only.read_only = true;
  h.loams->Graph()->Query("g", "MATCH (n) RETURN n", {}, read_only);
  LOAMS_CHECK(!Has(h.Sent(4), "consistency"), "a graph read takes no session token");

  Harness plain([](Options& o) { o.default_namespace = "app"; });
  plain.Ok(R"({"token":"v1:s1/p0@5"})");
  plain.Ok();
  plain.loams->Documents()->Upsert("notes", {Document::FromJson(1, "{}")});
  plain.loams->Documents()->Get("notes", {1});
  LOAMS_CHECK(!Has(plain.Sent(1), "consistency"), "off by default");
}

// --- R11 -----------------------------------------------------------------------------------------

void DefaultNamespace() {
  Harness h([](Options& o) { o.default_namespace = "app"; });
  h.Ok(R"({"name":"notes"})");
  h.Ok(R"({"name":"notes"})");
  h.loams->Collections()->Get("notes");
  LOAMS_CHECK_EQ(Str(At(h.Sent(0), "namespace")), std::string("app"), "the default fills an empty namespace");
  CallOptions other;
  other.ns = "other";
  h.loams->Collections()->Get("notes", other);
  LOAMS_CHECK_EQ(Str(At(h.Sent(1), "namespace")), std::string("other"), "an explicit namespace wins");

  Harness bare;
  try {
    bare.loams->Collections()->Get("notes");
    Record(__FILE__, __LINE__, "no namespace at all was not refused");
  } catch (const InvalidArgumentError& error) {
    LOAMS_CHECK(HasReason(error, "namespace_required"), "the reason is namespace_required");
  }
  LOAMS_CHECK(bare.transport->Requests().empty(), "refused before any network call");
}

// --- R12 -----------------------------------------------------------------------------------------

/// A transport whose every call runs out of time, as libcurl reports it.
class TimingOut : public ScriptedTransport {
 public:
  HttpResponse Send(const HttpRequest& request) override {
    ScriptedTransport::Send(request);
    ThrowClientSide(Code::kDeadlineExceeded, "", kClientDeadlineExceeded, "loams: Timeout was reached");
  }
};

void TimeoutDeadline() {
  Harness h([](Options& o) {
    o.default_namespace = "app";
    o.timeout = std::chrono::milliseconds(5000);
  });
  h.Ok();
  h.Ok();
  h.loams->Collections()->Get("notes");
  const long ms = std::stol(h.Header(0, "connect-timeout-ms"));
  LOAMS_CHECK(ms > 0 && ms <= 5000, "the client default goes out as connect-timeout-ms");
  LOAMS_CHECK(h.transport->Requests()[0].timeout.count() <= 5000, "and is the transport's limit");
  CallOptions quick;
  quick.timeout = std::chrono::milliseconds(250);
  h.loams->Collections()->Get("notes", quick);
  LOAMS_CHECK(std::stol(h.Header(1, "connect-timeout-ms")) <= 250, "a call overrides it");

  auto slow = std::make_shared<TimingOut>();
  slow->AddAnswer({});
  slow->AddAnswer({});
  Options options;
  options.endpoint = "http://127.0.0.1:4400";
  options.transport = slow;
  options.default_namespace = "app";
  options.content_type = ContentType::kJson;
  std::shared_ptr<Loams> loams = MakeLoams(std::move(options));
  bool threw = false;
  try {
    loams->Collections()->Get("notes");
  } catch (const DeadlineExceededError& error) {
    threw = true;
    LOAMS_CHECK(HasReason(error, kClientDeadlineExceeded), "the client's own reason");
  }
  LOAMS_CHECK(threw, "a spent budget throws DeadlineExceededError");
  LOAMS_CHECK_EQ(slow->Requests().size(), static_cast<std::size_t>(1), "and is never retried");

  Harness none([](Options& o) {
    o.default_namespace = "app";
    o.timeout = std::chrono::milliseconds(0);
  });
  none.Ok();
  none.loams->Collections()->Get("notes");
  LOAMS_CHECK(none.Header(0, "connect-timeout-ms").empty(), "no budget, no header");
}

// --- R13 -----------------------------------------------------------------------------------------

void WriteBatchChunks() {
  std::vector<Document> docs;
  for (int i = 1; i <= 5; ++i) {
    docs.push_back(Document::FromJson(i, "{\"n\":" + std::to_string(i) + "}"));
  }
  Harness h([](Options& o) { o.default_namespace = "app"; });
  h.Ok(R"({"token":"v1:s1/p0@2"})");
  h.Ok(R"({"token":"v1:s1/p1@2"})");
  h.Ok(R"({"token":"v1:s1/p0@5"})");
  BatchOptions two;
  two.chunk_rows = 2;
  const BatchResult result = h.loams->Documents()->WriteBatch("notes", docs, two);
  LOAMS_CHECK_EQ(result.token, std::string("v1:s1/p0@5,s1/p1@2"), "the merged token");
  LOAMS_CHECK_EQ(result.chunks, static_cast<std::size_t>(3), "three chunks");
  LOAMS_CHECK_EQ(result.rows, static_cast<std::size_t>(5), "five rows");
  std::vector<std::string> keys;
  for (std::size_t i = 0; i < 3; ++i) {
    LOAMS_CHECK_EQ(At(h.Sent(i), "ops").list_value().values_size(), i < 2 ? 2 : 1, "chunk sizes");
    keys.push_back(Str(At(h.Sent(i), "idempotencyKey")));
  }
  LOAMS_CHECK(!keys[0].empty() && keys[0] != keys[1] && keys[1] != keys[2], "every chunk has its own key");

  Harness f([](Options& o) { o.default_namespace = "app"; });
  f.Ok(R"({"token":"v1:s1/p0@2"})");
  f.Fail(503, "unavailable", "down");
  f.Ok(R"({"token":"v1:s1/p0@4"})");
  f.Fail(400, "invalid_argument", "bad doc");
  bool threw = false;
  try {
    f.loams->Documents()->WriteBatch("notes", docs, two);
  } catch (const BatchError& error) {
    threw = true;
    LOAMS_CHECK_EQ(error.ChunksApplied(), static_cast<std::size_t>(2), "two chunks applied");
    LOAMS_CHECK_EQ(error.RowsApplied(), static_cast<std::size_t>(4), "four rows applied");
    LOAMS_CHECK_EQ(error.Token(), std::string("v1:s1/p0@4"), "their token");
    LOAMS_CHECK(error.ReasonValue() == ReasonFromString("batch_partially_applied").value_or(Reason::kUnknown) &&
                    (error.ReasonValue() != Reason::kUnknown || error.UnknownReason() == "batch_partially_applied"),
                "the reason");
    LOAMS_CHECK(error.CodeValue() == Code::kAborted, "a stopped batch is aborted (SDKP-23)");
    LOAMS_CHECK_EQ(error.Metadata().at("chunks_applied"), std::string("2"), "metadata");
  }
  LOAMS_CHECK(threw, "a failed chunk stops the batch");
  LOAMS_CHECK_EQ(Str(At(f.Sent(1), "idempotencyKey")), Str(At(f.Sent(2), "idempotencyKey")),
                 "a retried chunk reuses its key");

  Harness b([](Options& o) { o.default_namespace = "app"; });
  for (int i = 0; i < 5; ++i) b.Ok();
  BatchOptions small;
  small.chunk_bytes = 20;
  b.loams->Documents()->WriteBatch("notes", docs, small);
  LOAMS_CHECK(b.transport->Requests().size() > 1, "the byte budget closes a chunk");
}

// --- R14 -----------------------------------------------------------------------------------------

struct Rec {
  std::uint64_t offset;
  std::string value;
};

void StreamsSubscribeResumes() {
  std::vector<Rec> log;
  for (std::uint64_t i = 0; i < 5; ++i) {
    log.push_back({i, "r" + std::to_string(i)});
  }
  std::vector<std::uint64_t> fetched;
  std::vector<long> sleeps;
  Subscription<Rec> sub(
      [&](std::uint64_t offset) {
        fetched.push_back(offset);
        std::vector<Rec> out;
        if (fetched.size() == 2 || fetched.size() == 3) {
          return out;
        }
        for (const Rec& r : log) {
          if (r.offset >= offset && out.size() < 2) out.push_back(r);
        }
        return out;
      },
      [](const Rec& r) { return r.offset; }, 1, [&](std::chrono::milliseconds d) { sleeps.push_back(d.count()); });
  std::vector<std::string> got;
  while (auto record = sub.Next()) {
    got.push_back(record->value);
    if (got.size() == 4) sub.Cancel();
  }
  LOAMS_CHECK(got == std::vector<std::string>({"r1", "r2", "r3", "r4"}), "the records in order");
  LOAMS_CHECK_EQ(sub.Offset(), static_cast<std::uint64_t>(5), "the next offset is the last + 1");
  LOAMS_CHECK(fetched == std::vector<std::uint64_t>({1, 3, 3, 3}), "fetches resume at the offset");
  LOAMS_CHECK(sleeps == std::vector<long>({100, 200}), "an empty fetch backs off, doubling");

  std::vector<long> capped;
  Subscription<Rec>* self = nullptr;
  Subscription<Rec> empty([](std::uint64_t) { return std::vector<Rec>(); }, [](const Rec& r) { return r.offset; }, 0,
                          [&](std::chrono::milliseconds d) {
                            capped.push_back(d.count());
                            if (capped.size() == 6) self->Cancel();
                          });
  self = &empty;
  LOAMS_CHECK(!empty.Next().has_value(), "a cancelled subscription answers nothing");
  LOAMS_CHECK(capped == std::vector<long>({100, 200, 400, 800, 1000, 1000}), "the backoff stops at 1 s");
}

// --- The builders --------------------------------------------------------------------------------

Filter FilterOf(const google::protobuf::Value& call);

collection_v1::Fusion FusionOf(const google::protobuf::Value& call) {
  const std::string op = Str(At(call, "op"));
  const auto& args = At(call, "args").list_value();
  if (op == "rrf") {
    return args.values_size() > 0 ? Fusion::Rrf(U32(args.values(0))) : Fusion::Rrf();
  }
  if (op == "dbsf") {
    return Fusion::Dbsf();
  }
  std::vector<float> weights;
  for (const auto& w : args.values()) weights.push_back(static_cast<float>(w.number_value()));
  return Fusion::Weighted(weights);
}

collection_v1::SortKey SortOf(const google::protobuf::Value& call) {
  const std::string op = Str(At(call, "op"));
  const auto& args = At(call, "args").list_value();
  const auto arg = [&](int i) { return i < args.values_size() ? args.values(i).string_value() : std::string(); };
  if (op == "score") return Sort::Score(arg(0));
  if (op == "pk") return Sort::Pk(arg(0));
  return Sort::Field(arg(0), arg(1), arg(2));
}

std::vector<Filter> Filters(const google::protobuf::ListValue& args, int from = 0) {
  std::vector<Filter> out;
  for (int i = from; i < args.values_size(); ++i) out.push_back(FilterOf(args.values(i)));
  return out;
}

Filter FilterOf(const google::protobuf::Value& call) {
  const std::string op = Str(At(call, "op"));
  const auto& args = At(call, "args").list_value();
  const auto arg = [&](int i) -> const google::protobuf::Value& { return args.values(i); };
  if (op == "all") return Filter::All();
  if (op == "none") return Filter::None();
  if (op == "eq") return Filter::Eq(Str(arg(0)), arg(1));
  if (op == "in") {
    std::vector<JsonValue> values(arg(1).list_value().values().begin(), arg(1).list_value().values().end());
    return Filter::In(Str(arg(0)), values);
  }
  if (op == "range") {
    Bounds bounds;
    for (const char* key : {"gt", "gte", "lt", "lte"}) {
      if (Has(arg(1), key)) {
        const JsonValue value = At(arg(1), key);
        if (std::string(key) == "gt") bounds.gt = value;
        if (std::string(key) == "gte") bounds.gte = value;
        if (std::string(key) == "lt") bounds.lt = value;
        if (std::string(key) == "lte") bounds.lte = value;
      }
    }
    return Filter::Range(Str(arg(0)), bounds);
  }
  if (op == "exists") return Filter::Exists(Str(arg(0)));
  if (op == "is_null") return Filter::IsNull(Str(arg(0)));
  if (op == "is_empty") return Filter::IsEmpty(Str(arg(0)));
  if (op == "prefix") return Filter::Prefix(Str(arg(0)), Str(arg(1)));
  if (op == "wildcard") return Filter::Wildcard(Str(arg(0)), Str(arg(1)));
  if (op == "match") return Filter::Match(Str(arg(0)), Str(arg(1)));
  if (op == "phrase") return Filter::Phrase(Str(arg(0)), Str(arg(1)));
  if (op == "multi_match") return Filter::MultiMatch(Strings(arg(0)), Str(arg(1)));
  if (op == "query_string") return Filter::QueryString(Str(arg(0)));
  if (op == "ids") {
    std::vector<DocId> ids;
    for (const auto& id : args.values()) {
      if (id.has_number_value()) {
        ids.emplace_back(static_cast<std::uint64_t>(id.number_value()));
      } else if (id.has_string_value()) {
        ids.emplace_back(id.string_value());
      } else {
        ids.push_back(DocId::Uuid(Str(At(id, "uuid"))));
      }
    }
    return Filter::Ids(ids);
  }
  if (op == "and") return Filter::And(Filters(args));
  if (op == "or") return Filter::Or(Filters(args));
  if (op == "not") return Filter::Not(Filters(args));
  if (op == "filter") return Filter::NonScoring(Filters(args));
  if (op == "any_of") {
    return Filter::AnyOf(Filters(arg(0).list_value()), arg(1).has_number_value()
                                                            ? std::to_string(static_cast<long>(arg(1).number_value()))
                                                            : Str(arg(1)));
  }
  if (op == "boost") return Filter::Boost(FilterOf(arg(0)), static_cast<float>(arg(1).number_value()));
  if (op == "constant_score") return Filter::ConstantScore(FilterOf(arg(0)), static_cast<float>(arg(1).number_value()));
  throw Failed("unknown filter op " + op);
}

void ApplySearch(Search& s, const google::protobuf::Value& call) {
  const std::string op = Str(At(call, "op"));
  const auto& args = At(call, "args").list_value();
  const auto arg = [&](int i) -> const google::protobuf::Value& { return args.values(i); };
  const auto opt = [&](int i) { return i < args.values_size() ? args.values(i) : google::protobuf::Value(); };
  if (op == "text") {
    if (Has(arg(0), "op")) {
      s.Text(FilterOf(arg(0)));
    } else {
      TextRetrieverOptions options;
      options.fields = Strings(At(opt(1), "fields"));
      if (Has(opt(1), "k")) options.k = U32(At(opt(1), "k"));
      s.Text(Str(arg(0)), options);
    }
  } else if (op == "vector") {
    VectorOptions options;
    const auto o = opt(2);
    if (Has(o, "k")) options.k = U32(At(o, "k"));
    if (Has(o, "exact")) options.exact = At(o, "exact").bool_value();
    if (Has(o, "nprobes")) options.nprobes = U32(At(o, "nprobes"));
    if (Has(o, "refine_factor")) options.refine_factor = U32(At(o, "refine_factor"));
    if (Has(o, "ef")) options.ef = U32(At(o, "ef"));
    if (Has(o, "distance")) options.distance = Str(At(o, "distance"));
    s.Vector(Str(arg(0)), Floats(arg(1)), options);
  } else if (op == "fuse") {
    s.Fuse(FusionOf(arg(0)));
  } else if (op == "filter") {
    s.Where(FilterOf(arg(0)));
  } else if (op == "sort") {
    std::vector<collection_v1::SortKey> keys;
    for (const auto& key : args.values()) keys.push_back(SortOf(key));
    s.SortBy(keys);
  } else if (op == "limit") {
    s.Limit(U32(arg(0)));
  } else if (op == "offset") {
    s.Offset(U32(arg(0)));
  } else if (op == "after") {
    std::vector<JsonValue> values(arg(0).list_value().values().begin(), arg(0).list_value().values().end());
    s.After(values);
  } else if (op == "min_score") {
    s.MinScore(static_cast<float>(arg(0).number_value()));
  } else if (op == "select") {
    SelectOptions options;
    options.include = Strings(At(arg(0), "include"));
    options.exclude = Strings(At(arg(0), "exclude"));
    options.no_source = At(arg(0), "no_source").bool_value();
    options.vectors = Strings(At(arg(0), "vectors"));
    options.fields = Strings(At(arg(0), "fields"));
    s.Select(options);
  } else if (op == "highlight") {
    s.Highlight(Str(arg(0)));
  } else if (op == "group_by") {
    s.GroupBy(Str(arg(0)));
  } else if (op == "expand") {
    ExpandOptions options;
    const auto o = opt(1);
    options.from_field = Str(At(o, "from_field"));
    if (Has(o, "hops")) options.hops = U32(At(o, "hops"));
    options.direction = Str(At(o, "direction"));
    s.Expand(Str(arg(0)), options);
  } else if (op == "consistency") {
    s.Consistency(Str(arg(0)));
  } else {
    throw Failed("unknown search op " + op);
  }
}

void ApplySchema(Schema& s, const google::protobuf::Value& call) {
  const std::string op = Str(At(call, "op"));
  const auto& args = At(call, "args").list_value();
  const std::string name = args.values(0).string_value();
  if (op == "text") s.Text(name);
  else if (op == "keyword") s.Keyword(name);
  else if (op == "i64") s.I64(name);
  else if (op == "f64") s.F64(name);
  else if (op == "bool") s.Bool(name);
  else if (op == "timestamp") s.Timestamp(name);
  else if (op == "uuid") s.Uuid(name);
  else if (op == "vector") s.Vector(name, U32(args.values(1)), args.values_size() > 2 ? Str(args.values(2)) : "cosine");
  else if (op == "sparse_vector") s.SparseVector(name);
  else if (op == "dynamic") s.Dynamic(name);
  else throw Failed("unknown schema op " + op);
}

template <typename M>
void CheckSame(const std::string& name, const M& got, const google::protobuf::Value& expect) {
  M want;
  std::string json;
  (void)google::protobuf::util::MessageToJsonString(expect, &json);
  LOAMS_REQUIRE(google::protobuf::util::JsonStringToMessage(json, &want).ok(), name + ": expect does not decode");
  LOAMS_CHECK(MessageDifferencer::Equals(want, got), name + ": got " + ToJson(got) + ", want " + json);
}

void BuildersMatchCorpus() {
  // Inline, so the cases hold where the corpus is absent.
  CheckSame("schema_notes", Schema().Text("title").Keyword("tags").I64("n").Vector("embedding", 4).ToStruct(),
            Parse(R"({"fields":[{"name":"title","source_path":"title","kind":{"text":{"analyzer":"standard","positions":true}},"indexed":true,"fast":false},
              {"name":"tags","source_path":"tags","kind":"keyword","indexed":true,"fast":true},
              {"name":"n","source_path":"n","kind":"i64","indexed":true,"fast":true}],
              "vectors":[{"name":"embedding","dim":4,"distance":"cosine"}],"sparse_vectors":[],"dynamic":"ignore","max_fields":1000})"));
  CheckSame("filter_ids", Filter::Ids({1, "k-str", DocId::Uuid("0190f5c4-6c1e-7b3a-9d2e-4f5a6b7c8d9e")}).Query(),
            Parse(R"({"ids":[{"uint":"1"},"k-str",{"uuid":"0190f5c4-6c1e-7b3a-9d2e-4f5a6b7c8d9e"}]})"));
  Bounds gte2;
  gte2.gte = Json(2);
  CheckSame("filter_bool_nested",
            (Filter::Eq("tags", Json("policy")) & (Filter::Range("n", gte2) | ~Filter::Exists("archived"))).Query(),
            Parse(R"({"bool":{"must":[{"term":{"field":"tags","value":"policy"}},{"bool":{"should":[{"range":{"field":"n","gte":2}},{"bool":{"mustNot":[{"exists":{"field":"archived"}}]}}]}}]}})"));
  CheckSame("search_hybrid_weighted",
            Search::On("notes")
                .Text("refund", {{"body"}, {}})
                .Vector("embedding", {1, 0, 0, 0}, [] {
                  VectorOptions o;
                  o.k = 5;
                  return o;
                }())
                .Fuse(Fusion::Weighted({0.7f, 0.3f}))
                .Limit(3)
                .Build(),
            Parse(R"({"collection":"notes","retrievers":[{"text":{"query":{"match":{"field":"body","text":"refund"}}}},{"vector":{"field":"embedding","query":[1,0,0,0],"k":5}}],"fusion":{"weightedSum":{"weights":[0.7,0.3]}},"limit":3})"));
  LOAMS_CHECK_EQ(ToJson(Fusion::Rrf()), std::string(R"({"rrf":{}})"), "rrf() leaves k to the server");
  LOAMS_CHECK_THROWS(Search::On("n").Text("x").Vector("v", {1}).Fuse(Fusion::Weighted({0.7f})).Build(),
                     InvalidArgumentError, "a weighted fusion needs one weight per retriever");
  LOAMS_CHECK_THROWS(Search::On("n").Consistency("not-a-token"), InvalidArgumentError, "a malformed token");
  const auto wire = Search::Wire(Search::On("n").Text("x").Vector("v", {1}, [] {
                                                   VectorOptions o;
                                                   o.k = 3;
                                                   return o;
                                                 }())
                                     .Limit(25)
                                     .Request());
  LOAMS_CHECK_EQ(wire.retrievers(0).text().k(), 0u, "k goes out only when given");
  LOAMS_CHECK_EQ(wire.retrievers(1).vector().k(), 3u, "and keeps a k that is set");

  const std::string dir = FixturesDir() + "/builders";
  DIR* handle = ::opendir(dir.c_str());
  if (handle == nullptr) {
    std::cout << "no builder corpus in " << dir << "; the inline cases ran\n";
    return;
  }
  std::vector<std::string> files;
  while (const dirent* entry = ::readdir(handle)) {
    const std::string file = entry->d_name;
    if (file.size() > 5 && file.substr(file.size() - 5) == ".json") files.push_back(dir + "/" + file);
  }
  ::closedir(handle);
  for (const std::string& file : files) {
    const google::protobuf::Value c = Parse(ReadFile(file));
    const std::string name = Str(At(c, "name"));
    const std::string builder = Str(At(c, "builder"));
    const auto& spec = At(c, "spec");
    if (builder == "schema") {
      Schema s;
      for (const auto& call : spec.list_value().values()) ApplySchema(s, call);
      CheckSame(name, s.ToStruct(), At(c, "expect"));
    } else if (builder == "filter") {
      CheckSame(name, FilterOf(spec).Query(), At(c, "expect"));
    } else {
      Search s = Search::On(Str(At(spec, "on")));
      for (const auto& call : At(spec, "calls").list_value().values()) ApplySearch(s, call);
      CheckSame(name, s.Build(), At(c, "expect"));
    }
  }
  std::cout << files.size() << " corpus case(s)\n";
}

// --- The handle and QueryAll -------------------------------------------------------------------------

void CollectionHandleDelegates() {
  Harness h([](Options& o) { o.default_namespace = "app"; });
  h.Ok();
  h.Ok();
  h.Ok(R"({"count":"3"})");
  h.Ok();
  h.Ok();
  h.Ok();
  h.Ok(R"({"dropped":true})");
  CollectionHandle notes = h.loams->Collection("notes", "other");
  notes.Upsert({Document::FromJson("a", R"({"x":1})")});
  notes.Get({"a"});
  LOAMS_CHECK_EQ(notes.Count().count(), 3u, "count");
  notes.Search(Search::On("ignored").Text("x", {{"body"}, {}}));
  notes.VectorSearch({1, 0}, "embedding");
  notes.Info();
  LOAMS_CHECK(notes.Drop().dropped(), "drop");
  const std::vector<std::string> want = {"WriteDocuments", "GetDocuments", "CountDocuments", "Search",
                                         "Search",         "GetCollection", "DropCollection"};
  for (std::size_t i = 0; i < want.size(); ++i) {
    LOAMS_CHECK_EQ(h.Method(i), want[i], "the RPC");
    LOAMS_CHECK_EQ(Str(At(h.Sent(i), "namespace")), std::string("other"), "the handle's namespace");
    LOAMS_CHECK_EQ(Str(At(h.Sent(i), "collection")), std::string("notes"), "the handle's collection");
  }
}

void SearchQueryAllRequiresTiebreak() {
  Harness h([](Options& o) { o.default_namespace = "app"; });
  LOAMS_CHECK_THROWS(h.loams->Search()->QueryAll(Search::On("notes").Text("x").SortBy({Sort::Score()})),
                     InvalidArgumentError, "no tiebreak");
  LOAMS_CHECK(h.transport->Requests().empty(), "refused before any network call");
  h.Ok(R"({"hits":[{"pk":{"uint":"1"},"sortValues":[0.9,1]},{"pk":{"uint":"2"},"sortValues":[0.8,2]}]})");
  h.Ok(R"({"hits":[{"pk":{"uint":"3"},"sortValues":[0.7,3]}]})");
  const auto hits = h.loams->Search()->QueryAll(Search::On("notes").Text("x").SortBy({Sort::Score(), Sort::Pk()}).Limit(2));
  LOAMS_CHECK_EQ(hits.size(), static_cast<std::size_t>(3), "three hits over two pages");
  const auto after = At(h.Sent(1), "searchAfter").list_value();
  LOAMS_CHECK_EQ(after.values(1).number_value(), 2.0, "the last hit's sort_values, as answered");
}

void SqlAndStreamsSendTheRecordedBodies() {
  Harness h([](Options& o) { o.default_namespace = "app"; });
  for (int i = 0; i < 2; ++i) h.Ok(R"({"columns":[{"name":"tags"},{"name":"c"}],"rows":[["policy",2]]})");
  for (int i = 0; i < 5; ++i) h.Ok();
  const auto answer = h.loams->Sql()->Query("SELECT tags, count(*) AS c FROM notes WHERE n > $1 GROUP BY tags ORDER BY tags", {Json(1)});
  const auto same = [&](std::size_t i, const char* want, const std::string& what) {
    LOAMS_CHECK(MessageDifferencer::Equals(h.Sent(i), Parse(want)), what + ": sent " + h.transport->Requests()[i].body);
  };
  same(0, R"({"namespace":"app","sql":"SELECT tags, count(*) AS c FROM notes WHERE n > $1 GROUP BY tags ORDER BY tags","params":[1]})", "sql.query");
  const auto records = SqlModule::Records(answer);
  LOAMS_REQUIRE(records.size() == 1, "one row");
  LOAMS_CHECK_EQ(records[0].at("tags").string_value(), std::string("policy"), "records");
  SqlOptions two;
  two.max_rows = 2;
  h.loams->Sql()->Query("SELECT title, n FROM notes ORDER BY n", {}, two);
  same(1, R"({"namespace":"app","sql":"SELECT title, n FROM notes ORDER BY n","maxRows":2})", "max_rows");
  h.loams->Streams()->Create("note-events");
  same(2, R"({"namespace":"app","name":"note-events","partitions":1})", "streams.create");
  ProduceOptions producer;
  producer.producer_id = "app-producer-1";
  producer.producer_seq = 1;
  h.loams->Streams()->Produce("note-events", {{std::string("1"), std::string(R"({"note":1,"op":"created"})"), {}},
                                              {std::string("5"), std::string(R"({"note":5,"op":"deleted"})"), {}}},
                              producer);
  same(3, R"({"namespace":"app","stream":"note-events","records":[{"key":"MQ==","value":"eyJub3RlIjoxLCJvcCI6ImNyZWF0ZWQifQ=="},{"key":"NQ==","value":"eyJub3RlIjo1LCJvcCI6ImRlbGV0ZWQifQ=="}],"producerId":"app-producer-1","producerSeq":"1"})", "streams.produce with keys");
  FetchOptions ten;
  ten.max_records = 10;
  h.loams->Streams()->Fetch("note-events", ten);
  same(4, R"({"namespace":"app","stream":"note-events","maxRecords":10})", "a zero partition and offset are omitted");
  ten.offset = 2;
  h.loams->Streams()->Fetch("note-events", ten);
  same(5, R"({"namespace":"app","stream":"note-events","offset":"2","maxRecords":10})", "streams.fetch at an offset");
  h.loams->Streams()->Get("note-events");
  same(6, R"({"namespace":"app","stream":"note-events"})", "streams.get");
}

void StreamsModuleSubscribe() {
  Harness h([](Options& o) { o.default_namespace = "app"; });
  h.Ok(R"({"records":[{"offset":"1","value":"YQ=="},{"offset":"2","value":"Yg=="}]})");
  h.Ok(R"({"records":[{"offset":"3","value":"Yw=="}]})");
  FetchOptions from;
  from.offset = 1;
  auto sub = h.loams->Streams()->Subscribe("events", from);
  std::vector<std::string> got;
  while (auto record = sub.Next()) {
    got.push_back(record->value());
    if (got.size() == 3) sub.Cancel();
  }
  LOAMS_CHECK(got == std::vector<std::string>({"a", "b", "c"}), "the records in order");
  LOAMS_CHECK_EQ(sub.Offset(), static_cast<std::uint64_t>(4), "the next offset is the last + 1");
  LOAMS_CHECK_EQ(Str(At(h.Sent(1), "offset")), std::string("3"), "the next fetch starts after the last record");
}

void FacadeMatchesIndex() {
  // Every app binding is in the table, and against the facade index when it exists.
  const std::vector<std::string> rpcs = {
      "loams.collection.v1.NamespaceService/CreateNamespace", "loams.collection.v1.CollectionService/CreateCollection",
      "loams.collection.v1.DocumentService/WriteDocuments",   "loams.collection.v1.QueryService/Search",
      "loams.graph.v1.GraphService/Execute",                  "loams.graph.v1.GraphAdminService/CreateGraph"};
  for (const auto& rpc : rpcs) {
    LOAMS_CHECK(BindingFor(rpc).rpc == rpc, rpc + " is bound");
  }
  const std::string path = FixturesDir() + "/facade-index.json";
  if (!Exists(path)) {
    std::cout << "no facade index at " << path << "\n";
    return;
  }
  const google::protobuf::Value index = Parse(ReadFile(path));
  const std::vector<std::string> modules = {"namespaces", "collections", "documents", "search",
                                            "vector",     "graph",       "sql",       "streams"};
  for (const auto& call : At(index, "calls").list_value().values()) {
    const std::string module = Str(At(call, "module"));
    if (std::find(modules.begin(), modules.end(), module) == modules.end() || At(call, "stream").bool_value()) {
      continue;
    }
    const std::string rpc = Str(At(call, "rpc"));
    bool bound = false;
    for (const auto& binding : AllBindings()) {
      if (binding.rpc == rpc) {
        bound = true;
        const std::string retry = Str(At(call, "retry"));
        const bool keyed = !binding.idempotency_field.empty();
        const std::string mine = binding.retry_class == RetryClass::kSafe ? "safe" : keyed ? "keyed" : "never";
        if (!retry.empty() && !(module == "graph" && Str(At(call, "call")) == "query")) {
          LOAMS_CHECK_EQ(mine, retry, module + "." + Str(At(call, "call")) + " retry class");
        }
      }
    }
    LOAMS_CHECK(bound, rpc + " is bound");
  }
}


// --- The recorded app scenario ----------------------------------------------------------------------
//
// `sdks/fixtures/recorded/app/*.json` (`app-scenario.md`), replayed through the
// public facade against the fixture server: every step goes through
// `Loams::Call` with its recorded body, a step that names a builder case is
// first checked against that builder, the answer is compared with the
// recording (its `volatile` fields masked), a refusal by its code, and the
// fixture is added to `results/cpp.json`'s `ran`.

google::protobuf::Value Masked(const google::protobuf::Value& value, const google::protobuf::Value& volatile_keys) {
  google::protobuf::Value out = value;
  if (value.has_struct_value()) {
    auto* fields = out.mutable_struct_value()->mutable_fields();
    for (const auto& key : volatile_keys.list_value().values()) fields->erase(key.string_value());
    for (auto& field : *fields) field.second = Masked(field.second, volatile_keys);
  } else if (value.has_list_value()) {
    for (auto& item : *out.mutable_list_value()->mutable_values()) item = Masked(item, volatile_keys);
  }
  return out;
}

/// Empty when equal; numbers compare to float32 precision.
std::string Compare(const google::protobuf::Value& want, const google::protobuf::Value& got, const std::string& at) {
  if (want.has_struct_value()) {
    if (!got.has_struct_value()) return at + ": not an object";
    std::set<std::string> keys;
    for (const auto& f : want.struct_value().fields()) keys.insert(f.first);
    for (const auto& f : got.struct_value().fields()) keys.insert(f.first);
    for (const auto& key : keys) {
      const std::string problem = Compare(At(want, key.c_str()), At(got, key.c_str()), at + "." + key);
      if (!problem.empty()) return problem;
    }
    return "";
  }
  if (want.has_list_value()) {
    if (!got.has_list_value() || got.list_value().values_size() != want.list_value().values_size()) {
      return at + ": not " + std::to_string(want.list_value().values_size()) + " items";
    }
    for (int i = 0; i < want.list_value().values_size(); ++i) {
      const std::string problem = Compare(want.list_value().values(i), got.list_value().values(i), at + "[" + std::to_string(i) + "]");
      if (!problem.empty()) return problem;
    }
    return "";
  }
  if (want.has_number_value()) {
    const double w = want.number_value();
    const bool ok = got.has_number_value() && std::fabs(w - got.number_value()) <= 1e-6 * std::max(1.0, std::fabs(w));
    return ok ? "" : at + ": " + ToJson(got) + ", recorded " + ToJson(want);
  }
  return MessageDifferencer::Equals(want, got) ? "" : at + ": " + ToJson(got) + ", recorded " + ToJson(want);
}

std::string CheckBuilder(const std::string& name, const google::protobuf::Value& request) {
  const std::string path = FixturesDir() + "/builders/" + name + ".json";
  if (!Exists(path)) return "no builder case " + name;
  const google::protobuf::Value c = Parse(ReadFile(path));
  const std::string builder = Str(At(c, "builder"));
  const auto& spec = At(c, "spec");
  std::string built;
  google::protobuf::Value part;
  if (builder == "schema") {
    Schema s;
    for (const auto& call : spec.list_value().values()) ApplySchema(s, call);
    built = ToJson(s.ToStruct());
    part = At(request, "schema");
  } else if (builder == "filter") {
    built = ToJson(FilterOf(spec).Query());
    part = Has(request, "where") ? At(request, "where") : At(request, "filter");
  } else {
    Search s = Search::On(Str(At(spec, "on")));
    for (const auto& call : At(spec, "calls").list_value().values()) ApplySearch(s, call);
    built = ToJson(s.Build());
    part = request;
    part.mutable_struct_value()->mutable_fields()->erase("namespace");
  }
  const std::string problem = Compare(part, Parse(built), "$");
  return problem.empty() ? "" : "the " + name + " builder builds " + built + ": " + problem;
}

void AddRan(const std::string& fixture) {
  const std::string path = FixturesDir() + "/results/cpp.json";
  google::protobuf::Value report = Exists(path) ? Parse(ReadFile(path)) : Parse(R"({"transport":"connect","ran":[]})");
  auto* fields = report.mutable_struct_value()->mutable_fields();
  std::set<std::string> ran;
  for (const auto& name : (*fields)["ran"].list_value().values()) ran.insert(name.string_value());
  ran.insert(fixture);
  google::protobuf::Value list;
  list.mutable_list_value();
  for (const auto& name : ran) *list.mutable_list_value()->add_values() = Json(name);
  (*fields)["ran"] = list;
  ::mkdir((FixturesDir() + "/results").c_str(), 0755);
  std::ofstream(path, std::ios::trunc) << ToJson(report) << "\n";
}

void Replay(const std::string& name) {
  const std::string file = FixturesDir() + "/recorded/app/" + name + ".json";
  if (!Exists(file)) {
    std::cout << "no recording for " << name << "\n";
    return;
  }
  const google::protobuf::Value fixture = Parse(ReadFile(file));
  const auto& volatile_keys = At(fixture, "volatile");
  Endpoint endpoint;
  Options options;
  options.endpoint = endpoint.Url();
  options.content_type = ContentType::kJson;
  options.max_retries = 0;
  std::shared_ptr<Loams> loams = MakeLoams(std::move(options));
  const auto& steps = At(fixture, "steps").list_value();
  for (int index = 0; index < steps.values_size(); ++index) {
    const auto& step = steps.values(index);
    const std::string label = name + " step " + std::to_string(index) + " (" + Str(At(step, "call")) + ")";
    const std::string body = Str(At(At(step, "request"), "body"));
    const google::protobuf::Value request = Parse(body.empty() ? "{}" : body);
    if (Has(step, "builder")) {
      const std::string problem = CheckBuilder(Str(At(step, "builder")), request);
      if (!problem.empty()) Record(__FILE__, __LINE__, label + ": " + problem);
    }
    const auto& expected = At(step, "response");
    const bool answered = At(expected, "status").number_value() == 200;
    CallOptions call;
    call.headers = {{"loams-fixture-name", name}, {"loams-fixture-step", std::to_string(index)}};
    try {
      const std::string answer = loams->Call(Str(At(step, "call")), body, call);
      if (!answered) {
        Record(__FILE__, __LINE__, label + ": answered, but the recording is a refusal");
        continue;
      }
      const std::string problem = Compare(Masked(At(expected, "body"), volatile_keys), Masked(Parse(answer), volatile_keys), "$");
      if (!problem.empty()) Record(__FILE__, __LINE__, label + ": " + problem);
    } catch (const LoamsError& error) {
      const std::string want = Str(At(At(expected, "body"), "code"));
      if (answered) {
        Record(__FILE__, __LINE__, label + ": refused (" + std::string(ToString(error.CodeValue())) + ") where the recording answered: " + error.what());
      } else if (std::string(ToString(error.CodeValue())) != want) {
        Record(__FILE__, __LINE__, label + ": refused with " + std::string(ToString(error.CodeValue())) + ", the recording with " + want);
      }
    }
  }
  if (Failures() == 0) AddRan(name);
}

}  // namespace

int main(int argc, char** argv) {
  // The full names, as `check-languages.mjs` looks for them in this file.
  const std::map<std::string, std::function<void()>> tests = {
      {"cpp_consistency_token_merge", ConsistencyTokenMerge},
      {"cpp_session_read_your_writes", SessionReadYourWrites},
      {"cpp_default_namespace", DefaultNamespace},
      {"cpp_timeout_deadline", TimeoutDeadline},
      {"cpp_write_batch_chunks", WriteBatchChunks},
      {"cpp_streams_subscribe_resumes", StreamsSubscribeResumes},
      {"cpp_builders_match_corpus", BuildersMatchCorpus},
      {"cpp_collection_handle_delegates", CollectionHandleDelegates},
      {"cpp_search_query_all_requires_tiebreak", SearchQueryAllRequiresTiebreak},
      {"cpp_facade_matches_index", FacadeMatchesIndex},
      {"cpp_sql_and_streams_send_the_recorded_bodies", SqlAndStreamsSendTheRecordedBodies},
      {"cpp_streams_module_subscribe", StreamsModuleSubscribe},
      {"cpp_app_collections", [] { Replay("app_collections"); }},
      {"cpp_app_search", [] { Replay("app_search"); }},
      {"cpp_app_graph", [] { Replay("app_graph"); }},
      {"cpp_app_sql", [] { Replay("app_sql"); }},
      {"cpp_app_streams", [] { Replay("app_streams"); }},
  };

  std::vector<std::string> run;
  for (int i = 1; i < argc; ++i) run.emplace_back(argv[i]);
  if (run.empty()) {
    for (const auto& test : tests) run.push_back(test.first);
  }
  for (const std::string& name : run) {
    const auto found = tests.find(name);
    if (found == tests.end()) {
      std::cerr << "no test " << name << "\n";
      return 2;
    }
    try {
      found->second();
    } catch (const std::exception& error) {
      Record(__FILE__, __LINE__, name + " threw: " + error.what());
    }
  }
  return Finish("cpp_app");
}
