// The HTTP layer: what the transport interface is, and the one implementation
// this SDK ships (libcurl).
//
// The wire is **Connect over HTTP**, not gRPC (design §44 §4, D600): one port
// serves the Connect protocol, gRPC and gRPC-Web, and the C++ SDK speaks the
// two HTTP-shaped ones. That is why there is no grpc++ dependency: a C++ client
// that needs a full gRPC stack to make an HTTP POST is a heavier install for
// the same bytes.
//
// The interface exists for two reasons, both load-bearing:
//
//   - a test can substitute a transport and assert on the request the SDK
//     would have sent, byte for byte, without a server;
//   - a C++ program that already links libcurl (or already has an HTTP/2 stack
//     of its own) can supply it, and this SDK's runtime is then free of any
//     HTTP dependency at all.
//
// Every request has a **timeout**. A client that can wait forever is a client
// whose thread never comes back, and `Options::timeout` is the only thing
// standing between a hung server and a hung process.

#ifndef LOAMS_HTTP_HPP
#define LOAMS_HTTP_HPP

#include <chrono>
#include <cstddef>
#include <memory>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace loams {

/// One HTTP request. Only what the Connect protocol needs: a method, a URL,
/// headers, and a body. There is no multipart and no form encoding, because the
/// wire has neither.
struct HttpRequest {
  /// `POST` for every Connect and gRPC-Web call, including the ones a read
  /// could use `GET` for. A read *may* be a `GET`, but a client that has to
  /// choose per call and get it wrong is worse than one that always sends
  /// `POST`, which every server accepts.
  std::string method = "POST";
  /// The absolute URL, without a query string. Tokens never travel in one.
  std::string url;
  /// Request headers, in order. Names are sent as given; libcurl's `CURLOPT_HTTPHEADER`
  /// list is case-insensitive and the server is too.
  std::vector<std::pair<std::string, std::string>> headers;
  /// The request body, already framed if the encoding frames.
  std::string body;
  /// How long one attempt may take. Zero means "no limit", which is only right
  /// for a caller that has its own deadline.
  std::chrono::milliseconds timeout{20000};
};

/// One HTTP response. The body is **always** fully read: for a unary call that
/// is the whole point, and for a stream `ByteReader` below hands it over as it
/// arrives.
struct HttpResponse {
  /// The HTTP status. **Not** the error: Connect answers a failed RPC with 501
  /// and a JSON body, and gRPC-Web answers 200 with the code in the trailers.
  /// A client that reads only this sees a successful gRPC-Web call fail.
  long status = 0;
  /// Response headers, lower-cased names, in order.
  std::vector<std::pair<std::string, std::string>> headers;
  /// The response body.
  std::string body;

  /// The first value of a header, matched case-insensitively. Empty when the
  /// header is absent — which is different from a header that is present and
  /// empty, and the two are treated differently everywhere they matter (a
  /// gRPC-Web `grpc-status: 0` is present-and-empty-means-absent, while
  /// `grpc-status: 0` is a real value).
  std::string Header(std::string_view name) const;
};

/// A response body that arrives in pieces, which is what a server stream is.
///
/// `Next` returns false at end of body **or** on failure, and `Error` says
/// which: a null `error` means the body ended, a set one means the transfer
/// broke. That distinction is the whole contract of the iterator — a loop over
/// `Next` alone cannot tell "done" from "broken", and a caller that ignores
/// `Error` sees a silently truncated stream.
class ByteReader {
 public:
  virtual ~ByteReader() = default;

  /// Reads the next piece of the body. Blocks until one arrives, the body ends,
  /// or the transfer fails.
  ///
  /// `into` is appended to, never cleared, so a caller that wants the whole body
  /// as one string can concatenate and a caller that wants frames can parse
  /// incrementally. `*at_eof` is set when the body has ended.
  ///
  /// Returns false when the body has ended or the transfer failed. On failure
  /// `*error` holds a `std::exception_ptr` and the reader is spent.
  virtual bool Next(std::string* into, bool* at_eof, std::exception_ptr* error) = 0;

  /// The HTTP status, available once the response headers have arrived. Reading
  /// it before the first `Next` is a caller bug that this cannot detect, so it
  /// is documented rather than enforced.
  virtual long Status() const = 0;

  /// The response headers, lower-cased names. Available once the headers have
  /// arrived, which is before the body starts.
  virtual const std::vector<std::pair<std::string, std::string>>& Headers() const = 0;

  /// Releases the connection. Safe to call more than once, and after the body
  /// has ended. Every `Open` must be paired with a `Close`, or the connection
  /// is left in the pool forever.
  virtual void Close() = 0;
};

/// How the SDK reaches a server.
class HttpTransport {
 public:
  virtual ~HttpTransport() = default;

  /// Sends one request and reads the whole response. Throws
  /// `loams::TransportError` on a failure below the API: a refused connection,
  /// a DNS failure, a timeout. It never throws for an HTTP error status — a 501
  /// is an answer, and the error mapper is what turns it into a typed failure.
  virtual HttpResponse Send(const HttpRequest& request) = 0;

  /// Opens a request and returns a reader for its body, arriving as it comes.
  /// Throws `loams::TransportError` on a failure below the API.
  virtual std::unique_ptr<ByteReader> Open(const HttpRequest& request) = 0;
};

/// The libcurl transport. `curl-licence`-licensed (MIT-like), and the reason it
/// is here is in `DEPENDENCIES.md`.
///
/// One `CURL` easy handle per call: handles are not thread-safe, and one per
/// call costs a microsecond. Connections are still reused, because libcurl's
/// share or its connection cache is what makes that possible.
std::shared_ptr<HttpTransport> MakeCurlTransport();

}  // namespace loams

#endif  // LOAMS_HTTP_HPP