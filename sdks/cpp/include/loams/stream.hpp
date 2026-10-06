// Server streams (design §44 §7.4, D610; runtime contract R7).
//
// The API has server streams only (D420): no client streaming, no bidi, because
// a browser cannot stream full duplex over `fetch` and half-duplex works through
// every proxy. A server stream in C++ is a **reader**, which is design §44 §7.1's
// wording for this language:
//
//     loams::MessageStream stream = client.Live().Watch(request);
//     loams::Transition transition;
//     while (stream.Receive(&transition)) {
//       Apply(transition);
//     }
//     if (auto error = stream.Error()) {
//       std::rethrow_exception(error);
//     }
//
// # Why a stream is not just a retry
//
// A stream is the one call where "send it again" is not enough. The server hands
// out cursors, and a reconnect has to resume from the last one the client
// **applied**, or the client silently misses everything that changed in between
// — which is worse than an error, because a sync UI that is quietly stale looks
// exactly like a sync UI that works.
//
// So a `MessageStream` with a resume tracks the cursor of every message, re-opens
// from it on a retryable failure, and does **not** re-yield what it already
// yielded. The cursor reader is **required**, with no default: a default would
// have to guess a field name, and `WatchApprovalsResponse`'s cursor is a
// `cursor` field while `LiveService/Watch`'s is a `StateVersion`. Guessing would
// be a method that silently resumes from nothing.

#ifndef LOAMS_STREAM_HPP
#define LOAMS_STREAM_HPP

#include <exception>
#include <functional>
#include <memory>
#include <mutex>
#include <string>

#include <google/protobuf/message.h>

#include "loams/wire.hpp"

namespace loams {

/// A stream's response body, framed. This is the layer between an
/// `loams::ByteReader` and a typed `MessageStream`: it hands back whole envelope
/// frames and, on a Connect stream, reads the failure out of the end-of-stream
/// frame.
class FrameStream {
 public:
  virtual ~FrameStream() = default;

  /// The next frame. Returns false at the end of the stream **or** on failure;
  /// `*error` says which (null means a clean end).
  virtual bool Next(EnvelopeFrame* frame, std::exception_ptr* error) = 0;

  /// The HTTP status the response arrived with. Available after the first
  /// `Next`, and meaningful: a Connect stream that fails carries HTTP 200.
  virtual long Status() const = 0;

  /// Releases the connection. Idempotent.
  virtual void Close() = 0;
};

/// How a server stream re-opens from a cursor (R7).
class StreamResume {
 public:
  virtual ~StreamResume() = default;

  /// The request to re-open with, given the last cursor applied and the original
  /// request. Returning the original re-opens from the beginning, which is
  /// correct — and loses nothing but time — for a stream whose snapshot is
  /// complete.
  ///
  /// `request` is the message the caller passed to `Open`, cloned, so a resume
  /// may modify it freely without changing what the caller still holds.
  virtual std::unique_ptr<google::protobuf::Message> Reopen(
      const std::string& cursor, const google::protobuf::Message& request) = 0;

  /// Reads the cursor off a message. Required, and there is no default: a
  /// stream's cursor is that stream's business.
  virtual std::string CursorOf(const google::protobuf::Message& message) = 0;
};

/// A typed server stream, driven by the caller's loop.
///
/// `Receive` returns false at the end of the stream **or** on a failure, and
/// `Error` says which: a null `error_ptr` means the stream finished, a set one
/// means it broke and nothing more will arrive. That distinction is the whole
/// contract of the reader — `while (stream.Receive(&msg))` alone cannot tell
/// "done" from "broken", and a caller that ignores `Error` sees a silently
/// truncated stream.
class MessageStream {
 public:
  MessageStream(MessageStream&&) noexcept;
  MessageStream& operator=(MessageStream&&) noexcept;
  MessageStream(const MessageStream&) = delete;
  MessageStream& operator=(const MessageStream&) = delete;
  ~MessageStream();

  /// Parses one frame into `out`, re-opening from the cursor if the stream broke
  /// in a way a retry covers. Returns whether there was a message.
  ///
  /// Throws nothing: a failure is reported through `Error`, because a throw out
  /// of a loop body is a much easier thing to write badly than a post-loop
  /// check.
  bool Receive(google::protobuf::Message* out);

  /// Why the stream ended, or null if it ended cleanly. Non-null after `Receive`
  /// returned false for any reason other than end-of-stream.
  std::exception_ptr Error() const;

  /// The last cursor the stream applied, or empty if it carries none.
  std::string Cursor() const;

  /// Releases the stream. Safe to call more than once, and after it has ended.
  void Close();

  /// Called after each message with the cursor it carried. Used by a caller that
  /// wants to persist a cursor; the stream's own bookkeeping does not need it.
  void SetOnCursor(std::function<void(const std::string&, const google::protobuf::Message&)> on_cursor);

 private:
  friend class Client;
  MessageStream(std::unique_ptr<FrameStream> source, ContentType content_type,
                std::function<std::unique_ptr<FrameStream>(const std::string&)> reopen,
                std::shared_ptr<StreamResume> resume, std::string rpc, bool retry_safe,
                int max_retries, std::chrono::milliseconds backoff_base);

  /// Reads one message off `source`, leaving it positioned after that message.
  /// Returns false at end of stream or on failure, `*error` says which.
  bool PumpOnce(google::protobuf::Message* out, std::exception_ptr* error);
  bool DecodeFrame(const EnvelopeFrame& frame, google::protobuf::Message* out, std::exception_ptr* error);
  bool ReportWireError(const WireError& wire, std::exception_ptr* error);

  struct Impl;
  std::unique_ptr<Impl> impl_;
};

}  // namespace loams

#endif  // LOAMS_STREAM_HPP