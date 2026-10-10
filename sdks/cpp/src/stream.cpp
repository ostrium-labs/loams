// The typed stream reader, as `stream.hpp` documents it.

#include "loams/stream.hpp"

#include <utility>

#include <google/protobuf/util/json_util.h>

#include "loams/error.hpp"
#include "loams/retry.hpp"

namespace loams {

struct MessageStream::Impl {
  std::unique_ptr<FrameStream> source;
  std::function<std::unique_ptr<FrameStream>(const std::string&)> reopen;
  std::shared_ptr<StreamResume> resume;
  ContentType content_type;
  std::string rpc;
  std::string cursor;
  std::exception_ptr error;
  std::function<void(const std::string&, const google::protobuf::Message&)> on_cursor;
  int max_retries;
  bool ended = false;
};

MessageStream::MessageStream(std::unique_ptr<FrameStream> source, ContentType content_type,
                             std::function<std::unique_ptr<FrameStream>(const std::string&)> reopen,
                             std::shared_ptr<StreamResume> resume, std::string rpc, bool retry_safe,
                             int max_retries, std::chrono::milliseconds backoff_base)
    : impl_(std::make_unique<Impl>()) {
  impl_->source = std::move(source);
  impl_->reopen = std::move(reopen);
  impl_->resume = std::move(resume);
  impl_->content_type = content_type;
  impl_->rpc = std::move(rpc);
  // A stream resumes only when the caller supplied a resume, whatever the
  // binding's retry class says: re-opening a stream whose cursor is unknown would
  // silently start it over, which is the failure R7 exists to prevent.
  impl_->max_retries = (retry_safe && resume != nullptr) ? max_retries : 0;
  static_cast<void>(backoff_base);
}

MessageStream::MessageStream(MessageStream&& other) noexcept : impl_(std::move(other.impl_)) {}

MessageStream& MessageStream::operator=(MessageStream&& other) noexcept {
  if (this != &other) {
    impl_ = std::move(other.impl_);
  }
  return *this;
}

MessageStream::~MessageStream() = default;

bool MessageStream::ReportWireError(const WireError& wire, std::exception_ptr* error) {
  // `kOk` means the end-of-stream frame said nothing went wrong, which is a
  // clean end rather than a failure with no message.
  if (wire.code == Code::kOk) {
    return false;
  }
  // Through `MakeLoamsError`, not a bare `LoamsError`: a stream failure has to
  // land on the same type a unary failure would, or R5's "one `catch` covers the
  // guard and the refusal" is true for unary calls and false for streams.
  *error = MakeLoamsError(wire.code, impl_->rpc, wire.reason, wire.unknown_reason, wire.metadata, wire.hint,
                          wire.message, wire.request_id);
  return true;
}

bool MessageStream::DecodeFrame(const EnvelopeFrame& frame, google::protobuf::Message* out,
                                std::exception_ptr* error) {
  if (frame.flags == kFrameTrailers && IsGrpcWeb(impl_->content_type)) {
    return false;
  }
  if ((frame.flags & kFrameCompressed) != 0) {
    // This SDK never sends `connect-accept-encoding`, so a compressed frame is a
    // server that compressed anyway. Reported rather than guessed at: inflating
    // an unknown payload is how a client reports a message the server never sent.
    *error = std::make_exception_ptr(TransportError(impl_->rpc,
                                                    "loams: " + impl_->rpc +
                                                        "'s stream sent a compressed frame, which this SDK did not "
                                                        "ask for"));
    return false;
  }
  // A Connect **JSON** stream carries the proto3 JSON mapping, exactly as its
  // unary half does; the proto encodings carry binary. Decoding a JSON frame as
  // protobuf happens to succeed on an empty payload and to produce nonsense on
  // anything else, which is the worst possible failure shape.
  const bool parsed = IsJson(impl_->content_type)
                          ? google::protobuf::util::JsonStringToMessage(frame.payload, out).ok()
                          : out->ParseFromString(frame.payload);
  if (!parsed) {
    *error = std::make_exception_ptr(TransportError(
        impl_->rpc, "loams: " + impl_->rpc + "'s stream message did not parse as " +
                        std::string(IsJson(impl_->content_type) ? "proto3 JSON" : "protobuf")));
    return false;
  }
  return true;
}

bool MessageStream::PumpOnce(google::protobuf::Message* out, std::exception_ptr* error) {
  *error = nullptr;
  EnvelopeFrame frame;
  std::exception_ptr failure;
  if (!impl_->source->Next(&frame, &failure)) {
    if (failure) {
      *error = failure;
    }
    // No frame and no failure: the body ended without an end-of-stream frame.
    // That is a clean end for Connect, whose end-of-stream frame `{}` arrives as
    // a frame below, and for gRPC-Web, whose trailers frame does the same job.
    return false;
  }
  if (failure) {
    *error = failure;
    return false;
  }

  if (frame.flags == kFrameTrailers && IsGrpcWeb(impl_->content_type)) {
    // The trailers frame is the last one, and the status lives in it. Everything
    // after it is collected so a status in a later frame would still be found —
    // a server this SDK does not understand, rather than one that is guessed at.
    std::vector<EnvelopeFrame> frames{frame};
    EnvelopeFrame more;
    while (impl_->source->Next(&more, &failure) && !failure) {
      frames.push_back(more);
    }
    HttpResponse headers_only;
    headers_only.status = impl_->source->Status();
    const std::optional<WireError> wire = DecodeStreamError(impl_->content_type, frames, headers_only);
    if (wire.has_value()) {
      ReportWireError(*wire, error);
    }
    return false;
  }

  if (frame.flags == kFrameEndOfStream && !IsGrpcWeb(impl_->content_type)) {
    // Connect's end-of-stream frame: `{}` for a clean end, `{"error": ...}` for
    // a failure. This is the `live_watch` case, where the refusal arrives inside
    // the envelope after HTTP **200** — a client that reads only the status sees
    // a successful call fail.
    const std::vector<EnvelopeFrame> frames{frame};
    HttpResponse headers_only;
    headers_only.status = impl_->source->Status();
    const std::optional<WireError> wire = DecodeStreamError(impl_->content_type, frames, headers_only);
    if (wire.has_value()) {
      ReportWireError(*wire, error);
    }
    return false;
  }

  return DecodeFrame(frame, out, error);
}

bool MessageStream::Receive(google::protobuf::Message* out) {
  if (out == nullptr) {
    ThrowInternal(impl_->rpc, "loams: " + impl_->rpc + "'s stream was given no message to fill");
  }
  if (!impl_->source) {
    return false;
  }
  std::exception_ptr failure;
  if (PumpOnce(out, &failure)) {
    if (impl_->resume) {
      const std::string cursor = impl_->resume->CursorOf(*out);
      if (!cursor.empty()) {
        impl_->cursor = cursor;
      }
    }
    if (impl_->on_cursor) {
      impl_->on_cursor(impl_->cursor, *out);
    }
    return true;
  }

  if (!failure) {
    // A clean end: the server finished. Not an error, and not something to resume
    // from — resuming a finished stream would re-open a stream that is over.
    impl_->ended = true;
    impl_->source->Close();
    impl_->source.reset();
    return false;
  }

  // A failure the retry class does not cover — notably an `unimplemented` stream,
  // which is what every `loams.live.v1` RPC answers in the standard variant — is
  // **reported** rather than spun on (R7).
  bool retryable = false;
  try {
    std::rethrow_exception(failure);
  } catch (const std::exception& error) {
    retryable = ShouldRetry(error, /*retry_safe=*/impl_->resume != nullptr, 0, impl_->max_retries);
  }
  if (!retryable) {
    impl_->error = failure;
    impl_->source->Close();
    impl_->source.reset();
    return false;
  }

  impl_->source->Close();
  impl_->source.reset();
  try {
    // Re-open from the last cursor **applied**, which is what stops a reconnect
    // from silently missing everything that changed in between, and from
    // re-yielding what it already yielded.
    impl_->source = impl_->reopen(impl_->cursor);
  } catch (...) {
    impl_->error = std::current_exception();
    return false;
  }
  return Receive(out);
}

std::exception_ptr MessageStream::Error() const { return impl_->error; }

std::string MessageStream::Cursor() const { return impl_->cursor; }

void MessageStream::Close() {
  if (impl_->source) {
    impl_->source->Close();
    impl_->source.reset();
  }
}

void MessageStream::SetOnCursor(
    std::function<void(const std::string&, const google::protobuf::Message&)> on_cursor) {
  impl_->on_cursor = std::move(on_cursor);
}

}  // namespace loams