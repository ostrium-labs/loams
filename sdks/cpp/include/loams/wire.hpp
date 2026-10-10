// The wire: Connect and gRPC-Web framing, content types and error decoding
// (design §44 §4, D600; runtime contract R8).
//
// Everything here is the part of the protocol that is **not** the HTTP status.
// Four content types are spoken, and the difference between them is the whole
// difficulty of this SDK:
//
//   | content type            | request body        | response          | failure        |
//   |-------------------------|---------------------|-------------------|----------------|
//   | application/json        | proto3 JSON         | proto3 JSON       | HTTP 501 + JSON|
//   | application/proto        | protobuf binary     | protobuf binary   | HTTP 501 + JSON|
//   | application/grpc-web+proto | one 5-byte frame  | 5-byte frames     | HTTP 200 + trailers |
//   | application/grpc-web+json  | one 5-byte frame  | 5-byte frames     | HTTP 200 + trailers |
//   | application/connect+json| one 5-byte frame    | 5-byte frames     | error in the last frame |
//   | application/connect+proto| one 5-byte frame   | 5-byte frames     | error in the last frame |
//
// A **failed RPC** carries a Connect code plus one base64-protobuf
// `loams.errors.v1.ErrorInfo` in `details` (field 1 `reason`, field 2
// `map<string,string> metadata`, field 3 `hint`). Where those details arrive
// differs per encoding and is what `DecodeUnaryError` hides.

#ifndef LOAMS_WIRE_HPP
#define LOAMS_WIRE_HPP

#include <cstdint>
#include <optional>
#include <string>
#include <string_view>
#include <vector>

#include "loams/base64.hpp"
#include "loams/error.hpp"
#include "loams/http.hpp"

namespace loams {

/// The content types the SDK can send and accept.
enum class ContentType {
  /// `application/json`: the proto3 JSON mapping, which is what `curl` sends
  /// (design §44 §4).
  kJson,
  /// `application/proto`: protobuf binary, the default for an SDK.
  kProto,
  /// `application/grpc-web+proto`: the framing a browser sends.
  kGrpcWebProto,
  /// `application/grpc-web+json`: gRPC-Web carrying the proto3 JSON mapping.
  kGrpcWebJson,
  /// `application/connect+proto`: the Connect streaming envelope, binary.
  kConnectProto,
  /// `application/connect+json`: the Connect streaming envelope, JSON.
  kConnectJson,
};

/// The `Content-Type` header value for an encoding.
std::string_view ContentTypeName(ContentType type);

/// The encoding a `Content-Type` header names, or `std::nullopt` for one this
/// SDK does not speak. The parameters after `;` are ignored, because
/// `application/json; charset=utf-8` is the same encoding.
std::optional<ContentType> ContentTypeFromName(std::string_view name);

/// The **family** of a content type: `json`, `proto`, `grpc_web`,
/// `grpc_web_json`, `connect`. The fixture corpus is keyed on the family rather
/// than the exact string, so this is what the in-process replay and
/// `fixture-server.mjs` match on and what a test compares.
std::string ContentFamily(std::string_view content_type);

/// Whether an encoding frames messages in the 5-byte envelope. Connect
/// streaming and both gRPC-Web encodings do; `application/json` and
/// `application/proto` do not.
bool IsFramed(ContentType type);

/// Whether an encoding is a gRPC-Web one, whose trailers carry the status.
bool IsGrpcWeb(ContentType type);

/// Whether an encoding carries the **proto3 JSON mapping** rather than protobuf
/// binary. `application/json`, `application/grpc-web+json` and
/// `application/connect+json` do; `application/proto`,
/// `application/grpc-web+proto` and `application/connect+proto` do not.
///
/// The three JSON encodings are what `curl` sends and what a browser sends, so a
/// client that cannot speak them cannot be pointed at by a browser at all — and
/// the corpus records all three.
bool IsJson(ContentType type);

/// One envelope frame: a flag byte and a payload.
///
/// Flags, per the Connect streaming protocol:
///
///   - `0x00` a message.
///   - `0x02` the end-of-stream frame. Over Connect its payload is JSON, either
///     `{}` for a clean end or `{"error": {...}}`. Over gRPC-Web it is absent
///     and the trailers carry the status instead.
///   - `0x80` compressed (with `0x81` for compressed trailers). This SDK never
///     sends `connect-accept-encoding`, so a compressed frame is a server that
///     ignored the request; it is reported rather than guessed at.
std::string EncodeEnvelopeFrame(std::uint8_t flags, std::string_view payload);

/// One decoded frame.
struct EnvelopeFrame {
  std::uint8_t flags = 0;
  std::string payload;
};

/// The frame flag bits this SDK understands.
inline constexpr std::uint8_t kFrameMessage = 0x00;
inline constexpr std::uint8_t kFrameEndOfStream = 0x02;
inline constexpr std::uint8_t kFrameCompressed = 0x80;
inline constexpr std::uint8_t kFrameTrailers = 0x80;

/// Splits an envelope body into its frames.
///
/// Returns `std::nullopt` for a body that is not framed at all (fewer than five
/// bytes, or a length that runs past the end). A truncated body is **not**
/// silently treated as the frames that did arrive: half a message is not a
/// message, and a stream that yielded three of four messages and then claimed
/// to have ended cleanly would be a stream that lost data without saying so.
std::optional<std::vector<EnvelopeFrame>> DecodeEnvelopeFrames(std::string_view body);

/// A failure as it came off the wire, before it becomes a typed error.
struct WireError {
  /// The Connect code. `kOk` when the wire carried no code at all, which is the
  /// case `TransportError` covers.
  Code code = Code::kUnknown;
  /// The code string the wire carried, whether or not this SDK recognises it.
  std::string code_name;
  /// The server's message. For a person; nothing branches on it.
  std::string message;
  /// The stable cause, when the details carried an `ErrorInfo` this SDK's
  /// registry has.
  Reason reason = Reason::kUnknown;
  /// The registry string, when it is one this SDK's registry does not have — a
  /// reason from a newer server, surfaced and flagged rather than dropped (R8).
  std::string unknown_reason;
  /// `ErrorInfo.metadata`, structured context. Never secrets.
  std::map<std::string, std::string> metadata;
  /// `ErrorInfo.hint`, a next step in the caller's locale.
  std::string hint;
  /// Whether an `ErrorInfo` was found at all. `false` means the failure came
  /// from below the API or from a server that sent no detail, and it is the
  /// third of R8's three distinct cases.
  bool has_error_info = false;
  /// The request id, when the server sent one.
  std::string request_id;
};

/// The failure a **unary** response carries.
///
/// `content_type` is the *request's* encoding, which is also the response's for
/// a success; it decides where the status lives:
///
///   - Connect (`application/json`, `application/proto`): the HTTP status is the
///     mapping and the body is a JSON `{code, message, details}` object.
///   - gRPC-Web (either encoding): HTTP 200, the frames carry the status in a
///     **trailers** frame for a stream, and in the `grpc-status` /
///     `grpc-status-details-bin` **headers** for unary.
///
/// A response that carries no failure at all yields a `WireError` with
/// `code == Code::kOk` and no error info, which the caller must not mistake for
/// a failure: `std::nullopt` says "this was a success".
std::optional<WireError> DecodeUnaryError(ContentType request_type, const HttpResponse& response);

/// The failure a **streamed** response carries, from its end-of-stream frame
/// (Connect) or its trailers frame (gRPC-Web).
std::optional<WireError> DecodeStreamError(ContentType request_type,
                                           const std::vector<EnvelopeFrame>& frames,
                                           const HttpResponse& headers_only);

/// The trailers of a gRPC-Web **headers-only** unary response, decoded out of
/// the header block. `grpc-status` absent means the call succeeded: a
/// trailers-only gRPC-Web response with no status is status 0.
std::optional<WireError> DecodeGrpcWebHeaderTrailers(const HttpResponse& response);

/// Builds the `google.rpc.Status` protobuf gRPC-Web puts in
/// `grpc-status-details-bin`, from a `WireError`. This is the **inverse** of
/// `DecodeUnaryError`, and it exists because the conformance suite needs to
/// answer a request with a hand-built failure to test the mapping — a stub
/// server is the only way to produce a retryable `unavailable` on demand.
std::string EncodeGrpcWebStatusDetails(const WireError& error);

/// Builds the Connect streaming end-of-stream frame that carries a failure.
std::string EncodeConnectEndOfStreamFrame(const WireError& error);

}  // namespace loams

#endif  // LOAMS_WIRE_HPP