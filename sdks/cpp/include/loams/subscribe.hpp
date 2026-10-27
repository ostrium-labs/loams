// R14: `streams.subscribe`, a loop over unary `Fetch` (SDKP-2), so it works on
// any transport and through any proxy.
//
// A pull reader: `Next()` answers the next record, fetching when the buffer is
// empty. The next offset is the last record's offset + 1. An empty fetch backs
// off 100 ms, doubling up to 1 s; a non-empty one resets it. `Offset()` is where
// the next fetch starts, so a caller can resume from it, and `Cancel()` makes
// `Next()` answer nothing from then on (from any thread).
//
// The `streams` module binds it to `StreamService/Fetch` when that RPC is on the
// main port (SDKP P0c); until then it is the generic loop over any fetch.

#ifndef LOAMS_SUBSCRIBE_HPP
#define LOAMS_SUBSCRIBE_HPP

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cstdint>
#include <deque>
#include <functional>
#include <optional>
#include <thread>
#include <utility>
#include <vector>

namespace loams {

template <typename Record>
class Subscription {
 public:
  /// The records at and after an offset.
  using Fetch = std::function<std::vector<Record>(std::uint64_t offset)>;
  /// A record's offset.
  using OffsetOf = std::function<std::uint64_t(const Record&)>;
  using Sleep = std::function<void(std::chrono::milliseconds)>;

  static constexpr std::chrono::milliseconds kInitialBackoff{100};
  static constexpr std::chrono::milliseconds kMaxBackoff{1000};

  Subscription(Fetch fetch, OffsetOf offset_of, std::uint64_t from_offset = 0, Sleep sleep = {})
      : fetch_(std::move(fetch)),
        offset_of_(std::move(offset_of)),
        offset_(from_offset),
        sleep_(sleep ? std::move(sleep) : Sleep([](std::chrono::milliseconds d) { std::this_thread::sleep_for(d); })) {}

  /// The next record, or nothing once cancelled.
  std::optional<Record> Next() {
    auto backoff = kInitialBackoff;
    while (!cancelled_.load()) {
      if (!buffer_.empty()) {
        Record record = std::move(buffer_.front());
        buffer_.pop_front();
        offset_ = offset_of_(record) + 1;
        return record;
      }
      std::vector<Record> records = fetch_(offset_);
      if (records.empty()) {
        sleep_(backoff);
        backoff = std::min(backoff * 2, kMaxBackoff);
        continue;
      }
      backoff = kInitialBackoff;
      for (auto& record : records) {
        buffer_.push_back(std::move(record));
      }
    }
    return std::nullopt;
  }

  /// Where the next fetch starts: the last record's offset + 1.
  std::uint64_t Offset() const { return offset_; }

  void Cancel() { cancelled_.store(true); }
  bool Cancelled() const { return cancelled_.load(); }

 private:
  Fetch fetch_;
  OffsetOf offset_of_;
  std::uint64_t offset_;
  Sleep sleep_;
  std::deque<Record> buffer_;
  std::atomic<bool> cancelled_{false};
};

}  // namespace loams

#endif  // LOAMS_SUBSCRIBE_HPP
