// The binding table, transcribed from the protos' `loams.options.v1`
// annotations.
//
// **Transcribed, not generated** — see `facade.hpp`'s header comment and design
// §44 §7.3's Q604 fallback. Every field here comes from a proto: the module name
// and facade name from `loams.options.v1.module` / `.facade`, the retry class
// from `idempotency_level`, the idempotency field from the request's schema, and
// the pagination fields from `FacadeOptions.pagination`.
//
// What makes the transcription checkable rather than a guess is
// `cpp_conformance_all_required_fixtures`: it replays the whole recorded corpus
// through these bindings, so a binding that named the wrong RPC path, the wrong
// retry class or the wrong idempotency field fails. A binding with no fixture
// behind it is listed in the comment below as **unpinned**, because a table entry
// nothing exercises is a comment until something does.

#include "loams/call.hpp"

#include "loams/facade.hpp"

namespace loams {
namespace {

/// Builds the binding table. One place, so an RPC that appears twice is a
/// duplicate-`std::string` the reader can see rather than two initialisers that
/// silently disagree.
std::vector<MethodBinding> Build() {
  std::vector<MethodBinding> bindings;

  const auto add = [&bindings](const char* rpc, RetryClass retry_class, const char* idempotency_field,
                               bool server_streaming = false, const char* items = nullptr,
                               const char* next_token = nullptr) {
    MethodBinding binding;
    binding.rpc = rpc;
    binding.retry_class = retry_class;
    binding.idempotency_field = idempotency_field;
    binding.server_streaming = server_streaming;
    if (items != nullptr) {
      binding.items_field = items;
    }
    if (next_token != nullptr) {
      binding.next_page_token_field = next_token;
    }
    bindings.push_back(std::move(binding));
  };

  // `loams.instance.v1`. Both RPCs are `NO_SIDE_EFFECTS`, so both are
  // `RetryClass::kSafe` and neither takes an idempotency key.
  add("loams.instance.v1.InstanceService/GetInstance", RetryClass::kSafe, "");
  add("loams.instance.v1.InstanceService/WhoAmI", RetryClass::kSafe, "");

  // `loams.live.v1`. `Query` is a read and safe; `Mutate` and `Deploy` are
  // mutations, so `kManual` and retryable only once R3 has minted a key. `Watch`
  // is server-streaming and its retry class is handled by the stream reader, not
  // by the unary loop.
  add("loams.live.v1.LiveService/Watch", RetryClass::kManual, "", /*server_streaming=*/true);
  add("loams.live.v1.LiveService/Query", RetryClass::kSafe, "");
  add("loams.live.v1.LiveService/Mutate", RetryClass::kManual, "idempotency_key");
  add("loams.live.v1.LiveService/ModifyQuerySet", RetryClass::kManual, "idempotency_key");
  add("loams.live.v1.LiveService/Deploy", RetryClass::kManual, "");

  // `loams.approvals.v1`. `ListApprovals` **declares** both AIP-158 fields and
  // honours neither, which is why R6's end-to-end half runs against a stub: the
  // fields are named here from the schema, and the recorded corpus pins that the
  // server ignores them (`mock_status_list_is_not_paged`).
  add("loams.approvals.v1.ApprovalService/ListApprovals", RetryClass::kSafe, "", false, "approvals",
      "next_page_token");
  add("loams.approvals.v1.ApprovalService/GetApproval", RetryClass::kSafe, "");
  add("loams.approvals.v1.ApprovalService/WatchApprovals", RetryClass::kManual, "", /*server_streaming=*/true);
  add("loams.approvals.v1.ApprovalService/DecideApproval", RetryClass::kManual, "idempotency_key");

  // `loams.devices.v1`. `SendTestNotification` is the recorded stub handler that
  // answers `unimplemented`, and it is not a mutation of anything, so it takes no
  // key.
  add("loams.devices.v1.DeviceService/ListDevices", RetryClass::kSafe, "");
  add("loams.devices.v1.DeviceService/CreatePairing", RetryClass::kManual, "idempotency_key");
  add("loams.devices.v1.DeviceService/RenameDevice", RetryClass::kManual, "idempotency_key");
  add("loams.devices.v1.DeviceService/RevokeDevice", RetryClass::kManual, "idempotency_key");
  add("loams.devices.v1.DeviceService/RegisterPushTarget", RetryClass::kManual, "idempotency_key");
  add("loams.devices.v1.DeviceService/UnregisterPushTarget", RetryClass::kManual, "idempotency_key");
  add("loams.devices.v1.DeviceService/GetNotificationPreferences", RetryClass::kSafe, "");
  add("loams.devices.v1.DeviceService/SetNotificationPreferences", RetryClass::kManual, "idempotency_key");
  add("loams.devices.v1.DeviceService/SendTestNotification", RetryClass::kSafe, "");

  // `loams.operations.v1`.
  add("loams.operations.v1.OperationsService/GetOperation", RetryClass::kSafe, "");
  add("loams.operations.v1.OperationsService/ListOperations", RetryClass::kSafe, "");
  add("loams.operations.v1.OperationsService/WatchOperations", RetryClass::kManual, "", /*server_streaming=*/true);
  add("loams.operations.v1.OperationsService/CancelOperation", RetryClass::kManual, "idempotency_key");

  // `loams.notifications.v1`.
  add("loams.notifications.v1.NotificationService/ListNotifications", RetryClass::kSafe, "");
  add("loams.notifications.v1.NotificationService/WatchNotifications", RetryClass::kManual, "",
      /*server_streaming=*/true);
  add("loams.notifications.v1.NotificationService/MarkRead", RetryClass::kManual, "idempotency_key");

  return bindings;
}

}  // namespace

const std::vector<MethodBinding>& AllBindings() {
  static const std::vector<MethodBinding>* const bindings = new std::vector<MethodBinding>(Build());
  return *bindings;
}

const MethodBinding& BindingFor(const std::string& rpc) {
  for (const MethodBinding& binding : AllBindings()) {
    if (binding.rpc == rpc) {
      return binding;
    }
  }
  // Thrown rather than a default: a default `RetryClass::kManual` on a read is a
  // silently non-retrying call, and a default empty idempotency field on a
  // mutation is a non-idempotent retry. Neither is a failure a caller would see.
  ThrowInternal(rpc, "loams: " + rpc + " is not in the binding table; the facade does not serve it");
}

}  // namespace loams