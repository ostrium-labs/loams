// The `sql` and `streams` modules, as `data.hpp` documents them.

#include "loams/data.hpp"

namespace loams {
namespace {

constexpr char kSqlQuery[] = "loams.sql.v1.SqlService/Query";
constexpr char kSqlQueryArrow[] = "loams.sql.v1.SqlService/QueryArrow";
constexpr char kCreateStream[] = "loams.stream.v1.StreamService/CreateStream";
constexpr char kDescribeStream[] = "loams.stream.v1.StreamService/DescribeStream";
constexpr char kListStreams[] = "loams.stream.v1.StreamService/ListStreams";
constexpr char kProduce[] = "loams.stream.v1.StreamService/Produce";
constexpr char kFetch[] = "loams.stream.v1.StreamService/Fetch";
constexpr char kProduceEvents[] = "loams.stream.v1.StreamService/ProduceCloudEvents";
constexpr char kFetchEvents[] = "loams.stream.v1.StreamService/FetchCloudEvents";

template <typename Request>
void FillSql(Request* request, const std::string& sql, const std::vector<JsonValue>& params, const SqlOptions& options) {
  request->set_sql(sql);
  for (const JsonValue& param : params) {
    *request->add_params() = param;
  }
  if (options.max_rows) request->set_max_rows(*options.max_rows);
  if (!options.consistency.empty()) *request->mutable_consistency() = ConsistencyOf(options.consistency);
}

template <typename Request>
void FillFetch(Request* request, const std::string& stream, const FetchOptions& fetch) {
  request->set_stream(stream);
  request->set_partition(fetch.partition);
  request->set_offset(fetch.offset);
  if (fetch.max_bytes) request->set_max_bytes(*fetch.max_bytes);
  if (fetch.max_wait_ms) request->set_max_wait_ms(*fetch.max_wait_ms);
}

}  // namespace

sql_v1::QueryResponse SqlModule::Query(const std::string& sql, const std::vector<JsonValue>& params,
                                       const SqlOptions& sql_options, const CallOptions& options) {
  sql_v1::QueryRequest request;
  FillSql(&request, sql, params, sql_options);
  sql_v1::QueryResponse response;
  Call(kSqlQuery, &request, &response, options);
  return response;
}

std::string SqlModule::QueryArrow(const std::string& sql, const std::vector<JsonValue>& params,
                                  const SqlOptions& sql_options, const CallOptions& options) {
  sql_v1::QueryArrowRequest request;
  FillSql(&request, sql, params, sql_options);
  sql_v1::QueryArrowResponse response;
  Call(kSqlQueryArrow, &request, &response, options);
  return response.arrow_ipc();
}

std::vector<std::map<std::string, JsonValue>> SqlModule::Records(const sql_v1::QueryResponse& response) {
  std::vector<std::map<std::string, JsonValue>> out;
  for (const auto& row : response.rows()) {
    std::map<std::string, JsonValue> record;
    for (int i = 0; i < response.columns_size(); ++i) {
      record[response.columns(i).name()] = i < row.values_size() ? row.values(i) : JsonNull();
    }
    out.push_back(std::move(record));
  }
  return out;
}

stream_v1::CreateStreamResponse StreamsModule::Create(const std::string& name, std::uint32_t partitions,
                                                      const CallOptions& options) {
  stream_v1::CreateStreamRequest request;
  request.set_name(name);
  request.set_partitions(partitions);
  stream_v1::CreateStreamResponse response;
  Call(kCreateStream, &request, &response, options);
  return response;
}

stream_v1::DescribeStreamResponse StreamsModule::Get(const std::string& stream, const CallOptions& options) {
  stream_v1::DescribeStreamRequest request;
  request.set_stream(stream);
  stream_v1::DescribeStreamResponse response;
  Call(kDescribeStream, &request, &response, options);
  return response;
}

stream_v1::ListStreamsResponse StreamsModule::List(std::int32_t page_size, const std::string& page_token,
                                                   const CallOptions& options) {
  stream_v1::ListStreamsRequest request;
  request.set_page_size(page_size);
  request.set_page_token(page_token);
  stream_v1::ListStreamsResponse response;
  Call(kListStreams, &request, &response, options);
  return response;
}

std::vector<stream_v1::StreamInfo> StreamsModule::ListAll(std::int32_t page_size, const CallOptions& options) {
  std::vector<stream_v1::StreamInfo> out;
  std::string token;
  do {
    const auto page = List(page_size, token, options);
    out.insert(out.end(), page.streams().begin(), page.streams().end());
    if (page.next_page_token() == token) break;
    token = page.next_page_token();
  } while (!token.empty());
  return out;
}

stream_v1::ProduceResponse StreamsModule::Produce(const std::string& stream, const std::vector<ProduceRecord>& records,
                                                  const ProduceOptions& produce, const CallOptions& options) {
  stream_v1::ProduceRequest request;
  request.set_stream(stream);
  request.set_partition(produce.partition);
  for (const ProduceRecord& record : records) {
    auto* out = request.add_records();
    if (record.key) out->set_key(*record.key);
    if (record.value) out->set_value(*record.value);
    for (const auto& header : record.headers) {
      auto* h = out->add_headers();
      h->set_key(header.first);
      h->set_value(header.second);
    }
  }
  if (!produce.producer_id.empty()) request.set_producer_id(produce.producer_id);
  if (produce.producer_seq) request.set_producer_seq(*produce.producer_seq);
  stream_v1::ProduceResponse response;
  Call(kProduce, &request, &response, options);
  return response;
}

stream_v1::FetchResponse StreamsModule::Fetch(const std::string& stream, const FetchOptions& fetch,
                                              const CallOptions& options) {
  stream_v1::FetchRequest request;
  FillFetch(&request, stream, fetch);
  if (fetch.max_records) request.set_max_records(*fetch.max_records);
  stream_v1::FetchResponse response;
  Call(kFetch, &request, &response, options);
  return response;
}

stream_v1::ProduceCloudEventsResponse StreamsModule::ProduceEvents(const std::string& stream,
                                                                   const std::string& json_batch,
                                                                   std::optional<std::uint32_t> partition,
                                                                   const CallOptions& options) {
  stream_v1::ProduceCloudEventsRequest request;
  request.set_stream(stream);
  if (partition) request.set_partition(*partition);
  request.set_json_batch(json_batch);
  stream_v1::ProduceCloudEventsResponse response;
  Call(kProduceEvents, &request, &response, options);
  return response;
}

std::string StreamsModule::FetchEvents(const std::string& stream, const FetchOptions& fetch,
                                       const CallOptions& options) {
  stream_v1::FetchCloudEventsRequest request;
  FillFetch(&request, stream, fetch);
  stream_v1::FetchCloudEventsResponse response;
  Call(kFetchEvents, &request, &response, options);
  return response.json_batch();
}

Subscription<stream_v1::FetchedRecord> StreamsModule::Subscribe(const std::string& stream, FetchOptions fetch,
                                                                CallOptions options) {
  return Subscription<stream_v1::FetchedRecord>(
      [this, stream, fetch, options](std::uint64_t offset) {
        FetchOptions at = fetch;
        at.offset = offset;
        const stream_v1::FetchResponse response = Fetch(stream, at, options);
        return std::vector<stream_v1::FetchedRecord>(response.records().begin(), response.records().end());
      },
      [](const stream_v1::FetchedRecord& record) { return record.offset(); }, fetch.offset);
}

}  // namespace loams
