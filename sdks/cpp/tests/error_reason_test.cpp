// `cpp_error_reason_mapping`.
//
// Design §44 §7.4 (D611) and runtime contract R8: the **code** gives the class (a
// taxonomy that does not change within a major version) and the **`reason`** is
// the stable branch. `reason` is a type in the SDK, generated from the registry
// page `docs/api/reasons.md`, so a caller switching on it is exhaustive and a
// reason the registry has lost stops compiling.
//
// This walks **every** reason in the registry — all twenty-six — round a full
// encode/decode cycle, through the wire form the server actually sends: a Connect
// JSON error body with a base64 `loams.errors.v1.ErrorInfo` in `details`, and a
// gRPC-Web `grpc-status-details-bin` header. It asserts the reason survives, that
// the metadata and the hint survive, and that the class the code names is the
// class the SDK raised.
//
// R8's three cases are asserted as three separate facts, because conflating them
// is the failure the clause names:
//
//   1. a reason from a **newer** server, which this SDK's registry does not have:
//      surfaced as text and flagged, **not** dropped;
//   2. a failure from **below the API** — a socket, a timeout — carrying no
//      `reason` at all;
//   3. a `LoamsError`, returned unchanged if mapped twice.

#include "support.hpp"

#include <string>
#include <vector>

#include "loams/errors/v1/errors.pb.h"

namespace {

using namespace loams;
using namespace loams_test;


std::string ConnectErrorBodyWithReason(const std::string& reason_text, Code code, const std::string& message,
                                       const std::map<std::string, std::string>& metadata,
                                       const std::string& hint) {
  loams::errors::v1::ErrorInfo info;
  info.set_reason(reason_text);
  for (const auto& entry : metadata) {
    (*info.mutable_metadata())[entry.first] = entry.second;
  }
  info.set_hint(hint);
  std::string bytes;
  static_cast<void>(info.SerializeToString(&bytes));
  return std::string("{\"code\":\"") + std::string(ToString(code)) + "\",\"message\":\"" + message +
         "\",\"details\":[{\"type\":\"loams.errors.v1.ErrorInfo\",\"value\":\"" + Base64Encode(bytes) + "\"}]}";
}

/// The code the registry page records for a reason, as R8 says it does: the
/// **code** gives the class. Two reasons on the same code get the same class; two
/// reasons on different codes do not, which is what the second half asserts.
struct RegistryRow {
  Reason reason;
  Code code;
  const char* metadata_key;
};

const std::vector<RegistryRow>& Registry() {
  static const std::vector<RegistryRow>* const rows = new std::vector<RegistryRow>{
      {Reason::kApprovalExpired, Code::kFailedPrecondition, ""},
      {Reason::kApprovalAlreadyDecided, Code::kFailedPrecondition, ""},
      {Reason::kApprovalStaleRevision, Code::kFailedPrecondition, ""},
      {Reason::kRequesterCannotApprove, Code::kPermissionDenied, ""},
      {Reason::kDecisionProofInvalid, Code::kPermissionDenied, ""},
      {Reason::kStepUpRequired, Code::kUnauthenticated, ""},
      {Reason::kReasonRequired, Code::kInvalidArgument, ""},
      {Reason::kInvalidDecision, Code::kInvalidArgument, ""},
      {Reason::kPairingExpired, Code::kFailedPrecondition, ""},
      {Reason::kPairingUsed, Code::kFailedPrecondition, ""},
      {Reason::kDeviceRevoked, Code::kUnauthenticated, ""},
      {Reason::kPushTargetUnknown, Code::kNotFound, ""},
      {Reason::kNotImplemented, Code::kUnimplemented, ""},
      {Reason::kFeatureNotInVariant, Code::kUnimplemented, "variant"},
      {Reason::kInvalidArgument, Code::kInvalidArgument, "field"},
      {Reason::kNotFound, Code::kNotFound, ""},
      {Reason::kAlreadyExists, Code::kAlreadyExists, ""},
      {Reason::kPermissionDenied, Code::kPermissionDenied, ""},
      {Reason::kTokenExpired, Code::kUnauthenticated, ""},
      {Reason::kUnauthenticated, Code::kUnauthenticated, ""},
      {Reason::kFailedPrecondition, Code::kFailedPrecondition, ""},
      {Reason::kResourceExhausted, Code::kResourceExhausted, "retry_after_ms"},
      {Reason::kUnavailable, Code::kUnavailable, ""},
      {Reason::kDeadlineExceeded, Code::kDeadlineExceeded, ""},
      {Reason::kAborted, Code::kAborted, ""},
      {Reason::kInternal, Code::kInternal, ""},
  };
  return *rows;
}

/// The class a code must raise. Returns null when the code is one D611 does not
/// name, in which case a plain `LoamsError` is correct.
const char* ExpectedClass(Code code) {
  switch (code) {
    case Code::kInvalidArgument:
      return "loams::InvalidArgumentError";
    case Code::kNotFound:
      return "loams::NotFoundError";
    case Code::kAlreadyExists:
      return "loams::AlreadyExistsError";
    case Code::kPermissionDenied:
      return "loams::PermissionDeniedError";
    case Code::kUnauthenticated:
      return "loams::UnauthenticatedError";
    case Code::kFailedPrecondition:
      return "loams::FailedPreconditionError";
    case Code::kResourceExhausted:
      return "loams::ResourceExhaustedError";
    case Code::kUnavailable:
      return "loams::UnavailableError";
    case Code::kDeadlineExceeded:
      return "loams::DeadlineExceededError";
    case Code::kAborted:
      return "loams::AbortedError";
    case Code::kInternal:
      return "loams::InternalError";
    case Code::kUnimplemented:
      return "loams::UnimplementedError";
    default:
      return nullptr;
  }
}

/// Decodes one Connect failure body and checks the reason, code, class, metadata
/// and hint all survived.
void CheckOneConnect(Reason reason, Code code, const char* metadata_key, ScriptedTransport* transport) {
  std::map<std::string, std::string> metadata;
  if (metadata_key != nullptr && metadata_key[0] != '\0') {
    metadata[metadata_key] = "standard";
  }
  const std::string body =
      ConnectErrorBodyWithReason(std::string(ToString(reason)), code, "the server said so", metadata, "try again");
  transport->Clear();
  ScriptedTransport::Answer answer;
  // Connect answers a failed RPC with a 4xx/5xx and a JSON body. 501 is
  // `unimplemented`; the status is only a fallback, because the **body** carries
  // the code — which is the whole of R8's "HTTP status is not the error".
  answer.status = 501;
  answer.content_type = "application/json";
  answer.body = body;
  transport->AddAnswer(answer);

  Options options;
  options.endpoint = "http://127.0.0.1:1";
  options.transport = nullptr;
  options.content_type = ContentType::kJson;
  options.max_retries = 0;
  std::shared_ptr<HttpTransport> owned = std::shared_ptr<HttpTransport>(transport);
  options.transport = owned;
  std::shared_ptr<Loams> loams = MakeLoams(std::move(options));

  const std::string label = std::string(ToString(reason));
  bool threw = false;
  try {
    instance::v1::GetInstanceResponse response;
    loams->Instance()->GetInstanceWith(&response, ContentType::kJson);
  } catch (const LoamsError& error) {
    threw = true;
    LOAMS_CHECK_EQ(error.ReasonValue(), reason, label + ": the reason did not survive the round trip");
    LOAMS_CHECK_EQ(static_cast<int>(error.CodeValue()), static_cast<int>(code), label + ": the code");
    LOAMS_CHECK_EQ(error.Hint(), std::string("try again"), label + ": the hint");
    if (metadata_key != nullptr && metadata_key[0] != '\0') {
      const auto found = error.Metadata().find(metadata_key);
      LOAMS_CHECK(found != error.Metadata().end() && found->second == "standard",
                  label + ": metadata[" + metadata_key + "] did not survive");
    }
    LOAMS_CHECK_EQ(error.Rpc(), std::string("loams.instance.v1.InstanceService/GetInstance"),
                   label + ": the RPC name");
    if (const char* const expected = ExpectedClass(code)) {
      if (std::strcmp(expected, "loams::UnimplementedError") == 0) {
        LOAMS_CHECK(dynamic_cast<const UnimplementedError*>(&error) != nullptr,
                    label + ": the class should be an UnimplementedError, got " + error.what());
      } else if (std::strcmp(expected, "loams::InvalidArgumentError") == 0) {
        LOAMS_CHECK(dynamic_cast<const InvalidArgumentError*>(&error) != nullptr, label + ": the class");
      } else if (std::strcmp(expected, "loams::NotFoundError") == 0) {
        LOAMS_CHECK(dynamic_cast<const NotFoundError*>(&error) != nullptr, label + ": the class");
      } else if (std::strcmp(expected, "loams::AlreadyExistsError") == 0) {
        LOAMS_CHECK(dynamic_cast<const AlreadyExistsError*>(&error) != nullptr, label + ": the class");
      } else if (std::strcmp(expected, "loams::PermissionDeniedError") == 0) {
        LOAMS_CHECK(dynamic_cast<const PermissionDeniedError*>(&error) != nullptr, label + ": the class");
      } else if (std::strcmp(expected, "loams::UnauthenticatedError") == 0) {
        LOAMS_CHECK(dynamic_cast<const UnauthenticatedError*>(&error) != nullptr, label + ": the class");
      } else if (std::strcmp(expected, "loams::FailedPreconditionError") == 0) {
        LOAMS_CHECK(dynamic_cast<const FailedPreconditionError*>(&error) != nullptr, label + ": the class");
      } else if (std::strcmp(expected, "loams::ResourceExhaustedError") == 0) {
        LOAMS_CHECK(dynamic_cast<const ResourceExhaustedError*>(&error) != nullptr, label + ": the class");
      } else if (std::strcmp(expected, "loams::UnavailableError") == 0) {
        LOAMS_CHECK(dynamic_cast<const UnavailableError*>(&error) != nullptr, label + ": the class");
      } else if (std::strcmp(expected, "loams::DeadlineExceededError") == 0) {
        LOAMS_CHECK(dynamic_cast<const DeadlineExceededError*>(&error) != nullptr, label + ": the class");
      } else if (std::strcmp(expected, "loams::AbortedError") == 0) {
        LOAMS_CHECK(dynamic_cast<const AbortedError*>(&error) != nullptr, label + ": the class");
      } else if (std::strcmp(expected, "loams::InternalError") == 0) {
        LOAMS_CHECK(dynamic_cast<const InternalError*>(&error) != nullptr, label + ": the class");
      }
    }
  } catch (const std::exception& error) {
    Record(__FILE__, __LINE__, label + ": threw something that is not a LoamsError: " + error.what());
    threw = true;
  }
  LOAMS_CHECK(threw, label + ": the call answered, but the fixture says it must fail");
}

/// The same walk over the gRPC-Web form, which is where the "HTTP status is not
/// the error" half of R8 lives: gRPC-Web answers **200** and puts the code in
/// `grpc-status-details-bin`.
void CheckOneGrpcWeb(Reason reason, Code code) {
  WireError wire;
  wire.code = code;
  wire.message = "the server said so";
  wire.reason = reason;
  wire.has_error_info = true;
  wire.hint = "try again";

  ScriptedTransport transport;
  ScriptedTransport::Answer answer;
  answer.status = 200;
  answer.content_type = "application/grpc-web+proto";
  // A gRPC-Web unary failure is a trailers-only response: an empty body and the
  // status in the headers. A client that reads only the HTTP status sees a success.
  answer.body = std::string();
  answer.trailers.emplace_back("grpc-status", std::to_string(static_cast<int>(code)));
  answer.trailers.emplace_back("grpc-message", "the server said so");
  answer.trailers.emplace_back("grpc-status-details-bin", EncodeGrpcWebStatusDetails(wire));
  transport.AddAnswer(answer);

  Options options;
  options.endpoint = "http://127.0.0.1:1";
  options.transport = std::shared_ptr<HttpTransport>(&transport, [](HttpTransport*) {});
  options.content_type = ContentType::kGrpcWebProto;
  options.max_retries = 0;
  std::shared_ptr<Loams> loams = MakeLoams(std::move(options));

  const std::string label = std::string(ToString(reason)) + " over gRPC-Web";
  bool threw = false;
  try {
    instance::v1::GetInstanceResponse response;
    loams->Instance()->GetInstanceWith(&response, ContentType::kGrpcWebProto);
  } catch (const LoamsError& error) {
    threw = true;
    LOAMS_CHECK_EQ(error.ReasonValue(), reason, label + ": the reason did not survive the trailers");
    LOAMS_CHECK_EQ(static_cast<int>(error.CodeValue()), static_cast<int>(code), label + ": the code");
  }
  LOAMS_CHECK(threw, label + ": the call answered with HTTP 200, which is not the error");
}

}  // namespace

int main() {
  // The registry and the enum must agree, in both directions. A reason added to
  // `docs/api/reasons.md` and not to `loams::Reason` would silently stop being
  // matched, which is how a caller stops branching on it.
  const std::vector<Reason>& all = AllReasons();
  LOAMS_REQUIRE(!all.empty(), "the reason registry is empty");
  LOAMS_CHECK_EQ(all.size(), std::size_t{26}, "the registry should carry all twenty-six reasons");
  LOAMS_CHECK_EQ(Registry().size(), all.size(), "this test's registry table and the enum disagree in size");
  for (const Reason reason : all) {
    LOAMS_CHECK(!ToString(reason).empty(), "a reason with no registry spelling");
    // `snake_case` and unique, which `docs/api/reasons.md`'s own rule states.
    const std::string spelling(ToString(reason));
    for (const char character : spelling) {
      const bool ok = (character >= 'a' && character <= 'z') || (character >= '0' && character <= '9') ||
                      character == '_';
      LOAMS_CHECK(ok, std::string("the spelling of ") + spelling + " is not snake_case");
    }
    const std::optional<Reason> parsed = ReasonFromString(spelling);
    LOAMS_CHECK(parsed.has_value() && *parsed == reason, "the spelling of " + spelling + " does not round trip");
    LOAMS_CHECK(IsRegistryReason(reason), spelling + " should be a registry reason");
  }
  // `kUnknown` is not a registry reason: it is "there was no reason", which is a
  // different fact from "there was a reason and I do not know it".
  LOAMS_CHECK_EQ(std::string(ToString(Reason::kUnknown)), std::string(),
                 "kUnknown must spell as the empty string, not as \"unknown\"");

  // Every reason, round a Connect error body.
  for (const RegistryRow& row : Registry()) {
    CheckOneConnect(row.reason, row.code, row.metadata_key, new ScriptedTransport());
  }

  // Every reason, round the gRPC-Web trailers. This is the encoding that answers
  // HTTP 200, so it is the one that catches a status-code-only client.
  for (const RegistryRow& row : Registry()) {
    CheckOneGrpcWeb(row.reason, row.code);
  }

  // --- R8's three distinct cases ------------------------------------------------------------------
  {
    // 1. A reason from a **newer** server, which this SDK's registry does not
    //    have. Surfaced as text and flagged, not dropped: losing it would leave a
    //    caller unable to tell "not supported here" from "not supported at all".
    const std::string future = "collection_pin_exhausted";
    LOAMS_CHECK(!ReasonFromString(future).has_value(), "the reason \"" + future +
                                                          "\" should not be in this SDK's registry yet, or the test "
                                                          "needs updating");
    ScriptedTransport transport;
    ScriptedTransport::Answer answer;
    answer.status = 501;
    answer.content_type = "application/json";
    answer.body = ConnectErrorBodyWithReason(future, Code::kResourceExhausted, "from a newer server",
                                             {{"retry_after_ms", "5000"}}, "wait");
    transport.AddAnswer(answer);

    Options options;
    options.endpoint = "http://127.0.0.1:1";
    options.transport = std::shared_ptr<HttpTransport>(&transport, [](HttpTransport*) {});
    options.content_type = ContentType::kJson;
    options.max_retries = 0;
    std::shared_ptr<Loams> loams = MakeLoams(std::move(options));

    bool threw = false;
    try {
      instance::v1::GetInstanceResponse response;
      loams->Instance()->GetInstanceWith(&response, ContentType::kJson);
    } catch (const ResourceExhaustedError& error) {
      threw = true;
      LOAMS_CHECK_EQ(error.ReasonValue(), Reason::kUnknown,
                     "a reason this SDK does not have must not be reported as a known one");
      LOAMS_CHECK_EQ(error.UnknownReason(), future, "the newer server's reason was dropped");
      LOAMS_CHECK(std::string(error.what()).find(future) != std::string::npos,
                  "what() should name the unknown reason, got: " + std::string(error.what()));
      // And the metadata survived, so a caller can still act on it.
      const auto found = error.Metadata().find("retry_after_ms");
      LOAMS_CHECK(found != error.Metadata().end() && found->second == "5000",
                  "the metadata of an unknown reason was dropped");
      LOAMS_CHECK_EQ(error.Hint(), std::string("wait"), "the hint of an unknown reason was dropped");
    }
    LOAMS_CHECK(threw, "the newer server's reason produced no error at all");
  }
  {
    // 2. A failure from **below the API**, which carries no `reason` at all. A
    //    transport that throws has no reason, and reporting one would be a lie.
    ScriptedTransport transport;
    Options options;
    options.endpoint = "http://127.0.0.1:1";
    options.transport = std::shared_ptr<HttpTransport>(&transport, [](HttpTransport*) {});
    options.max_retries = 0;
    std::shared_ptr<Loams> loams = MakeLoams(std::move(options));

    bool threw = false;
    try {
      instance::v1::GetInstanceResponse response;
      loams->Instance()->GetInstanceWith(&response, ContentType::kJson);
    } catch (const std::exception& error) {
      threw = true;
      // What makes it R8's second case is that it carries **no reason**, and that
      // it is the dedicated `TransportError` type — not that it is outside the
      // hierarchy, which would leave a caller unable to catch it at all.
      LOAMS_CHECK(IsTransportError(error), "a failure below the API is a TransportError");
      LOAMS_CHECK(CodeOf(error) == Code::kUnknown, "a failure below the API carries no code");
      LOAMS_CHECK(ReasonOf(error) == Reason::kUnknown, "a failure below the API carries no reason");
      LOAMS_CHECK(std::string(error.what()).find("no answer left") != std::string::npos,
                  "and the message names the cause: " + std::string(error.what()));
    }
    LOAMS_CHECK(threw, "the scripted transport ran out of answers and should have failed");
  }
  {
    // 3. A `LoamsError` mapped twice is returned unchanged. In C++ there is no
    //    `errors.As` chain to walk, so "returned unchanged" is `ReasonOf` on an
    //    error that has already been through `ThrowMapped`.
    try {
      ThrowMapped(Code::kNotFound, "loams.instance.v1.InstanceService/GetInstance", Reason::kNotFound, "",
                  {{"field", "collection"}}, "", "no such collection", "req-1");
    } catch (const NotFoundError& first) {
      LOAMS_CHECK_EQ(first.ReasonValue(), Reason::kNotFound, "the first mapping's reason");
      // Mapping it again — which is what wrapping an SDK error does.
      try {
        ThrowMapped(first.CodeValue(), first.Rpc(), first.ReasonValue(), first.UnknownReason(), first.Metadata(),
                    first.Hint(), first.what(), first.RequestId());
      } catch (const NotFoundError& second) {
        LOAMS_CHECK_EQ(second.ReasonValue(), Reason::kNotFound, "a twice-mapped error kept its reason");
        LOAMS_CHECK_EQ(second.Rpc(), first.Rpc(), "a twice-mapped error kept its RPC");
      }
    }
  }

  // R8's typed surface: `feature_not_in_variant` is a **dedicated type**, and the
  // variant is read out of the metadata rather than parsed out of the message.
  {
    ScriptedTransport transport;
    ScriptedTransport::Answer answer;
    answer.status = 501;
    answer.content_type = "application/json";
    answer.body = ConnectErrorBodyWithReason("feature_not_in_variant", Code::kUnimplemented,
                                             "loams.live.v1.LiveService/Query is not in the standard variant",
                                             {{"variant", "standard"}}, "");
    transport.AddAnswer(answer);
    Options options;
    options.endpoint = "http://127.0.0.1:1";
    options.transport = std::shared_ptr<HttpTransport>(&transport, [](HttpTransport*) {});
    options.content_type = ContentType::kJson;
    options.max_retries = 0;
    std::shared_ptr<Loams> loams = MakeLoams(std::move(options));

    bool threw = false;
    try {
      live::v1::QueryRequest request;
      live::v1::QueryResponse response;
      loams->Live()->QueryWith(&request, &response, ContentType::kJson);
    } catch (const FeatureNotInVariantError& error) {
      threw = true;
      LOAMS_CHECK_EQ(error.Variant(), std::string("standard"), "the variant comes from metadata.variant");
      LOAMS_CHECK_EQ(error.ReasonValue(), Reason::kFeatureNotInVariant, "the reason");
      // One `catch` covers "the guard said no" and "the server refused", which is
      // the point of the guard raising the same type.
      const UnimplementedError& as_class = error;
      LOAMS_CHECK_EQ(static_cast<int>(as_class.CodeValue()), static_cast<int>(Code::kUnimplemented), "the code");
    }
    LOAMS_CHECK(threw, "the variant refusal produced no error");
  }

  // The registry's own uniqueness: two reasons with the same spelling would make a
  // caller's `switch` unreachable for one of them.
  {
    std::vector<std::string> spellings;
    for (const Reason reason : AllReasons()) {
      spellings.push_back(std::string(ToString(reason)));
    }
    std::sort(spellings.begin(), spellings.end());
    const auto duplicate = std::adjacent_find(spellings.begin(), spellings.end());
    LOAMS_CHECK(duplicate == spellings.end(), "two reasons share the spelling " + *duplicate);
  }

  // The code-to-string table must cover every code the enum names, in both
  // directions: a code with no spelling would be reported as `<unknown code>`.
  for (const Code code : {Code::kOk, Code::kCanceled, Code::kUnknown, Code::kInvalidArgument,
                          Code::kDeadlineExceeded, Code::kNotFound, Code::kAlreadyExists, Code::kPermissionDenied,
                          Code::kResourceExhausted, Code::kFailedPrecondition, Code::kAborted, Code::kOutOfRange,
                          Code::kUnimplemented, Code::kInternal, Code::kUnavailable, Code::kDataLoss,
                          Code::kUnauthenticated}) {
    const std::string spelling(ToString(code));
    LOAMS_CHECK(!spelling.empty() && spelling != "<unknown code>", "a code with no spelling");
    const std::optional<Code> parsed = CodeFromString(spelling);
    LOAMS_CHECK(parsed.has_value() && *parsed == code, "the spelling of " + spelling + " does not round trip");
  }
  // A code string from a **newer** server is `nullopt`, not `kUnknown`: dropping it
  // would lose the fact that the server is newer.
  LOAMS_CHECK(!CodeFromString("data_truncated").has_value(),
              "an unrecognised code string must be absent rather than kUnknown");

  return Finish("cpp_error_reason_mapping");
}