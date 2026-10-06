// The facade modules (design §44 §7.1–§7.3, D606).
//
// **This is the hand-written facade — the Q604 fallback**, not generated output.
// Design §44 §7.3 says the module/method surface is generated from annotations
// on the protos by `protoc-gen-loams-facade`, and that "if the plugin proves too
// costly for a language, that language falls back to a hand-written facade
// checked by the same conformance suite (D606, Q604)". No `templates/cpp/` exists
// and no C++ renderer does, so this file is that fallback and says so where a
// reader would otherwise look for the generator.
//
// The same applies to the binding table in `bindings.cpp`: `loams.options.v1`
// annotates every method with its module name, facade name, retry class and
// pagination fields, and a generator would read them off the descriptors. Here
// they are transcribed, and `cpp_conformance_all_required_fixtures` checks the
// transcription against the wire by replaying the corpus through it — which is
// what stops a transcription from being wrong in a way nobody notices.
//
// What this file adds over the generated stubs is the part that is not
// mechanical: the modules, the retry-class resolution, R3's keying, and the
// `PascalCase` names design §44 §7.1 asks for in C++.

#ifndef LOAMS_FACADE_HPP
#define LOAMS_FACADE_HPP

#include <memory>
#include <string>
#include <vector>

#include <google/protobuf/message.h>

#include "loams/approvals/v1/approvals.pb.h"
#include "loams/call.hpp"
#include "loams/client.hpp"
#include "loams/devices/v1/devices.pb.h"
#include "loams/instance/v1/instance.pb.h"
#include "loams/live/v1/live.pb.h"
#include "loams/notifications/v1/notifications.pb.h"
#include "loams/operations/v1/operations.pb.h"
#include "loams/pagination.hpp"
#include "loams/stream.hpp"

namespace loams {

// The generated messages live in `namespace loams::<package>::v1`, beside this
// SDK's own `namespace loams`. The aliases below say which one each signature
// means, because writing `instance_v1::GetInstanceResponse` inside
// `namespace loams` is a mouthful the reader has to parse every time.
namespace instance_v1 = ::loams::instance::v1;
namespace live_v1 = ::loams::live::v1;
namespace approvals_v1 = ::loams::approvals::v1;
namespace devices_v1 = ::loams::devices::v1;
namespace operations_v1 = ::loams::operations::v1;
namespace notifications_v1 = ::loams::notifications::v1;

/// The binding table: what the facade knows about every RPC it serves.
///
/// Transcribed from the protos' `loams.options.v1` annotations rather than
/// generated. `kBindings` is the whole list, and `BindingFor` looks one up by
/// `package.Service/Method` — an unknown RPC throws rather than returning a
/// default, because a default `RetryClass::kManual` on a read is a silently
/// non-retrying call.
const std::vector<MethodBinding>& AllBindings();

/// The binding for one RPC. Throws `loams::LoamsError` (`internal`) when the RPC
/// is not in the table.
const MethodBinding& BindingFor(const std::string& rpc);

// --- Modules -------------------------------------------------------------------------------------

/// `loams.instance`: what this instance is, and who the caller is on it.
class InstanceModule {
 public:
  explicit InstanceModule(Client* client) : client_(client) {}

  /// `InstanceService/GetInstance`. Needs no auth, which is why it is the first
  /// thing any client calls.
  void GetInstance(instance_v1::GetInstanceResponse* response);

  /// `InstanceService/WhoAmI`. Throws `loams::UnauthenticatedError` on a build
  /// with no authentication yet, with reason `not_implemented`.
  void WhoAmI(instance_v1::WhoAmIResponse* response);

  /// `InstanceService/GetInstance` with the encoding a caller picks, for the
  /// conformance suite's four encodings. Not a module method an application
  /// wants; it exists so the encoding is a parameter of a **real** call rather
  /// than something a test reaches past the facade for.
  void GetInstanceWith(instance_v1::GetInstanceResponse* response, ContentType content_type);

  /// `InstanceService/WhoAmI` in a chosen encoding, for the same reason.
  void WhoAmIWith(instance_v1::WhoAmIResponse* response, ContentType content_type);

 private:
  Client* client_;
};

/// `loams.live`: the live tables' watch and query RPCs.
class LiveModule {
 public:
  explicit LiveModule(Client* client) : client_(client) {}

  /// `LiveService/Query`. Throws `loams::FeatureNotInVariantError` in the
  /// standard variant, which is where the R5 typed refusal is read from.
  void Query(live_v1::QueryRequest* request, live_v1::QueryResponse* response);

  /// `LiveService/Mutate`. Carries `idempotency_key`, so it is retryable once
  /// the SDK has minted one (R3).
  void Mutate(live_v1::MutateRequest* request, live_v1::MutateResponse* response);

  /// `LiveService/Watch`, a server stream with **no** resume: `loams.live.v1`'s
  /// cursor is a `StateVersion`, and a stream that cannot resume must report a
  /// broken stream rather than silently start over (R7). The refusal in the
  /// standard variant arrives inside the envelope, after HTTP 200.
  MessageStream Watch(live_v1::WatchRequest* request);

  /// `LiveService/Query` in a chosen encoding, for the conformance suite.
  void QueryWith(live_v1::QueryRequest* request, live_v1::QueryResponse* response,
                 ContentType content_type);

 private:
  Client* client_;
};

/// `loams.tables`: the same service as `loams.live`, under its other facade name
/// (design §44 §7.2). One RPC, two module names.
class TablesModule {
 public:
  explicit TablesModule(Client* client) : client_(client) {}

  /// `LiveService/Query`, through the `tables` name.
  void Query(live_v1::QueryRequest* request, live_v1::QueryResponse* response);
  /// `LiveService/Mutate`, through the `tables` name.
  void Mutate(live_v1::MutateRequest* request, live_v1::MutateResponse* response);
  /// `LiveService/Deploy`, through the `tables` name.
  void Deploy(live_v1::DeployRequest* request, live_v1::DeployResponse* response);

 private:
  Client* client_;
};

/// `loams.approvals`: the approval flow, including its watch stream — the one
/// server stream the corpus can actually serve (R7).
class ApprovalsModule {
 public:
  explicit ApprovalsModule(Client* client) : client_(client) {}

  /// `ApprovalService/ListApprovals`. Declares both AIP-158 fields and honours
  /// neither, which is why R6's end-to-end half is a stub: see
  /// `loams::PageIterator`.
  void ListApprovals(approvals_v1::ListApprovalsRequest* request,
                     approvals_v1::ListApprovalsResponse* response);

  /// `ApprovalService/GetApproval`.
  void GetApproval(approvals_v1::GetApprovalRequest* request,
                   approvals_v1::GetApprovalResponse* response);

  /// `ApprovalService/DecideApproval`. Carries `idempotency_key`, so the same
  /// key goes out on every retry and a replayed decide returns the first
  /// answer rather than deciding twice (R3).
  void DecideApproval(approvals_v1::DecideApprovalRequest* request,
                      approvals_v1::DecideApprovalResponse* response);

  /// `ApprovalService/WatchApprovals`, a server stream **with** resume: the
  /// response carries a `cursor`, so a reconnect resumes from the last cursor
  /// applied and does not re-yield what it already yielded (R7).
  MessageStream WatchApprovals(approvals_v1::WatchApprovalsRequest* request);

 private:
  Client* client_;
};

/// `loams.devices`.
class DevicesModule {
 public:
  explicit DevicesModule(Client* client) : client_(client) {}

  /// `DeviceService/ListDevices`.
  void ListDevices(devices_v1::ListDevicesRequest* request,
                   devices_v1::ListDevicesResponse* response);

  /// `DeviceService/SendTestNotification`. In the recorded corpus this is the
  /// stub handler that answers `unimplemented`, and the one fixture that pins
  /// "the same refusal in every encoding" — Connect answers 501 with a JSON body
  /// and gRPC-Web answers 200 with the code in the trailers (R8, R10).
  void SendTestNotification(devices_v1::SendTestNotificationRequest* request,
                            devices_v1::SendTestNotificationResponse* response);

  /// `DeviceService/SendTestNotification` in a chosen encoding, for the four
  /// encodings of that one refusal.
  void SendTestNotificationWith(devices_v1::SendTestNotificationRequest* request,
                                devices_v1::SendTestNotificationResponse* response,
                                ContentType content_type);

 private:
  Client* client_;
};

/// `loams.operations`.
class OperationsModule {
 public:
  explicit OperationsModule(Client* client) : client_(client) {}

  /// `OperationsService/ListOperations`.
  void ListOperations(operations_v1::ListOperationsRequest* request,
                      operations_v1::ListOperationsResponse* response);

 private:
  Client* client_;
};

/// `loams.notifications`.
class NotificationsModule {
 public:
  explicit NotificationsModule(Client* client) : client_(client) {}

  /// `NotificationService/ListNotifications`.
  void ListNotifications(notifications_v1::ListNotificationsRequest* request,
                         notifications_v1::ListNotificationsResponse* response);

 private:
  Client* client_;
};

// --- The client with its modules ------------------------------------------------------------------

/// A `Client` with its facade modules attached. This is what an application
/// holds; `loams::Client` on its own is the runtime.
class Loams {
 public:
  explicit Loams(std::shared_ptr<Client> client);

  /// The runtime, for a caller that needs the transport or the token source.
  ///
  /// Named `Runtime` rather than `Client` for the same reason `Client::Encoding`
  /// is not named `ContentType`: a member function named after the type it returns
  /// changes that type's meaning for the rest of the class body, which clang
  /// rejects outright with `-Wchanges-meaning`.
  Client* Runtime() const { return client_.get(); }

  InstanceModule* Instance();
  LiveModule* Live();
  TablesModule* Tables();
  ApprovalsModule* Approvals();
  DevicesModule* Devices();
  OperationsModule* Operations();
  NotificationsModule* Notifications();
  SystemModule* System();

 private:
  std::shared_ptr<Client> client_;
  InstanceModule instance_;
  LiveModule live_;
  TablesModule tables_;
  ApprovalsModule approvals_;
  DevicesModule devices_;
  OperationsModule operations_;
  NotificationsModule notifications_;
};

/// Builds a `Loams` from options. The one line an application starts with.
std::shared_ptr<Loams> MakeLoams(Options options);

}  // namespace loams

#endif  // LOAMS_FACADE_HPP