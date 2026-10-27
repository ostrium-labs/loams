// The typed builders, as `builders.hpp` documents them.

#include "loams/builders.hpp"

#include <algorithm>
#include <cstdlib>

#include <google/protobuf/util/json_util.h>

#include "loams/error.hpp"

namespace loams {
namespace {

[[noreturn]] void Invalid(const std::string& message, std::map<std::string, std::string> metadata = {}) {
  ThrowMapped(Code::kInvalidArgument, "", Reason::kUnknown, "", std::move(metadata), "", "loams: " + message, "");
}

JsonValue Object(std::initializer_list<std::pair<const char*, JsonValue>> fields) {
  JsonValue out;
  auto* object = out.mutable_struct_value()->mutable_fields();
  for (const auto& field : fields) {
    (*object)[field.first] = field.second;
  }
  return out;
}

collection_v1::SortOrder Order(const std::string& order) {
  if (order.empty()) {
    return collection_v1::SORT_ORDER_UNSPECIFIED;
  }
  if (order == "asc") {
    return collection_v1::SORT_ORDER_ASC;
  }
  if (order == "desc") {
    return collection_v1::SORT_ORDER_DESC;
  }
  Invalid("a sort order is asc or desc, not \"" + order + "\"");
}

void SetFuzziness(collection_v1::Fuzziness* out, const std::string& edits) {
  if (edits == "auto") {
    out->mutable_auto_();
  } else {
    out->set_edits(static_cast<std::uint32_t>(std::strtoul(edits.c_str(), nullptr, 10)));
  }
}

collection_v1::MultiMatchField Boosted(const std::string& spec) {
  collection_v1::MultiMatchField out;
  const std::size_t at = spec.rfind('^');
  if (at != std::string::npos && at > 0) {
    char* end = nullptr;
    const std::string tail = spec.substr(at + 1);
    const float boost = std::strtof(tail.c_str(), &end);
    if (!tail.empty() && end != nullptr && *end == '\0') {
      out.set_field(spec.substr(0, at));
      out.set_boost(boost);
      return out;
    }
  }
  out.set_field(spec);
  return out;
}

collection_v1::Retriever* AddRetriever(collection_v1::SearchRequest* request) { return request->add_retrievers(); }

}  // namespace

// --- JSON values -----------------------------------------------------------------------------------

JsonValue Json(const char* value) { return Json(std::string(value)); }

JsonValue Json(const std::string& value) {
  JsonValue out;
  out.set_string_value(value);
  return out;
}

JsonValue Json(double value) {
  JsonValue out;
  out.set_number_value(value);
  return out;
}

JsonValue Json(int value) { return Json(static_cast<double>(value)); }

JsonValue Json(std::int64_t value) { return Json(static_cast<double>(value)); }

JsonValue Json(bool value) {
  JsonValue out;
  out.set_bool_value(value);
  return out;
}

JsonValue JsonNull() {
  JsonValue out;
  out.set_null_value(google::protobuf::NULL_VALUE);
  return out;
}

JsonValue JsonList(std::initializer_list<JsonValue> values) {
  JsonValue out;
  for (const JsonValue& value : values) {
    *out.mutable_list_value()->add_values() = value;
  }
  return out;
}

google::protobuf::Struct StructFromJson(const std::string& json) {
  google::protobuf::Struct out;
  if (!google::protobuf::util::JsonStringToMessage(json, &out).ok()) {
    Invalid("not a JSON object: " + json);
  }
  return out;
}

JsonValue ValueFromJson(const std::string& json) {
  JsonValue out;
  if (!google::protobuf::util::JsonStringToMessage(json, &out).ok()) {
    Invalid("not JSON: " + json);
  }
  return out;
}

std::string ToJson(const google::protobuf::Message& message) {
  std::string out;
  if (!google::protobuf::util::MessageToJsonString(message, &out).ok()) {
    ThrowInternal("", "loams: a message would not encode as proto3 JSON");
  }
  return out;
}

// --- Document ids ----------------------------------------------------------------------------------

DocId::DocId(int value) : kind_(Kind::kUint) {
  if (value < 0) {
    Invalid("a numeric document id is non-negative");
  }
  uint_ = static_cast<std::uint64_t>(value);
}

DocId DocId::Uuid(std::string value) { return DocId(Kind::kUuid, std::move(value)); }

DocId DocId::From(const collection_v1::DocumentId& id) {
  switch (id.id_case()) {
    case collection_v1::DocumentId::kUint:
      return DocId(static_cast<std::uint64_t>(id.uint()));
    case collection_v1::DocumentId::kUuid:
      return Uuid(id.uuid());
    default:
      return DocId(id.string());
  }
}

collection_v1::DocumentId DocId::ToMessage() const {
  collection_v1::DocumentId out;
  switch (kind_) {
    case Kind::kUint:
      out.set_uint(uint_);
      break;
    case Kind::kString:
      out.set_string(text_);
      break;
    case Kind::kUuid:
      out.set_uuid(text_);
      break;
  }
  return out;
}

JsonValue DocId::ToQueryValue() const {
  switch (kind_) {
    case Kind::kUint:
      return Object({{"uint", Json(std::to_string(uint_))}});
    case Kind::kUuid:
      return Object({{"uuid", Json(text_)}});
    case Kind::kString:
      break;
  }
  return Json(text_);
}

std::string DocId::Text() const { return kind_ == Kind::kUint ? std::to_string(uint_) : text_; }

// --- Schema ----------------------------------------------------------------------------------------

Schema& Schema::Field(const std::string& name, const JsonValue& kind, bool indexed, bool fast,
                      const std::string& source_path) {
  fields_.push_back(Object({{"name", Json(name)},
                            {"source_path", Json(source_path.empty() ? name : source_path)},
                            {"kind", kind},
                            {"indexed", Json(indexed)},
                            {"fast", Json(fast)}}));
  return *this;
}

Schema& Schema::Text(const std::string& name, const TextOptions& options) {
  const JsonValue kind =
      Object({{"text", Object({{"analyzer", Json(options.analyzer)}, {"positions", Json(options.positions)}})}});
  return Field(name, kind, options.indexed.value_or(true), options.fast.value_or(false), options.source_path);
}

Schema& Schema::Keyword(const std::string& name, const FieldOptions& options) {
  return Field(name, Json("keyword"), options.indexed.value_or(true), options.fast.value_or(true), options.source_path);
}

Schema& Schema::I64(const std::string& name, const FieldOptions& options) {
  return Field(name, Json("i64"), options.indexed.value_or(true), options.fast.value_or(true), options.source_path);
}

Schema& Schema::F64(const std::string& name, const FieldOptions& options) {
  return Field(name, Json("f64"), options.indexed.value_or(true), options.fast.value_or(true), options.source_path);
}

Schema& Schema::Bool(const std::string& name, const FieldOptions& options) {
  return Field(name, Json("bool"), options.indexed.value_or(true), options.fast.value_or(true), options.source_path);
}

Schema& Schema::Timestamp(const std::string& name, const FieldOptions& options) {
  return Field(name, Json("date"), options.indexed.value_or(true), options.fast.value_or(true), options.source_path);
}

Schema& Schema::Uuid(const std::string& name, const FieldOptions& options) {
  return Field(name, Json("uuid"), options.indexed.value_or(true), options.fast.value_or(true), options.source_path);
}

Schema& Schema::Vector(const std::string& name, std::uint32_t dim, const std::string& distance) {
  if (dim == 0) {
    Invalid("a vector's dim must be positive");
  }
  vectors_.push_back(
      Object({{"name", Json(name)}, {"dim", Json(static_cast<double>(dim))}, {"distance", Json(distance)}}));
  return *this;
}

Schema& Schema::SparseVector(const std::string& name, const std::string& modifier) {
  sparse_.push_back(modifier.empty() ? Object({{"name", Json(name)}})
                                     : Object({{"name", Json(name)}, {"modifier", Json(modifier)}}));
  return *this;
}

Schema& Schema::Dynamic(const std::string& mode) {
  if (mode != "ignore" && mode != "strict" && mode != "map") {
    Invalid("dynamic is ignore, strict or map, not \"" + mode + "\"");
  }
  dynamic_ = mode;
  return *this;
}

Schema& Schema::MaxFields(std::uint32_t count) {
  max_fields_ = count;
  return *this;
}

google::protobuf::Struct Schema::ToStruct() const {
  google::protobuf::Struct out;
  auto& fields = *out.mutable_fields();
  const auto list = [](const std::vector<JsonValue>& items) {
    JsonValue value;
    value.mutable_list_value();
    for (const JsonValue& item : items) {
      *value.mutable_list_value()->add_values() = item;
    }
    return value;
  };
  fields["fields"] = list(fields_);
  fields["vectors"] = list(vectors_);
  fields["sparse_vectors"] = list(sparse_);
  fields["dynamic"] = Json(dynamic_);
  fields["max_fields"] = Json(static_cast<double>(max_fields_));
  return out;
}

// --- Filter ----------------------------------------------------------------------------------------

Filter Filter::All() {
  collection_v1::Query q;
  q.mutable_match_all();
  return Filter(q);
}

Filter Filter::None() {
  collection_v1::Query q;
  q.mutable_match_none();
  return Filter(q);
}

Filter Filter::Eq(const std::string& field, const JsonValue& value) {
  collection_v1::Query q;
  q.mutable_term()->set_field(field);
  *q.mutable_term()->mutable_value() = value;
  return Filter(q);
}

Filter Filter::In(const std::string& field, const std::vector<JsonValue>& values) {
  collection_v1::Query q;
  q.mutable_terms()->set_field(field);
  for (const JsonValue& value : values) {
    *q.mutable_terms()->add_values() = value;
  }
  return Filter(q);
}

Filter Filter::Range(const std::string& field, const Bounds& bounds) {
  collection_v1::Query q;
  auto* range = q.mutable_range();
  range->set_field(field);
  if (bounds.gt) *range->mutable_gt() = *bounds.gt;
  if (bounds.gte) *range->mutable_gte() = *bounds.gte;
  if (bounds.lt) *range->mutable_lt() = *bounds.lt;
  if (bounds.lte) *range->mutable_lte() = *bounds.lte;
  return Filter(q);
}

Filter Filter::Exists(const std::string& field) {
  collection_v1::Query q;
  q.mutable_exists()->set_field(field);
  return Filter(q);
}

Filter Filter::IsNull(const std::string& field) {
  collection_v1::Query q;
  q.mutable_is_null()->set_field(field);
  return Filter(q);
}

Filter Filter::IsEmpty(const std::string& field) {
  collection_v1::Query q;
  q.mutable_is_empty()->set_field(field);
  return Filter(q);
}

Filter Filter::Prefix(const std::string& field, const std::string& value) {
  collection_v1::Query q;
  q.mutable_prefix()->set_field(field);
  q.mutable_prefix()->set_value(value);
  return Filter(q);
}

Filter Filter::Wildcard(const std::string& field, const std::string& pattern) {
  collection_v1::Query q;
  q.mutable_wildcard()->set_field(field);
  q.mutable_wildcard()->set_pattern(pattern);
  return Filter(q);
}

Filter Filter::Fuzzy(const std::string& field, const std::string& value, const std::string& edits) {
  collection_v1::Query q;
  q.mutable_fuzzy()->set_field(field);
  q.mutable_fuzzy()->set_value(value);
  if (!edits.empty()) {
    SetFuzziness(q.mutable_fuzzy()->mutable_fuzziness(), edits);
  }
  return Filter(q);
}

Filter Filter::Match(const std::string& field, const std::string& text, const MatchOptions& options) {
  collection_v1::Query q;
  auto* match = q.mutable_match();
  match->set_field(field);
  match->set_text(text);
  match->set_operator_(options.op);
  if (!options.minimum_should_match.empty()) {
    match->set_minimum_should_match(options.minimum_should_match);
  }
  if (!options.fuzziness.empty()) {
    SetFuzziness(match->mutable_fuzziness(), options.fuzziness);
  }
  match->set_analyzer(options.analyzer);
  return Filter(q);
}

Filter Filter::Phrase(const std::string& field, const std::string& text, std::optional<std::uint32_t> slop) {
  collection_v1::Query q;
  q.mutable_match_phrase()->set_field(field);
  q.mutable_match_phrase()->set_text(text);
  q.mutable_match_phrase()->set_slop(slop.value_or(0));
  return Filter(q);
}

Filter Filter::MultiMatch(const std::vector<std::string>& fields, const std::string& text) {
  collection_v1::Query q;
  for (const std::string& field : fields) {
    *q.mutable_multi_match()->add_fields() = Boosted(field);
  }
  q.mutable_multi_match()->set_text(text);
  return Filter(q);
}

Filter Filter::QueryString(const std::string& query, const std::vector<std::string>& fields) {
  collection_v1::Query q;
  q.mutable_query_string()->set_query(query);
  for (const std::string& field : fields) {
    q.mutable_query_string()->add_default_fields(field);
  }
  return Filter(q);
}

Filter Filter::Ids(const std::vector<DocId>& ids) {
  collection_v1::Query q;
  for (const DocId& id : ids) {
    *q.add_ids() = id.ToQueryValue();
  }
  return Filter(q);
}

Filter Filter::And(const std::vector<Filter>& clauses) {
  collection_v1::Query q;
  for (const Filter& clause : clauses) {
    *q.mutable_bool_()->add_must() = clause.query_;
  }
  return Filter(q);
}

Filter Filter::Or(const std::vector<Filter>& clauses) {
  collection_v1::Query q;
  for (const Filter& clause : clauses) {
    *q.mutable_bool_()->add_should() = clause.query_;
  }
  return Filter(q);
}

Filter Filter::AnyOf(const std::vector<Filter>& clauses, const std::string& minimum_should_match) {
  Filter out = Or(clauses);
  out.query_.mutable_bool_()->set_minimum_should_match(minimum_should_match);
  return out;
}

Filter Filter::Not(const std::vector<Filter>& clauses) {
  collection_v1::Query q;
  for (const Filter& clause : clauses) {
    *q.mutable_bool_()->add_must_not() = clause.query_;
  }
  return Filter(q);
}

Filter Filter::NonScoring(const std::vector<Filter>& clauses) {
  collection_v1::Query q;
  for (const Filter& clause : clauses) {
    *q.mutable_bool_()->add_filter() = clause.query_;
  }
  return Filter(q);
}

Filter Filter::Boost(const Filter& clause, float boost) {
  collection_v1::Query q;
  *q.mutable_boost()->mutable_query() = clause.query_;
  q.mutable_boost()->set_boost(boost);
  return Filter(q);
}

Filter Filter::ConstantScore(const Filter& clause, float score) {
  collection_v1::Query q;
  *q.mutable_constant_score()->mutable_query() = clause.query_;
  q.mutable_constant_score()->set_score(score);
  return Filter(q);
}

// --- Fusion and sort keys -------------------------------------------------------------------------

collection_v1::Fusion Fusion::Rrf(std::optional<std::uint32_t> k) {
  collection_v1::Fusion out;
  out.mutable_rrf();
  if (k) {
    out.mutable_rrf()->set_k(*k);
  }
  return out;
}

collection_v1::Fusion Fusion::Dbsf() {
  collection_v1::Fusion out;
  out.mutable_dbsf();
  return out;
}

collection_v1::Fusion Fusion::Weighted(const std::vector<float>& weights) {
  collection_v1::Fusion out;
  for (const float weight : weights) {
    out.mutable_weighted_sum()->add_weights(weight);
  }
  out.mutable_weighted_sum();
  return out;
}

collection_v1::SortKey Sort::Score(const std::string& order) {
  collection_v1::SortKey out;
  out.mutable_score()->set_order(Order(order));
  return out;
}

collection_v1::SortKey Sort::Pk(const std::string& order) {
  collection_v1::SortKey out;
  out.mutable_pk()->set_order(Order(order));
  return out;
}

collection_v1::SortKey Sort::Field(const std::string& name, const std::string& order, const std::string& missing) {
  collection_v1::SortKey out;
  out.mutable_field()->set_field(name);
  out.mutable_field()->set_order(Order(order));
  if (missing == "first") {
    out.mutable_field()->set_missing(collection_v1::MISSING_ORDER_FIRST);
  } else if (missing == "last") {
    out.mutable_field()->set_missing(collection_v1::MISSING_ORDER_LAST);
  } else if (!missing.empty()) {
    Invalid("missing is first or last, not \"" + missing + "\"");
  }
  return out;
}

// --- Search ----------------------------------------------------------------------------------------

collection_v1::Consistency ConsistencyOf(const std::string& level) {
  collection_v1::Consistency out;
  if (level == "strong") {
    out.set_freshness(collection_v1::FRESHNESS_STRONG);
  } else if (level == "eventual") {
    out.set_freshness(collection_v1::FRESHNESS_EVENTUAL);
  } else {
    out.set_at_least(ConsistencyToken::Parse(level).Value());
  }
  return out;
}

Search Search::On(const std::string& collection, const std::string& ns) {
  Search out;
  out.request_.set_namespace_(ns);
  out.request_.set_collection(collection);
  return out;
}

Search& Search::Namespace(const std::string& ns) {
  request_.set_namespace_(ns);
  return *this;
}

Search& Search::Collection(const std::string& collection) {
  request_.set_collection(collection);
  return *this;
}

Search& Search::Text(const std::string& text, const TextRetrieverOptions& options) {
  const ::loams::Filter query = options.fields.empty()       ? ::loams::Filter::QueryString(text)
                                : options.fields.size() == 1 ? ::loams::Filter::Match(options.fields.front(), text)
                                                             : ::loams::Filter::MultiMatch(options.fields, text);
  return Text(query, options.k);
}

Search& Search::Text(const ::loams::Filter& query, std::optional<std::uint32_t> k) {
  auto* text = AddRetriever(&request_)->mutable_text();
  *text->mutable_query() = query.Query();
  text->set_k(k.value_or(0));
  return *this;
}

Search& Search::Vector(const std::string& field, const std::vector<float>& values, const VectorOptions& options) {
  auto* vector = AddRetriever(&request_)->mutable_vector();
  vector->set_field(field);
  for (const float value : values) {
    vector->add_query(value);
  }
  vector->set_k(options.k.value_or(0));
  if (options.exact || options.nprobes || options.refine_factor || options.ef || options.oversampling ||
      !options.distance.empty()) {
    auto* params = vector->mutable_params();
    params->set_exact(options.exact.value_or(false));
    if (options.nprobes) params->set_nprobes(*options.nprobes);
    if (options.refine_factor) params->set_refine_factor(*options.refine_factor);
    if (options.ef) params->set_ef(*options.ef);
    if (options.oversampling) params->set_oversampling(*options.oversampling);
    params->set_distance(options.distance);
  }
  if (options.filter) {
    *vector->mutable_filter() = options.filter->Query();
  }
  return *this;
}

Search& Search::Sparse(const std::string& field, const std::vector<std::uint32_t>& indices,
                       const std::vector<float>& values, std::optional<std::uint32_t> k) {
  auto* sparse = AddRetriever(&request_)->mutable_sparse();
  sparse->set_field(field);
  auto& query = *sparse->mutable_query()->mutable_fields();
  JsonValue index_list;
  JsonValue value_list;
  index_list.mutable_list_value();
  value_list.mutable_list_value();
  for (const std::uint32_t index : indices) {
    *index_list.mutable_list_value()->add_values() = Json(static_cast<double>(index));
  }
  for (const float value : values) {
    *value_list.mutable_list_value()->add_values() = Json(static_cast<double>(value));
  }
  query["indices"] = index_list;
  query["values"] = value_list;
  sparse->set_k(k.value_or(0));
  return *this;
}

Search& Search::Fuse(const collection_v1::Fusion& fusion) {
  *request_.mutable_fusion() = fusion;
  return *this;
}

Search& Search::Where(const ::loams::Filter& filter) {
  *request_.mutable_filter() = filter.Query();
  return *this;
}

Search& Search::SortBy(const std::vector<collection_v1::SortKey>& keys) {
  request_.clear_sort();
  for (const auto& key : keys) {
    *request_.add_sort() = key;
  }
  return *this;
}

Search& Search::Limit(std::uint32_t limit) {
  request_.set_limit(limit);
  return *this;
}

Search& Search::Offset(std::uint32_t offset) {
  request_.set_offset(offset);
  return *this;
}

Search& Search::After(const std::vector<JsonValue>& sort_values) {
  request_.clear_search_after();
  for (const JsonValue& value : sort_values) {
    *request_.add_search_after() = value;
  }
  return *this;
}

Search& Search::MinScore(float score) {
  request_.set_score_threshold(score);
  return *this;
}

Search& Search::Select(const SelectOptions& options) {
  auto& select = *request_.mutable_select()->mutable_fields();
  select.clear();
  const auto list = [](const std::vector<std::string>& items) {
    JsonValue value;
    value.mutable_list_value();
    for (const std::string& item : items) {
      *value.mutable_list_value()->add_values() = Json(item);
    }
    return value;
  };
  if (options.no_source) {
    select["source"] = Json("none");
  } else if (!options.include.empty() || !options.exclude.empty()) {
    JsonValue source;
    auto& fields = *source.mutable_struct_value()->mutable_fields();
    if (!options.include.empty()) fields["include"] = list(options.include);
    if (!options.exclude.empty()) fields["exclude"] = list(options.exclude);
    select["source"] = source;
  }
  if (!options.vectors.empty()) select["vectors"] = list(options.vectors);
  if (!options.fields.empty()) select["fields"] = list(options.fields);
  return *this;
}

Search& Search::Highlight(const std::string& field, const HighlightOptions& options) {
  auto* entry = request_.mutable_highlight()->add_fields();
  entry->set_field(field);
  if (options.pre) entry->set_pre_tag(*options.pre);
  if (options.post) entry->set_post_tag(*options.post);
  if (options.fragment_size) entry->set_fragment_size(*options.fragment_size);
  if (options.fragments) entry->set_number_of_fragments(*options.fragments);
  return *this;
}

Search& Search::GroupBy(const std::string& field, std::optional<std::uint32_t> size,
                        std::optional<std::uint32_t> limit) {
  auto* group = request_.mutable_group_by();
  group->set_field(field);
  if (size) group->set_group_size(*size);
  if (limit) group->set_limit(*limit);
  return *this;
}

Search& Search::Expand(const std::string& graph, const ExpandOptions& options) {
  auto* spec = request_.mutable_expand_spec();
  spec->Clear();
  spec->set_graph(graph);
  spec->set_from_field(options.from_field);
  spec->set_seed_label(options.seed_label);
  spec->set_seeds(options.seeds.value_or(0));
  spec->set_hops(options.hops.value_or(0));
  spec->set_limit_per_seed(options.limit_per_seed.value_or(0));
  spec->set_limit(options.limit.value_or(0));
  if (options.direction == "out") {
    spec->set_direction(collection_v1::EXPAND_DIRECTION_OUT);
  } else if (options.direction == "in") {
    spec->set_direction(collection_v1::EXPAND_DIRECTION_IN);
  } else if (options.direction == "both") {
    spec->set_direction(collection_v1::EXPAND_DIRECTION_BOTH);
  } else if (!options.direction.empty()) {
    Invalid("direction is out, in or both, not \"" + options.direction + "\"");
  }
  for (const std::string& type : options.edge_types) {
    spec->add_edge_types(type);
  }
  if (options.output == "nested") {
    spec->set_output(collection_v1::EXPAND_OUTPUT_NESTED);
  } else if (options.output == "flat") {
    spec->set_output(collection_v1::EXPAND_OUTPUT_FLAT);
  } else if (!options.output.empty()) {
    Invalid("output is nested or flat, not \"" + options.output + "\"");
  }
  return *this;
}

Search& Search::Consistency(const std::string& level) {
  *request_.mutable_consistency() = ConsistencyOf(level);
  return *this;
}

bool Search::HasTiebreak() const { return HasTiebreak(request_); }

bool Search::HasTiebreak(const collection_v1::SearchRequest& request) {
  return request.sort_size() > 0 && request.sort(request.sort_size() - 1).has_pk();
}

void Search::Check(const collection_v1::SearchRequest& request) {
  if (request.has_fusion() && request.fusion().has_weighted_sum() &&
      request.fusion().weighted_sum().weights_size() != request.retrievers_size()) {
    Invalid("a weighted fusion needs one weight per retriever: " +
                std::to_string(request.fusion().weighted_sum().weights_size()) + " weights for " +
                std::to_string(request.retrievers_size()) + " retrievers",
            {{"field", "fusion.weighted_sum.weights"}});
  }
}

collection_v1::SearchRequest Search::Build() const {
  Check(request_);
  return request_;
}

collection_v1::SearchRequest Search::Wire(const collection_v1::SearchRequest& request) {
  // A retriever's `k` goes out only when the caller gave one; the engine fills
  // `max(10, offset + limit)` itself (SDKP-15, SDKP-18).
  Check(request);
  return request;
}

}  // namespace loams
