// The app-building `sql` and `streams` modules (SDKP §2.5, part C).

#ifndef LOAMS_DATA_HPP
#define LOAMS_DATA_HPP

#include <cstdint>
#include <map>
#include <optional>
#include <string>
#include <vector>

#include "loams/app.hpp"
#include "loams/sql/v1/sql.pb.h"
#include "loams/stream/v1/stream.pb.h"
#include "loams/subscribe.hpp"

namespace loams {

namespace sql_v1 = ::loams::sql::v1;
namespace stream_v1 = ::loams::stream::v1;

struct SqlOptions {
  std::optional<std::uint32_t> max_rows;
  /// `strong`, `eventual` or a token; empty is the session's (or the server's) default.
  std::string consistency;
};

/// `sql`: one read-only statement over a namespace's collections, with `$1…$n`
/// parameters. SQL reads see the session's collection writes.
class SqlModule : public AppModule {
 public:
  using AppModule::AppModule;
  /// The answer's `rows` are lists in `columns` order; see `Records`.
  sql_v1::QueryResponse Query(const std::string& sql, const std::vector<JsonValue>& params = {},
                              const SqlOptions& sql_options = {}, const CallOptions& options = {});
  /// The same statement, answered as Arrow IPC bytes.
  std::string QueryArrow(const std::string& sql, const std::vector<JsonValue>& params = {},
                         const SqlOptions& sql_options = {}, const CallOptions& options = {});
  /// A `QueryResponse`'s rows, column name to value.
  static std::vector<std::map<std::string, JsonValue>> Records(const sql_v1::QueryResponse& response);
};

/// One record to produce. The key is optional (SDKP-25).
struct ProduceRecord {
  std::optional<std::string> key;
  std::optional<std::string> value;
  std::map<std::string, std::string> headers;
};

struct ProduceOptions {
  std::uint32_t partition = 0;
  std::string producer_id;
  std::optional<std::uint64_t> producer_seq;
};

struct FetchOptions {
  std::uint32_t partition = 0;
  std::uint64_t offset = 0;
  std::optional<std::uint32_t> max_records;
  std::optional<std::uint32_t> max_bytes;
  std::optional<std::uint32_t> max_wait_ms;
};

/// `streams`: a namespace's logs. Records are bytes (`std::string`).
class StreamsModule : public AppModule {
 public:
  using AppModule::AppModule;
  stream_v1::CreateStreamResponse Create(const std::string& name, std::uint32_t partitions = 1,
                                         const CallOptions& options = {});
  stream_v1::DescribeStreamResponse Get(const std::string& stream, const CallOptions& options = {});
  stream_v1::ListStreamsResponse List(std::int32_t page_size = 0, const std::string& page_token = "",
                                      const CallOptions& options = {});
  std::vector<stream_v1::StreamInfo> ListAll(std::int32_t page_size = 0, const CallOptions& options = {});
  /// Appends `records` to one partition. With `producer_id` and `producer_seq` a
  /// repeat is deduplicated (`duplicate`). Never retried.
  stream_v1::ProduceResponse Produce(const std::string& stream, const std::vector<ProduceRecord>& records,
                                     const ProduceOptions& produce = {}, const CallOptions& options = {});
  /// Records at and after `fetch.offset`.
  stream_v1::FetchResponse Fetch(const std::string& stream, const FetchOptions& fetch = {},
                                 const CallOptions& options = {});
  /// CloudEvents as one JSON array (text), appended as a batch.
  stream_v1::ProduceCloudEventsResponse ProduceEvents(const std::string& stream, const std::string& json_batch,
                                                      std::optional<std::uint32_t> partition = {},
                                                      const CallOptions& options = {});
  /// CloudEvents at and after `fetch.offset`, as one JSON array (text).
  std::string FetchEvents(const std::string& stream, const FetchOptions& fetch = {}, const CallOptions& options = {});
  /// R14: every record from `fetch.offset` on, as a pull reader over `Fetch`.
  /// A fetch that fails stops the subscription.
  Subscription<stream_v1::FetchedRecord> Subscribe(const std::string& stream, FetchOptions fetch = {},
                                                   CallOptions options = {});
};

}  // namespace loams

#endif  // LOAMS_DATA_HPP
