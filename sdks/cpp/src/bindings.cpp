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
  add("loams.live.v1.LiveService/Authenticate", RetryClass::kManual, "");
  // Deploy moved to LiveAdminService (LV1 Task 10); `tables` keeps the call.
  add("loams.live.v1.LiveAdminService/Deploy", RetryClass::kManual, "");

  // The app directory of `LiveAdminService` (LV1 Task 13). The three mutations are
  // `kManual` with no key (no request declares `idempotency_key`), so they are never
  // retried; the two reads are `retry_safe`. `ListApps` pages on `apps` and
  // `next_page_token`.
  add("loams.live.v1.LiveAdminService/CreateApp", RetryClass::kManual, "");
  add("loams.live.v1.LiveAdminService/ListApps", RetryClass::kSafe, "", false, "apps", "next_page_token");
  add("loams.live.v1.LiveAdminService/GetApp", RetryClass::kSafe, "");
  add("loams.live.v1.LiveAdminService/UpdateApp", RetryClass::kManual, "");
  add("loams.live.v1.LiveAdminService/DeleteApp", RetryClass::kManual, "");

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

  // The app-building API (SDKP §2.2–§2.5). The retry classes are SDKP's: the
  // creates and schema changes are `retry_safe` (idempotent by name, or
  // additive), so `kSafe`; `WriteDocuments` and the graph mutations carry an
  // `idempotency_key`, so `kManual` and retried once keyed; `Scan` and the
  // filter writes are never retried. A read-only `Execute` is retried as a read
  // by the graph module itself.
  add("loams.collection.v1.NamespaceService/CreateNamespace", RetryClass::kSafe, "");
  add("loams.collection.v1.NamespaceService/ListNamespaces", RetryClass::kSafe, "", false, "namespaces",
      "next_page_token");
  add("loams.collection.v1.CollectionService/CreateCollection", RetryClass::kSafe, "");
  add("loams.collection.v1.CollectionService/ListCollections", RetryClass::kSafe, "", false, "collections",
      "next_page_token");
  add("loams.collection.v1.CollectionService/GetCollection", RetryClass::kSafe, "");
  add("loams.collection.v1.CollectionService/DropCollection", RetryClass::kSafe, "");
  add("loams.collection.v1.CollectionService/AddFields", RetryClass::kSafe, "");
  add("loams.collection.v1.CollectionService/ListVersions", RetryClass::kSafe, "", false, "versions",
      "next_page_token");
  add("loams.collection.v1.CollectionService/UpdateAliases", RetryClass::kSafe, "");
  add("loams.collection.v1.CollectionService/Scan", RetryClass::kManual, "");
  add("loams.collection.v1.CollectionService/SetHot", RetryClass::kSafe, "");
  add("loams.collection.v1.CollectionService/WarmCollection", RetryClass::kSafe, "");
  add("loams.collection.v1.DocumentService/WriteDocuments", RetryClass::kManual, "idempotency_key");
  add("loams.collection.v1.DocumentService/GetDocuments", RetryClass::kSafe, "");
  add("loams.collection.v1.DocumentService/ScrollDocuments", RetryClass::kSafe, "");
  add("loams.collection.v1.DocumentService/CountDocuments", RetryClass::kSafe, "");
  add("loams.collection.v1.DocumentService/DeleteByFilter", RetryClass::kManual, "");
  add("loams.collection.v1.DocumentService/PatchByFilter", RetryClass::kManual, "");
  add("loams.collection.v1.QueryService/Search", RetryClass::kSafe, "");
  add("loams.graph.v1.GraphAdminService/CreateGraph", RetryClass::kManual, "idempotency_key");
  add("loams.graph.v1.GraphAdminService/GetGraph", RetryClass::kSafe, "");
  add("loams.graph.v1.GraphAdminService/ListGraphs", RetryClass::kSafe, "", false, "graphs", "next_page_token");
  add("loams.graph.v1.GraphAdminService/UpdateGraph", RetryClass::kManual, "idempotency_key");
  add("loams.graph.v1.GraphAdminService/DeleteGraph", RetryClass::kManual, "idempotency_key");
  add("loams.graph.v1.GraphAdminService/GetSchema", RetryClass::kSafe, "");
  add("loams.graph.v1.GraphService/Execute", RetryClass::kManual, "idempotency_key");
  add("loams.graph.v1.GraphService/ExecuteBatch", RetryClass::kManual, "idempotency_key");
  add("loams.graph.v1.GraphService/Explain", RetryClass::kSafe, "");

  // `loams.sql.v1` and `loams.stream.v1` (SDKP part C), with the facade
  // index's retry classes: the reads are safe, and `Produce` and
  // `ProduceCloudEvents` are never retried (a repeat is deduplicated only when
  // the caller gave a producer sequence).
  add("loams.sql.v1.SqlService/Query", RetryClass::kSafe, "");
  add("loams.sql.v1.SqlService/QueryArrow", RetryClass::kSafe, "");
  add("loams.stream.v1.StreamService/CreateStream", RetryClass::kSafe, "");
  add("loams.stream.v1.StreamService/DescribeStream", RetryClass::kSafe, "");
  add("loams.stream.v1.StreamService/ListStreams", RetryClass::kSafe, "", false, "streams", "next_page_token");
  add("loams.stream.v1.StreamService/Produce", RetryClass::kManual, "");
  add("loams.stream.v1.StreamService/Fetch", RetryClass::kSafe, "");
  add("loams.stream.v1.StreamService/ProduceCloudEvents", RetryClass::kManual, "");
  add("loams.stream.v1.StreamService/FetchCloudEvents", RetryClass::kSafe, "");

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