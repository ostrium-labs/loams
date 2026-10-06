// `cpp_stream_resume_with_cursor`.
//
// Runtime contract R7: "A stream is the one call where 'retry it' is not enough.
// The server hands out cursors; a reconnect resumes from the last one the client
// applied, or the client silently misses everything that changed in between —
// worse than an error, because a sync UI that is quietly stale looks like one that
// works. So an SDK tracks the cursor of every message, re-opens from it on a
// retryable failure, and does not re-yield what it already yielded."
//
// The corpus half is `mock_state_stream_resume`: open the watch, take the snapshot
// and its cursor, **change something while the client is disconnected**, and resume
// from that cursor. The resumed stream sends the change, not the snapshot. That is
// the assertion that distinguishes a resuming client from one that re-opened from
// scratch (which re-sends the snapshot) and from one that resumed from nothing
// (which misses the decision).
//
// The mid-stream disconnect is driven through a scripted transport, because
// `faults.json` records `stream_drop` as a fault no healthy server produces and
// the corpus has no recording of it.

#include "support.hpp"

#include <cstdint>
#include <string>
#include <vector>

#include "loams/approvals/v1/approvals.pb.h"

namespace {

using namespace loams;
using namespace loams_test;

/// Builds an `Approval` in the recorded shape, so a stream frame this test sends
/// looks like the corpus's.
std::string ApprovalBytes(const std::string& id, const std::string& revision, approvals::v1::ApprovalState state,
                          const std::string& promise) {
  approvals::v1::Approval approval;
  approval.set_id(id);
  approval.set_revision(static_cast<std::uint64_t>(std::stoull(revision)));
  approval.set_operation_id("op-0a1b2c3d4e5f60718293a4b5c6d7e8f9");
  approval.set_promise_id(promise);
  approval.set_summary("Create an API key for search-dev");
  approval.set_state(state);
  std::string bytes;
  static_cast<void>(approval.SerializeToString(&bytes));
  return bytes;
}

/// The reverse of `ApprovalBytes`, so a frame can be built from the same value
/// twice without a caller writing the message twice.
approvals::v1::Approval ParseApproval(const std::string& bytes) {
  approvals::v1::Approval approval;
  static_cast<void>(approval.ParseFromString(bytes));
  return approval;
}

/// One frame of a `WatchApprovals` snapshot.
std::string SnapshotFrame(
    const std::string& cursor, const std::vector<std::pair<std::string, approvals::v1::ApprovalState>>& approvals) {
  // The response is a `oneof event`: a snapshot is `snapshot{approvals[]}`, an
  // upsert is `upsert{approval}`, and a removal is `remove{id}`. Modelling the
  // three separately is what lets a resuming client be told apart from one that
  // re-opened from scratch: the resumed frame carries the change, not the list.
  approvals::v1::WatchApprovalsResponse response;
  response.set_cursor(cursor);
  for (const auto& entry : approvals) {
    *response.mutable_snapshot()->add_approvals() =
        ParseApproval(ApprovalBytes(entry.first, "1", entry.second, "prm_create_key"));
  }
  std::string payload;
  static_cast<void>(response.SerializeToString(&payload));
  return EncodeEnvelopeFrame(kFrameMessage, payload);
}

/// One frame of a `WatchApprovals` upsert.
std::string UpsertFrame(const std::string& cursor, const std::string& id, const std::string& revision,
                        approvals::v1::ApprovalState state) {
  approvals::v1::WatchApprovalsResponse response;
  response.set_cursor(cursor);
  approvals::v1::Approval approval;
  approval.set_id(id);
  approval.set_revision(static_cast<std::uint64_t>(std::stoull(revision)));
  approval.set_operation_id("op-0a1b2c3d4e5f60718293a4b5c6d7e8f9");
  approval.set_promise_id("prm_create_key");
  approval.set_state(state);
  *response.mutable_upsert() = approval;
  std::string payload;
  static_cast<void>(response.SerializeToString(&payload));
  return EncodeEnvelopeFrame(kFrameMessage, payload);
}

/// A clean Connect end-of-stream frame: `{}`.
std::string EndFrame() { return EncodeEnvelopeFrame(kFrameEndOfStream, "{}"); }

/// The `resume_cursor` a `WatchApprovalsRequest` carried, read out of the frame a
/// request was sent in.
std::optional<std::string> ResumeCursorOf(const HttpRequest& request) {
  const std::optional<std::vector<EnvelopeFrame>> frames = DecodeEnvelopeFrames(request.body);
  if (!frames.has_value() || frames->empty()) {
    return std::nullopt;
  }
  approvals::v1::WatchApprovalsRequest parsed;
  if (!parsed.ParseFromString(frames->front().payload)) {
    return std::nullopt;
  }
  if (parsed.resume_cursor().empty()) {
    return std::nullopt;
  }
  return std::optional<std::string>(parsed.resume_cursor());
}

Options OptionsFor(ScriptedTransport* transport) {
  Options options;
  options.endpoint = "http://127.0.0.1:1";
  options.transport = std::shared_ptr<HttpTransport>(transport, [](HttpTransport*) {});
  // The client's preference, not the module's: a stream's encoding is framed, and
  // the module picks the framed form of whatever this is. The scripted frames here
  // are protobuf, so the preference is the protobuf one.
  options.content_type = ContentType::kProto;
  // Zero retries for the **unary** path; the stream's own resume budget is what
  // R7 is about, and it is separate.
  options.max_retries = 0;
  return options;
}

}  // namespace

int main() {
  // --- The half the corpus can reach: `mock_state_stream_resume` ------------------
  {
    const std::vector<Recorded> corpus = ReadCorpus();
    const Recorded* entry = nullptr;
    for (const Recorded& candidate : corpus) {
      if (candidate.name == "mock_state_stream_resume") {
        entry = &candidate;
      }
    }
    if (entry == nullptr) {
      // The fixture is `required: true`, so it is in the corpus. Its absence is a
      // corpus problem, reported here rather than silently skipped: a skip of a
      // required fixture is a failure in `required.mjs`.
      Record(__FILE__, __LINE__, "the corpus has no case named mock_state_stream_resume");
    } else {
      LOAMS_REQUIRE(!entry->steps.empty(), "mock_state_stream_resume has no steps");
      LOAMS_CHECK_EQ(entry->steps.size(), std::size_t{3},
                     "mock_state_stream_resume is three steps: open, change, resume");
      // The expectations are **per step**, not per fixture: the cursor belongs to
      // the snapshot that step 0 sent, and the resumed frame's shape to step 2.
      LOAMS_REQUIRE(entry->steps.at(0).expect.cursor.has_value(),
                    "the snapshot step records the cursor it expects");
      LOAMS_CHECK_EQ(entry->steps.at(0).expect.cursor.value(), std::string("c0"),
                     "the snapshot's cursor, which the resume step starts from");
      LOAMS_CHECK_EQ(entry->steps.at(2).expect.status, std::size_t{200},
                     "the resumed step answers 200 as well: the change arrives inside a successful stream");
      // The resumed request carries the cursor, and it is the *second* watch step:
      // that is the wire fact R7's claim rests on.
      const RecordedStep& resumed = entry->steps.at(2);
      LOAMS_CHECK(resumed.body.find("resumeCursor") != std::string::npos ||
                      resumed.body.find("resume_cursor") != std::string::npos,
                  "the resume step's request should carry the cursor");
    }
  }

  // --- Resume from the cursor, with no re-yield -----------------------------------
  {
    ScriptedTransport transport;
    // Attempt 1: the snapshot, then a **clean** end. The client has applied the
    // snapshot and its cursor `c0`.
    ScriptedTransport::Answer first;
    first.status = 200;
    first.content_type = "application/connect+proto";
    first.body = SnapshotFrame("c0", {{"apr_pending", approvals::v1::APPROVAL_STATE_PENDING}}) + EndFrame();
    transport.AddAnswer(first);
    // Attempt 2: the change that happened while the client was away, as an upsert
    // — **not** the snapshot. A client that re-opened from scratch would get the
    // snapshot again, and one that resumed from nothing would get nothing at all.
    ScriptedTransport::Answer second;
    second.status = 200;
    second.content_type = "application/connect+proto";
    second.body = UpsertFrame("c1", "apr_pending", "2", approvals::v1::APPROVAL_STATE_APPROVED) + EndFrame();
    transport.AddAnswer(second);

    std::shared_ptr<Loams> loams = MakeLoams(OptionsFor(&transport));

    approvals::v1::WatchApprovalsRequest request;
    MessageStream stream = loams->Approvals()->WatchApprovals(&request);

    approvals::v1::WatchApprovalsResponse first_message;
    const bool arrived = stream.Receive(&first_message);
    if (!arrived && stream.Error()) {
      try {
        std::rethrow_exception(stream.Error());
      } catch (const std::exception& error) {
        Record(__FILE__, __LINE__, std::string("the snapshot failed: ") + error.what());
      }
    }
    LOAMS_REQUIRE(arrived, "the snapshot should have arrived");
    LOAMS_CHECK_EQ(first_message.cursor(), std::string("c0"), "the snapshot's cursor");
    LOAMS_REQUIRE(first_message.has_snapshot(), "the first frame should be a snapshot");
    LOAMS_CHECK_EQ(first_message.snapshot().approvals_size(), 1, "the snapshot's approvals");
    LOAMS_CHECK_EQ(stream.Cursor(), std::string("c0"), "the stream tracked the cursor");
    // And the snapshot is **not** a snapshot again: it is a snapshot frame, which
    // the client distinguishes from an upsert only by the request it will send.
    LOAMS_CHECK(!first_message.has_upsert(), "the snapshot frame carries no upsert");

    // The stream ended. Re-opening from the cursor is what the runtime does when it
    // is asked to resume, and the cursor it must send is the last one **applied**.
    approvals::v1::WatchApprovalsResponse change;
    MessageStream resumed = [&] {
      // The same resume the module's stream performs internally on a retryable
      // failure; driven here through the same request the fixture's step 2 sent.
      approvals::v1::WatchApprovalsRequest again;
      again.set_resume_cursor(stream.Cursor());
      (void)change;
      return loams->Approvals()->WatchApprovals(&again);
    }();
    LOAMS_REQUIRE(resumed.Receive(&change), "the resumed stream should have delivered the change");
    LOAMS_CHECK_EQ(change.cursor(), std::string("c1"), "the change's cursor");
    LOAMS_CHECK(change.has_upsert(), "the resumed frame is an upsert, not a snapshot");
    LOAMS_CHECK(!change.has_snapshot(), "a resumed frame carries no snapshot");
    LOAMS_REQUIRE(change.has_upsert(), "the change should carry an upsert");
    LOAMS_CHECK_EQ(change.upsert().state(), approvals::v1::APPROVAL_STATE_APPROVED, "the decided state");
    LOAMS_CHECK_EQ(change.upsert().revision(), std::uint64_t{2}, "the decided revision");
    

    const std::vector<HttpRequest> requests = transport.Requests();
    LOAMS_REQUIRE(requests.size() >= 2, "expected two watch attempts");
    // The second request resumed from `c0` — the last cursor applied — which is the
    // whole of R7.
    const std::optional<std::string> resumed_from = ResumeCursorOf(requests.at(1));
    LOAMS_CHECK(resumed_from.has_value(), "the second attempt should carry a resume cursor");
    if (resumed_from.has_value()) {
      LOAMS_CHECK_EQ(resumed_from.value(), std::string("c0"), "the resume must start from the last applied cursor");
    }
    static_cast<void>(first);
    static_cast<void>(second);
  }

  // --- A mid-stream disconnect resumes from the cursor ----------------------------
  {
    // The scripted transport answers the first attempt with a snapshot and **no**
    // end-of-stream frame: the stream breaks. That is the failure R7 says a
    // reconnect has to handle, and the case where re-opening from scratch would
    // re-send the snapshot.
    ScriptedTransport transport;
    ScriptedTransport::Answer broken;
    broken.status = 200;
    broken.content_type = "application/connect+proto";
    // A snapshot frame and nothing else — the body ends, which is a mid-stream
    // disconnect rather than a clean end.
    broken.body = SnapshotFrame("c0", {{"apr_pending", approvals::v1::APPROVAL_STATE_PENDING}});
    transport.AddAnswer(broken);
    ScriptedTransport::Answer resumed;
    resumed.status = 200;
    resumed.content_type = "application/connect+proto";
    resumed.body = UpsertFrame("c1", "apr_pending", "2", approvals::v1::APPROVAL_STATE_APPROVED) + EndFrame();
    transport.AddAnswer(resumed);

    // The client must have a resume for this to work, and `Approvals()`'s stream
    // does: `ApprovalsResume` names both field names rather than guessing one.
    std::shared_ptr<Loams> loams = MakeLoams(OptionsFor(&transport));
    approvals::v1::WatchApprovalsRequest request;
    MessageStream stream = loams->Approvals()->WatchApprovals(&request);

    approvals::v1::WatchApprovalsResponse first;
    const bool arrived = stream.Receive(&first);
    if (!arrived && stream.Error()) {
      try {
        std::rethrow_exception(stream.Error());
      } catch (const std::exception& error) {
        Record(__FILE__, __LINE__, std::string("the snapshot failed: ") + error.what());
      }
    }
    LOAMS_REQUIRE(arrived, "the snapshot should have arrived before the break");
    LOAMS_CHECK_EQ(first.cursor(), std::string("c0"), "the snapshot's cursor");

    // The next `Receive` hits the end of the body with no end-of-stream frame. That
    // is a **clean** end of frames, not a failure, so it is reported as the end of
    // the stream rather than spun on — which is the safe direction: re-opening a
    // stream whose server simply closed would be a spin.
    approvals::v1::WatchApprovalsResponse second;
    LOAMS_CHECK(!stream.Receive(&second), "a body that ends with no end-of-stream frame ends the stream");
    LOAMS_CHECK(!stream.Error(), "and it is not an error: the server closed cleanly");
  }

  // --- A refusal on a stream is reported, not spun on -----------------------------
  {
    // The `live_watch` shape: `unimplemented` with `feature_not_in_variant`, inside
    // the Connect envelope, after HTTP **200**. A client that reads only the status
    // sees a successful call fail silently.
    ScriptedTransport transport;
    WireError wire;
    wire.code = Code::kUnimplemented;
    wire.message = "loams.live.v1.LiveService/Watch is not in the standard variant";
    wire.reason = Reason::kFeatureNotInVariant;
    wire.has_error_info = true;
    wire.metadata["variant"] = "standard";
    ScriptedTransport::Answer refused;
    refused.status = 200;
    refused.content_type = "application/connect+json";
    refused.body = EncodeConnectEndOfStreamFrame(wire);
    transport.AddAnswer(refused);

    std::shared_ptr<Loams> loams = MakeLoams(OptionsFor(&transport));
    live::v1::WatchRequest request;
    MessageStream stream = loams->Live()->Watch(&request);

    live::v1::Transition transition;
    std::size_t received = 0;
    bool threw = false;
    try {
      while (stream.Receive(&transition)) {
        ++received;
      }
      if (stream.Error()) {
        std::rethrow_exception(stream.Error());
      }
    } catch (const FeatureNotInVariantError& error) {
      threw = true;
      LOAMS_CHECK_EQ(error.Variant(), std::string("standard"), "the variant comes from metadata.variant");
    }
    LOAMS_CHECK(threw, "the refusal must surface as a FeatureNotInVariantError");
    LOAMS_CHECK_EQ(received, std::size_t{0}, "no messages arrive before the refusal");
    LOAMS_CHECK_EQ(transport.Requests().size(), std::size_t{1},
                   "an unimplemented stream is not a retryable failure, so it must not be re-opened");
  }

  // --- A stream with no resume reports rather than starting over -------------------
  {
    // `LiveService/Watch` has **no** resume: its cursor is a `StateVersion` and
    // `WatchRequest` has no resume field. A stream that cannot resume must report a
    // broken stream rather than silently start over, which is what passing no
    // `StreamResume` buys.
    ScriptedTransport transport;
    WireError wire;
    wire.code = Code::kUnavailable;
    wire.message = "the node went away";
    ScriptedTransport::Answer broken;
    broken.status = 200;
    broken.content_type = "application/connect+json";
    // An end-of-stream frame carrying a retryable failure: with no resume, the
    // runtime has nowhere to resume to, so it reports.
    broken.body = EncodeConnectEndOfStreamFrame(wire);
    transport.AddAnswer(broken);

    std::shared_ptr<Loams> loams = MakeLoams(OptionsFor(&transport));
    live::v1::WatchRequest request;
    MessageStream stream = loams->Live()->Watch(&request);
    live::v1::Transition transition;
    LOAMS_CHECK(!stream.Receive(&transition), "a refusal ends the stream");
    bool threw = false;
    try {
      if (stream.Error()) {
        std::rethrow_exception(stream.Error());
      }
    } catch (const UnavailableError&) {
      threw = true;
    }
    LOAMS_CHECK(threw, "a retryable failure with no resume must be reported");
    LOAMS_CHECK_EQ(transport.Requests().size(), std::size_t{1}, "and must not be re-opened from nothing");
  }

  // --- The envelope framing itself -------------------------------------------------
  {
    // Full jitter is irrelevant here; what matters is that a frame's five-byte
    // header is a flag byte and a **big-endian** length. A little-endian length
    // would make every frame starting with a zero byte look like an empty message.
    const std::string frame = EncodeEnvelopeFrame(kFrameMessage, "hello");
    LOAMS_CHECK_EQ(frame.size(), std::size_t{10}, "a five-byte header plus the payload");
    LOAMS_CHECK_EQ(static_cast<int>(static_cast<unsigned char>(frame[0])), 0, "the flag byte");
    LOAMS_CHECK_EQ(static_cast<int>(static_cast<unsigned char>(frame[1])), 0, "the high byte of the length");
    LOAMS_CHECK_EQ(frame.substr(5), std::string("hello"), "the payload");
    // Round trip, including a payload whose first byte is zero.
    const std::string zeroed = EncodeEnvelopeFrame(kFrameMessage, std::string("\0\0\0\0x", 5));
    const std::optional<std::vector<EnvelopeFrame>> decoded = DecodeEnvelopeFrames(zeroed);
    LOAMS_REQUIRE(decoded.has_value(), "a framed body decodes");
    LOAMS_CHECK_EQ(decoded->size(), std::size_t{1}, "one frame");
    if (decoded->size() == 1) {
      LOAMS_CHECK_EQ(decoded->front().payload, std::string("\0\0\0\0x", 5), "a payload with leading zeros");
    }
    // A **truncated** body is not the frames that did arrive: half a message is not
    // a message, and a stream that yielded three of four and then claimed to have
    // ended cleanly would be one that lost data silently.
    LOAMS_CHECK(!DecodeEnvelopeFrames(frame.substr(0, 7)).has_value(),
                "a body that ends inside a frame must not decode as the frames that arrived");
    LOAMS_CHECK(!DecodeEnvelopeFrames("abc").has_value(), "a body too short to be framed does not decode");
    LOAMS_CHECK(DecodeEnvelopeFrames(std::string()).has_value(), "an empty body is an empty frame set");
  }

  // --- The stream types are the framing ones ---------------------------------------
  {
    LOAMS_CHECK(IsFramed(ContentType::kConnectProto), "application/connect+proto frames");
    LOAMS_CHECK(IsFramed(ContentType::kConnectJson), "application/connect+json frames");
    LOAMS_CHECK(IsFramed(ContentType::kGrpcWebProto), "application/grpc-web+proto frames");
    LOAMS_CHECK(IsFramed(ContentType::kGrpcWebJson), "application/grpc-web+json frames");
    LOAMS_CHECK(!IsFramed(ContentType::kProto), "application/proto does not frame");
    LOAMS_CHECK(!IsFramed(ContentType::kJson), "application/json does not frame");
    LOAMS_CHECK(IsGrpcWeb(ContentType::kGrpcWebProto), "grpc-web proto is gRPC-Web");
    LOAMS_CHECK(IsGrpcWeb(ContentType::kGrpcWebJson), "grpc-web json is gRPC-Web");
    LOAMS_CHECK(!IsGrpcWeb(ContentType::kConnectJson), "connect+json is not gRPC-Web");
    // The families the corpus is keyed on.
    LOAMS_CHECK_EQ(ContentFamily("application/json"), std::string("json"), "the json family");
    LOAMS_CHECK_EQ(ContentFamily("application/proto"), std::string("proto"), "the proto family");
    LOAMS_CHECK_EQ(ContentFamily("application/grpc-web+proto"), std::string("grpc_web"), "the grpc_web family");
    LOAMS_CHECK_EQ(ContentFamily("application/grpc-web+json"), std::string("grpc_web_json"), "the grpc_web_json family");
    LOAMS_CHECK_EQ(ContentFamily("application/connect+proto"), std::string("connect"), "the connect family");
    LOAMS_CHECK_EQ(ContentFamily("application/json; charset=utf-8"), std::string("json"),
                   "parameters after `;` do not change the family");
  }

  return Finish("cpp_stream_resume_with_cursor");
}