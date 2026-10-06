// The facade modules, as `facade.hpp` documents them.
//
// The hand-written Q604 fallback. What is *not* here is any per-RPC policy: each
// method is a name, a binding and a typed call, and everything the runtime
// contract asks for happens in `Client::Unary` and `Client::OpenStream`. A
// method that added a retry, a key or an error special case of its own would be
// the place two SDKs start to disagree.

#include "loams/facade.hpp"

#include "loams/approvals/v1/approvals.pb.h"
#include "loams/devices/v1/devices.pb.h"
#include "loams/instance/v1/instance.pb.h"
#include "loams/live/v1/live.pb.h"
#include "loams/notifications/v1/notifications.pb.h"
#include "loams/operations/v1/operations.pb.h"

namespace loams {
namespace {

/// The resume for `WatchApprovals`: the response's `cursor`, and a re-open that
/// writes it into the request's `resume_cursor`.
///
/// The two names are **not** guessed. `WatchApprovalsResponse` carries `cursor`
/// and `WatchApprovalsRequest` carries `resume_cursor`, and the class names both
/// so a reader can see the coupling in one place. A default that guessed a field
/// name would be a method that silently resumes from nothing — the exact failure
/// R7 exists to prevent.
class ApprovalsResume final : public StreamResume {
 public:
  std::unique_ptr<google::protobuf::Message> Reopen(const std::string& cursor,
                                                    const google::protobuf::Message& request) override {
    auto* const typed = dynamic_cast<const approvals_v1::WatchApprovalsRequest*>(&request);
    if (typed == nullptr) {
      ThrowInternal(kRpc, "loams: WatchApprovals' resume was handed a request of the wrong type");
    }
    auto next = std::unique_ptr<approvals_v1::WatchApprovalsRequest>(
        new approvals_v1::WatchApprovalsRequest(*typed));
    if (!cursor.empty()) {
      next->set_resume_cursor(cursor);
    }
    return next;
  }

  std::string CursorOf(const google::protobuf::Message& message) override {
    const auto* const typed = dynamic_cast<const approvals_v1::WatchApprovalsResponse*>(&message);
    return typed == nullptr ? std::string() : typed->cursor();
  }

 private:
  static constexpr char kRpc[] = "loams.approvals.v1.ApprovalService/WatchApprovals";
};

/// The streaming encoding a module uses for a server stream.
///
/// A server stream is **framed**, so it cannot use `application/json` or
/// `application/proto`: those have no frame boundaries and guessing where one
/// message ends is how a reader silently truncates a stream. The client's
/// preference decides between the framed ones — JSON preferences get
/// `application/connect+json`, gRPC-Web preferences keep their encoding, and
/// everything else gets `application/connect+proto`.
ContentType FramedEncoding(ContentType preferred) {
  if (IsFramed(preferred)) {
    return preferred;
  }
  return IsJson(preferred) ? ContentType::kConnectJson : ContentType::kConnectProto;
}

}  // namespace

void InstanceModule::GetInstance(instance_v1::GetInstanceResponse* response) {
  GetInstanceWith(response, client_->Encoding());
}

void InstanceModule::GetInstanceWith(instance_v1::GetInstanceResponse* response, ContentType content_type) {
  client_->Unary(BindingFor("loams.instance.v1.InstanceService/GetInstance"),
                 instance_v1::GetInstanceRequest(), response, content_type);
}

void InstanceModule::WhoAmI(instance_v1::WhoAmIResponse* response) {
  WhoAmIWith(response, client_->Encoding());
}

void InstanceModule::WhoAmIWith(instance_v1::WhoAmIResponse* response, ContentType content_type) {
  client_->Unary(BindingFor("loams.instance.v1.InstanceService/WhoAmI"), instance_v1::WhoAmIRequest(),
                 response, content_type);
}

void LiveModule::Query(live_v1::QueryRequest* request, live_v1::QueryResponse* response) {
  QueryWith(request, response, client_->Encoding());
}

void LiveModule::QueryWith(live_v1::QueryRequest* request, live_v1::QueryResponse* response,
                           ContentType content_type) {
  client_->Unary(BindingFor("loams.live.v1.LiveService/Query"), *request, response, content_type);
}

void LiveModule::Mutate(live_v1::MutateRequest* request, live_v1::MutateResponse* response) {
  client_->Unary(BindingFor("loams.live.v1.LiveService/Mutate"), *request, response, client_->Encoding());
}

MessageStream LiveModule::Watch(live_v1::WatchRequest* request) {
  // **No resume.** `loams.live.v1`'s cursor is a `StateVersion` on the
  // `Transition`, and `WatchRequest` has no resume field at all, so a stream that
  // cannot resume must **report** a broken stream rather than silently start over
  // (R7). Passing a resume that cannot be built would be the worse mistake: it
  // would re-open from an empty cursor and re-yield the snapshot.
  return client_->OpenStream(BindingFor("loams.live.v1.LiveService/Watch"), *request,
                             FramedEncoding(client_->Encoding()), /*resume=*/nullptr);
}

void TablesModule::Query(live_v1::QueryRequest* request, live_v1::QueryResponse* response) {
  client_->Unary(BindingFor("loams.live.v1.LiveService/Query"), *request, response, client_->Encoding());
}

void TablesModule::Mutate(live_v1::MutateRequest* request, live_v1::MutateResponse* response) {
  client_->Unary(BindingFor("loams.live.v1.LiveService/Mutate"), *request, response, client_->Encoding());
}

void TablesModule::Deploy(live_v1::DeployRequest* request, live_v1::DeployResponse* response) {
  client_->Unary(BindingFor("loams.live.v1.LiveService/Deploy"), *request, response, client_->Encoding());
}

void ApprovalsModule::ListApprovals(approvals_v1::ListApprovalsRequest* request,
                                    approvals_v1::ListApprovalsResponse* response) {
  client_->Unary(BindingFor("loams.approvals.v1.ApprovalService/ListApprovals"), *request, response,
                 client_->Encoding());
}

void ApprovalsModule::GetApproval(approvals_v1::GetApprovalRequest* request,
                                  approvals_v1::GetApprovalResponse* response) {
  client_->Unary(BindingFor("loams.approvals.v1.ApprovalService/GetApproval"), *request, response,
                 client_->Encoding());
}

void ApprovalsModule::DecideApproval(approvals_v1::DecideApprovalRequest* request,
                                     approvals_v1::DecideApprovalResponse* response) {
  client_->Unary(BindingFor("loams.approvals.v1.ApprovalService/DecideApproval"), *request, response,
                 client_->Encoding());
}

MessageStream ApprovalsModule::WatchApprovals(approvals_v1::WatchApprovalsRequest* request) {
  return client_->OpenStream(BindingFor("loams.approvals.v1.ApprovalService/WatchApprovals"), *request,
                             FramedEncoding(client_->Encoding()), std::make_shared<ApprovalsResume>());
}

void DevicesModule::ListDevices(devices_v1::ListDevicesRequest* request,
                                devices_v1::ListDevicesResponse* response) {
  client_->Unary(BindingFor("loams.devices.v1.DeviceService/ListDevices"), *request, response,
                 client_->Encoding());
}

void DevicesModule::SendTestNotification(devices_v1::SendTestNotificationRequest* request,
                                         devices_v1::SendTestNotificationResponse* response) {
  SendTestNotificationWith(request, response, client_->Encoding());
}

void DevicesModule::SendTestNotificationWith(devices_v1::SendTestNotificationRequest* request,
                                             devices_v1::SendTestNotificationResponse* response,
                                             ContentType content_type) {
  client_->Unary(BindingFor("loams.devices.v1.DeviceService/SendTestNotification"), *request, response,
                 content_type);
}

void OperationsModule::ListOperations(operations_v1::ListOperationsRequest* request,
                                      operations_v1::ListOperationsResponse* response) {
  client_->Unary(BindingFor("loams.operations.v1.OperationsService/ListOperations"), *request, response,
                 client_->Encoding());
}

void NotificationsModule::ListNotifications(notifications_v1::ListNotificationsRequest* request,
                                             notifications_v1::ListNotificationsResponse* response) {
  client_->Unary(BindingFor("loams.notifications.v1.NotificationService/ListNotifications"), *request, response,
                 client_->Encoding());
}

Loams::Loams(std::shared_ptr<Client> client)
    : client_(std::move(client)),
      instance_(client_.get()),
      live_(client_.get()),
      tables_(client_.get()),
      approvals_(client_.get()),
      devices_(client_.get()),
      operations_(client_.get()),
      notifications_(client_.get()) {}

InstanceModule* Loams::Instance() { return &instance_; }
LiveModule* Loams::Live() { return &live_; }
TablesModule* Loams::Tables() { return &tables_; }
ApprovalsModule* Loams::Approvals() { return &approvals_; }
DevicesModule* Loams::Devices() { return &devices_; }
OperationsModule* Loams::Operations() { return &operations_; }
NotificationsModule* Loams::Notifications() { return &notifications_; }
SystemModule* Loams::System() { return client_->System(); }

std::shared_ptr<Loams> MakeLoams(Options options) {
  return std::make_shared<Loams>(Client::Make(std::move(options)));
}

}  // namespace loams