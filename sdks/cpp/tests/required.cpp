// The required-fixture driver: every required fixture, read from the corpus and
// driven through the SDK. See `required.hpp` for what it is for and why coverage
// is derived rather than listed.

#include "required.hpp"

#include <algorithm>
#include <map>
#include <set>

#include <chrono>
#include <cstdlib>
#include <iostream>

#include <google/protobuf/descriptor.h>
#include <google/protobuf/message.h>
#include <google/protobuf/util/json_util.h>

#include "loams/approvals/v1/approvals.pb.h"
#include "loams/devices/v1/devices.pb.h"
#include "loams/instance/v1/instance.pb.h"
#include "loams/live/v1/live.pb.h"
#include "loams/notifications/v1/notifications.pb.h"
#include "loams/operations/v1/operations.pb.h"

namespace loams_test {
namespace {

using loams::Code;
using loams::ContentType;
using loams::LoamsError;
using loams::Reason;

/// Every service descriptor this SDK's generated files declare.
///
/// Read off the **generated files**, which `protoc --cpp_out` emits a descriptor
/// for even though it emits no service *class* (C++ has no per-service
/// generated type the way Go has a client). Reached through a message type rather
/// than through the generated descriptor pool so that referencing it pulls the
/// object's static initialiser in, which is what registers the file.
///
/// From the generated files rather than from a list of names, so a fixture on a
/// service this SDK does not speak says so — "the corpus drives it and the fixture
/// cannot be run until the SDK speaks the service" — instead of failing obscurely.
const std::vector<const google::protobuf::ServiceDescriptor*>& Services() {
  static const std::vector<const google::protobuf::ServiceDescriptor*>* const services =
      [] {
        auto* const built = new std::vector<const google::protobuf::ServiceDescriptor*>();
        const std::vector<const google::protobuf::FileDescriptor*> files{
            loams::approvals::v1::ListApprovalsRequest::descriptor()->file(),
            loams::devices::v1::ListDevicesRequest::descriptor()->file(),
            loams::instance::v1::GetInstanceRequest::descriptor()->file(),
            loams::live::v1::QueryRequest::descriptor()->file(),
            loams::notifications::v1::ListNotificationsRequest::descriptor()->file(),
            loams::operations::v1::ListOperationsRequest::descriptor()->file(),
        };
        for (const google::protobuf::FileDescriptor* const file : files) {
          for (int at = 0; at < file->service_count(); ++at) {
            built->push_back(file->service(at));
          }
        }
        return built;
      }();
  return *services;
}

/// A new instance of a generated message type, from its descriptor.
///
/// The **generated** factory rather than a `DynamicMessage`: a dynamic message
/// would serialise identically but would not be the type the facade's signatures
/// name, and the point of the driver is that the SDK's own encoder is exercised.
google::protobuf::Message* NewMessage(const google::protobuf::Descriptor* descriptor) {
  return google::protobuf::MessageFactory::generated_factory()->GetPrototype(descriptor)->New();
}

/// The service a recorded path names, and the method on it.
struct Route {
  const google::protobuf::ServiceDescriptor* service = nullptr;
  const google::protobuf::MethodDescriptor* method = nullptr;
  std::string rpc;
};

/// Reads `/package.Service/Method` off a recorded path.
///
/// Off the path rather than from a table, because the path is what the call
/// actually goes to: a table would be a second list of RPCs to keep in step with
/// the protos, and it would quietly skip a fixture whose service this SDK has
/// never heard of instead of saying so.
Route RouteOf(const std::string& path) {
  const std::size_t last = path.find_last_of('/');
  if (last == std::string::npos || last == 0) {
    throw Failed{"the recorded path " + path + " names no service"};
  }
  const std::string service_name = path.substr(1, last - 1);
  const std::string method_name = path.substr(last + 1);
  Route route;
  route.rpc = service_name + "/" + method_name;
  for (const google::protobuf::ServiceDescriptor* const service : Services()) {
    if (std::string(service->full_name()) == service_name) {
      route.service = service;
      break;
    }
  }
  if (route.service == nullptr) {
    throw Failed{"this SDK has no generated descriptor for " + service_name +
                 "; the corpus drives it and the fixture cannot be run until the SDK speaks the service"};
  }
  route.method = route.service->FindMethodByName(method_name);
  if (route.method == nullptr) {
    throw Failed{std::string(route.service->full_name()) + " has no method " + method_name};
  }
  return route;
}

/// The recorded request, decoded into the generated message the RPC takes.
///
/// This is what makes the fixture server's byte-for-byte comparison of the
/// request a real check on **this SDK's encoder**: field order, an enum's
/// spelling, a `uint64` as a string. Sending the recorded bytes verbatim would
/// pass no matter what the SDK wrote.
std::unique_ptr<google::protobuf::Message> DecodeRequest(const Route& route, const RecordedStep& step) {
  const std::optional<ContentType> type = loams::ContentTypeFromName(step.content_type);
  if (!type.has_value()) {
    throw Failed{"the recorded request declares an encoding this SDK does not speak: " + step.content_type};
  }
  std::string payload = step.body;
  if (loams::IsFramed(*type) && payload.size() >= 5) {
    // gRPC-Web and Connect both wrap the message in a 5-byte envelope: a flag
    // byte then a big-endian length.
    payload = payload.substr(5);
  }
  std::unique_ptr<google::protobuf::Message> request(NewMessage(route.method->input_type()));
  if (loams::IsJson(*type)) {
    if (!google::protobuf::util::JsonStringToMessage(payload, request.get()).ok()) {
      throw Failed{"the recorded request body is not the proto3 JSON of " + route.rpc};
    }
  } else if (!request->ParseFromString(payload)) {
    throw Failed{"the recorded request body is not the protobuf of " + route.rpc};
  }
  return request;
}

/// Whether this step is one the SDK's own keyed path cannot reproduce.
///
/// **D610 gives every mutation an idempotency key**, and several app-mock
/// mutations were recorded *without* one. Putting a key on the wire would change
/// the request, and the fixture server — correctly — refuses a request that is not
/// the recorded one.
///
/// Decided from the **request schema** and the decoded message rather than from a
/// list of fixture names, so a corpus that grows a keyed mutation is handled by
/// the same rule and not by somebody remembering to move it.
bool KeylessMutation(const google::protobuf::Message& request) {
  if (!loams::DeclaresIdempotencyKey(request)) {
    return false;
  }
  const std::optional<std::string> carried = loams::ReadIdempotencyKey(request);
  return !carried.has_value() || carried->empty();
}

/// The Connect code a recorded refusal carries.
///
/// Read from the recording's own bytes, in the order the protocol files them,
/// because **an HTTP status cannot tell these refusals apart**: `400` covers
/// `invalid_argument` *and* `failed_precondition`, and a gRPC-Web refusal is a
/// `200` with the code in its trailers. A driver that mapped the status would be
/// asserting a coincidence and would start failing the moment the corpus grew a
/// refusal in a combination the table had no row for.
std::optional<Code> RecordedCode(const RecordedStep& step) {
  if (step.expect.grpc_status.has_value()) {
    return static_cast<Code>(*step.expect.grpc_status);
  }
  if (!step.response_body.empty()) {
    // Connect unary: `{"code": "unimplemented", ...}`.
    const std::optional<google::protobuf::Struct> body = ParseJson(step.response_body);
    if (body.has_value()) {
      const std::string name = StringAt(*body, "code");
      const std::optional<Code> code = loams::CodeFromString(name);
      if (code.has_value()) {
        return code;
      }
    }
  }
  // gRPC-Web: the code is in a trailers frame inside a `200`.
  if (const std::optional<std::vector<loams::EnvelopeFrame>> frames = loams::DecodeEnvelopeFrames(step.response_body);
      frames.has_value()) {
    for (const loams::EnvelopeFrame& frame : *frames) {
      if (frame.flags == loams::kFrameTrailers) {
        const std::size_t at = frame.payload.find("grpc-status:");
        if (at != std::string::npos) {
          return static_cast<Code>(std::strtol(frame.payload.c_str() + at + 12, nullptr, 10));
        }
      }
    }
  }
  // Connect stream: the failure is in the end-of-stream frame. The `loams dev`
  // streaming refusal files its answer in `bodyBase64` with **no** `frames` array —
  // one frame, split out of the bytes — so the whole body is a candidate too.
  std::vector<std::string> candidates = step.frames;
  if (candidates.empty() && !step.response_body.empty()) {
    const std::optional<std::vector<loams::EnvelopeFrame>> decoded =
        loams::DecodeEnvelopeFrames(step.response_body);
    if (decoded.has_value()) {
      for (const loams::EnvelopeFrame& frame : *decoded) {
        candidates.push_back(loams::EncodeEnvelopeFrame(frame.flags, frame.payload));
      }
    }
  }
  for (const std::string& frame : candidates) {
    const std::optional<std::vector<loams::EnvelopeFrame>> decoded = loams::DecodeEnvelopeFrames(frame);
    if (!decoded.has_value() || decoded->empty()) {
      continue;
    }
    if (decoded->front().flags != loams::kFrameEndOfStream) {
      continue;
    }
    const std::optional<google::protobuf::Struct> end = ParseJson(decoded->front().payload);
    if (!end.has_value()) {
      continue;
    }
    const google::protobuf::Value* const error = Field(*end, "error");
    if (error == nullptr || !error->has_struct_value()) {
      continue;
    }
    const std::optional<Code> code = loams::CodeFromString(StringAt(error->struct_value(), "code"));
    if (code.has_value()) {
      return code;
    }
  }
  return std::nullopt;
}

/// The gRPC code an HTTP status carries when nothing in the body says better.
Code CodeForStatus(long status) {
  switch (status) {
    case 400:
      return Code::kInvalidArgument;
    case 401:
      return Code::kUnauthenticated;
    case 403:
      return Code::kPermissionDenied;
    case 404:
      return Code::kNotFound;
    case 408:
    case 504:
      return Code::kDeadlineExceeded;
    case 409:
      return Code::kAborted;
    case 412:
      return Code::kFailedPrecondition;
    case 429:
      return Code::kResourceExhausted;
    case 501:
      return Code::kUnimplemented;
    case 503:
      return Code::kUnavailable;
    default:
      return Code::kInternal;
  }
}

/// Whether a step's recorded answer is a refusal.
///
/// Three signals, because the corpus files a refusal three ways: an HTTP status
/// of 400 or more (Connect), a `grpc-status` in the trailers under a `200`
/// (gRPC-Web), and a declared `reason` — which is what makes the difference
/// visible, since `mock_status_unauthenticated` declares `reason: null` and is
/// still a refusal, and a gRPC-Web refusal declares a reason under a `200`.
bool IsRefusal(const RecordedStep& step) {
  return step.expect.grpc_status.has_value() || step.expect.status >= 400 || step.expect.has_reason;
}

/// Whether the only thing that went wrong is that the recording stopped.
///
/// A recorded stream is a bounded **prefix**: the recorder closed the connection,
/// so there is no end frame and the reader reports the missing one once the
/// recorded frames are through. Accepting *any* error here would make the
/// truncation a loophole, so it is narrowed twice: the recording has to declare
/// itself truncated, and the failure has to be the missing end frame and nothing
/// else.
bool IsEndOfRecording(const RecordedStep& step, const std::exception& error) {
  if (!step.truncated) {
    return false;
  }
  const std::string message = error.what();
  return message.find("stream ended") != std::string::npos || message.find("missing end") != std::string::npos;
}

/// The name of an enum value, read through the field's own descriptor.
///
/// Declared before its use because the assertions are written in the order a
/// reader wants them — refusal, frames, cursor, state — and hoisting the enum
/// reader above them would bury the one that matters.
std::string ReadEnumName(const google::protobuf::Message& message, const google::protobuf::FieldDescriptor* field);

/// What one step produced, after the SDK had its way with it.
struct Outcome {
  /// The messages the SDK read, in order. Empty when the call was refused.
  std::vector<std::unique_ptr<google::protobuf::Message>> messages;
  /// The same messages as proto3 JSON, which is how two answers are compared.
  std::vector<std::string> json;
  /// The mapped failure, or null when the call answered.
  std::exception_ptr error;
  /// The cursor this step's **request** asked to resume from.
  ///
  /// Carried separately from the messages because R7 is a claim about the two
  /// directions agreeing: the cursor in the request has to be the one a previous
  /// response handed out, and reading it back off a response would make that
  /// vacuous.
  std::optional<std::string> asked_resume_cursor;
};

/// The `cursor` a stream response handed back, or nothing.
std::optional<std::string> HandedCursor(const google::protobuf::Message& message) {
  const google::protobuf::Descriptor* const descriptor = message.GetDescriptor();
  if (descriptor == nullptr) {
    return std::nullopt;
  }
  const google::protobuf::FieldDescriptor* const field = descriptor->FindFieldByName("cursor");
  if (field == nullptr || field->cpp_type() != google::protobuf::FieldDescriptor::CPPTYPE_STRING) {
    return std::nullopt;
  }
  const std::string value = message.GetReflection()->GetString(message, field);
  return value.empty() ? std::nullopt : std::optional<std::string>(value);
}

/// The `resume_cursor` a stream request carried, or nothing.
std::optional<std::string> AskedCursor(const google::protobuf::Message& request) {
  const google::protobuf::Descriptor* const descriptor = request.GetDescriptor();
  if (descriptor == nullptr) {
    return std::nullopt;
  }
  const google::protobuf::FieldDescriptor* const field = descriptor->FindFieldByName("resume_cursor");
  if (field == nullptr || field->cpp_type() != google::protobuf::FieldDescriptor::CPPTYPE_STRING) {
    return std::nullopt;
  }
  const std::string value = request.GetReflection()->GetString(request, field);
  return value.empty() ? std::nullopt : std::optional<std::string>(value);
}

/// The branch of a stream message's `oneof`, which is what a frame **is**:
/// `snapshot`, `upsert`, `remove` or `heartbeat`.
std::string EventOf(const google::protobuf::Message& message) {
  const google::protobuf::Descriptor* const descriptor = message.GetDescriptor();
  if (descriptor == nullptr) {
    return "none";
  }
  // The `oneof` is found by its **name**, not through a field of that name: a
  // `oneof event { snapshot, upsert, remove, heartbeat }` has four fields and none
  // is called `event`, so `FindFieldByName("event")` finds nothing and every frame
  // reads as `none`.
  const google::protobuf::OneofDescriptor* oneof = descriptor->FindOneofByName("event");
  if (oneof == nullptr) {
    // A message whose branch is a single field rather than a oneof: the field's
    // own name is the branch.
    const google::protobuf::FieldDescriptor* const field = descriptor->FindFieldByName("event");
    if (field == nullptr || !message.GetReflection()->HasField(message, field)) {
      return "none";
    }
    return std::string(field->name());
  }
  // The **set** field's name is the branch, read through the oneof rather than
  // through the field we found by name: a `oneof event { snapshot, upsert, remove,
  // heartbeat }` is four fields with one value, and which one is set is what
  // "the frame is an upsert" means.
  const google::protobuf::FieldDescriptor* const set =
      message.GetReflection()->GetOneofFieldDescriptor(message, oneof);
  return set == nullptr ? std::string("none") : std::string(set->name());
}

/// The proto3 JSON of a message, for comparing two answers.
std::string ToJson(const google::protobuf::Message& message) {
  std::string json;
  if (!google::protobuf::util::MessageToJsonString(message, &json).ok()) {
    return std::string();
  }
  return json;
}

/// Replays one recorded step through the SDK and returns what it made of it.
Outcome ReplayStep(const std::string& fixture, const RecordedStep& step, std::size_t index,
                   loams::Client& client) {
  const Route route = RouteOf(step.path);
  const std::optional<ContentType> type = loams::ContentTypeFromName(step.content_type);
  if (!type.has_value()) {
    throw Failed{"the recorded request declares an encoding this SDK does not speak: " + step.content_type};
  }
  const std::unique_ptr<google::protobuf::Message> request = DecodeRequest(route, step);
  const loams::MethodBinding& binding = loams::BindingFor(route.rpc);

  // Which recording answers, when several answer the same key, and which step of
  // it. Both are the fixture server's own contract: six recorded scenarios are
  // the same RPC over the same encoding, and two of them send byte-identical
  // requests twice, so the step cannot be inferred.
  std::vector<std::pair<std::string, std::string>> headers{
      {"loams-fixture-name", fixture},
      {"loams-fixture-step", std::to_string(index)},
  };
  Outcome outcome;
  outcome.asked_resume_cursor = AskedCursor(*request);

  if (binding.server_streaming) {
    loams::MessageStream stream = client.OpenStream(binding, *request, *type, nullptr, headers);
    while (true) {
      std::unique_ptr<google::protobuf::Message> message(NewMessage(route.method->output_type()));
      if (!stream.Receive(message.get())) {
        outcome.error = stream.Error();
        break;
      }
      outcome.json.push_back(ToJson(*message));
      outcome.messages.push_back(std::move(message));
    }
    return outcome;
  }

  if (!KeylessMutation(*request)) {
    // The SDK's own keyed path: R3's key is minted before the first attempt and
    // every retry re-sends it.
    std::unique_ptr<google::protobuf::Message> response(NewMessage(route.method->output_type()));
    try {
      client.Unary(binding, *request, response.get(), *type, std::string(), headers);
      outcome.json.push_back(ToJson(*response));
      outcome.messages.push_back(std::move(response));
    } catch (...) {
      outcome.error = std::current_exception();
    }
    return outcome;
  }

  // A recorded keyless mutation: driven through `Invoke` with no key, which is
  // the SDK's own call path with R3 deliberately not applied — same transport,
  // same credentials, same retry decision, same error mapping. The body is the
  // SDK's own encoding of the decoded request, so the fixture server's byte-for
  // byte comparison is still a check on this encoder.
  loams::CallPlan plan;
  plan.binding = binding;
  plan.content_type = *type;
  plan.body = client.EncodeBody(*request, *type);
  plan.headers = headers;
  std::unique_ptr<google::protobuf::Message> response(NewMessage(route.method->output_type()));
  try {
    client.Invoke(plan, client.Transport(),
                  [&client](const loams::HttpRequest& sent, loams::HttpResponse& answered) {
                    answered = client.Transport().Send(sent);
                  },
                  [&client, &response, type = *type, &binding](const loams::HttpResponse& answered) {
                    static_cast<void>(client);
                    std::string body = answered.body;
                    if (loams::IsFramed(type)) {
                      const std::optional<std::vector<loams::EnvelopeFrame>> frames =
                          loams::DecodeEnvelopeFrames(answered.body);
                      if (!frames.has_value() || frames->empty()) {
                        loams::ThrowInternal(binding.rpc, "the response was not one frame");
                      }
                      body = frames->front().payload;
                    }
                    const bool parsed = loams::IsJson(type)
                                            ? google::protobuf::util::JsonStringToMessage(body, response.get()).ok()
                                            : response->ParseFromString(body);
                    if (!parsed) {
                      loams::ThrowInternal(binding.rpc, "the response body did not parse");
                    }
                  });
    outcome.json.push_back(ToJson(*response));
    outcome.messages.push_back(std::move(response));
  } catch (...) {
    outcome.error = std::current_exception();
  }
  return outcome;
}

/// Asserts one step's outcome against what the recording says it must be.
void AssertStep(const std::string& fixture, std::size_t index, const RecordedStep& step, const Outcome& outcome,
                const std::vector<std::vector<std::string>>& earlier) {
  const std::string where = fixture + " step " + std::to_string(index);
  const Expect& expect = step.expect;

  if (IsRefusal(step)) {
    if (!outcome.error) {
      throw Failed{where + " is recorded as a refusal and the SDK returned " +
                   std::to_string(outcome.messages.size()) + " message(s)"};
    }
    try {
      std::rethrow_exception(outcome.error);
    } catch (const LoamsError& error) {
      const std::optional<Code> wanted = RecordedCode(step);
      const Code expected_code = wanted.value_or(CodeForStatus(expect.status));
      if (error.CodeValue() != expected_code) {
        throw Failed{where + " is recorded as code " + std::string(loams::ToString(expected_code)) +
                     " and the SDK raised " + std::string(loams::ToString(error.CodeValue())) + ": " + error.what()};
      }
      if (expect.has_reason) {
        // `reason: null` is a recorded fact — the server sent no `ErrorInfo` — so
        // the SDK has to report no reason either, and `kUnknown` with an empty
        // `UnknownReason` is what "none" looks like on the wire.
        const bool sent_none = !error.UnknownReason().empty() || error.ReasonValue() != Reason::kUnknown;
        if (!expect.reason.has_value() && sent_none) {
          throw Failed{where + " is recorded as carrying no reason and the SDK read " +
                       std::string(loams::ToString(error.ReasonValue()))};
        }
        if (expect.reason.has_value()) {
          const std::string read = error.ReasonValue() == Reason::kUnknown ? error.UnknownReason()
                                                                           : std::string(loams::ToString(error.ReasonValue()));
          if (read != *expect.reason) {
            throw Failed{where + " carries reason " + *expect.reason + " and the SDK read " + read};
          }
        }
      }
    }
    return;
  }

  if (outcome.error) {
    bool acceptable = false;
    try {
      std::rethrow_exception(outcome.error);
    } catch (const std::exception& error) {
      acceptable = IsEndOfRecording(step, error);
      if (!acceptable) {
        throw Failed{where + " answers " + std::to_string(expect.status) + " and the SDK raised: " + error.what()};
      }
    }
  }

  if (expect.frames.has_value() &&
      outcome.messages.size() != static_cast<std::size_t>(*expect.frames)) {
    throw Failed{where + " recorded " + std::to_string(*expect.frames) + " frame(s) and the SDK read " +
                 std::to_string(outcome.messages.size())};
  }
  if (!expect.frame_kinds.empty()) {
    std::vector<std::string> kinds;
    for (const auto& message : outcome.messages) {
      kinds.push_back(EventOf(*message));
    }
    std::string read;
    for (const std::string& kind : kinds) {
      read += (read.empty() ? "" : ", ") + kind;
    }
    std::string wanted;
    for (const std::string& kind : expect.frame_kinds) {
      wanted += (wanted.empty() ? "" : ", ") + kind;
    }
    if (read != wanted) {
      throw Failed{where + " recorded frames [" + wanted + "] and the SDK read [" + read + "]"};
    }
  }
  if (expect.cursor.has_value()) {
    bool seen = false;
    for (const auto& message : outcome.messages) {
      if (HandedCursor(*message) == expect.cursor) {
        seen = true;
      }
    }
    if (!seen) {
      throw Failed{where + " recorded cursor " + *expect.cursor + " and the SDK read none of them"};
    }
  }
  if (expect.snapshot_reset.has_value()) {
    const google::protobuf::Descriptor* const descriptor =
        outcome.messages.empty() ? nullptr : outcome.messages.front()->GetDescriptor();
    const google::protobuf::FieldDescriptor* const field =
        descriptor == nullptr ? nullptr : descriptor->FindFieldByName("snapshot_reset");
    bool seen = false;
    for (const auto& message : outcome.messages) {
      if (field != nullptr && message->GetReflection()->GetBool(*message, field) == *expect.snapshot_reset) {
        seen = true;
      }
    }
    if (!seen) {
      throw Failed{where + " recorded snapshotReset " + std::string(*expect.snapshot_reset ? "true" : "false") +
                   " and the SDK read none of them"};
    }
  }
  if (!expect.api_versions.empty()) {
    std::string served;
    for (const auto& message : outcome.messages) {
      const google::protobuf::Descriptor* const descriptor = message->GetDescriptor();
      const google::protobuf::FieldDescriptor* const field =
          descriptor == nullptr ? nullptr : descriptor->FindFieldByName("api_versions");
      if (field == nullptr) {
        continue;
      }
      for (int at = 0; at < message->GetReflection()->FieldSize(*message, field); ++at) {
        served += message->GetReflection()->GetRepeatedString(*message, field, at) + " ";
      }
    }
    for (const std::string& version : expect.api_versions) {
      if (served.find(version) == std::string::npos) {
        throw Failed{where + " serves [" + served + "], which does not include " + version};
      }
    }
  }
  if (!outcome.messages.empty()) {
    const google::protobuf::Message& answer = *outcome.messages.front();
    const google::protobuf::Descriptor* const descriptor = answer.GetDescriptor();
    const google::protobuf::FieldDescriptor* const approval_field =
        descriptor == nullptr ? nullptr : descriptor->FindFieldByName("approval");
    if (approval_field != nullptr && approval_field->message_type() != nullptr) {
      const google::protobuf::Message& approval =
          answer.GetReflection()->GetMessage(answer, approval_field);
      if (expect.state.has_value()) {
        const google::protobuf::FieldDescriptor* const state =
            approval.GetDescriptor()->FindFieldByName("state");
        // An enum is a number in memory and a name in the recording, so it is read
        // back through the **response's own descriptor** and compared by name.
        // Comparing `String(2)` against `APPROVAL_STATE_APPROVED` would pass for
        // every approval state in the proto and fail for every reason that matters.
        const std::string named = state == nullptr ? std::string() : ReadEnumName(approval, state);
        if (named != *expect.state) {
          throw Failed{where + " answers state " + (named.empty() ? std::string("<none>") : named) + ", not " +
                         *expect.state};
        }
      }
      if (expect.revision.has_value()) {
        const google::protobuf::FieldDescriptor* const revision =
            approval.GetDescriptor()->FindFieldByName("revision");
        if (revision != nullptr) {
          const std::uint64_t value =
              static_cast<std::uint64_t>(approval.GetReflection()->GetUInt64(approval, revision));
          if (std::to_string(value) != *expect.revision) {
            throw Failed{where + " answers revision " + std::to_string(value) + ", not " + *expect.revision};
          }
        }
      }
    }
  }
  if (expect.identical_to_step.has_value()) {
    const std::size_t at = static_cast<std::size_t>(*expect.identical_to_step);
    if (at >= earlier.size()) {
      throw Failed{where + " is recorded as identical to step " + std::to_string(at) + ", which does not exist"};
    }
    if (earlier[at] != outcome.json) {
      throw Failed{where + " is recorded as byte-identical to step " + std::to_string(at) +
                   ", and the SDK's two answers differ"};
    }
  }
}

std::string ReadEnumName(const google::protobuf::Message& message, const google::protobuf::FieldDescriptor* field) {
  const google::protobuf::EnumDescriptor* const enumeration = field->enum_type();
  if (enumeration == nullptr) {
    return std::string();
  }
  // `GetEnumValue` and not `GetEnum`: the latter is deprecated, this build turns
  // deprecation warnings into errors, and a harness that cannot compile is not one
  // anybody trusts. `GetEnumValue` returns the **number**, so the descriptor is
  // looked up from it here.
  const google::protobuf::EnumValueDescriptor* const found =
      enumeration->FindValueByNumber(message.GetReflection()->GetEnumValue(message, field));
  return found == nullptr ? std::string() : std::string(found->name());
}

/// Drives one fixture's steps in order, asserting each against its recording.
void DriveFixture(const std::string& name, const Recorded& recorded, loams::Client& client) {
  std::vector<std::vector<std::string>> answers;
  std::optional<std::string> last_cursor;
  for (std::size_t index = 0; index < recorded.steps.size(); ++index) {
    const RecordedStep& step = recorded.steps[index];
    const Outcome outcome = ReplayStep(name, step, index, client);
    AssertStep(name, index, step, outcome, answers);
    answers.push_back(outcome.json);
    // R7 across steps: a step that resumes must carry a cursor a previous step
    // handed out, not one written into the test. That is the whole invariant of
    // `mock_state_stream_resume`, and it is checked here rather than per fixture.
    if (outcome.asked_resume_cursor.has_value() && last_cursor.has_value() &&
        *outcome.asked_resume_cursor != *last_cursor) {
      throw Failed{name + " step " + std::to_string(index) + " resumes from " + *outcome.asked_resume_cursor +
                   ", but the stream last handed out " + *last_cursor};
    }
    for (const auto& message : outcome.messages) {
      const std::optional<std::string> cursor = HandedCursor(*message);
      if (cursor.has_value()) {
        last_cursor = cursor;
      }
    }
  }
}

/// A client for the endpoint, carrying the recorded requests' own bearer.
///
/// The bearer is not invented: each step sends the `authorization` header its
/// recording carried, so the fixture server sees what it saw when the corpus was
/// recorded.
std::shared_ptr<loams::Client> MakeClient(const Endpoint& endpoint) {
  loams::Options options;
  options.endpoint = endpoint.Url();
  if (endpoint.Transport() != nullptr) {
    // Tier 3's replay is owned by the process-wide table the endpoint shares, so a
    // non-owning `shared_ptr` is the right relationship: owning it would take the
    // replay with it at the end of the first client.
    options.transport = std::shared_ptr<loams::HttpTransport>(endpoint.Transport(), [](loams::HttpTransport*) {});
  }
  // No retries: a replay is not a client. An answer a retry would have asked for
  // again must surface as the failure it is, rather than be retried into a second
  // recorded answer.
  options.max_retries = 0;
  // A short deadline: the fixture server answers in milliseconds, so a call that
  // has taken ten seconds is a call that is never going to, and twenty-eight
  // fixtures at the SDK's default would turn a hung endpoint into a suite that
  // looks slow rather than one that looks broken.
  options.timeout = std::chrono::seconds(10);
  return loams::Client::Make(options);
}

}  // namespace

std::vector<ManifestFixture> Manifest() {
  std::vector<ManifestFixture> fixtures;
  const std::optional<google::protobuf::Struct> parsed = ParseJson(ReadFile(FixturesDir() + "/manifest.json"));
  if (!parsed.has_value()) {
    throw Failed{"sdks/fixtures/manifest.json does not parse"};
  }
  const google::protobuf::Value* const list = Field(*parsed, "fixtures");
  if (list == nullptr || !list->has_list_value()) {
    throw Failed{"sdks/fixtures/manifest.json has no fixtures array"};
  }
  for (const google::protobuf::Value& value : list->list_value().values()) {
    if (!value.has_struct_value()) {
      continue;
    }
    const google::protobuf::Struct& row = value.struct_value();
    ManifestFixture fixture;
    fixture.name = StringAt(row, "name");
    fixture.kind = StringAt(row, "kind");
    fixture.server = StringAt(row, "server");
    fixture.file = StringAt(row, "file");
    fixture.transport = StringAt(row, "transport");
    if (const google::protobuf::Value* const required = Field(row, "required")) {
      fixture.required = required->bool_value();
    }
    if (const google::protobuf::Value* const reason = Field(row, "reason"); reason != nullptr) {
      fixture.reason = reason->has_string_value() ? std::optional<std::string>(reason->string_value())
                                                  : std::nullopt;
    }
    fixtures.push_back(std::move(fixture));
  }
  return fixtures;
}

std::vector<ManifestFixture> RequiredFixtures() {
  std::vector<ManifestFixture> required;
  for (ManifestFixture& fixture : Manifest()) {
    if (fixture.required) {
      required.push_back(std::move(fixture));
    }
  }
  return required;
}

std::vector<std::string> DriveRequiredFixtures(const Endpoint& endpoint) {
  // Every recording, by name, read once: thirty-three files read per fixture would
  // be thirty-three files read twenty-eight times.
  //
  // The vector is a **named** local, not a temporary in the range-for: the map
  // holds pointers into it, and a range-for over a temporary extends the
  // temporary's lifetime only for the loop. Every fixture then read a dangling
  // name and reported "a recording is filed under the wrong name", which is the
  // shape of that bug rather than anything about the corpus.
  const std::vector<Recorded> recordings = ReadCorpus();
  std::map<std::string, const Recorded*> corpus;
  for (const Recorded& entry : recordings) {
    corpus[entry.name] = &entry;
  }

  const std::vector<ManifestFixture> required = RequiredFixtures();
  std::shared_ptr<loams::Client> client = MakeClient(endpoint);

  std::vector<std::string> held;
  std::vector<std::string> failures;
  for (const ManifestFixture& fixture : required) {
    try {
      const auto found = corpus.find(fixture.name);
      if (found == corpus.end()) {
        // A **missing** fixture is a failure, not a skip: that is the case where a
        // language would be excused for a test nobody wrote.
        throw Failed{"the manifest names " + fixture.name + " but there is no recording for it"};
      }
      if (found->second->name != fixture.name) {
        throw Failed{"a recording is filed under the wrong name"};
      }
      // One line per fixture, flushed: a run that hangs should say which fixture it
      // hung on, and a run that is slow should be watchable rather than opaque.
      std::cout << "  driving " << fixture.name << std::endl;
      DriveFixture(fixture.name, *found->second, *client);
      held.push_back(fixture.name);
    } catch (const std::exception& error) {
      failures.push_back(fixture.name + ": " + error.what());
    }
  }
  std::vector<std::string> gaps;
  for (const ManifestFixture& fixture : required) {
    const bool held_it = std::find(held.begin(), held.end(), fixture.name) != held.end();
    const bool failed_it = std::any_of(failures.begin(), failures.end(), [&fixture](const std::string& entry) {
      return entry.rfind(fixture.name + ":", 0) == 0;
    });
    if (!held_it && !failed_it) {
      gaps.push_back(fixture.name);
    }
  }
  if (!gaps.empty()) {
    failures.push_back(std::to_string(gaps.size()) + " required fixture(s) were never driven: " +
                       JoinWith(gaps, ", "));
  }
  if (!failures.empty()) {
    // Every failure named, not the first: a suite that missed four of them should
    // learn about all four in one run.
    throw Failed{std::to_string(failures.size()) + " problem(s) across " + std::to_string(required.size()) +
                 " required fixture(s):\n  - " + JoinWith(failures, "\n  - ")};
  }
  return held;
}

}  // namespace loams_test