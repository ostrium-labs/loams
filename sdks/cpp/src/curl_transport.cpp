// The libcurl transport.
//
// One easy handle per call, which is why this file is as small as it is: a
// handle is not thread-safe, so sharing one across threads would need a lock
// around every byte on the wire. Connections are still reused, through libcurl's
// connection cache keyed on the URL, which is what makes "one handle per call"
// cost a microsecond rather than a TCP handshake.
//
// A server stream needs the body **as it arrives**, so `Open` runs the transfer
// on its own thread and hands pieces to the reader through a bounded queue. The
// bound is the point: an unbounded queue on a watch stream that produces messages
// faster than a caller reads them would grow until the process died, and a
// bounded one turns that into a slow reader instead.
//
// `curl-licence`-licensed (MIT-like). Why it is a dependency at all is in
// `DEPENDENCIES.md`.

#include <curl/curl.h>

#include <cstdlib>

#include <condition_variable>
#include <deque>
#include <mutex>
#include <thread>

#include "loams/error.hpp"
#include "loams/http.hpp"

namespace loams {
namespace {

/// The user agent, read once. `curl_easy_setopt` copies the string, so the
/// `std::string` has to outlive the call — a function-local static does.
const std::string& UserAgentString() {
  static const std::string* const agent = new std::string("loams-cpp/0.1.0 (Connect over HTTP)");
  return *agent;
}

std::string Lower(std::string value) {
  for (char& character : value) {
    if (character >= 'A' && character <= 'Z') {
      character = static_cast<char>(character - 'A' + 'a');
    }
  }
  return value;
}

std::string Trim(std::string value) {
  while (!value.empty() && (value.back() == ' ' || value.back() == '\t')) {
    value.pop_back();
  }
  std::size_t start = 0;
  while (start < value.size() && (value[start] == ' ' || value[start] == '\t')) {
    ++start;
  }
  return value.substr(start);
}

/// One request in flight on its own thread, feeding a bounded queue.
class CurlReader final : public ByteReader {
 public:
  CurlReader() {
    handle_ = curl_easy_init();
    if (handle_ == nullptr) {
      ThrowInternal("", "loams: curl_easy_init returned nothing, so no request can be sent");
    }
  }

  ~CurlReader() override { Close(); }

  CurlReader(const CurlReader&) = delete;
  CurlReader& operator=(const CurlReader&) = delete;

  void Start(const HttpRequest& request) {
    // **Copies**, because libcurl does not: `CURLOPT_URL` and `CURLOPT_POSTFIELDS`
    // keep the pointer and read it when the transfer runs — on this reader's worker
    // thread, long after the caller's `HttpRequest` has gone out of scope. A pointer
    // into a destroyed local is a truncated upload, which is why every server stream
    // arrived as a partial body.
    url_ = request.url;
    body_ = request.body;
    curl_easy_setopt(handle_, CURLOPT_URL, url_.c_str());
    curl_easy_setopt(handle_, CURLOPT_FOLLOWLOCATION, 0L);
    curl_easy_setopt(handle_, CURLOPT_NOSIGNAL, 1L);
    curl_easy_setopt(handle_, CURLOPT_WRITEFUNCTION, &CurlReader::Write);
    curl_easy_setopt(handle_, CURLOPT_WRITEDATA, this);
    curl_easy_setopt(handle_, CURLOPT_HEADERFUNCTION, &CurlReader::Header);
    curl_easy_setopt(handle_, CURLOPT_HEADERDATA, this);
    curl_easy_setopt(handle_, CURLOPT_USERAGENT, UserAgentString().c_str());
    if (request.method == "POST") {
      curl_easy_setopt(handle_, CURLOPT_POST, 1L);
    } else {
      curl_easy_setopt(handle_, CURLOPT_CUSTOMREQUEST, request.method.c_str());
    }
    // `POSTFIELDS` with `POSTFIELDSIZE_LARGE` rather than `POSTFIELDSIZE`: the
    // latter takes a `long`, which is 32 bits on some platforms and would
    // truncate a 4 MiB bulk write to a size the server then rejects.
    curl_easy_setopt(handle_, CURLOPT_POSTFIELDS, body_.data());
    curl_easy_setopt(handle_, CURLOPT_POSTFIELDSIZE_LARGE, static_cast<curl_off_t>(body_.size()));
    if (request.timeout.count() > 0) {
      curl_easy_setopt(handle_, CURLOPT_TIMEOUT_MS, static_cast<long>(request.timeout.count()));
    }

    struct curl_slist* list = nullptr;
    for (const auto& header : request.headers) {
      list = curl_slist_append(list, (header.first + ": " + header.second).c_str());
    }
    if (list != nullptr) {
      curl_easy_setopt(handle_, CURLOPT_HTTPHEADER, list);
      owned_headers_ = list;
    }

    worker_ = std::thread([this] { Run(); });
  }

  bool Next(std::string* into, bool* at_eof, std::exception_ptr* error) override {
    *at_eof = false;
    std::unique_lock<std::mutex> lock(mutex_);
    available_.wait(lock, [this] { return !queue_.empty() || finished_ || aborted_; });
    if (!queue_.empty()) {
      into->append(std::move(queue_.front()));
      queue_.pop_front();
      lock.unlock();
      not_full_.notify_one();
      return true;
    }
    if (failed_) {
      *error = failure_;
      return false;
    }
    // The queue is empty and the transfer is over: a clean end.
    *at_eof = true;
    return false;
  }

  long Status() const override {
    std::lock_guard<std::mutex> const guard(mutex_);
    return status_;
  }

  const std::vector<std::pair<std::string, std::string>>& Headers() const override { return headers_; }

  void Close() override {
    {
      std::lock_guard<std::mutex> const guard(mutex_);
      if (closed_) {
        return;
      }
      closed_ = true;
      aborted_ = true;
    }
    not_full_.notify_all();
    available_.notify_all();
    if (worker_.joinable()) {
      worker_.join();
    }
    if (owned_headers_ != nullptr) {
      curl_slist_free_all(owned_headers_);
      owned_headers_ = nullptr;
    }
    if (handle_ != nullptr) {
      curl_easy_cleanup(handle_);
      handle_ = nullptr;
    }
  }

 private:
  static std::size_t Write(char* data, std::size_t size, std::size_t count, void* self) {
    auto* const reader = static_cast<CurlReader*>(self);
    const std::size_t bytes = size * count;
    std::unique_lock<std::mutex> lock(reader->mutex_);
    // Bounded: a reader slower than the server must not turn into unbounded
    // memory growth. Waiting here is the backpressure, and `Close` sets
    // `aborted_` and notifies so a caller giving up is never stuck behind it.
    reader->not_full_.wait(lock, [reader] { return reader->aborted_ || reader->queue_.size() < 64; });
    if (reader->aborted_) {
      // Returning a short count tells libcurl to abort the transfer.
      return 0;
    }
    reader->queue_.emplace_back(data, bytes);
    lock.unlock();
    reader->available_.notify_one();
    return bytes;
  }

  static std::size_t Header(char* data, std::size_t size, std::size_t count, void* self) {
    auto* const reader = static_cast<CurlReader*>(self);
    const std::size_t bytes = size * count;
    const std::string line(data, bytes);
    const std::size_t colon = line.find(':');
    if (colon != std::string::npos) {
      std::lock_guard<std::mutex> const guard(reader->mutex_);
      reader->headers_.emplace_back(Lower(Trim(line.substr(0, colon))), Trim(line.substr(colon + 1)));
      if (reader->status_ == 0 && reader->headers_.size() == 1) {
        // The status line is the first header block's first line. Parsed from
        // the block rather than through `CURLINFO_RESPONSE_CODE`, because the
        // status is needed as soon as the headers arrive, and `CURLINFO` is only
        // readable once the transfer ends.
        const char* const begin = line.c_str();
        char* end = nullptr;
        const long parsed = std::strtol(begin, &end, 10);
        if (end != begin) {
          reader->status_ = parsed;
        }
      }
    }
    return bytes;
  }

  void Run() {
    const CURLcode code = curl_easy_perform(handle_);
    long status = 0;
    curl_easy_getinfo(handle_, CURLINFO_RESPONSE_CODE, &status);
    {
      std::lock_guard<std::mutex> const guard(mutex_);
        if (code != CURLE_OK) {
        // Not every curl failure is retryable, and only the retry policy's three
        // codes are: a name that does not resolve will not resolve on the third
        // attempt either. Everything below the API is `kUnknown` with a message,
        // which R8's "a failure from below the API carries no reason" covers.
        failure_ = std::make_exception_ptr(TransportError(rpc_, std::string("loams: ") + curl_easy_strerror(code)));
        failed_ = true;
      } else if (status_ == 0) {
        status_ = status;
      }
      finished_ = true;
    }
    // The reader waits on `!queue_.empty() || finished_ || aborted_`, so the
    // transition to `finished_` has to wake it. Without this notify the reader
    // drains the last piece and then blocks for ever: every call hung, and only
    // the ones whose body arrived in several writes made any progress at all,
    // which reads exactly like a slow server rather than like a lost wake-up.
    available_.notify_all();
  }

  CURL* handle_ = nullptr;
  struct curl_slist* owned_headers_ = nullptr;
  std::thread worker_;

  mutable std::mutex mutex_;
  std::condition_variable available_;
  std::condition_variable not_full_;
  std::deque<std::string> queue_;
  std::vector<std::pair<std::string, std::string>> headers_;
  std::string url_;
  std::string body_;
  std::string rpc_;
  std::exception_ptr failure_;
  long status_ = 0;
  bool finished_ = false;
  bool failed_ = false;
  bool aborted_ = false;
  bool closed_ = false;
};

class CurlTransport final : public HttpTransport {
 public:
  HttpResponse Send(const HttpRequest& request) override {
    // Read to the end rather than taking the first piece: a unary answer is
    // whatever the server sent, and stopping at the first write callback would be
    // a truncation shaped like a short response.
    std::unique_ptr<ByteReader> reader = Open(request);
    HttpResponse response;
    bool at_eof = false;
    std::exception_ptr error;
    while (reader->Next(&response.body, &at_eof, &error)) {
      // Appended to `response.body` by `Next`; the loop is the read.
    }
    if (error) {
      std::rethrow_exception(error);
    }
    response.status = reader->Status();
    response.headers = reader->Headers();
    return response;
  }

  std::unique_ptr<ByteReader> Open(const HttpRequest& request) override {
    auto reader = std::make_unique<CurlReader>();
    reader->Start(request);
    return reader;
  }
};

}  // namespace

std::string HttpResponse::Header(std::string_view name) const {
  const std::string wanted = Lower(std::string(name));
  for (const auto& header : headers) {
    if (header.first == wanted) {
      return header.second;
    }
  }
  return std::string();
}

std::shared_ptr<HttpTransport> MakeCurlTransport() {
  // `curl_global_init` is not thread-safe and must happen exactly once. A
  // function-local static gives that for free, in C++11 and later: the
  // initialisation runs on the first thread that reaches it and the others wait.
  static const int initialised = curl_global_init(CURL_GLOBAL_DEFAULT);
  if (initialised != CURLE_OK) {
    ThrowInternal("", "loams: curl_global_init failed, so no HTTP transport can be built");
  }
  return std::make_shared<CurlTransport>();
}

}  // namespace loams