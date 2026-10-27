// The SDK surface of design §44 §7.3: the module catalogue, the binding table
// the runtime dispatches through, and the reason registry.
//
// # Provenance — read this before editing
//
// **This file is hand-written, and it is the fallback design §44 §7.3 and D606
// explicitly allow:**
//
//   "If the plugin proves too costly for a language, that language falls back to
//   a hand-written facade checked by the same conformance suite (D606, Q604)."
//
// The Swift renderer does not exist. `crates/loams-facade-gen` ships
// `typescript.rs`, `python.rs`, `go.rs` and `rust.rs`; there is no `swift.rs`,
// `src/bin/protoc-gen-loams-facade.rs`'s `file_name()` has no `swift` arm and
// returns an error for it, `scripts/sdk/gen.sh` accepts only
// `typescript|typescript-live|python|go|rust`, and `sdks/templates/swift/` does
// not exist. So there is nothing that could generate this file today.
//
// It is a transcription of `sdks/go/gen/facade/facade.go` and
// `sdks/typescript/packages/client/src/gen/facade.ts`, and of the
// `loams.options.v1` annotations in `proto/`, field for field — which is the
// same fallback the Go SDK took, for the same reason, and Go records the same
// provenance note at the top of its own facade.
//
// **Do not grow it** beyond what the protos annotate. If a call is missing, the
// proto is missing the `loams.options.v1` annotation — see
// `docs/design/13-decision-log.md` Q604. `live.admin` (LV1 Task 10,
// `proto/loams/live/v1/admin.proto`) is annotated, which is why it is here: it is
// the TypeScript and Rust facades' `live.admin` module, call for call.
//
// When the renderer lands, this file is deleted and `scripts/sdk/gen.sh swift`
// writes a replacement with the same exported names. That is why every type here
// is named to match the generated shape rather than to be pleasant: a Swift
// renderer would emit the same names, so the runtime does not change when this
// file does.

import Foundation

/// The proto revision this SDK was generated from (`LOAMS_PROTO_REV`, design
/// §44 §10.3). `LoamsSystem.version()` reports it beside the server's
/// `GetInstance.apiVersions`. Pre-1.0, so it is the API major rather than a
/// release tag.
public let loamsProtoRevision = "v1"

/// `idempotency_level` off the proto method.
///
/// It is what the retry class is derived from (D610), so it is read rather than
/// guessed: annotating a proto is enough to change how a call retries.
public enum IdempotencyLevel: String, Sendable, Equatable {
    /// `NO_SIDE_EFFECTS`: a read, retryable on its own.
    case noSideEffects = "no_side_effects"
    /// `IDEMPOTENT`: repeating it is the same call.
    case idempotent
    /// No idempotency level: a mutation, which may only be retried when it
    /// carries an idempotency key.
    case none
}

/// Whether the SDK may retry a call on its own.
public enum RetryClass: String, Sendable, Equatable {
    /// A read or an idempotent RPC: the SDK retries it.
    case safe
    /// A mutation: the SDK retries it only once it carries an idempotency key.
    case manual
}

/// Whether a call answers with one message or with a stream.
public enum Streaming: String, Sendable, Equatable {
    /// One request, one response.
    case unary
    /// A server stream. There is no client streaming and no bidi (D420).
    case server
}

/// The two fields a paged call pages on, as `FacadeOptions.pagination`
/// (`"<items>:<next page token>"`) names them.
///
/// The field names are proto field names, which is what the Swift runtime sets
/// and reads; the Go SDK's equivalent carries Go PascalCase names because that is
/// what reflection finds there.
public struct Pagination: Sendable, Equatable {
    /// The response's repeated field, holding the items.
    public let itemsField: String
    /// The response's token field, empty when the last page has been sent.
    public let nextPageTokenField: String
    /// The request's `page_size` field.
    public let pageSizeField: String
    /// The request's token field.
    public let pageTokenField: String

    public init(
        itemsField: String,
        nextPageTokenField: String,
        pageSizeField: String = "page_size",
        pageTokenField: String = "page_token"
    ) {
        self.itemsField = itemsField
        self.nextPageTokenField = nextPageTokenField
        self.pageSizeField = pageSizeField
        self.pageTokenField = pageTokenField
    }
}

/// One facade call, as the runtime dispatches it.
public struct CallBinding: Sendable, Equatable {
    /// The SDK module the call is exposed on, in snake_case.
    public let module: String
    /// The call's name, in PascalCase (design §44 §7.1).
    public let name: String
    /// The name `FacadeOptions` gave it, verbatim.
    public let protoName: String
    /// The proto method that backs the call.
    public let method: String
    /// `package.Service/Method`, which is the path curl and grpcurl use.
    public let rpc: String
    /// The fully qualified service name.
    public let service: String
    /// The proto package, which is the module catalogue's key (design §44 §4,
    /// D600).
    public let package: String
    /// The method's `idempotency_level`.
    public let idempotency: IdempotencyLevel
    /// The class derived from `idempotency`, or from `FacadeOptions.retry_safe`.
    public let retry: RetryClass
    /// `.unary` or `.server`.
    public let streaming: Streaming
    /// The paging shape, or `nil` for a call that does not page.
    public let pagination: Pagination?
    /// Whether the request *message* declares an `idempotency_key` field.
    ///
    /// A property of the message rather than of the call, and it is what makes a
    /// mutation retryable (D610). The Go facade says the same thing about
    /// `Mutate`: read off the generated message, not guessed, because
    /// `DeployRequest` has no such field and must not be keyed.
    public let takesIdempotencyKey: Bool

    public init(
        module: String,
        name: String,
        protoName: String,
        method: String,
        rpc: String,
        service: String,
        package: String,
        idempotency: IdempotencyLevel,
        retry: RetryClass,
        streaming: Streaming,
        pagination: Pagination? = nil,
        takesIdempotencyKey: Bool = false
    ) {
        self.module = module
        self.name = name
        self.protoName = protoName
        self.method = method
        self.rpc = rpc
        self.service = service
        self.package = package
        self.idempotency = idempotency
        self.retry = retry
        self.streaming = streaming
        self.pagination = pagination
        self.takesIdempotencyKey = takesIdempotencyKey
    }

    /// Whether the SDK may retry this call on its own.
    ///
    /// `.safe` always; `.manual` only once the request carries an idempotency
    /// key, which is what makes the repeat the same write rather than two.
    public var isRetrySafe: Bool { retry == .safe }
}

/// One SDK module and its calls.
public struct ModuleBinding: Sendable, Equatable {
    /// The module's name in snake_case, as `loams.<name>`.
    public let name: String
    /// One line for the module's reference docs.
    public let summary: String
    /// The service behind the module.
    public let service: String
    /// The proto package, which is what `GetInstance.services[]` keys on.
    public let package: String
    /// Whether the package's wire contract may still change, so `buf breaking`
    /// skips it and an SDK marks the module experimental (§44 §10.3).
    public let unstable: Bool
    /// Whether this is a second facade name for the same RPCs, which has no
    /// summary of its own.
    public let derived: Bool
    /// The module's facade calls.
    public let calls: [CallBinding]

    public init(
        name: String,
        summary: String,
        service: String,
        package: String,
        unstable: Bool = false,
        derived: Bool = false,
        calls: [CallBinding]
    ) {
        self.name = name
        self.summary = summary
        self.service = service
        self.package = package
        self.unstable = unstable
        self.derived = derived
        self.calls = calls
    }
}

// MARK: - The catalogue

/// The service names the annotations produced.
enum LoamsService {
    static let instance = "loams.instance.v1.InstanceService"
    static let live = "loams.live.v1.LiveService"
    /// `proto/loams/live/v1/admin.proto` (LV1 Task 10): deploys, rollbacks, index
    /// status and logs. Same package as ``live``, separate service, so the
    /// engine can authorize it on its own.
    static let liveAdmin = "loams.live.v1.LiveAdminService"
}

/// Every annotated service, as the module catalogue, ordered by module name.
///
/// `instance`, `live` and `live.admin` are the modules the protos this SDK
/// speaks annotate. `tables` is a **derived** module: a second facade name for
/// `loams.live.v1`'s RPCs, which is why `guard(_:"tables")` and `guard(_:"live")`
/// answer the same question and one refusal on either means the engine is
/// absent. `live.admin` shares that package too.
///
/// `Deploy` is `live.admin`'s, not `tables`': LV1 Task 10 moved it from
/// `LiveService` to `LiveAdminService`, and the engine no longer serves
/// `loams.live.v1.LiveService/Deploy` (T10-7, T12-1).
public let loamsModules: [ModuleBinding] = loamsCoreModules + loamsAppModules

let loamsCoreModules: [ModuleBinding] = [
    ModuleBinding(
        name: "instance",
        summary: "What this instance is, and who the caller is on it.",
        service: LoamsService.instance,
        package: "loams.instance.v1",
        calls: [
            CallBinding(
                module: "instance",
                name: "GetInstance",
                protoName: "getInstance",
                method: "GetInstance",
                rpc: LoamsService.instance + "/GetInstance",
                service: LoamsService.instance,
                package: "loams.instance.v1",
                idempotency: .noSideEffects,
                retry: .safe,
                streaming: .unary
            ),
            CallBinding(
                module: "instance",
                name: "WhoAmI",
                protoName: "whoAmI",
                method: "WhoAmI",
                rpc: LoamsService.instance + "/WhoAmI",
                service: LoamsService.instance,
                package: "loams.instance.v1",
                idempotency: .noSideEffects,
                retry: .safe,
                streaming: .unary
            ),
        ]
    ),
    ModuleBinding(
        name: "live",
        summary: "Live sync: watch a query set over a server stream.",
        service: LoamsService.live,
        package: "loams.live.v1",
        unstable: true,
        calls: [
            CallBinding(
                module: "live",
                name: "Authenticate",
                protoName: "authenticate",
                method: "Authenticate",
                rpc: LoamsService.live + "/Authenticate",
                service: LoamsService.live,
                package: "loams.live.v1",
                idempotency: .none,
                retry: .manual,
                streaming: .unary
            ),
            CallBinding(
                module: "live",
                name: "ModifyQuerySet",
                protoName: "modifyQuerySet",
                method: "ModifyQuerySet",
                rpc: LoamsService.live + "/ModifyQuerySet",
                service: LoamsService.live,
                package: "loams.live.v1",
                idempotency: .none,
                retry: .manual,
                streaming: .unary
            ),
            CallBinding(
                module: "live",
                name: "Watch",
                protoName: "watch",
                method: "Watch",
                rpc: LoamsService.live + "/Watch",
                service: LoamsService.live,
                package: "loams.live.v1",
                idempotency: .none,
                retry: .manual,
                streaming: .server
            ),
        ]
    ),
    ModuleBinding(
        name: "live.admin",
        summary: "Live administration: deploy, roll back, index status and function logs.",
        service: LoamsService.liveAdmin,
        package: "loams.live.v1",
        unstable: true,
        calls: [
            CallBinding(
                module: "live.admin",
                name: "ActivateDeployment",
                protoName: "activateDeployment",
                method: "ActivateDeployment",
                rpc: LoamsService.liveAdmin + "/ActivateDeployment",
                service: LoamsService.liveAdmin,
                package: "loams.live.v1",
                idempotency: .none,
                retry: .manual,
                streaming: .unary
            ),
            CallBinding(
                module: "live.admin",
                name: "CreateApp",
                protoName: "createApp",
                method: "CreateApp",
                rpc: LoamsService.liveAdmin + "/CreateApp",
                service: LoamsService.liveAdmin,
                package: "loams.live.v1",
                idempotency: .none,
                retry: .manual,
                streaming: .unary
            ),
            CallBinding(
                module: "live.admin",
                name: "DeleteApp",
                protoName: "deleteApp",
                method: "DeleteApp",
                rpc: LoamsService.liveAdmin + "/DeleteApp",
                service: LoamsService.liveAdmin,
                package: "loams.live.v1",
                idempotency: .none,
                retry: .manual,
                streaming: .unary
            ),
            CallBinding(
                module: "live.admin",
                name: "Deploy",
                protoName: "deploy",
                method: "Deploy",
                rpc: LoamsService.liveAdmin + "/Deploy",
                service: LoamsService.liveAdmin,
                package: "loams.live.v1",
                idempotency: .none,
                retry: .manual,
                streaming: .unary
            ),
            CallBinding(
                module: "live.admin",
                name: "GetApp",
                protoName: "getApp",
                method: "GetApp",
                rpc: LoamsService.liveAdmin + "/GetApp",
                service: LoamsService.liveAdmin,
                package: "loams.live.v1",
                idempotency: .none,
                retry: .safe,
                streaming: .unary
            ),
            // The reads have no `idempotency_level`; `FacadeOptions.retry_safe`
            // (admin.proto) is what makes them `.safe`, as in the TypeScript and
            // Rust facades.
            CallBinding(
                module: "live.admin",
                name: "GetDeployment",
                protoName: "getDeployment",
                method: "GetDeployment",
                rpc: LoamsService.liveAdmin + "/GetDeployment",
                service: LoamsService.liveAdmin,
                package: "loams.live.v1",
                idempotency: .none,
                retry: .safe,
                streaming: .unary
            ),
            CallBinding(
                module: "live.admin",
                name: "GetIndexStatus",
                protoName: "getIndexStatus",
                method: "GetIndexStatus",
                rpc: LoamsService.liveAdmin + "/GetIndexStatus",
                service: LoamsService.liveAdmin,
                package: "loams.live.v1",
                idempotency: .none,
                retry: .safe,
                streaming: .unary
            ),
            CallBinding(
                module: "live.admin",
                name: "ListApps",
                protoName: "listApps",
                method: "ListApps",
                rpc: LoamsService.liveAdmin + "/ListApps",
                service: LoamsService.liveAdmin,
                package: "loams.live.v1",
                idempotency: .none,
                retry: .safe,
                streaming: .unary,
                pagination: Pagination(itemsField: "apps", nextPageTokenField: "next_page_token")
            ),
            CallBinding(
                module: "live.admin",
                name: "ListDeployments",
                protoName: "listDeployments",
                method: "ListDeployments",
                rpc: LoamsService.liveAdmin + "/ListDeployments",
                service: LoamsService.liveAdmin,
                package: "loams.live.v1",
                idempotency: .none,
                retry: .safe,
                streaming: .unary,
                pagination: Pagination(itemsField: "deployments", nextPageTokenField: "next_page_token")
            ),
            CallBinding(
                module: "live.admin",
                name: "TailLogs",
                protoName: "tailLogs",
                method: "TailLogs",
                rpc: LoamsService.liveAdmin + "/TailLogs",
                service: LoamsService.liveAdmin,
                package: "loams.live.v1",
                idempotency: .none,
                retry: .manual,
                streaming: .server
            ),
            CallBinding(
                module: "live.admin",
                name: "UpdateApp",
                protoName: "updateApp",
                method: "UpdateApp",
                rpc: LoamsService.liveAdmin + "/UpdateApp",
                service: LoamsService.liveAdmin,
                package: "loams.live.v1",
                idempotency: .none,
                retry: .manual,
                streaming: .unary
            ),
        ]
    ),
    ModuleBinding(
        name: "tables",
        summary: "",
        service: LoamsService.live,
        package: "loams.live.v1",
        unstable: true,
        derived: true,
        calls: [
            CallBinding(
                module: "tables",
                name: "Mutate",
                protoName: "mutate",
                method: "Mutate",
                rpc: LoamsService.live + "/Mutate",
                service: LoamsService.live,
                package: "loams.live.v1",
                idempotency: .none,
                retry: .manual,
                streaming: .unary,
                // `MutateRequest.idempotency_key` (live.proto:163).
                takesIdempotencyKey: true
            ),
            CallBinding(
                module: "tables",
                name: "Query",
                protoName: "query",
                method: "Query",
                rpc: LoamsService.live + "/Query",
                service: LoamsService.live,
                package: "loams.live.v1",
                idempotency: .none,
                retry: .manual,
                streaming: .unary
            ),
        ]
    ),
]

/// Every proto package in the module, as `GetInstance.apiVersions` names them.
public let loamsProtoPackages: [String] = [
    "google.protobuf",
    "loams.approvals.v1",
    "loams.collection.v1",
    "loams.devices.v1",
    "loams.errors.v1",
    "loams.graph.v1",
    "loams.instance.v1",
    "loams.live.v1",
    "loams.notifications.v1",
    "loams.operations.v1",
    "loams.options.v1",
    "loams.sql.v1",
    "loams.stream.v1",
]

/// The `type` a server puts in an error detail, so the runtime finds `ErrorInfo`
/// **by type** rather than by position.
///
/// By type and never by index: a service that adds a detail of its own must not
/// move `reason` out from under a caller (R8).
public let loamsErrorInfoTypeURL = "loams.errors.v1.ErrorInfo"

// MARK: - Lookup

/// The binding a module and call name identify, or `nil`.
///
/// `call` matches either the facade's `name` (`GetInstance`) or its `protoName`
/// (`getInstance`), because both spellings appear in application code and the
/// cost of a miss here is a runtime error on a call that exists.
public func loamsBinding(module: String, call: String) -> CallBinding? {
    for entry in loamsModules where entry.name == module {
        for candidate in entry.calls where candidate.name == call || candidate.protoName == call {
            return candidate
        }
    }
    return nil
}

/// The module binding named `name`, or `nil`.
public func loamsModule(_ name: String) -> ModuleBinding? {
    loamsModules.first { $0.name == name }
}

/// Which module owns a proto package. `loams.live` and `loams.tables` are two
/// names for one package, so the first module in registration order that claims
/// it wins; use ``loamsModules`` when the whole list of facade names matters.
public func loamsModuleOwning(_ pkg: String) -> String? {
    loamsModules.first { $0.package == pkg }?.name
}

/// Every module name that is a facade for one proto package.
///
/// This is what `LoamsSystem.guard(_:)` consults, because a guard on `live` must
/// also cover `tables`, and a refusal on either means the engine is absent.
public func loamsModulesForPackage(_ pkg: String) -> [String] {
    loamsModules.filter { $0.package == pkg }.map(\.name).sorted()
}