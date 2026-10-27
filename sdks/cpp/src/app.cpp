// The app-building modules, as `app.hpp` documents them.

#include "loams/app.hpp"

#include <algorithm>
#include <utility>

#include <google/protobuf/descriptor.h>
#include <google/protobuf/message.h>
#include <memory>
#include <google/protobuf/reflection.h>
#include <google/protobuf/util/json_util.h>

#include "loams/consistency.hpp"
#include "loams/facade.hpp"
#include "loams/instance/v1/instance.pb.h"
#include "loams/sql/v1/sql.pb.h"
#include "loams/stream/v1/stream.pb.h"
#include "loams/idempotency.hpp"

namespace loams {
namespace {

using google::protobuf::FieldDescriptor;
using google::protobuf::Message;

constexpr char kCreateNamespace[] = "loams.collection.v1.NamespaceService/CreateNamespace";
constexpr char kListNamespaces[] = "loams.collection.v1.NamespaceService/ListNamespaces";
constexpr char kCreateCollection[] = "loams.collection.v1.CollectionService/CreateCollection";
constexpr char kListCollections[] = "loams.collection.v1.CollectionService/ListCollections";
constexpr char kGetCollection[] = "loams.collection.v1.CollectionService/GetCollection";
constexpr char kDropCollection[] = "loams.collection.v1.CollectionService/DropCollection";
constexpr char kAddFields[] = "loams.collection.v1.CollectionService/AddFields";
constexpr char kListVersions[] = "loams.collection.v1.CollectionService/ListVersions";
constexpr char kUpdateAliases[] = "loams.collection.v1.CollectionService/UpdateAliases";
constexpr char kScan[] = "loams.collection.v1.CollectionService/Scan";
constexpr char kSetHot[] = "loams.collection.v1.CollectionService/SetHot";
constexpr char kWarm[] = "loams.collection.v1.CollectionService/WarmCollection";
constexpr char kWrite[] = "loams.collection.v1.DocumentService/WriteDocuments";
constexpr char kGet[] = "loams.collection.v1.DocumentService/GetDocuments";
constexpr char kScroll[] = "loams.collection.v1.DocumentService/ScrollDocuments";
constexpr char kCount[] = "loams.collection.v1.DocumentService/CountDocuments";
constexpr char kDeleteByFilter[] = "loams.collection.v1.DocumentService/DeleteByFilter";
constexpr char kPatchByFilter[] = "loams.collection.v1.DocumentService/PatchByFilter";
constexpr char kSearch[] = "loams.collection.v1.QueryService/Search";
constexpr char kCreateGraph[] = "loams.graph.v1.GraphAdminService/CreateGraph";
constexpr char kGetGraph[] = "loams.graph.v1.GraphAdminService/GetGraph";
constexpr char kListGraphs[] = "loams.graph.v1.GraphAdminService/ListGraphs";
constexpr char kUpdateGraph[] = "loams.graph.v1.GraphAdminService/UpdateGraph";
constexpr char kDeleteGraph[] = "loams.graph.v1.GraphAdminService/DeleteGraph";
constexpr char kGetSchema[] = "loams.graph.v1.GraphAdminService/GetSchema";
constexpr char kExecute[] = "loams.graph.v1.GraphService/Execute";
constexpr char kExecuteBatch[] = "loams.graph.v1.GraphService/ExecuteBatch";
constexpr char kExplain[] = "loams.graph.v1.GraphService/Explain";

[[noreturn]] void Invalid(const std::string& rpc, const std::string& message) {
  ThrowMapped(Code::kInvalidArgument, rpc, Reason::kUnknown, "", {}, "", "loams: " + message, "");
}

/// The session's scope: the proto package an RPC belongs to, except that SQL
/// reads (and adds to) the collection token, since it reads collections.
std::string ScopeOf(const std::string& rpc) {
  const std::string service = rpc.substr(0, rpc.find('/'));
  const std::string package = service.substr(0, service.rfind('.'));
  return package == "loams.sql.v1" ? "loams.collection.v1" : package;
}

/// R11 on any request: fills an empty `namespace` field.
void FillNamespace(Message* request, const std::string& ns, const std::string& rpc) {
  const FieldDescriptor* field = request->GetDescriptor()->FindFieldByName("namespace");
  if (field == nullptr || field->cpp_type() != FieldDescriptor::CPPTYPE_STRING || field->is_repeated()) {
    return;
  }
  const auto* reflection = request->GetReflection();
  if (!reflection->GetString(*request, field).empty()) {
    return;
  }
  if (ns.empty()) {
    ThrowClientSide(Code::kInvalidArgument, rpc, "namespace_required",
                    "loams: " + rpc + " needs a namespace: set CallOptions::ns or Options::default_namespace");
  }
  reflection->SetString(request, field, ns);
}

/// R4 on a read: `consistency.at_least = session` when `consistency` is unset.
/// Only collection (and SQL) reads take the session's token (SDKP-17): a graph's
/// tokens name one graph's stream.
void ApplySession(SessionTokenStore* session, const std::string& scope, Message* request) {
  if (session == nullptr || scope != "loams.collection.v1") {
    return;
  }
  const FieldDescriptor* field = request->GetDescriptor()->FindFieldByName("consistency");
  if (field == nullptr || field->cpp_type() != FieldDescriptor::CPPTYPE_MESSAGE || field->is_repeated()) {
    return;
  }
  const auto* reflection = request->GetReflection();
  if (reflection->HasField(*request, field)) {
    return;
  }
  const ConsistencyToken token = session->Merged(scope);
  if (!token.Present() || token.Empty()) {
    return;
  }
  Message* consistency = reflection->MutableMessage(request, field);
  const FieldDescriptor* at_least = consistency->GetDescriptor()->FindFieldByName("at_least");
  if (at_least == nullptr) {
    reflection->ClearField(request, field);
    return;
  }
  consistency->GetReflection()->SetString(consistency, at_least, token.Value());
}

/// R4 on an answer: every token field merges into the session.
void ObserveSession(SessionTokenStore* session, const std::string& scope, const Message& response) {
  if (session == nullptr) {
    return;
  }
  for (const char* name : {"token", "read_token", "consistency_token"}) {
    const FieldDescriptor* field = response.GetDescriptor()->FindFieldByName(name);
    if (field != nullptr && field->cpp_type() == FieldDescriptor::CPPTYPE_STRING && !field->is_repeated()) {
      session->ObserveText(response.GetReflection()->GetString(response, field), scope);
    }
  }
}

/// Sets the typed `where` (SDKP-3) and the native `filter` on a document request.
void ApplyMatch(Message* request, const DocumentFilter& match, const std::string& rpc) {
  const auto* reflection = request->GetReflection();
  if (match.where) {
    const FieldDescriptor* where = request->GetDescriptor()->FindFieldByName("where");
    if (where == nullptr) {
      Invalid(rpc, rpc + " takes no typed where in this SDK's protos; pass DocumentFilter::filter (the native JSON)");
    }
    // `where` is a `Struct` holding the `Query`'s proto3 JSON (SDKP-12); the
    // JSON parses into whichever message the field is.
    if (!google::protobuf::util::JsonStringToMessage(ToJson(match.where->Query()),
                                                     reflection->MutableMessage(request, where))
             .ok()) {
      Invalid(rpc, "a filter would not encode as " + rpc + "'s where");
    }
  }
  if (match.filter) {
    const FieldDescriptor* filter = request->GetDescriptor()->FindFieldByName("filter");
    reflection->MutableMessage(request, filter)->CopyFrom(*match.filter);
  }
}

collection_v1::Patch PatchOf(const DocId& id, const PatchChange& change) {
  collection_v1::Patch patch;
  *patch.mutable_id() = id.ToMessage();
  patch.set_mode(change.mode);
  if (change.set.fields_size() > 0) {
    *patch.mutable_source() = change.set;
  }
  for (const std::string& key : change.unset) {
    patch.add_delete_keys(key);
  }
  return patch;
}

std::string NsOf(const CallOptions& options, Client* client) {
  return options.ns.empty() ? client->DefaultNamespace() : options.ns;
}

}  // namespace

// --- Documents and batches -------------------------------------------------------------------------

Document Document::FromJson(DocId id, const std::string& source_json, std::map<std::string, std::vector<float>> vectors) {
  Document doc{std::move(id), StructFromJson(source_json), std::move(vectors)};
  return doc;
}

collection_v1::WriteDocument Document::ToMessage() const {
  collection_v1::WriteDocument out;
  *out.mutable_id() = id.ToMessage();
  *out.mutable_source() = source;
  for (const auto& vector : vectors) {
    JsonValue list;
    list.mutable_list_value();
    for (const float value : vector.second) {
      *list.mutable_list_value()->add_values() = Json(static_cast<double>(value));
    }
    (*out.mutable_vectors())[vector.first] = list;
  }
  return out;
}

BatchError::BatchError(const LoamsError& cause, std::size_t chunks_applied, std::size_t rows_applied,
                       std::string token)
    // SDKP-23: registered under `aborted`; the failed chunk's error is the cause.
    : LoamsError(Code::kAborted, cause.Rpc(), ReasonFromString("batch_partially_applied").value_or(Reason::kUnknown),
                 ReasonFromString("batch_partially_applied") ? std::string() : std::string("batch_partially_applied"),
                 {{"chunks_applied", std::to_string(chunks_applied)}, {"rows_applied", std::to_string(rows_applied)}},
                 cause.Hint(),
                 "loams: WriteBatch stopped at chunk " + std::to_string(chunks_applied + 1) + ": " +
                     std::to_string(chunks_applied) + " chunks (" + std::to_string(rows_applied) +
                     " operations) were applied before it; " + cause.what(),
                 cause.RequestId()),
      chunks_applied_(chunks_applied),
      rows_applied_(rows_applied),
      token_(std::move(token)) {}

// --- The call path ---------------------------------------------------------------------------------

void AppModule::Call(const std::string& rpc, Message* request, Message* response, const CallOptions& options,
                     const MethodBinding* binding) const {
  FillNamespace(request, NsOf(options, client_), rpc);
  const std::string scope = ScopeOf(rpc);
  ApplySession(client_->Session(), scope, request);
  client_->Unary(binding != nullptr ? *binding : BindingFor(rpc), *request, response, client_->Encoding(),
                 options.idempotency_key, options.headers, options.timeout);
  ObserveSession(client_->Session(), scope, *response);
}

// --- namespaces ------------------------------------------------------------------------------------

collection_v1::CreateNamespaceResponse NamespacesModule::Create(const std::string& ns, const CallOptions& options) {
  collection_v1::CreateNamespaceRequest request;
  request.set_namespace_(ns);
  collection_v1::CreateNamespaceResponse response;
  Call(kCreateNamespace, &request, &response, options);
  return response;
}

collection_v1::ListNamespacesResponse NamespacesModule::List(std::int32_t page_size, const std::string& page_token,
                                                             const CallOptions& options) {
  collection_v1::ListNamespacesRequest request;
  request.set_page_size(page_size);
  request.set_page_token(page_token);
  collection_v1::ListNamespacesResponse response;
  Call(kListNamespaces, &request, &response, options);
  return response;
}

std::vector<collection_v1::NamespaceInfo> NamespacesModule::ListAll(std::int32_t page_size, const CallOptions& options) {
  std::vector<collection_v1::NamespaceInfo> out;
  std::string token;
  do {
    const auto page = List(page_size, token, options);
    out.insert(out.end(), page.namespaces().begin(), page.namespaces().end());
    if (page.next_page_token() == token) {
      break;
    }
    token = page.next_page_token();
  } while (!token.empty());
  return out;
}

// --- collections -----------------------------------------------------------------------------------

collection_v1::CollectionInfo CollectionsModule::Create(const std::string& name, const Schema& schema,
                                                        std::optional<std::uint32_t> partitions,
                                                        const CallOptions& options) {
  collection_v1::CreateCollectionRequest request;
  request.set_name(name);
  *request.mutable_schema() = schema.ToStruct();
  if (partitions) {
    request.set_partitions(*partitions);
  }
  collection_v1::CollectionInfo response;
  Call(kCreateCollection, &request, &response, options);
  return response;
}

collection_v1::CollectionInfo CollectionsModule::Get(const std::string& name, const CallOptions& options) {
  collection_v1::GetCollectionRequest request;
  request.set_collection(name);
  collection_v1::CollectionInfo response;
  Call(kGetCollection, &request, &response, options);
  return response;
}

collection_v1::ListCollectionsResponse CollectionsModule::List(std::int32_t page_size, const std::string& page_token,
                                                               const CallOptions& options) {
  collection_v1::ListCollectionsRequest request;
  request.set_page_size(page_size);
  request.set_page_token(page_token);
  collection_v1::ListCollectionsResponse response;
  Call(kListCollections, &request, &response, options);
  return response;
}

std::vector<collection_v1::CollectionInfo> CollectionsModule::ListAll(std::int32_t page_size,
                                                                      const CallOptions& options) {
  std::vector<collection_v1::CollectionInfo> out;
  std::string token;
  do {
    const auto page = List(page_size, token, options);
    out.insert(out.end(), page.collections().begin(), page.collections().end());
    if (page.next_page_token() == token) {
      break;
    }
    token = page.next_page_token();
  } while (!token.empty());
  return out;
}

collection_v1::DropCollectionResponse CollectionsModule::Drop(const std::string& name, const CallOptions& options) {
  collection_v1::DropCollectionRequest request;
  request.set_collection(name);
  collection_v1::DropCollectionResponse response;
  Call(kDropCollection, &request, &response, options);
  return response;
}

collection_v1::AddFieldsResponse CollectionsModule::AddFields(const std::string& name, const Schema& fields,
                                                              const CallOptions& options) {
  collection_v1::AddFieldsRequest request;
  request.set_collection(name);
  const google::protobuf::Struct schema = fields.ToStruct();
  for (const auto& field : schema.fields().at("fields").list_value().values()) {
    *request.add_fields() = field.struct_value();
  }
  for (const auto& vector : schema.fields().at("vectors").list_value().values()) {
    *request.add_vectors() = vector.struct_value();
  }
  collection_v1::AddFieldsResponse response;
  Call(kAddFields, &request, &response, options);
  return response;
}

collection_v1::ListVersionsResponse CollectionsModule::ListVersions(const std::string& name, std::int32_t page_size,
                                                                    const std::string& page_token,
                                                                    const CallOptions& options) {
  collection_v1::ListVersionsRequest request;
  request.set_collection(name);
  request.set_page_size(page_size);
  request.set_page_token(page_token);
  collection_v1::ListVersionsResponse response;
  Call(kListVersions, &request, &response, options);
  return response;
}

collection_v1::UpdateAliasesResponse CollectionsModule::UpdateAliases(collection_v1::UpdateAliasesRequest request,
                                                                      const CallOptions& options) {
  collection_v1::UpdateAliasesResponse response;
  Call(kUpdateAliases, &request, &response, options);
  return response;
}

collection_v1::ScanPlan CollectionsModule::Scan(collection_v1::ScanRequest request, const CallOptions& options) {
  collection_v1::ScanPlan response;
  Call(kScan, &request, &response, options);
  return response;
}

collection_v1::SetHotResponse CollectionsModule::SetHot(const std::string& name, const collection_v1::HotConfig& hot,
                                                        const CallOptions& options) {
  collection_v1::SetHotRequest request;
  request.set_collection(name);
  *request.mutable_hot() = hot;
  collection_v1::SetHotResponse response;
  Call(kSetHot, &request, &response, options);
  return response;
}

collection_v1::WarmCollectionResponse CollectionsModule::Warm(const std::string& name, const CallOptions& options) {
  collection_v1::WarmCollectionRequest request;
  request.set_collection(name);
  collection_v1::WarmCollectionResponse response;
  Call(kWarm, &request, &response, options);
  return response;
}

// --- documents -------------------------------------------------------------------------------------

collection_v1::WriteDocumentsResponse DocumentsModule::Write(const std::string& collection,
                                                             const std::vector<collection_v1::WriteOp>& ops,
                                                             const CallOptions& options) {
  collection_v1::WriteDocumentsRequest request;
  request.set_collection(collection);
  for (const auto& op : ops) {
    *request.add_ops() = op;
  }
  collection_v1::WriteDocumentsResponse response;
  Call(kWrite, &request, &response, options);
  return response;
}

collection_v1::WriteDocumentsResponse DocumentsModule::Upsert(const std::string& collection,
                                                              const std::vector<Document>& docs,
                                                              const CallOptions& options) {
  std::vector<collection_v1::WriteOp> ops(docs.size());
  for (std::size_t i = 0; i < docs.size(); ++i) {
    *ops[i].mutable_upsert() = docs[i].ToMessage();
  }
  return Write(collection, ops, options);
}

collection_v1::WriteDocumentsResponse DocumentsModule::Delete(const std::string& collection,
                                                              const std::vector<DocId>& ids,
                                                              const CallOptions& options) {
  std::vector<collection_v1::WriteOp> ops(ids.size());
  for (std::size_t i = 0; i < ids.size(); ++i) {
    *ops[i].mutable_delete_()->mutable_id() = ids[i].ToMessage();
  }
  return Write(collection, ops, options);
}

collection_v1::WriteDocumentsResponse DocumentsModule::Patch(const std::string& collection, const DocId& id,
                                                             const PatchChange& change, const CallOptions& options) {
  std::vector<collection_v1::WriteOp> ops(1);
  *ops[0].mutable_patch() = PatchOf(id, change);
  return Write(collection, ops, options);
}

BatchResult DocumentsModule::WriteBatch(const std::string& collection, const std::vector<Document>& docs,
                                        const BatchOptions& batch, const CallOptions& options) {
  std::size_t next = 0;
  return WriteBatch(
      collection,
      [&docs, &next](Document* out) {
        if (next >= docs.size()) {
          return false;
        }
        *out = docs[next++];
        return true;
      },
      batch, options);
}

BatchResult DocumentsModule::WriteBatch(const std::string& collection, const std::function<bool(Document*)>& next,
                                        const BatchOptions& batch, const CallOptions& options) {
  if (batch.chunk_rows == 0 || batch.chunk_bytes == 0) {
    Invalid(kWrite, "WriteBatch: chunk_rows and chunk_bytes must be positive");
  }
  ConsistencyToken token;
  BatchResult result;
  std::vector<collection_v1::WriteOp> chunk;
  std::size_t chunk_size = 0;
  const auto flush = [&]() {
    if (chunk.empty()) {
      return;
    }
    CallOptions call = options;
    call.idempotency_key = UuidV7();
    collection_v1::WriteDocumentsResponse response;
    try {
      response = Write(collection, chunk, call);
    } catch (const LoamsError& error) {
      throw BatchError(error, result.chunks, result.rows, token.Present() ? token.Value() : std::string());
    }
    if (ConsistencyToken::Valid(response.token())) {
      token = token.Merge(ConsistencyToken::Parse(response.token()));
    }
    result.chunks += 1;
    result.rows += chunk.size();
    chunk.clear();
    chunk_size = 0;
  };
  Document doc;
  while (next(&doc)) {
    collection_v1::WriteOp op;
    *op.mutable_upsert() = doc.ToMessage();
    const std::size_t bytes = op.ByteSizeLong();
    if (!chunk.empty() && (chunk.size() >= batch.chunk_rows || chunk_size + bytes > batch.chunk_bytes)) {
      flush();
    }
    chunk.push_back(std::move(op));
    chunk_size += bytes;
  }
  flush();
  result.token = token.Present() ? token.Value() : std::string();
  return result;
}

collection_v1::GetDocumentsResponse DocumentsModule::Get(const std::string& collection, const std::vector<DocId>& ids,
                                                         const ReadOptions& read, const CallOptions& options) {
  collection_v1::GetDocumentsRequest request;
  request.set_collection(collection);
  for (const DocId& id : ids) {
    *request.add_ids() = id.ToMessage();
  }
  if (!read.consistency.empty()) {
    *request.mutable_consistency() = ConsistencyOf(read.consistency);
  }
  if (read.select) {
    *request.mutable_select() = *read.select;
  }
  collection_v1::GetDocumentsResponse response;
  Call(kGet, &request, &response, options);
  return response;
}

collection_v1::ScrollDocumentsResponse DocumentsModule::Scroll(const std::string& collection,
                                                               const ScrollOptions& scroll,
                                                               const CallOptions& options) {
  collection_v1::ScrollDocumentsRequest request;
  request.set_collection(collection);
  ApplyMatch(&request, scroll.match, kScroll);
  if (scroll.limit) {
    request.set_limit(*scroll.limit);
  }
  if (scroll.after) {
    *request.mutable_after() = scroll.after->ToMessage();
  }
  if (!scroll.read.consistency.empty()) {
    *request.mutable_consistency() = ConsistencyOf(scroll.read.consistency);
  }
  if (scroll.read.select) {
    *request.mutable_select() = *scroll.read.select;
  }
  collection_v1::ScrollDocumentsResponse response;
  Call(kScroll, &request, &response, options);
  return response;
}

void DocumentsModule::ScrollEach(const std::string& collection, const ScrollOptions& scroll,
                                 const std::function<bool(const collection_v1::Document&)>& each,
                                 const CallOptions& options) {
  ScrollOptions page = scroll;
  while (true) {
    const auto answer = Scroll(collection, page, options);
    for (const auto& doc : answer.documents()) {
      if (!each(doc)) {
        return;
      }
    }
    if (!answer.has_next() || answer.next().id_case() == collection_v1::DocumentId::ID_NOT_SET) {
      return;
    }
    page.after = DocId::From(answer.next());
  }
}

std::vector<collection_v1::Document> DocumentsModule::ScrollAll(const std::string& collection,
                                                               const ScrollOptions& scroll,
                                                               const CallOptions& options) {
  std::vector<collection_v1::Document> out;
  ScrollEach(
      collection, scroll,
      [&out](const collection_v1::Document& doc) {
        out.push_back(doc);
        return true;
      },
      options);
  return out;
}

collection_v1::CountDocumentsResponse DocumentsModule::Count(const std::string& collection,
                                                             const DocumentFilter& match, const ReadOptions& read,
                                                             const CallOptions& options) {
  collection_v1::CountDocumentsRequest request;
  request.set_collection(collection);
  ApplyMatch(&request, match, kCount);
  if (!read.consistency.empty()) {
    *request.mutable_consistency() = ConsistencyOf(read.consistency);
  }
  collection_v1::CountDocumentsResponse response;
  Call(kCount, &request, &response, options);
  return response;
}

collection_v1::FilterWriteResponse DocumentsModule::DeleteByFilter(const std::string& collection,
                                                                   const DocumentFilter& match,
                                                                   const CallOptions& options) {
  collection_v1::DeleteByFilterRequest request;
  request.set_collection(collection);
  ApplyMatch(&request, match, kDeleteByFilter);
  collection_v1::FilterWriteResponse response;
  Call(kDeleteByFilter, &request, &response, options);
  return response;
}

collection_v1::FilterWriteResponse DocumentsModule::PatchByFilter(const std::string& collection,
                                                                  const DocumentFilter& match,
                                                                  const PatchChange& change,
                                                                  const CallOptions& options) {
  collection_v1::PatchByFilterRequest request;
  request.set_collection(collection);
  ApplyMatch(&request, match, kPatchByFilter);
  collection_v1::Patch patch = PatchOf(DocId(std::string()), change);
  patch.clear_id();
  *request.mutable_patch() = patch;
  collection_v1::FilterWriteResponse response;
  Call(kPatchByFilter, &request, &response, options);
  return response;
}

// --- search and vector -----------------------------------------------------------------------------

collection_v1::SearchResponse SearchModule::Query(const collection_v1::SearchRequest& request,
                                                  const CallOptions& options) {
  collection_v1::SearchRequest wire = ::loams::Search::Wire(request);
  collection_v1::SearchResponse response;
  Call(kSearch, &wire, &response, options);
  return response;
}

collection_v1::SearchResponse SearchModule::Query(const ::loams::Search& request, const CallOptions& options) {
  return Query(request.Request(), options);
}

collection_v1::SearchResponse SearchModule::Text(const std::string& collection, const std::string& text,
                                                 const SearchShorthand& shorthand, const CallOptions& options) {
  ::loams::Search search = ::loams::Search::On(collection);
  search.Text(text, {shorthand.fields, shorthand.k});
  if (shorthand.filter) search.Where(*shorthand.filter);
  if (shorthand.limit) search.Limit(*shorthand.limit);
  return Query(search, options);
}

collection_v1::SearchResponse SearchModule::Hybrid(const std::string& collection, const std::string& text,
                                                   const std::vector<float>& vector, const std::string& field,
                                                   const SearchShorthand& shorthand, const CallOptions& options) {
  ::loams::Search search = ::loams::Search::On(collection);
  VectorOptions vector_options;
  vector_options.k = shorthand.k;
  search.Text(text, {shorthand.fields, shorthand.k})
      .Vector(field, vector, vector_options)
      .Fuse(shorthand.fusion.value_or(Fusion::Rrf()));
  if (shorthand.filter) search.Where(*shorthand.filter);
  if (shorthand.limit) search.Limit(*shorthand.limit);
  return Query(search, options);
}

void SearchModule::QueryEach(const ::loams::Search& request,
                             const std::function<bool(const collection_v1::Hit&)>& each,
                             const CallOptions& options) {
  if (!request.HasTiebreak()) {
    ThrowClientSide(Code::kInvalidArgument, kSearch, "search_after_needs_tiebreak",
                    "loams: QueryAll needs a sort that ends with the primary key, such as SortBy({Sort::Score(), Sort::Pk()})",
                    {{"field", "sort"}});
  }
  collection_v1::SearchRequest page = request.Request();
  if (!page.has_limit()) {
    page.set_limit(::loams::Search::kDefaultK);
  }
  const int limit = static_cast<int>(page.limit());
  while (true) {
    const auto answer = Query(page, options);
    for (const auto& hit : answer.hits()) {
      if (!each(hit)) {
        return;
      }
    }
    if (answer.hits_size() == 0 || answer.hits_size() < limit) {
      return;
    }
    *page.mutable_search_after() = answer.hits(answer.hits_size() - 1).sort_values();
  }
}

std::vector<collection_v1::Hit> SearchModule::QueryAll(const ::loams::Search& request, const CallOptions& options) {
  std::vector<collection_v1::Hit> out;
  QueryEach(
      request,
      [&out](const collection_v1::Hit& hit) {
        out.push_back(hit);
        return true;
      },
      options);
  return out;
}

collection_v1::SearchResponse VectorModule::Search(const std::string& collection, const std::vector<float>& vector,
                                                   const std::string& field, const SearchShorthand& shorthand,
                                                   const CallOptions& options) {
  ::loams::Search search = ::loams::Search::On(collection);
  VectorOptions vector_options;
  vector_options.k = shorthand.k;
  search.Vector(field, vector, vector_options);
  if (shorthand.filter) search.Where(*shorthand.filter);
  if (shorthand.limit) search.Limit(*shorthand.limit);
  collection_v1::SearchRequest wire = ::loams::Search::Wire(search.Request());
  collection_v1::SearchResponse response;
  Call(kSearch, &wire, &response, options);
  return response;
}

// --- graph -----------------------------------------------------------------------------------------

graph_v1::Value GraphValue(const char* value) { return GraphValue(std::string(value)); }

graph_v1::Value GraphValue(const std::string& value) {
  graph_v1::Value out;
  out.set_string(value);
  return out;
}

graph_v1::Value GraphValue(std::int64_t value) {
  graph_v1::Value out;
  out.set_int64(value);
  return out;
}

graph_v1::Value GraphValue(int value) { return GraphValue(static_cast<std::int64_t>(value)); }

graph_v1::Value GraphValue(double value) {
  graph_v1::Value out;
  out.set_float64(value);
  return out;
}

graph_v1::Value GraphValue(bool value) {
  graph_v1::Value out;
  out.set_boolean(value);
  return out;
}

graph_v1::Graph GraphModule::Create(const std::string& name, const CallOptions& options) {
  graph_v1::CreateGraphRequest request;
  request.set_name(name);
  return Create(std::move(request), options);
}

graph_v1::Graph GraphModule::Create(graph_v1::CreateGraphRequest request, const CallOptions& options) {
  graph_v1::Graph response;
  Call(kCreateGraph, &request, &response, options);
  return response;
}

graph_v1::Graph GraphModule::Get(const std::string& name, const CallOptions& options) {
  graph_v1::GetGraphRequest request;
  request.set_name(name);
  graph_v1::Graph response;
  Call(kGetGraph, &request, &response, options);
  return response;
}

graph_v1::ListGraphsResponse GraphModule::List(std::int32_t page_size, const std::string& page_token,
                                               const CallOptions& options) {
  graph_v1::ListGraphsRequest request;
  request.set_page_size(page_size);
  request.set_page_token(page_token);
  graph_v1::ListGraphsResponse response;
  Call(kListGraphs, &request, &response, options);
  return response;
}

std::vector<graph_v1::Graph> GraphModule::ListAll(std::int32_t page_size, const CallOptions& options) {
  std::vector<graph_v1::Graph> out;
  std::string token;
  do {
    const auto page = List(page_size, token, options);
    out.insert(out.end(), page.graphs().begin(), page.graphs().end());
    if (page.next_page_token() == token) {
      break;
    }
    token = page.next_page_token();
  } while (!token.empty());
  return out;
}

graph_v1::Graph GraphModule::Update(graph_v1::UpdateGraphRequest request, const CallOptions& options) {
  if (request.graph().namespace_().empty()) {
    const std::string ns = NsOf(options, client_);
    if (ns.empty()) {
      Invalid(kUpdateGraph, std::string(kUpdateGraph) + " needs a namespace on its graph");
    }
    request.mutable_graph()->set_namespace_(ns);
  }
  graph_v1::Graph response;
  Call(kUpdateGraph, &request, &response, options);
  return response;
}

::loams::operations::v1::Operation GraphModule::Delete(const std::string& name, const CallOptions& options) {
  graph_v1::DeleteGraphRequest request;
  request.set_name(name);
  ::loams::operations::v1::Operation response;
  Call(kDeleteGraph, &request, &response, options);
  return response;
}

graph_v1::GraphSchema GraphModule::Schema(const std::string& name, const CallOptions& options) {
  graph_v1::GetSchemaRequest request;
  request.set_name(name);
  graph_v1::GraphSchema response;
  Call(kGetSchema, &request, &response, options);
  return response;
}

graph_v1::ExecuteResponse GraphModule::Query(const std::string& graph, const std::string& statement,
                                             const GraphParams& parameters, const GraphQueryOptions& query,
                                             const CallOptions& options) {
  graph_v1::ExecuteRequest request;
  request.set_graph(graph);
  request.set_statement(statement);
  request.set_language(query.language);
  for (const auto& parameter : parameters) {
    (*request.mutable_parameters())[parameter.first] = parameter.second;
  }
  request.set_read_only(query.read_only);
  request.set_max_rows(query.max_rows);
  if (query.consistency == "strong") {
    request.mutable_consistency()->mutable_strong();
  } else if (query.consistency == "eventual") {
    request.mutable_consistency()->mutable_eventual();
  } else if (!query.consistency.empty()) {
    request.mutable_consistency()->set_at_least(ConsistencyToken::Parse(query.consistency).Value());
  }
  graph_v1::ExecuteResponse response;
  if (query.read_only) {
    // A read-only statement is a read: retried as one, with no key.
    MethodBinding binding = BindingFor(kExecute);
    binding.retry_class = RetryClass::kSafe;
    binding.idempotency_field.clear();
    Call(kExecute, &request, &response, options, &binding);
  } else {
    Call(kExecute, &request, &response, options);
  }
  return response;
}

graph_v1::ExecuteBatchResponse GraphModule::QueryBatch(graph_v1::ExecuteBatchRequest request,
                                                       const CallOptions& options) {
  graph_v1::ExecuteBatchResponse response;
  Call(kExecuteBatch, &request, &response, options);
  return response;
}

graph_v1::Plan GraphModule::Explain(graph_v1::ExplainRequest request, const CallOptions& options) {
  graph_v1::Plan response;
  Call(kExplain, &request, &response, options);
  return response;
}

namespace {

JsonValue JsonOfGraph(const graph_v1::Value& value) {
  switch (value.kind_case()) {
    case graph_v1::Value::kNull:
      return JsonNull();
    case graph_v1::Value::kBoolean:
      return Json(value.boolean());
    case graph_v1::Value::kInt64:
      return Json(static_cast<double>(value.int64()));
    case graph_v1::Value::kUint64:
      return Json(static_cast<double>(value.uint64()));
    case graph_v1::Value::kFloat64:
      return Json(value.float64());
    case graph_v1::Value::kString:
      return Json(value.string());
    case graph_v1::Value::kList: {
      JsonValue out;
      out.mutable_list_value();
      for (const auto& item : value.list().values()) {
        *out.mutable_list_value()->add_values() = JsonOfGraph(item);
      }
      return out;
    }
    case graph_v1::Value::kMap: {
      JsonValue out;
      out.mutable_struct_value();
      for (const auto& entry : value.map().entries()) {
        (*out.mutable_struct_value()->mutable_fields())[entry.first] = JsonOfGraph(entry.second);
      }
      return out;
    }
    default:
      // Nodes, relationships, paths, temporal values: their proto3 JSON.
      return ValueFromJson(ToJson(value));
  }
}

}  // namespace

std::vector<std::map<std::string, JsonValue>> GraphModule::Records(const graph_v1::ExecuteResponse& response) {
  std::vector<std::map<std::string, JsonValue>> out;
  const auto& columns = response.rows().columns();
  for (const auto& row : response.rows().rows()) {
    std::map<std::string, JsonValue> record;
    for (int i = 0; i < columns.size(); ++i) {
      record[columns.Get(i)] = i < row.values_size() ? JsonOfGraph(row.values(i)) : JsonNull();
    }
    out.push_back(std::move(record));
  }
  return out;
}

// --- Raw calls ---------------------------------------------------------------------------------------

namespace {

/// The RPC behind every facade-index name the raw path accepts.
const std::map<std::string, std::string>& FacadeCalls() {
  static const std::map<std::string, std::string> calls = {
      {"instance.get_instance", "loams.instance.v1.InstanceService/GetInstance"},
      {"instance.who_am_i", "loams.instance.v1.InstanceService/WhoAmI"},
      {"namespaces.create", kCreateNamespace},
      {"namespaces.list", kListNamespaces},
      {"collections.create", kCreateCollection},
      {"collections.list", kListCollections},
      {"collections.get", kGetCollection},
      {"collections.drop", kDropCollection},
      {"collections.add_fields", kAddFields},
      {"collections.list_versions", kListVersions},
      {"collections.update_aliases", kUpdateAliases},
      {"collections.scan", kScan},
      {"collections.set_hot", kSetHot},
      {"collections.warm", kWarm},
      {"documents.write", kWrite},
      {"documents.get", kGet},
      {"documents.scroll", kScroll},
      {"documents.count", kCount},
      {"documents.delete_by_filter", kDeleteByFilter},
      {"documents.patch_by_filter", kPatchByFilter},
      {"search.query", kSearch},
      {"vector.search", kSearch},
      {"graph.create", kCreateGraph},
      {"graph.get", kGetGraph},
      {"graph.list", kListGraphs},
      {"graph.update", kUpdateGraph},
      {"graph.delete", kDeleteGraph},
      {"graph.schema", kGetSchema},
      {"graph.query", kExecute},
      {"graph.query_batch", kExecuteBatch},
      {"graph.explain", kExplain},
      {"sql.query", "loams.sql.v1.SqlService/Query"},
      {"sql.query_arrow", "loams.sql.v1.SqlService/QueryArrow"},
      {"streams.create", "loams.stream.v1.StreamService/CreateStream"},
      {"streams.get", "loams.stream.v1.StreamService/DescribeStream"},
      {"streams.list", "loams.stream.v1.StreamService/ListStreams"},
      {"streams.produce", "loams.stream.v1.StreamService/Produce"},
      {"streams.fetch", "loams.stream.v1.StreamService/Fetch"},
      {"streams.produce_events", "loams.stream.v1.StreamService/ProduceCloudEvents"},
      {"streams.fetch_events", "loams.stream.v1.StreamService/FetchCloudEvents"},
  };
  return calls;
}

}  // namespace

std::string RawModule::Call(const std::string& name, const std::string& request_json, const CallOptions& options) {
  // A generated file registers its descriptors when its object is linked, and a
  // static library links only the objects something references: naming one
  // message of each app package here is what puts them in the generated pool.
  static const bool linked = [] {
    return collection_v1::SearchRequest::descriptor() != nullptr && graph_v1::ExecuteRequest::descriptor() != nullptr &&
           ::loams::sql::v1::QueryRequest::descriptor() != nullptr &&
           ::loams::stream::v1::ProduceRequest::descriptor() != nullptr &&
           ::loams::instance::v1::GetInstanceRequest::descriptor() != nullptr;
  }();
  (void)linked;
  const auto found = FacadeCalls().find(name);
  if (found == FacadeCalls().end()) {
    Invalid("", "no facade call " + name);
  }
  const std::string& rpc = found->second;
  const std::string service = rpc.substr(0, rpc.find('/'));
  const auto* method = google::protobuf::DescriptorPool::generated_pool()->FindMethodByName(
      service + "." + rpc.substr(rpc.find('/') + 1));
  if (method == nullptr) {
    Invalid(rpc, "no generated descriptor for " + rpc);
  }
  auto* factory = google::protobuf::MessageFactory::generated_factory();
  std::unique_ptr<Message> request(factory->GetPrototype(method->input_type())->New());
  std::unique_ptr<Message> response(factory->GetPrototype(method->output_type())->New());
  if (!google::protobuf::util::JsonStringToMessage(request_json.empty() ? "{}" : request_json, request.get()).ok()) {
    Invalid(rpc, rpc + ": the request is not a " + std::string(method->input_type()->full_name()));
  }
  // A read-only graph statement is a read (see GraphModule::Query).
  const auto* read_only = request->GetDescriptor()->FindFieldByName("read_only");
  if (rpc == kExecute && read_only != nullptr && request->GetReflection()->GetBool(*request, read_only)) {
    MethodBinding binding = BindingFor(kExecute);
    binding.retry_class = RetryClass::kSafe;
    binding.idempotency_field.clear();
    AppModule::Call(rpc, request.get(), response.get(), options, &binding);
  } else if (ScopeOf(rpc).rfind("loams.instance.", 0) == 0) {
    client_->Unary(BindingFor(rpc), *request, response.get(), client_->Encoding(), options.idempotency_key,
                   options.headers, options.timeout);
  } else {
    AppModule::Call(rpc, request.get(), response.get(), options);
  }
  return ToJson(*response);
}

// --- The collection handle -------------------------------------------------------------------------

CallOptions CollectionHandle::With(CallOptions options) const {
  if (options.ns.empty()) {
    options.ns = ns_;
  }
  return options;
}

collection_v1::WriteDocumentsResponse CollectionHandle::Upsert(const std::vector<Document>& docs, CallOptions options) {
  return modules_->documents.Upsert(name_, docs, With(std::move(options)));
}

collection_v1::GetDocumentsResponse CollectionHandle::Get(const std::vector<DocId>& ids, const ReadOptions& read,
                                                          CallOptions options) {
  return modules_->documents.Get(name_, ids, read, With(std::move(options)));
}

collection_v1::WriteDocumentsResponse CollectionHandle::Delete(const std::vector<DocId>& ids, CallOptions options) {
  return modules_->documents.Delete(name_, ids, With(std::move(options)));
}

collection_v1::WriteDocumentsResponse CollectionHandle::Patch(const DocId& id, const PatchChange& change,
                                                              CallOptions options) {
  return modules_->documents.Patch(name_, id, change, With(std::move(options)));
}

BatchResult CollectionHandle::WriteBatch(const std::vector<Document>& docs, const BatchOptions& batch,
                                         CallOptions options) {
  return modules_->documents.WriteBatch(name_, docs, batch, With(std::move(options)));
}

collection_v1::CountDocumentsResponse CollectionHandle::Count(const DocumentFilter& match, const ReadOptions& read,
                                                              CallOptions options) {
  return modules_->documents.Count(name_, match, read, With(std::move(options)));
}

std::vector<collection_v1::Document> CollectionHandle::ScrollAll(const ScrollOptions& scroll, CallOptions options) {
  return modules_->documents.ScrollAll(name_, scroll, With(std::move(options)));
}

collection_v1::SearchResponse CollectionHandle::Search(::loams::Search search, CallOptions options) {
  search.Collection(name_);
  return modules_->search.Query(search, With(std::move(options)));
}

collection_v1::SearchResponse CollectionHandle::VectorSearch(const std::vector<float>& vector, const std::string& field,
                                                             const SearchShorthand& shorthand, CallOptions options) {
  return modules_->vector.Search(name_, vector, field, shorthand, With(std::move(options)));
}

collection_v1::DropCollectionResponse CollectionHandle::Drop(CallOptions options) {
  return modules_->collections.Drop(name_, With(std::move(options)));
}

collection_v1::CollectionInfo CollectionHandle::Info(CallOptions options) {
  return modules_->collections.Get(name_, With(std::move(options)));
}

}  // namespace loams
