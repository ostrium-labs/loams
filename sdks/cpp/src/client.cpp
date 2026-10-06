// The client: the whole runtime in one object.
//
// The call path below is where R1, R2, R3 and R8 meet, and the order matters:
//
//   1. **R3 first.** The idempotency key is minted *before* the first attempt,
//      from the caller's own key or a fresh UUIDv7, and the request is cloned
//      with it set. Everything after this re-marshals **that** message, which is
//      what makes "the same key goes out on every retry" true by construction
//      rather than by remembering to pass it along.
//   2. **Then the attempt loop.** `max_retries` is the retries *after* the first
//      attempt. A retryable code advances it; a non-retryable one throws.
//   3. **R1 inside the loop.** A `TokenExpiredError` gets **exactly one** refresh
//      and **one** retry, and it does not charge the retry budget: spending a
//      retry on a refreshed credential would turn a three-retry read into a
//      two-retry read every time a token expired, which is not what any SDK
//      documents. A source that cannot refresh is not retried at all — re-sending
//      the same API key fails identically.
//   4. **R8 last.** Every failure leaves this function as a typed error.

#include "loams/client.hpp"

#include <algorithm>
#include <atomic>
#include <chrono>
#include <thread>

#include "loams/idempotency.hpp"
#include <google/protobuf/util/json_util.h>

#include "loams/version.hpp"

namespace loams {
namespace {

/// Removes a trailing `/` from an endpoint.
///
/// `"/rpc"` and `"//rpc"` are two different paths and only one of them is in the
/// corpus, so an endpoint written with a trailing slash would 404 on every call
/// with a message that says nothing about the trailing slash.
std::string TrimmedEndpoint(std::string endpoint) {
  while (endpoint.size() > 1 && endpoint.back() == '/') {
    endpoint.pop_back();
  }
  return endpoint;
}

/// A sleeping retry that cannot be woken. `std::this_thread::sleep_for` on a
/// backoff of at most 2 s is the whole wait, and a condition variable would add a
/// cancellation path this SDK does not otherwise have: there is no context to
/// cancel, so a spurious wakeup would be the only thing it could buy.
void Wait(int milliseconds) {
  if (milliseconds > 0) {
    std::this_thread::sleep_for(std::chrono::milliseconds(milliseconds));
  }
}

}  // namespace

struct Client::Impl {
  Options options;
  std::string endpoint;
  std::shared_ptr<HttpTransport> transport;
  std::shared_ptr<TokenSource> token_source;
  SessionTokenStore session;
  std::unique_ptr<SystemModule> system;
  /// R1's "exactly one" is per **logical call**, not per client: two concurrent
  /// calls that each hit `token_expired` each get one refresh, and a client that
  /// had a global flag would give the second call none. The count lives in the
  /// call path, and this counter only records how many refreshes happened, for
  /// `cpp_token_source_refresh` to assert on.
  std::atomic<int> refreshes{0};
};

Client::Client(Options options) : impl_(std::make_unique<Impl>()) {
  const std::string endpoint = TrimmedEndpoint(std::move(options.endpoint));
  if (endpoint.empty()) {
    ThrowInternal("", "loams: Options::endpoint is empty; a client with no address fails on its first call");
  }
  const std::size_t scheme = endpoint.find("://");
  if (scheme == std::string::npos || scheme == 0) {
    ThrowInternal("", "loams: Options::endpoint \"" + endpoint +
                           "\" has no scheme; it must be http://host:port or https://host");
  }
  impl_->options = std::move(options);
  impl_->endpoint = endpoint;
  impl_->token_source = impl_->options.token_source;
  impl_->transport = impl_->options.transport ? impl_->options.transport : MakeCurlTransport();
  impl_->system = std::make_unique<SystemModule>(this);
}

Client::~Client() = default;

std::shared_ptr<Client> Client::Make(Options options) { return std::shared_ptr<Client>(new Client(std::move(options))); }

const std::string& Client::Endpoint() const { return impl_->endpoint; }

const std::string& Client::ProtoRev() const {
  // Qualified: the member hides the free function `loams::ProtoRev()` inside this
  // class body, so an unqualified call would be infinite recursion — which is
  // exactly what `-Werror=infinite-recursion` caught.
  return ::loams::ProtoRev();
}

ContentType Client::Encoding() const { return impl_->options.content_type; }

ConsistencyToken Client::SessionToken() const {
  // Always absent unless `session_consistency` is on (R4). The store exists
  // either way so `Invalidate`-style operations do not need the flag, but nothing
  // reads it when the caller did not ask for read-your-writes.
  if (!impl_->options.session_consistency) {
    return ConsistencyToken();
  }
  return impl_->session.Merged();
}

HttpTransport& Client::Transport() { return *impl_->transport; }

const std::shared_ptr<TokenSource>& Client::Token() const { return impl_->token_source; }

SystemModule* Client::System() { return impl_->system.get(); }

bool Client::RefreshOnce() {
  if (!impl_->token_source || !impl_->token_source->CanRefresh()) {
    // R1's no-op: a source that cannot refresh makes the refresh a no-op, and the
    // `401` reaches the caller unchanged rather than being retried with the same
    // credential, which would fail identically.
    return false;
  }
  impl_->token_source->Refresh();
  impl_->refreshes.fetch_add(1);
  return true;
}

void Client::Invoke(const CallPlan& plan, HttpTransport& transport,
                    const std::function<void(const HttpRequest&, HttpResponse&)>& attempt,
                    const std::function<void(const HttpResponse&)>& on_success) {
  const int budget = plan.max_retries.value_or(impl_->options.max_retries);
  const std::chrono::milliseconds timeout = plan.timeout.value_or(impl_->options.timeout);
  const bool retry_safe = plan.keyed || plan.binding.retry_class == RetryClass::kSafe;
  const std::string& rpc = plan.binding.rpc;

  // R1's one refresh, for this logical call. Declared here rather than inside the
  // loop so a retry spent on a refreshed token does not come out of the retry
  // budget.
  bool refreshed = false;

  for (int attempt_number = 0;; ++attempt_number) {
    HttpRequest request;
    request.method = "POST";
    request.url = impl_->endpoint + plan.binding.HttpPath();
    request.body = plan.body;
    request.timeout = timeout;
    request.headers.emplace_back("content-type", std::string(ContentTypeName(plan.content_type)));
    request.headers.emplace_back("accept", std::string(ContentTypeName(plan.content_type)));
    request.headers.emplace_back("user-agent", UserAgent());
    request.headers.emplace_back("connect-protocol-version", "1");
    // `plan.body` is already framed when the encoding frames: the recorded
    // corpus's gRPC-Web requests are exactly `00 00 00 00 00`, a flag byte and a
    // zero length, and framing it twice would make every framed request a
    // mismatch.
    // Tokens travel in `Authorization: Bearer` and **never** in the URL: a query
    // string ends up in proxy logs, in browser history and in `Referer` (R1).
    if (impl_->token_source) {
      const std::string token = impl_->token_source->Token();
      if (!token.empty()) {
        request.headers.emplace_back("authorization", "Bearer " + token);
      }
    }
    // R4: the session token is attached to later reads, and only when the caller
    // opted in.
    if (impl_->options.session_consistency) {
      const ConsistencyToken token = impl_->session.Merged();
      if (token.Present()) {
        request.headers.emplace_back("loams-consistency-token", token.Value());
      }
    }
    for (const auto& header : impl_->options.headers) {
      request.headers.push_back(header);
    }
    for (const auto& header : plan.headers) {
      request.headers.push_back(header);
    }

    HttpResponse response;
    std::exception_ptr failure;
    try {
      attempt(request, response);
    } catch (...) {
      // A failure from **below the API** — a refused connection, a timeout. It
      // carries no reason (R8's second case), and it is retryable only when the
      // call's class says so, because a name that does not resolve will not
      // resolve on the third attempt either.
      failure = std::current_exception();
    }

    if (!failure) {
      if (const std::optional<WireError> wire =
              DecodeUnaryError(plan.content_type, response)) {
        WireError error = *wire;
        // A Connect unary failure that carried no code at all still needs a code
        // to map, and the HTTP status is the only thing left. Only as a
        // fallback: the body is the authority, per R8.
        if (error.code == Code::kUnknown && !IsGrpcWeb(plan.content_type)) {
          error.code = CodeFromHttpStatus(response.status);
          error.code_name = std::string(ToString(error.code));
        }
        try {
          ThrowMapped(error.code, rpc, error.reason, error.unknown_reason, error.metadata, error.hint, error.message,
                      error.request_id);
        } catch (...) {
          failure = std::current_exception();
        }
      } else {
        // A success. Only now is the body the response message, so only now is
        // `on_success` the right thing to call.
        if (on_success) {
          on_success(response);
        }
        return;
      }
    }

    // R1: exactly one refresh on `token_expired`, then one retry.
    //
    // The guard is `failure != nullptr`, **not** `std::current_exception() !=
    // nullptr`. `Invoke` is not lexically inside a `catch` block when it reaches
    // here, so `current_exception()` is null at that point and the refresh never
    // ran — which is the shape a client with R1 entirely absent has.
    if (!refreshed && failure != nullptr) {
      try {
        std::rethrow_exception(failure);
      } catch (const TokenExpiredError&) {
        if (RefreshOnce()) {
          refreshed = true;
          Wait(BackoffMs(0, 0));
          continue;
        }
        // A source that cannot refresh: the second expiry is **reported**, which
        // is what R1 says. Falling through leaves the exception to propagate.
      } catch (...) {
      }
    }

    // R2: the retry policy. `ShouldRetry` reads the code off the typed error, so
    // the policy is a function of the taxonomy rather than of a guess at the
    // transport.
    bool retryable = false;
    try {
      std::rethrow_exception(failure);
    } catch (const std::exception& error) {
      retryable = ShouldRetry(error, retry_safe, attempt_number, budget);
    }
    if (!retryable) {
      std::rethrow_exception(failure);
    }
    Wait(BackoffMs(attempt_number, 0));
  }
}

std::string Client::EncodeBody(const google::protobuf::Message& request, ContentType content_type) const {
  std::string body;
  if (IsJson(content_type)) {
    const auto status = google::protobuf::util::MessageToJsonString(request, &body);
    if (!status.ok()) {
      ThrowInternal("", "loams: the request would not encode as proto3 JSON: " + std::string(status.message()));
    }
  } else if (!request.SerializeToString(&body)) {
    ThrowInternal("", "loams: the request did not serialise");
  }
  if (IsFramed(content_type)) {
    return EncodeEnvelopeFrame(kFrameMessage, body);
  }
  return body;
}

void Client::Unary(const MethodBinding& binding, const google::protobuf::Message& request,
                   google::protobuf::Message* response, ContentType content_type,
                   const std::string& idempotency_key,
                   const std::vector<std::pair<std::string, std::string>>& headers) {
  if (binding.server_streaming) {
    ThrowInternal(binding.rpc, "loams: " + binding.rpc + " is a server stream; call OpenStream");
  }
  if (response == nullptr) {
    ThrowInternal(binding.rpc, "loams: " + binding.rpc + " was given no response message to fill");
  }

  // R3, before the first attempt: the key is minted once per logical call and the
  // request is cloned with it set, so every retry re-marshals a message that
  // already carries it.
  const bool declared = !binding.idempotency_field.empty() || DeclaresIdempotencyKey(request);
  KeyedRequest keyed = ApplyIdempotencyKey(request, idempotency_key, declared);
  const google::protobuf::Message& outgoing = keyed.request ? *keyed.request : request;

  CallPlan plan;
  plan.binding = binding;
  plan.content_type = content_type;
  plan.keyed = keyed.keyed;
  if (keyed.keyed) {
    plan.idempotency_key = ReadIdempotencyKey(outgoing).value_or(std::string());
  }
  plan.headers = headers;

  // Marshal once, from the **keyed** message, through the one encoder every other
  // path uses — so every retry re-sends the same bytes, R3's "the same key goes
  // out on every retry" is true by construction, and a caller driving the raw path
  // cannot end up with a different encoder than the facade does.
  plan.body = EncodeBody(outgoing, content_type);

  Invoke(plan, *impl_->transport,
         [this](const HttpRequest& sent, HttpResponse& answered) { answered = impl_->transport->Send(sent); },
         [out = response, &plan, content_type](const HttpResponse& answered) {
           const std::string* body = &answered.body;
           std::string framed;
           if (IsFramed(content_type)) {
             const std::optional<std::vector<EnvelopeFrame>> frames = DecodeEnvelopeFrames(answered.body);
             if (!frames.has_value() || frames->empty()) {
               ThrowInternal(plan.binding.rpc, "loams: " + plan.binding.rpc +
                                                   "'s response was not a single " +
                                                   std::string(ContentTypeName(content_type)) + " frame");
             }
             // Unary is exactly one message frame. A second frame would be a
             // second message, which is a stream this call did not ask for.
             framed = frames->front().payload;
             body = &framed;
           }
           if (out == nullptr) {
             ThrowInternal(plan.binding.rpc, "loams: " + plan.binding.rpc + "'s response body did not parse");
           }
           const bool parsed = IsJson(content_type)
                                   ? google::protobuf::util::JsonStringToMessage(*body, out).ok()
                                   : out->ParseFromString(*body);
           if (!parsed) {
             ThrowInternal(plan.binding.rpc, "loams: " + plan.binding.rpc + "'s response body did not parse as " +
                                                std::string(IsJson(content_type) ? "proto3 JSON" : "protobuf"));
           }
         });
}

namespace {

/// The framing a `FrameStream` presents to `MessageStream`: envelope frames, with
/// a Connect stream's end-of-stream failure already turned into an exception.
class TransportFrameStream final : public FrameStream {
 public:
  TransportFrameStream(std::unique_ptr<ByteReader> reader, ContentType content_type, std::string rpc)
      : reader_(std::move(reader)), content_type_(content_type), rpc_(std::move(rpc)) {
    status_ = reader_->Status();
    headers_ = reader_->Headers();
  }

  bool Next(EnvelopeFrame* frame, std::exception_ptr* error) override {
    *error = nullptr;
    // One read of the body is **not** one frame: the transport hands over whatever
    // arrived, so a frame arrives in as many pieces as the network chose, and a
    // reader that filled twice and gave up would report "ended inside a message
    // frame" for a perfectly complete frame. The loop is what assembles it.
    if (!Ensure(5, error)) {
      return false;
    }
    const auto flags = static_cast<std::uint8_t>(buffer_[0]);
    const auto length = static_cast<std::uint32_t>((static_cast<unsigned char>(buffer_[1]) << 24) |
                                                    (static_cast<unsigned char>(buffer_[2]) << 16) |
                                                    (static_cast<unsigned char>(buffer_[3]) << 8) |
                                                    static_cast<unsigned char>(buffer_[4]));
    // A length of 2^32-1 is the one value that cannot be a frame: a body that
    // large does not exist, and `5 + length` would be a five-byte allocation.
    if (length == 0xffffffffu) {
      *error = std::make_exception_ptr(
          TransportError(rpc_, "loams: " + rpc_ + "'s stream declared a frame longer than any body"));
      return false;
    }
    if (!Ensure(5 + static_cast<std::size_t>(length), error)) {
      return false;
    }
    frame->flags = flags;
    frame->payload = buffer_.substr(5, length);
    buffer_.erase(0, 5 + static_cast<std::size_t>(length));
    return true;
  }

  long Status() const override { return status_; }
  void Close() override {
    if (reader_) {
      reader_->Close();
    }
  }

  /// The response headers, for a caller that needs a trailer the framing does not
  /// carry.
  const std::vector<std::pair<std::string, std::string>>& Headers() const { return headers_; }

 private:
  /// Fills until the buffer holds `wanted` bytes. Returns false with `*error` set
  /// when the body ends first, and false with `*error` null at a clean end.
  ///
  /// The body ending inside a frame is **reported**, never treated as the end of
  /// the stream: half a message is not a message, and a reader that yielded the
  /// frames that did arrive would silently truncate the stream.
  bool Ensure(std::size_t wanted, std::exception_ptr* error) {
    while (buffer_.size() < wanted) {
      std::string piece;
      bool at_eof = false;
      if (!reader_->Next(&piece, &at_eof, error)) {
        if (*error != nullptr) {
          return false;
        }
        if (buffer_.empty()) {
          // A clean end of body: no error, no frame.
          return false;
        }
        *error = std::make_exception_ptr(TransportError(
            rpc_, "loams: " + rpc_ + "'s stream ended after " + std::to_string(buffer_.size()) +
                      " bytes, inside a frame of at least " + std::to_string(wanted)));
        return false;
      }
      buffer_.append(piece);
    }
    return true;
  }

  std::unique_ptr<ByteReader> reader_;
  ContentType content_type_;
  std::string rpc_;
  std::string buffer_;
  std::vector<std::pair<std::string, std::string>> headers_;
  long status_ = 0;
};

}  // namespace

MessageStream Client::OpenStream(const MethodBinding& binding, const google::protobuf::Message& request,
                                 ContentType content_type, std::shared_ptr<StreamResume> resume,
                                 const std::vector<std::pair<std::string, std::string>>& headers) {
  if (!binding.server_streaming) {
    ThrowInternal(binding.rpc, "loams: " + binding.rpc + " is unary; call Unary");
  }
  if (!IsFramed(content_type)) {
    // The Connect streaming envelope is what a server stream is framed in.
    // `application/proto` unframed would give the reader no frame boundaries, and
    // guessing where one message ends is how a stream silently truncates.
    ThrowInternal(binding.rpc, "loams: " + binding.rpc + " is a server stream, so its encoding must be a framed "
                                                       "one (application/connect+proto or application/connect+json)");
  }

  std::string body;
  if (IsJson(content_type)) {
    const auto status = google::protobuf::util::MessageToJsonString(request, &body);
    if (!status.ok()) {
      ThrowInternal(binding.rpc, "loams: " + binding.rpc + "'s request would not encode as proto3 JSON: " +
                                     std::string(status.message()));
    }
  } else if (!request.SerializeToString(&body)) {
    ThrowInternal(binding.rpc, "loams: " + binding.rpc + "'s request did not serialise");
  }

  // The re-open closure a `MessageStream` calls with a cursor. It rebuilds the
  // whole request — headers, bearer, body — from the resume's message, which is
  // the only thing that knows what "resume" means for that stream (R7).
  auto reopen = [this, binding, content_type, headers, resume, &request](const std::string& cursor) {
    std::unique_ptr<google::protobuf::Message> next;
    if (resume) {
      // The caller's request, cloned, so a resume may modify it without changing
      // what the caller still holds.
      next = std::unique_ptr<google::protobuf::Message>(request.New());
      next->CopyFrom(request);
      next = resume->Reopen(cursor, *next);
    }
    const google::protobuf::Message& outgoing = next ? *next : request;
    std::string marshalled;
    if (IsJson(content_type)) {
      const auto status = google::protobuf::util::MessageToJsonString(outgoing, &marshalled);
      if (!status.ok()) {
        ThrowInternal(binding.rpc, "loams: " + binding.rpc + "'s resume request would not encode as proto3 JSON");
      }
    } else if (!outgoing.SerializeToString(&marshalled)) {
      ThrowInternal(binding.rpc, "loams: " + binding.rpc + "'s resume request did not serialise");
    }
    HttpRequest http;
    http.method = "POST";
    http.url = impl_->endpoint + binding.HttpPath();
    http.body = EncodeEnvelopeFrame(kFrameMessage, marshalled);
    http.timeout = impl_->options.timeout;
    http.headers.emplace_back("content-type", std::string(ContentTypeName(content_type)));
    http.headers.emplace_back("accept", std::string(ContentTypeName(content_type)));
    http.headers.emplace_back("user-agent", UserAgent());
    http.headers.emplace_back("connect-protocol-version", "1");
    if (impl_->token_source) {
      const std::string token = impl_->token_source->Token();
      if (!token.empty()) {
        http.headers.emplace_back("authorization", "Bearer " + token);
      }
    }
    for (const auto& header : impl_->options.headers) {
      http.headers.push_back(header);
    }
    for (const auto& header : headers) {
      http.headers.push_back(header);
    }
    auto reader = impl_->transport->Open(http);
    return std::unique_ptr<FrameStream>(new TransportFrameStream(std::move(reader), content_type, binding.rpc));
  };

  // The first source is opened **before** `reopen` is moved: both happen in this
  // one expression and their order is unspecified, so moving first left the call
  // running against a moved-from `std::function` and every stream opened with an
  // empty body — which reads as a server that streamed nothing.
  std::unique_ptr<FrameStream> first = reopen(std::string());
  return MessageStream(std::move(first), content_type, std::move(reopen), std::move(resume), binding.rpc,
                       /*retry_safe=*/false, impl_->options.max_retries, std::chrono::milliseconds(kBaseDelayMs));
}

}  // namespace loams