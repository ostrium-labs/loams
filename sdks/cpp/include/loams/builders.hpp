// The typed builders of the app-building API (SDKP §2.4 and §2.6): `Schema`,
// `Filter` and `Search`, plus `Fusion` and `Sort`.
//
// Each builds the message it names directly: `Filter` is a
// `loams.collection.v1.Query`, `Search` a `SearchRequest`, `Schema` the
// `google.protobuf.Struct` `CreateCollectionRequest.schema` carries. The builder
// corpus (`sdks/fixtures/builders`) pins their proto3 JSON, and
// `cpp_builders_match_corpus` checks every case.
//
// Nothing here talks to a server. `Search::Build` checks the one thing the SDK
// checks client-side (a weighted fusion's weights); `Search::Wire` is what the
// app modules send: `Build`, plus the `k` the server requires on a retriever.

#ifndef LOAMS_BUILDERS_HPP
#define LOAMS_BUILDERS_HPP

#include <cstdint>
#include <initializer_list>
#include <optional>
#include <string>
#include <vector>

#include <google/protobuf/struct.pb.h>

#include "loams/collection/v1/query.pb.h"
#include "loams/consistency.hpp"

namespace loams {

namespace collection_v1 = ::loams::collection::v1;

// --- JSON values -----------------------------------------------------------------------------------

/// A JSON value (`google.protobuf.Value`): what a term, a range bound or a
/// `search_after` entry is. Built from the C++ value it spells.
using JsonValue = google::protobuf::Value;

JsonValue Json(const char* value);
JsonValue Json(const std::string& value);
JsonValue Json(double value);
JsonValue Json(int value);
JsonValue Json(std::int64_t value);
JsonValue Json(bool value);
JsonValue JsonNull();
JsonValue JsonList(std::initializer_list<JsonValue> values);

/// Parses proto3 JSON into a `Struct` (a document's `source`). Throws
/// `InvalidArgumentError` when it is not a JSON object.
google::protobuf::Struct StructFromJson(const std::string& json);

/// Parses proto3 JSON into a `Value`. Throws `InvalidArgumentError`.
JsonValue ValueFromJson(const std::string& json);

/// A message as proto3 JSON.
std::string ToJson(const google::protobuf::Message& message);

// --- Document ids ----------------------------------------------------------------------------------

/// A primary key: an unsigned integer, a string, or a UUID.
class DocId {
 public:
  /// An empty string id.
  DocId() : kind_(Kind::kString) {}
  DocId(std::uint64_t value) : kind_(Kind::kUint), uint_(value) {}  // NOLINT(runtime/explicit)
  DocId(int value);                                                 // NOLINT(runtime/explicit)
  DocId(const char* value) : kind_(Kind::kString), text_(value) {}  // NOLINT(runtime/explicit)
  DocId(std::string value) : kind_(Kind::kString), text_(std::move(value)) {}  // NOLINT(runtime/explicit)
  static DocId Uuid(std::string value);
  /// A `DocumentId` message back as a `DocId`.
  static DocId From(const collection_v1::DocumentId& id);

  collection_v1::DocumentId ToMessage() const;
  /// How `Query.ids` spells it: a uint is `{"uint": "…"}`, a string stays a
  /// string, a UUID is `{"uuid": "…"}`.
  JsonValue ToQueryValue() const;
  /// The key's text: the decimal of a uint, the string, or the UUID.
  std::string Text() const;

  bool IsUint() const { return kind_ == Kind::kUint; }
  std::uint64_t Uint() const { return uint_; }
  friend bool operator==(const DocId& a, const DocId& b) {
    return a.kind_ == b.kind_ && a.uint_ == b.uint_ && a.text_ == b.text_;
  }

 private:
  enum class Kind { kUint, kString, kUuid };
  DocId(Kind kind, std::string text) : kind_(kind), text_(std::move(text)) {}
  Kind kind_;
  std::uint64_t uint_ = 0;
  std::string text_;
};

// --- Schema ----------------------------------------------------------------------------------------

/// A field's overrides; unset keeps the kind's default.
struct FieldOptions {
  std::optional<bool> indexed;
  std::optional<bool> fast;
  std::string source_path;
};

struct TextOptions {
  std::string analyzer = "standard";
  bool positions = true;
  std::optional<bool> indexed;
  std::optional<bool> fast;
  std::string source_path;
};

/// The `CreateCollectionRequest.schema` Struct, in the shape the server's schema
/// parser reads:
///
///     loams::Schema().Keyword("title").Text("body").I64("n").Vector("embedding", 4)
class Schema {
 public:
  Schema& Text(const std::string& name, const TextOptions& options = {});
  Schema& Keyword(const std::string& name, const FieldOptions& options = {});
  Schema& I64(const std::string& name, const FieldOptions& options = {});
  Schema& F64(const std::string& name, const FieldOptions& options = {});
  Schema& Bool(const std::string& name, const FieldOptions& options = {});
  /// A date-time field (the schema's `date` kind).
  Schema& Timestamp(const std::string& name, const FieldOptions& options = {});
  Schema& Uuid(const std::string& name, const FieldOptions& options = {});
  Schema& Vector(const std::string& name, std::uint32_t dim, const std::string& distance = "cosine");
  Schema& SparseVector(const std::string& name, const std::string& modifier = "");
  /// `ignore` (the builder's default), `strict` (the server's) or `map`.
  Schema& Dynamic(const std::string& mode);
  Schema& MaxFields(std::uint32_t count);

  google::protobuf::Struct ToStruct() const;

 private:
  Schema& Field(const std::string& name, const JsonValue& kind, bool indexed, bool fast,
                const std::string& source_path);
  std::vector<JsonValue> fields_;
  std::vector<JsonValue> vectors_;
  std::vector<JsonValue> sparse_;
  std::string dynamic_ = "ignore";
  std::uint32_t max_fields_ = 1000;
};

// --- Filter ----------------------------------------------------------------------------------------

/// A range's bounds; unset bounds are absent.
struct Bounds {
  std::optional<JsonValue> gt;
  std::optional<JsonValue> gte;
  std::optional<JsonValue> lt;
  std::optional<JsonValue> lte;
};

struct MatchOptions {
  std::string op;  // `or` (the default) or `and`
  std::string minimum_should_match;
  std::string fuzziness;  // `auto`, or a number of edits
  std::string analyzer;
};

/// One `loams.collection.v1.Query`:
///
///     Filter::Eq("tags", Json("policy")) & Filter::Exists("title")
class Filter {
 public:
  Filter() = default;
  explicit Filter(collection_v1::Query query) : query_(std::move(query)) {}

  const collection_v1::Query& Query() const { return query_; }
  operator const collection_v1::Query&() const { return query_; }  // NOLINT(runtime/explicit)

  static Filter All();
  static Filter None();
  /// `field` equals `value` (a term query).
  static Filter Eq(const std::string& field, const JsonValue& value);
  /// `field` is one of `values` (a terms query).
  static Filter In(const std::string& field, const std::vector<JsonValue>& values);
  static Filter Range(const std::string& field, const Bounds& bounds);
  static Filter Exists(const std::string& field);
  static Filter IsNull(const std::string& field);
  static Filter IsEmpty(const std::string& field);
  static Filter Prefix(const std::string& field, const std::string& value);
  static Filter Wildcard(const std::string& field, const std::string& pattern);
  /// `edits` is `auto` or a number; empty leaves the server's default.
  static Filter Fuzzy(const std::string& field, const std::string& value, const std::string& edits = "");
  static Filter Match(const std::string& field, const std::string& text, const MatchOptions& options = {});
  static Filter Phrase(const std::string& field, const std::string& text, std::optional<std::uint32_t> slop = {});
  /// Fields are names or `name^boost`.
  static Filter MultiMatch(const std::vector<std::string>& fields, const std::string& text);
  static Filter QueryString(const std::string& query, const std::vector<std::string>& fields = {});
  static Filter Ids(const std::vector<DocId>& ids);
  /// Every clause matches (`bool.must`).
  static Filter And(const std::vector<Filter>& clauses);
  /// Any clause matches (`bool.should`).
  static Filter Or(const std::vector<Filter>& clauses);
  /// At least `minimum_should_match` clauses match.
  static Filter AnyOf(const std::vector<Filter>& clauses, const std::string& minimum_should_match);
  /// No clause matches (`bool.must_not`).
  static Filter Not(const std::vector<Filter>& clauses);
  /// Every clause matches, without scoring (`bool.filter`).
  static Filter NonScoring(const std::vector<Filter>& clauses);
  static Filter Boost(const Filter& clause, float boost);
  static Filter ConstantScore(const Filter& clause, float score);

  friend Filter operator&(const Filter& a, const Filter& b) { return And({a, b}); }
  friend Filter operator|(const Filter& a, const Filter& b) { return Or({a, b}); }
  friend Filter operator~(const Filter& a) { return Not({a}); }

 private:
  collection_v1::Query query_;
};

// --- Fusion and sort keys -------------------------------------------------------------------------

/// How a hybrid search fuses its retrievers' rankings.
struct Fusion {
  /// Reciprocal rank fusion; no `k` leaves the server's default (60).
  static collection_v1::Fusion Rrf(std::optional<std::uint32_t> k = {});
  /// Distribution-based score fusion.
  static collection_v1::Fusion Dbsf();
  /// A weighted sum, one weight per retriever.
  static collection_v1::Fusion Weighted(const std::vector<float>& weights);
};

/// One sort key. `order` is `asc`, `desc` or empty (the key's default).
struct Sort {
  static collection_v1::SortKey Score(const std::string& order = "");
  /// The primary key: the tiebreaker `QueryAll` requires.
  static collection_v1::SortKey Pk(const std::string& order = "");
  /// `missing` is `first`, `last` or empty.
  static collection_v1::SortKey Field(const std::string& name, const std::string& order = "",
                                      const std::string& missing = "");
};

// --- Search ----------------------------------------------------------------------------------------

struct TextRetrieverOptions {
  /// One field is a match, several a multi-match, none a query string.
  std::vector<std::string> fields;
  std::optional<std::uint32_t> k;
};

struct VectorOptions {
  std::optional<std::uint32_t> k;
  std::optional<bool> exact;
  std::optional<std::uint32_t> nprobes;
  std::optional<std::uint32_t> refine_factor;
  std::optional<std::uint32_t> ef;
  std::optional<float> oversampling;
  std::string distance;
  std::optional<Filter> filter;
};

struct SelectOptions {
  std::vector<std::string> include;
  std::vector<std::string> exclude;
  bool no_source = false;
  std::vector<std::string> vectors;
  std::vector<std::string> fields;
};

struct HighlightOptions {
  std::optional<std::string> pre;
  std::optional<std::string> post;
  std::optional<std::uint32_t> fragment_size;
  std::optional<std::uint32_t> fragments;
};

struct ExpandOptions {
  std::string from_field;
  std::string seed_label;
  std::optional<std::uint32_t> seeds;
  std::optional<std::uint32_t> hops;
  std::string direction;  // `out`, `in` or `both`
  std::vector<std::string> edge_types;
  std::optional<std::uint32_t> limit_per_seed;
  std::optional<std::uint32_t> limit;
  std::string output;  // `nested` or `flat`
};

/// A `SearchRequest`, verb by verb:
///
///     loams::Search::On("notes")
///         .Text("refund", {{"body"}})
///         .Vector("embedding", {1, 0, 0, 0})
///         .Fuse(loams::Fusion::Rrf(60))
///         .Where(loams::Filter::Eq("tags", loams::Json("policy")))
///         .Limit(2)
class Search {
 public:
  /// The page size `QueryAll` uses when the request sets no `limit`.
  static constexpr std::uint32_t kDefaultK = 10;

  static Search On(const std::string& collection, const std::string& ns = "");
  Search() = default;
  explicit Search(collection_v1::SearchRequest request) : request_(std::move(request)) {}

  Search& Namespace(const std::string& ns);
  Search& Collection(const std::string& collection);
  /// A text retriever over a string: see `TextRetrieverOptions`.
  Search& Text(const std::string& text, const TextRetrieverOptions& options = {});
  /// A text retriever over any query.
  Search& Text(const ::loams::Filter& query, std::optional<std::uint32_t> k = {});
  Search& Vector(const std::string& field, const std::vector<float>& values, const VectorOptions& options = {});
  Search& Sparse(const std::string& field, const std::vector<std::uint32_t>& indices,
                 const std::vector<float>& values, std::optional<std::uint32_t> k = {});
  Search& Fuse(const collection_v1::Fusion& fusion);
  /// The request's filter (`filter`).
  Search& Where(const ::loams::Filter& filter);
  Search& SortBy(const std::vector<collection_v1::SortKey>& keys);
  Search& Limit(std::uint32_t limit);
  Search& Offset(std::uint32_t offset);
  /// Continue after a hit: its `sort_values`.
  Search& After(const std::vector<JsonValue>& sort_values);
  Search& MinScore(float score);
  Search& Select(const SelectOptions& options);
  Search& Highlight(const std::string& field, const HighlightOptions& options = {});
  Search& GroupBy(const std::string& field, std::optional<std::uint32_t> size = {},
                  std::optional<std::uint32_t> limit = {});
  /// Graph expansion of the hits (`expand_spec`).
  Search& Expand(const std::string& graph, const ExpandOptions& options = {});
  /// `strong`, `eventual`, or a token (checked here) as `at_least`.
  Search& Consistency(const std::string& level);

  /// Whether the last sort key is the primary key, which `QueryAll` needs.
  bool HasTiebreak() const;
  /// The request exactly as built: what the builder corpus pins.
  const collection_v1::SearchRequest& Request() const { return request_; }
  /// `Request()`, checked: a weighted fusion needs one weight per retriever.
  collection_v1::SearchRequest Build() const;

  /// `Build()` of `request`: what the app modules send. A retriever's `k` goes
  /// out only when the caller gave one; the engine fills the default.
  static collection_v1::SearchRequest Wire(const collection_v1::SearchRequest& request);
  static void Check(const collection_v1::SearchRequest& request);
  static bool HasTiebreak(const collection_v1::SearchRequest& request);

 private:
  collection_v1::SearchRequest request_;
};

/// `strong`, `eventual` or a token as a `loams.collection.v1.Consistency`.
/// Throws `InvalidArgumentError` for a malformed token.
collection_v1::Consistency ConsistencyOf(const std::string& level);

}  // namespace loams

#endif  // LOAMS_BUILDERS_HPP
