// The app-building modules (SDKP §2.2–§2.5): `namespaces`, `collections`,
// `documents`, `search`, `vector` and `graph`, and the collection handle (§2.8).
//
// Hand-written over the generated messages, like the rest of this facade (see
// `facade.hpp`'s header): every call names its RPC, whose binding (retry class,
// idempotency-key field) is in `bindings.cpp`, and goes through `Client::Unary`,
// so R1–R3 and R8 are the runtime's. What this layer adds:
//
// - **R11, the default namespace.** A request whose `namespace` is empty gets
//   `Options::default_namespace` (or `CallOptions::ns`); with neither it throws
//   `InvalidArgumentError` before any network call.
// - **R4, the session.** With `Options::session_consistency`, a read whose
//   `consistency` is unset reads `at_least` the session's token, and every
//   `token`, `read_token` or `consistency_token` an answer carries is merged in.
// - **R12, timeouts**, through `CallOptions::timeout`.
// - **R13, `WriteBatch`**, and the conveniences of §2.3–§2.5.
//
// Answers are the response messages. Errors are the runtime's typed exceptions.

#ifndef LOAMS_APP_HPP
#define LOAMS_APP_HPP

#include <chrono>
#include <cstddef>
#include <functional>
#include <map>
#include <optional>
#include <string>
#include <vector>

#include <google/protobuf/struct.pb.h>

#include "loams/builders.hpp"
#include "loams/client.hpp"
#include "loams/collection/v1/collection.pb.h"
#include "loams/collection/v1/document.pb.h"
#include "loams/collection/v1/query.pb.h"
#include "loams/error.hpp"
#include "loams/graph/v1/graph.pb.h"
#include "loams/operations/v1/operations.pb.h"

namespace loams {

namespace graph_v1 = ::loams::graph::v1;

/// What every app call takes besides its arguments.
struct CallOptions {
  /// This call's budget (R12); unset is `Options::timeout`, zero is none.
  std::optional<std::chrono::milliseconds> timeout;
  /// The idempotency key of a keyed call (R3); empty mints one.
  std::string idempotency_key;
  /// The namespace; empty is `Options::default_namespace` (R11).
  std::string ns;
  /// Extra request headers.
  std::vector<std::pair<std::string, std::string>> headers;
};

/// One document to write.
struct Document {
  DocId id;
  google::protobuf::Struct source;
  std::map<std::string, std::vector<float>> vectors;

  /// A document whose source is proto3 JSON.
  static Document FromJson(DocId id, const std::string& source_json,
                           std::map<std::string, std::vector<float>> vectors = {});
  collection_v1::WriteDocument ToMessage() const;
};

/// A patch: keys to set and keys to remove.
struct PatchChange {
  google::protobuf::Struct set;
  std::vector<std::string> unset;
  /// `merge_deep`, `merge_top` or `replace`; empty is the server's default.
  std::string mode;
};

/// How a read is served.
struct ReadOptions {
  /// `strong`, `eventual` or a token; empty is the server's default (or the session's).
  std::string consistency;
  /// The projection (`{"source": {"include": [...]}}`).
  std::optional<google::protobuf::Struct> select;
};

/// What `Scroll`, `Count` and the filter writes match.
struct DocumentFilter {
  /// A typed filter, sent as `where` (SDKP-3). Needs protos that declare it.
  std::optional<Filter> where;
  /// The server's native filter JSON, sent as `filter`.
  std::optional<JsonValue> filter;
};

struct ScrollOptions {
  DocumentFilter match;
  std::optional<std::uint32_t> limit;
  std::optional<DocId> after;
  ReadOptions read;
};

/// R13's chunking.
struct BatchOptions {
  std::size_t chunk_rows = 10000;
  std::size_t chunk_bytes = 4 * 1024 * 1024;
};

/// What a `WriteBatch` applied: the merged token, the chunks and the rows.
struct BatchResult {
  std::string token;
  std::size_t chunks = 0;
  std::size_t rows = 0;
};

/// A `WriteBatch` stopped at a failed chunk: the chunks before it were applied.
/// Its code is the failed chunk's, its unknown reason `batch_partially_applied`,
/// and its metadata carries `chunks_applied` and `rows_applied`.
class BatchError : public LoamsError {
 public:
  BatchError(const LoamsError& cause, std::size_t chunks_applied, std::size_t rows_applied, std::string token);
  std::size_t ChunksApplied() const { return chunks_applied_; }
  std::size_t RowsApplied() const { return rows_applied_; }
  /// The merged token of the applied chunks.
  const std::string& Token() const { return token_; }

 private:
  std::size_t chunks_applied_;
  std::size_t rows_applied_;
  std::string token_;
};

/// The shared call path of the app modules.
class AppModule {
 public:
  explicit AppModule(Client* client) : client_(client) {}

 protected:
  /// Sends `request` to `rpc` and fills `response`, with R4 and R11 applied.
  void Call(const std::string& rpc, google::protobuf::Message* request, google::protobuf::Message* response,
            const CallOptions& options, const MethodBinding* binding = nullptr) const;

  Client* client_;
};

/// `namespaces`: the container every collection and stream lives in.
class NamespacesModule : public AppModule {
 public:
  using AppModule::AppModule;
  collection_v1::CreateNamespaceResponse Create(const std::string& ns, const CallOptions& options = {});
  collection_v1::ListNamespacesResponse List(std::int32_t page_size = 0, const std::string& page_token = "",
                                             const CallOptions& options = {});
  /// Every namespace, across pages.
  std::vector<collection_v1::NamespaceInfo> ListAll(std::int32_t page_size = 0, const CallOptions& options = {});
};

/// `collections`: schema, aliases, versions and the hot tier.
class CollectionsModule : public AppModule {
 public:
  using AppModule::AppModule;
  collection_v1::CollectionInfo Create(const std::string& name, const Schema& schema,
                                       std::optional<std::uint32_t> partitions = {}, const CallOptions& options = {});
  collection_v1::CollectionInfo Get(const std::string& name, const CallOptions& options = {});
  collection_v1::ListCollectionsResponse List(std::int32_t page_size = 0, const std::string& page_token = "",
                                              const CallOptions& options = {});
  std::vector<collection_v1::CollectionInfo> ListAll(std::int32_t page_size = 0, const CallOptions& options = {});
  collection_v1::DropCollectionResponse Drop(const std::string& name, const CallOptions& options = {});
  /// Additive schema change: the fields and vectors of `fields`.
  collection_v1::AddFieldsResponse AddFields(const std::string& name, const Schema& fields,
                                             const CallOptions& options = {});
  collection_v1::ListVersionsResponse ListVersions(const std::string& name, std::int32_t page_size = 0,
                                                   const std::string& page_token = "",
                                                   const CallOptions& options = {});
  collection_v1::UpdateAliasesResponse UpdateAliases(collection_v1::UpdateAliasesRequest request,
                                                     const CallOptions& options = {});
  collection_v1::ScanPlan Scan(collection_v1::ScanRequest request, const CallOptions& options = {});
  collection_v1::SetHotResponse SetHot(const std::string& name, const collection_v1::HotConfig& hot,
                                       const CallOptions& options = {});
  collection_v1::WarmCollectionResponse Warm(const std::string& name, const CallOptions& options = {});
};

/// `documents`: write, get, scroll, count and the filter writes.
class DocumentsModule : public AppModule {
 public:
  using AppModule::AppModule;
  /// Raw write ops; the answer's `token` is the write's consistency token.
  collection_v1::WriteDocumentsResponse Write(const std::string& collection,
                                              const std::vector<collection_v1::WriteOp>& ops,
                                              const CallOptions& options = {});
  collection_v1::WriteDocumentsResponse Upsert(const std::string& collection, const std::vector<Document>& docs,
                                               const CallOptions& options = {});
  collection_v1::WriteDocumentsResponse Delete(const std::string& collection, const std::vector<DocId>& ids,
                                               const CallOptions& options = {});
  collection_v1::WriteDocumentsResponse Patch(const std::string& collection, const DocId& id,
                                              const PatchChange& change, const CallOptions& options = {});
  /// R13: `docs` in chunks, one in flight at a time, each with its own
  /// idempotency key (reused on that chunk's retries). Throws `BatchError`.
  BatchResult WriteBatch(const std::string& collection, const std::vector<Document>& docs,
                         const BatchOptions& batch = {}, const CallOptions& options = {});
  /// The same over a generator: `next` fills its argument and returns false at the end.
  BatchResult WriteBatch(const std::string& collection, const std::function<bool(Document*)>& next,
                         const BatchOptions& batch = {}, const CallOptions& options = {});
  collection_v1::GetDocumentsResponse Get(const std::string& collection, const std::vector<DocId>& ids,
                                          const ReadOptions& read = {}, const CallOptions& options = {});
  collection_v1::ScrollDocumentsResponse Scroll(const std::string& collection, const ScrollOptions& scroll = {},
                                                const CallOptions& options = {});
  /// Every document, page after page until the answer has no `next`.
  std::vector<collection_v1::Document> ScrollAll(const std::string& collection, const ScrollOptions& scroll = {},
                                                 const CallOptions& options = {});
  /// `ScrollAll`, one document at a time; `each` returns false to stop.
  void ScrollEach(const std::string& collection, const ScrollOptions& scroll,
                  const std::function<bool(const collection_v1::Document&)>& each, const CallOptions& options = {});
  collection_v1::CountDocumentsResponse Count(const std::string& collection, const DocumentFilter& match = {},
                                              const ReadOptions& read = {}, const CallOptions& options = {});
  collection_v1::FilterWriteResponse DeleteByFilter(const std::string& collection, const DocumentFilter& match,
                                                    const CallOptions& options = {});
  collection_v1::FilterWriteResponse PatchByFilter(const std::string& collection, const DocumentFilter& match,
                                                   const PatchChange& change, const CallOptions& options = {});
};

/// The shorthands' options.
struct SearchShorthand {
  std::vector<std::string> fields;
  std::optional<Filter> filter;
  std::optional<std::uint32_t> limit;
  std::optional<collection_v1::Fusion> fusion;
  std::optional<std::uint32_t> k;
};

/// `search`: one search over one collection.
class SearchModule : public AppModule {
 public:
  using AppModule::AppModule;
  /// The full request, checked (`Search::Build`) and sent as built.
  collection_v1::SearchResponse Query(const collection_v1::SearchRequest& request, const CallOptions& options = {});
  collection_v1::SearchResponse Query(const ::loams::Search& request, const CallOptions& options = {});
  /// A text search: a match on one field, a multi-match on several, or a query string.
  collection_v1::SearchResponse Text(const std::string& collection, const std::string& text,
                                     const SearchShorthand& shorthand = {}, const CallOptions& options = {});
  /// Text plus a vector on `field`, fused (`Fusion::Rrf()` by default).
  collection_v1::SearchResponse Hybrid(const std::string& collection, const std::string& text,
                                       const std::vector<float>& vector, const std::string& field,
                                       const SearchShorthand& shorthand = {}, const CallOptions& options = {});
  /// Every hit, page after page through `search_after`, until a page has fewer
  /// than `limit` hits. The sort must end with `Sort::Pk()`, or it throws
  /// `InvalidArgumentError` before any network call.
  std::vector<collection_v1::Hit> QueryAll(const ::loams::Search& request, const CallOptions& options = {});
  void QueryEach(const ::loams::Search& request, const std::function<bool(const collection_v1::Hit&)>& each,
                 const CallOptions& options = {});

};

/// `vector`: the same RPC as `search.query`, with the vector shape as its required arguments.
class VectorModule : public AppModule {
 public:
  using AppModule::AppModule;
  collection_v1::SearchResponse Search(const std::string& collection, const std::vector<float>& vector,
                                       const std::string& field, const SearchShorthand& shorthand = {},
                                       const CallOptions& options = {});
};

/// GQL parameters.
using GraphParams = std::map<std::string, graph_v1::Value>;

graph_v1::Value GraphValue(const char* value);
graph_v1::Value GraphValue(const std::string& value);
graph_v1::Value GraphValue(std::int64_t value);
graph_v1::Value GraphValue(int value);
graph_v1::Value GraphValue(double value);
graph_v1::Value GraphValue(bool value);

struct GraphQueryOptions {
  bool read_only = false;
  /// `strong`, `eventual` or a token.
  std::string consistency;
  /// `QUERY_LANGUAGE_GQL` when unset.
  graph_v1::QueryLanguage language = graph_v1::QUERY_LANGUAGE_GQL;
  std::uint32_t max_rows = 0;
};

/// `graph`: stored property graphs, queried with GQL. **Experimental**: the
/// package is `unstable`.
class GraphModule : public AppModule {
 public:
  using AppModule::AppModule;
  graph_v1::Graph Create(const std::string& name, const CallOptions& options = {});
  graph_v1::Graph Create(graph_v1::CreateGraphRequest request, const CallOptions& options = {});
  graph_v1::Graph Get(const std::string& name, const CallOptions& options = {});
  graph_v1::ListGraphsResponse List(std::int32_t page_size = 0, const std::string& page_token = "",
                                    const CallOptions& options = {});
  std::vector<graph_v1::Graph> ListAll(std::int32_t page_size = 0, const CallOptions& options = {});
  graph_v1::Graph Update(graph_v1::UpdateGraphRequest request, const CallOptions& options = {});
  /// Answers an `Operation`.
  ::loams::operations::v1::Operation Delete(const std::string& name, const CallOptions& options = {});
  graph_v1::GraphSchema Schema(const std::string& name, const CallOptions& options = {});
  /// One statement. A read-only statement is retried as a read; a write carries an
  /// idempotency key.
  graph_v1::ExecuteResponse Query(const std::string& graph, const std::string& statement,
                                  const GraphParams& parameters = {}, const GraphQueryOptions& query = {},
                                  const CallOptions& options = {});
  graph_v1::ExecuteBatchResponse QueryBatch(graph_v1::ExecuteBatchRequest request, const CallOptions& options = {});
  graph_v1::Plan Explain(graph_v1::ExplainRequest request, const CallOptions& options = {});

  /// An `ExecuteResponse`'s rows, column name to JSON value.
  static std::vector<std::map<std::string, JsonValue>> Records(const graph_v1::ExecuteResponse& response);
};

/// Any facade call by its facade-index name (`documents.write`,
/// `instance.get_instance`) over its request's proto3 JSON, answering the
/// response's proto3 JSON: the same call path as the typed modules.
class RawModule : public AppModule {
 public:
  using AppModule::AppModule;
  std::string Call(const std::string& name, const std::string& request_json, const CallOptions& options = {});
};

class AppModules;

/// `Collection("notes")`: every document and search call with the collection
/// (and namespace) filled in. It holds no state of its own.
class CollectionHandle {
 public:
  CollectionHandle(AppModules* modules, std::string name, std::string ns)
      : modules_(modules), name_(std::move(name)), ns_(std::move(ns)) {}

  const std::string& Name() const { return name_; }

  collection_v1::WriteDocumentsResponse Upsert(const std::vector<Document>& docs, CallOptions options = {});
  collection_v1::GetDocumentsResponse Get(const std::vector<DocId>& ids, const ReadOptions& read = {},
                                          CallOptions options = {});
  collection_v1::WriteDocumentsResponse Delete(const std::vector<DocId>& ids, CallOptions options = {});
  collection_v1::WriteDocumentsResponse Patch(const DocId& id, const PatchChange& change, CallOptions options = {});
  BatchResult WriteBatch(const std::vector<Document>& docs, const BatchOptions& batch = {}, CallOptions options = {});
  collection_v1::CountDocumentsResponse Count(const DocumentFilter& match = {}, const ReadOptions& read = {},
                                              CallOptions options = {});
  std::vector<collection_v1::Document> ScrollAll(const ScrollOptions& scroll = {}, CallOptions options = {});
  /// `search`'s collection is set to this one.
  collection_v1::SearchResponse Search(::loams::Search search, CallOptions options = {});
  collection_v1::SearchResponse VectorSearch(const std::vector<float>& vector, const std::string& field,
                                             const SearchShorthand& shorthand = {}, CallOptions options = {});
  collection_v1::DropCollectionResponse Drop(CallOptions options = {});
  collection_v1::CollectionInfo Info(CallOptions options = {});

 private:
  CallOptions With(CallOptions options) const;
  AppModules* modules_;
  std::string name_;
  std::string ns_;
};

/// The app modules of one client.
class AppModules {
 public:
  explicit AppModules(Client* client)
      : namespaces(client),
        collections(client),
        documents(client),
        search(client),
        vector(client),
        graph(client),
        raw(client) {}
  NamespacesModule namespaces;
  CollectionsModule collections;
  DocumentsModule documents;
  SearchModule search;
  VectorModule vector;
  GraphModule graph;
  RawModule raw;
};

}  // namespace loams

#endif  // LOAMS_APP_HPP
