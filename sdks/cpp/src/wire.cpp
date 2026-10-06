// The wire: content types, envelope framing, and the failure decoder.
//
// The failure decoder is the load-bearing part. **HTTP status is not the error**:
// Connect answers a failed RPC with 501 and a JSON body, and gRPC-Web answers
// **200** with the code in the trailers. A client that reads only the status sees
// a failed gRPC-Web call succeed, which is exactly what the
// `instance_who_am_i_grpc_web` and `live_watch` fixtures exist to catch.

#include "loams/wire.hpp"

#include <google/protobuf/struct.pb.h>
#include <google/protobuf/util/json_util.h>

#include "loams/errors/v1/errors.pb.h"

namespace loams {
namespace {

/// The JSON a Connect failure body carries:
///
///     {"code": "unimplemented",
///      "message": "...",
///      "details": [{"type": "loams.errors.v1.ErrorInfo", "value": "<base64>"}]}
///
/// Parsed as a `Struct` rather than by hand: no second JSON parser, and no
/// dependency on one.
std::optional<google::protobuf::Struct> ParseJsonObject(std::string_view body) {
  google::protobuf::Struct parsed;
  const auto status = google::protobuf::util::JsonStringToMessage(std::string(body), &parsed);
  if (!status.ok()) {
    return std::nullopt;
  }
  return parsed;
}

/// A field of a parsed JSON object, or null when it is absent. The key is taken
/// as a `std::string` because `Map::find` is a template on the key type and a
/// string literal does not deduce against it.
const google::protobuf::Value* Field(const google::protobuf::Struct& object, const std::string& name) {
  const auto& fields = object.fields();
  const auto found = fields.find(name);
  if (found == fields.end()) {
    return nullptr;
  }
  return &found->second;
}

std::string StringField(const google::protobuf::Struct& object, const std::string& name) {
  const google::protobuf::Value* const value = Field(object, name);
  if (value == nullptr) {
    return std::string();
  }
  return value->string_value();
}

/// Folds a **raw** `ErrorInfo` into a wire error. This is the gRPC-Web half:
/// a `google.protobuf.Any`'s `value` field is already the message's bytes, because
/// a protobuf `bytes` field carries raw bytes and only its *JSON* rendering is
/// base64.
void ApplyErrorInfoBytes(const std::string& bytes, WireError* out) {
  loams::errors::v1::ErrorInfo info;
  if (!info.ParseFromString(bytes)) {
    return;
  }
  out->has_error_info = true;
  out->reason = Reason::kUnknown;
  out->unknown_reason = info.reason();
  const auto known = ReasonFromString(info.reason());
  if (known.has_value()) {
    out->reason = *known;
    out->unknown_reason.clear();
  }
  for (const auto& entry : info.metadata()) {
    out->metadata[entry.first] = entry.second;
  }
  out->hint = info.hint();
}

/// Folds an `ErrorInfo` out of the base64 a Connect **JSON** body's `details[]`
/// carries. Looked up **by its type name**, not by position, so a service that
/// adds a detail of its own does not move `reason` out from under a caller.
void ApplyErrorInfo(const std::string& base64, WireError* out) {
  const std::optional<std::string> bytes = Base64Decode(base64);
  if (!bytes.has_value()) {
    // The detail's bytes did not decode. Reported as "no ErrorInfo" rather than
    // dropped silently, because the alternative is a Loams failure with no
    // reason and no hint — the one thing R8 says must not happen.
    return;
  }
  ApplyErrorInfoBytes(*bytes, out);
}

/// Reads the JSON Connect error object out of a body.
void ApplyConnectErrorBody(const std::string& body, WireError* out) {
  const std::optional<google::protobuf::Struct> parsed = ParseJsonObject(body);
  if (!parsed.has_value()) {
    return;
  }
  out->code_name = StringField(*parsed, "code");
  out->message = StringField(*parsed, "message");
  if (const auto known = CodeFromString(out->code_name)) {
    out->code = *known;
  }
  const google::protobuf::Value* const details = Field(*parsed, "details");
  if (details == nullptr || !details->has_list_value()) {
    return;
  }
  for (const google::protobuf::Value& entry : details->list_value().values()) {
    if (!entry.has_struct_value()) {
      continue;
    }
    const google::protobuf::Struct& one = entry.struct_value();
    const std::string type = StringField(one, "type");
    if (type != "loams.errors.v1.ErrorInfo") {
      continue;
    }
    ApplyErrorInfo(StringField(one, "value"), out);
    // One `ErrorInfo` is what the contract promises. A second is a server that
    // sent something this SDK does not understand, and the first is the one the
    // contract names, so the extra ones are ignored rather than allowed to
    // overwrite the reason.
    return;
  }
}

std::optional<WireError> DecodeGrpcWebStatusDetails(const std::string& base64) {
  const std::optional<std::string> bytes = Base64Decode(base64);
  if (!bytes.has_value()) {
    return std::nullopt;
  }
  // `google.rpc.Status`: field 1 `code` (an int), field 2 `message`, field 3
  // `details[]` of `{type: 1, value: 2}`.
  const std::string& body = *bytes;
  WireError error;
  int code_number = 2;  // `unknown`, which is what google.rpc.Status defaults to
  std::string message;
  std::string detail_type;
  std::string detail_value;

  // `key >> 3 == 2` would parse as `key >> (3 == 2)` — i.e. `key >> 0` — and no
  // branch below would ever match, so every field would fall through to the
  // "unknown field, skip it" arm and the whole Status would decode as an empty one.
  // The parentheses are load-bearing on all three.
  std::size_t at = 0;
  while (at < body.size()) {
    const auto key = static_cast<unsigned char>(body[at]);
    ++at;
    const int wire = key & 0x07;
    std::uint64_t value = 0;
    int shift = 0;
    bool varint = true;
    while (at < body.size()) {
      const auto byte = static_cast<unsigned char>(body[at]);
      ++at;
      value |= static_cast<std::uint64_t>(byte & 0x7f) << shift;
      if ((byte & 0x80) == 0) {
        break;
      }
      shift += 7;
      if (shift > 63) {
        varint = false;
        break;
      }
    }
    if (!varint) {
      return std::nullopt;
    }
    if (wire == 0 && (key >> 3) == 1) {
      code_number = static_cast<int>(value);
    } else if (wire == 2 && (key >> 3) == 2) {
      message = body.substr(at, static_cast<std::size_t>(value));
      at += static_cast<std::size_t>(value);
    } else if (wire == 2 && (key >> 3) == 3) {
      // A `google.protobuf.Any`: field 1 `type_url`, field 2 `value`.
      std::size_t inner = at;
      const std::size_t end = at + static_cast<std::size_t>(value);
      while (inner < end) {
        const auto inner_key = static_cast<unsigned char>(body[inner]);
        ++inner;
        std::uint64_t inner_value = 0;
        int inner_shift = 0;
        while (inner < end) {
          const auto byte = static_cast<unsigned char>(body[inner]);
          ++inner;
          inner_value |= static_cast<std::uint64_t>(byte & 0x7f) << inner_shift;
          if ((byte & 0x80) == 0) {
            break;
          }
          inner_shift += 7;
        }
        if ((inner_key & 0x07) != 2) {
          continue;
        }
        if ((inner_key >> 3) == 1) {
          detail_type = body.substr(inner, static_cast<std::size_t>(inner_value));
        } else if ((inner_key >> 3) == 2) {
          detail_value = body.substr(inner, static_cast<std::size_t>(inner_value));
        }
        inner += static_cast<std::size_t>(inner_value);
      }
      at = end;
    } else if (wire == 2) {
      at += static_cast<std::size_t>(value);
    } else if (wire == 0) {
      continue;
    } else {
      // Fixed32/fixed64: the status this SDK decodes carries none, and guessing
      // at their width would be how a decode silently reads the wrong field.
      return std::nullopt;
    }
  }

  error.code = static_cast<Code>(code_number);
  error.code_name = ToString(error.code);
  error.message = message;
  if (detail_type.empty() || detail_type != "type.googleapis.com/loams.errors.v1.ErrorInfo") {
    return error;
  }
  // The Any's `value` is already the message's raw bytes: a protobuf `bytes` field
  // carries raw bytes, and only its **JSON** rendering is base64. Running it
  // through the base64 reader silently decoded nothing and every gRPC-Web reason
  // came out empty.
  ApplyErrorInfoBytes(detail_value, &error);
  return error;
}

void PutVarint(std::string* out, std::uint64_t value) {
  do {
    auto byte = static_cast<unsigned char>(value & 0x7f);
    value >>= 7;
    if (value != 0) {
      byte |= 0x80;
    }
    out->push_back(static_cast<char>(byte));
  } while (value != 0);
}

void PutTag(std::string* out, int field, int wire) {
  PutVarint(out, (static_cast<std::uint64_t>(field) << 3) | static_cast<unsigned>(wire));
}

void PutBytes(std::string* out, int field, const std::string& bytes) {
  PutTag(out, field, 2);
  PutVarint(out, bytes.size());
  out->append(bytes);
}

std::optional<WireError> DecodeConnectEndOfStream(const std::string& payload) {
  // `{}` is a clean end; `{"error": {...}}` is the failure. Anything else — an
  // empty payload, a JSON array — is a server this SDK does not understand, and
  // reporting it as a success would be the one wrong answer.
  const std::optional<google::protobuf::Struct> parsed = ParseJsonObject(payload);
  if (!parsed.has_value()) {
    return std::nullopt;
  }
  const google::protobuf::Value* const error_field = Field(*parsed, "error");
  if (error_field == nullptr || !error_field->has_struct_value()) {
    return std::nullopt;
  }
  const google::protobuf::Struct& failure = error_field->struct_value();
  // Re-serialised and run through the same reader a unary Connect body takes, so
  // both shapes decode identically. There is one decoder, not two.
  std::string body;
  const auto status = google::protobuf::util::MessageToJsonString(failure, &body);
  if (!status.ok()) {
    return std::nullopt;
  }
  WireError error;
  ApplyConnectErrorBody(body, &error);
  return error;
}

}  // namespace

std::string_view ContentTypeName(ContentType type) {
  switch (type) {
    case ContentType::kJson:
      return "application/json";
    case ContentType::kProto:
      return "application/proto";
    case ContentType::kGrpcWebProto:
      return "application/grpc-web+proto";
    case ContentType::kGrpcWebJson:
      return "application/grpc-web+json";
    case ContentType::kConnectProto:
      return "application/connect+proto";
    case ContentType::kConnectJson:
      return "application/connect+json";
  }
  return "application/proto";
}

std::optional<ContentType> ContentTypeFromName(std::string_view name) {
  // Parameters after `;` are ignored: `application/json; charset=utf-8` is the
  // same encoding.
  const std::size_t semicolon = name.find(';');
  std::string base;
  if (semicolon == std::string_view::npos) {
    base = std::string(name);
  } else {
    base = std::string(name.substr(0, semicolon));
  }
  while (!base.empty() && (base.back() == ' ' || base.back() == '\t')) {
    base.pop_back();
  }
  for (const ContentType candidate : {ContentType::kJson, ContentType::kProto, ContentType::kGrpcWebProto,
                                      ContentType::kGrpcWebJson, ContentType::kConnectProto,
                                      ContentType::kConnectJson}) {
    if (base == ContentTypeName(candidate)) {
      return candidate;
    }
  }
  return std::nullopt;
}

std::string ContentFamily(std::string_view content_type) {
  const std::optional<ContentType> type = ContentTypeFromName(content_type);
  if (!type.has_value()) {
    return std::string(content_type);
  }
  switch (*type) {
    case ContentType::kJson:
      return "json";
    case ContentType::kProto:
      return "proto";
    case ContentType::kGrpcWebProto:
      return "grpc_web";
    case ContentType::kGrpcWebJson:
      return "grpc_web_json";
    case ContentType::kConnectProto:
    case ContentType::kConnectJson:
      return "connect";
  }
  return std::string();
}

bool IsFramed(ContentType type) {
  return type == ContentType::kGrpcWebProto || type == ContentType::kGrpcWebJson ||
         type == ContentType::kConnectProto || type == ContentType::kConnectJson;
}

bool IsGrpcWeb(ContentType type) { return type == ContentType::kGrpcWebProto || type == ContentType::kGrpcWebJson; }

bool IsJson(ContentType type) {
  return type == ContentType::kJson || type == ContentType::kGrpcWebJson || type == ContentType::kConnectJson;
}

std::string EncodeEnvelopeFrame(std::uint8_t flags, std::string_view payload) {
  std::string out;
  out.reserve(5 + payload.size());
  out.push_back(static_cast<char>(flags));
  const auto length = static_cast<std::uint32_t>(payload.size());
  // Big-endian, per the envelope. A little-endian length would make every frame
  // that starts with a zero byte look like an empty message.
  out.push_back(static_cast<char>((length >> 24) & 0xff));
  out.push_back(static_cast<char>((length >> 16) & 0xff));
  out.push_back(static_cast<char>((length >> 8) & 0xff));
  out.push_back(static_cast<char>(length & 0xff));
  out.append(payload);
  return out;
}

std::optional<std::vector<EnvelopeFrame>> DecodeEnvelopeFrames(std::string_view body) {
  if (body.empty()) {
    // An empty body is an empty frame set: a stream that carried no message is
    // legal, and calling it a truncation would report a broken stream for a stream
    // that simply had nothing to say.
    return std::vector<EnvelopeFrame>{};
  }
  if (body.size() < 5) {
    return std::nullopt;
  }
  std::vector<EnvelopeFrame> frames;
  std::size_t at = 0;
  while (at + 5 <= body.size()) {
    const auto flags = static_cast<std::uint8_t>(body[at]);
    const auto length = static_cast<std::uint32_t>((static_cast<unsigned char>(body[at + 1]) << 24) |
                                                    (static_cast<unsigned char>(body[at + 2]) << 16) |
                                                    (static_cast<unsigned char>(body[at + 3]) << 8) |
                                                    static_cast<unsigned char>(body[at + 4]));
    if (at + 5 + length > body.size()) {
      // A truncated body is **not** the frames that did arrive: half a message
      // is not a message, and a stream that yielded three of four and then
      // claimed to have ended cleanly would be one that lost data silently.
      return std::nullopt;
    }
    EnvelopeFrame frame;
    frame.flags = flags;
    frame.payload.assign(body.substr(at + 5, length));
    frames.push_back(std::move(frame));
    at += 5 + length;
  }
  if (at != body.size()) {
    // Trailing bytes that are not a frame header at all.
    return std::nullopt;
  }
  return frames;
}

std::optional<WireError> DecodeUnaryError(ContentType request_type, const HttpResponse& response) {
  if (IsGrpcWeb(request_type)) {
    // gRPC-Web has **two** legal places for a unary failure's trailers, and the
    // corpus records both. A server that can rely on HTTP trailers puts them in a
    // trailing header block; one that must work with a browser — which cannot see
    // them — puts them in a frame at the end of the body, and answers **200**.
    // Reading only the headers makes every browser-shaped refusal look like a
    // success, which is the whole reason the corpus records the same refusal in
    // four encodings.
    if (const std::optional<std::vector<EnvelopeFrame>> frames = DecodeEnvelopeFrames(response.body);
        frames.has_value() && !frames->empty()) {
      const std::optional<WireError> in_body = DecodeStreamError(request_type, *frames, response);
      if (in_body.has_value()) {
        return in_body;
      }
      // Frames present and no failure in them: a real message frame is a success.
      // A trailers-only body would have decoded above.
      if (!response.body.empty()) {
        return std::nullopt;
      }
    }
    return DecodeGrpcWebHeaderTrailers(response);
  }
  if (response.status == 200) {
    return std::nullopt;
  }
  WireError error;
  error.code = CodeFromHttpStatus(response.status);
  ApplyConnectErrorBody(response.body, &error);
  if (error.code_name.empty()) {
    error.code_name = ToString(error.code);
  }
  return error;
}

std::optional<WireError> DecodeGrpcWebHeaderTrailers(const HttpResponse& response) {
  // gRPC-Web unary puts its trailers in the **headers**, because HTTP/1.1 has no
  // trailer block browsers will give a `fetch`. A trailers-only response with no
  // `grpc-status` is status 0: a clean end.
  const std::string status = response.Header("grpc-status");
  if (status.empty() && response.status == 200) {
    return std::nullopt;
  }
  const std::string details = response.Header("grpc-status-details-bin");
  if (!details.empty()) {
    if (std::optional<WireError> decoded = DecodeGrpcWebStatusDetails(details)) {
      return decoded;
    }
  }
  WireError error;
  error.code = Code::kUnknown;
  const auto parsed = std::strtol(status.c_str(), nullptr, 10);
  if (parsed >= 0 && parsed <= 16) {
    error.code = static_cast<Code>(parsed);
  }
  error.code_name = ToString(error.code);
  error.message = response.Header("grpc-message");
  if (error.message.empty()) {
    error.message = "the server sent no message";
  }
  return error;
}

std::optional<WireError> DecodeStreamError(ContentType request_type,
                                           const std::vector<EnvelopeFrame>& frames,
                                           const HttpResponse& headers_only) {
  if (IsGrpcWeb(request_type)) {
    // gRPC-Web streaming carries its trailers in a **frame** with the 0x80 flag
    // set. Without one the stream ended without a status, which is a failure of
    // the transport rather than of the call.
    for (auto it = frames.rbegin(); it != frames.rend(); ++it) {
      if (it->flags != kFrameTrailers) {
        continue;
      }
      // The trailers frame's payload is `key: value\r\n` lines.
      std::string status;
      std::string message;
      std::string details;
      std::size_t at = 0;
      while (at < it->payload.size()) {
        const std::size_t end = it->payload.find("\r\n", at);
        const std::string line =
            it->payload.substr(at, end == std::string::npos ? std::string::npos : end - at);
        at = end == std::string::npos ? it->payload.size() : end + 2;
        const std::size_t colon = line.find(':');
        if (colon == std::string::npos) {
          continue;
        }
        std::string name = line.substr(0, colon);
        std::string value = line.substr(colon + 1);
        while (!name.empty() && name.front() == ' ') name.erase(name.begin());
        while (!value.empty() && value.front() == ' ') value.erase(value.begin());
        if (name == "grpc-status") status = value;
        if (name == "grpc-message") message = value;
        if (name == "grpc-status-details-bin") details = value;
      }
      if (status == "0" || status.empty()) {
        return std::nullopt;
      }
      if (!details.empty()) {
        if (std::optional<WireError> decoded = DecodeGrpcWebStatusDetails(details)) {
          return decoded;
        }
      }
      WireError error;
      const auto parsed = std::strtol(status.c_str(), nullptr, 10);
      error.code = (parsed >= 0 && parsed <= 16) ? static_cast<Code>(parsed) : Code::kUnknown;
      error.code_name = ToString(error.code);
      error.message = message.empty() ? "the server sent no message" : message;
      return error;
    }
    return std::nullopt;
  }

  // Connect streaming: the end-of-stream frame carries `{}` or `{"error": ...}`.
  for (auto it = frames.rbegin(); it != frames.rend(); ++it) {
    if (it->flags == kFrameEndOfStream) {
      return DecodeConnectEndOfStream(it->payload);
    }
  }
  return std::nullopt;
}

std::string EncodeGrpcWebStatusDetails(const WireError& error) {
  // `google.rpc.Status` { code: 1, message: 2, details: 3 }.
  std::string status;
  PutTag(&status, 1, 0);
  PutVarint(&status, static_cast<std::uint64_t>(error.code));
  PutBytes(&status, 2, error.message);
  if (error.has_error_info || !error.unknown_reason.empty()) {
    loams::errors::v1::ErrorInfo info;
    info.set_reason(error.reason == Reason::kUnknown ? error.unknown_reason : std::string(ToString(error.reason)));
    for (const auto& entry : error.metadata) {
      (*info.mutable_metadata())[entry.first] = entry.second;
    }
    info.set_hint(error.hint);
    std::string detail_bytes;
    if (!info.SerializeToString(&detail_bytes)) {
      // An `ErrorInfo` that will not serialise cannot be sent as a detail, and a
      // failure carrying no detail is the one thing R8 says must not happen — so
      // the frame carries the code and message and no detail, which is honest.
      detail_bytes.clear();
    }
    std::string any;
    PutBytes(&any, 1, "type.googleapis.com/loams.errors.v1.ErrorInfo");
    PutBytes(&any, 2, detail_bytes);
    PutBytes(&status, 3, any);
  }
  return Base64Encode(status);
}

std::string EncodeConnectEndOfStreamFrame(const WireError& error) {
  google::protobuf::Struct inner;
  (*inner.mutable_fields())["code"] = google::protobuf::Value();
  (*inner.mutable_fields())["code"].set_string_value(std::string(ToString(error.code)));
  (*inner.mutable_fields())["message"] = google::protobuf::Value();
  (*inner.mutable_fields())["message"].set_string_value(error.message);
  if (error.has_error_info || !error.unknown_reason.empty()) {
    loams::errors::v1::ErrorInfo info;
    info.set_reason(error.reason == Reason::kUnknown ? error.unknown_reason : std::string(ToString(error.reason)));
    for (const auto& entry : error.metadata) {
      (*info.mutable_metadata())[entry.first] = entry.second;
    }
    info.set_hint(error.hint);
    std::string detail_bytes;
    if (!info.SerializeToString(&detail_bytes)) {
      detail_bytes.clear();
    }
    google::protobuf::Struct detail;
    (*detail.mutable_fields())["type"] = google::protobuf::Value();
    (*detail.mutable_fields())["type"].set_string_value("loams.errors.v1.ErrorInfo");
    (*detail.mutable_fields())["value"] = google::protobuf::Value();
    (*detail.mutable_fields())["value"].set_string_value(Base64Encode(detail_bytes));
    google::protobuf::Value detail_value;
    detail_value.mutable_struct_value()->Swap(&detail);
    google::protobuf::Value details;
    *details.mutable_list_value()->add_values() = detail_value;
    (*inner.mutable_fields())["details"] = details;
  }
  // The Connect end-of-stream frame wraps the failure: `{"error": {...}}`. Without
  // the wrapper the payload is indistinguishable from a clean `{}`-shaped body,
  // and a reader that looks for `error` would report every stream failure as a
  // clean end.
  google::protobuf::Struct body;
  (*body.mutable_fields())["error"].mutable_struct_value()->Swap(&inner);
  std::string json;
  const auto status = google::protobuf::util::MessageToJsonString(body, &json);
  if (!status.ok()) {
    return EncodeEnvelopeFrame(kFrameEndOfStream, R"({"error":{"code":"internal"}})");
  }
  return EncodeEnvelopeFrame(kFrameEndOfStream, json);
}

}  // namespace loams