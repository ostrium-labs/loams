// The concrete message types the facade's module methods are typed against.
//
// # Provenance
//
// Hand-written with the rest of the facade, for the reason `Facade.swift`'s header
// gives: `protoc-gen-swift` has not been run over `proto/` anywhere in this
// repository, so there is no `Loams_Instance_V1_GetInstanceRequest` to import.
//
// Only `loams.instance.v1` is typed. The `loams.live.v1` messages stay
// `DynamicMessage`, which is honest about the fact that a hand-written facade
// should transcribe what the conformance corpus exercises — and the corpus
// exercises `GetInstance` (success) and refuses `WhoAmI` and `Query` without ever
// returning a `loams.live.v1` message (design §44 §10.4). Inventing typed
// structs for messages no test and no server has ever produced would be more
// hand-written surface to keep wrong, which is what D606's fallback is trying to
// avoid.
//
// When the generated stubs land these are deleted and replaced by imports; the
// module signatures in `Modules.swift` already name them.

import Foundation

// MARK: - loams.instance.v1

/// `loams.instance.v1.GetInstanceRequest`. Empty by construction, and that is the
/// point: it is the one call any client can make, because it needs no credential.
public struct GetInstanceRequest: LoamsMessage, Sendable, Equatable {
    public init() {}

    public static let protoTypeName = "loams.instance.v1.GetInstanceRequest"

    public func loamsFields() -> [String: JSONValue] { [:] }

    public init?(loamsFields fields: [String: JSONValue]) {
        // Accepts anything: the request has no fields, so a server that echoed
        // one would still be describing an empty request. Refusing here would make
        // the SDK the thing that breaks, not the server.
        self.init()
    }
}

/// One package of the API catalogue, as `GetInstance.services[]` reports it.
public struct ServiceStatus: Sendable, Equatable {
    /// The proto package, for example `loams.live.v1`.
    public let package: String
    /// The package's API version, for example `v1`.
    public let version: String
    /// Whether this binary serves the package.
    ///
    /// False means every one of its RPCs answers `unimplemented` with reason
    /// `feature_not_in_variant`.
    public let available: Bool
    /// The fully qualified service names in the package.
    public let services: [String]
    /// Whether the package's wire contract may still change.
    public let unstable: Bool

    public init(package: String, version: String, available: Bool, services: [String], unstable: Bool) {
        self.package = package
        self.version = version
        self.available = available
        self.services = services
        self.unstable = unstable
    }
}

/// `loams.instance.v1.GetInstanceResponse`.
public struct GetInstanceResponse: LoamsMessage, Sendable, Equatable {
    /// An empty response: the protocol requires `init()`, and every field of
    /// `GetInstanceResponse` is optional on the wire, so empty is a value a
    /// real server can send.
    public init() {
        self.name = ""
        self.serverVersion = ""
        self.apiVersions = []
        self.services = []
    }

    /// The instance's name. `Loams` in the recorded corpus.
    public let name: String
    /// The instance's own semver.
    public let serverVersion: String
    /// The packages this instance **serves**.
    ///
    /// Only what is served, so a package this SDK speaks and the server does not
    /// is *missing* rather than reported as available — which is what makes R9's
    /// comparison honest.
    public let apiVersions: [String]
    /// The catalogue: every package this binary knows about.
    public let services: [ServiceStatus]

    public init(name: String, serverVersion: String, apiVersions: [String], services: [ServiceStatus]) {
        self.name = name
        self.serverVersion = serverVersion
        self.apiVersions = apiVersions
        self.services = services
    }

    public static let protoTypeName = "loams.instance.v1.GetInstanceResponse"

    public func loamsFields() -> [String: JSONValue] {
        [
            "name": .string(name),
            "server_version": .string(serverVersion),
            "api_versions": .array(apiVersions.map { .string($0) }),
            "services": .array(services.map { status in
                .object([
                    "package": .string(status.package),
                    "version": .string(status.version),
                    "available": .bool(status.available),
                    "services": .array(status.services.map { .string($0) }),
                    "unstable": .bool(status.unstable),
                ])
            }),
        ]
    }

    public init?(loamsFields fields: [String: JSONValue]) {
        guard let name = fields["name"]?.stringValue else { return nil }
        self.name = name
        // Every other field is optional with a documented default, because a
        // minimal server answers `GetInstance` with a name and nothing else —
        // and refusing that would make the SDK fail on a legal response.
        self.serverVersion = fields["server_version"]?.stringValue ?? ""
        self.apiVersions = fields["api_versions"]?.arrayValue?.compactMap(\.stringValue) ?? []
        self.services = (fields["services"]?.arrayValue ?? []).compactMap { entry in
            guard let object = entry.objectValue,
                  let package = object["package"]?.stringValue
            else { return nil }
            return ServiceStatus(
                package: package,
                version: object["version"]?.stringValue ?? "",
                available: object["available"]?.boolValue ?? false,
                services: object["services"]?.arrayValue?.compactMap(\.stringValue) ?? [],
                unstable: object["unstable"]?.boolValue ?? false
            )
        }
    }
}

/// `loams.instance.v1.WhoAmIRequest`.
public struct WhoAmIRequest: LoamsMessage, Sendable, Equatable {
    public init() {}
    public static let protoTypeName = "loams.instance.v1.WhoAmIRequest"
    public func loamsFields() -> [String: JSONValue] { [:] }
    public init?(loamsFields fields: [String: JSONValue]) { self.init() }
}

/// `loams.instance.v1.WhoAmIResponse`.
///
/// It exists so the module method has a return type to name. **This build has no
/// authentication yet**, so every variant refuses the call with
/// `reason = not_implemented` — which is the recorded corpus's
/// `instance_who_am_i_*` case and is exactly what R8's reason mapping is pinned
/// against.
public struct WhoAmIResponse: LoamsMessage, Sendable, Equatable {
    public init() {}
    public static let protoTypeName = "loams.instance.v1.WhoAmIResponse"
    public func loamsFields() -> [String: JSONValue] { [:] }
    public init?(loamsFields fields: [String: JSONValue]) { self.init() }
}

// MARK: - loams.live.v1

/// The proto type name of `loams.live.v1.LiveService/Watch`'s message.
public let watchRequestTypeName = "loams.live.v1.WatchRequest"

/// The proto type name of `loams.live.v1.LiveService/Query`'s request.
public let queryRequestTypeName = "loams.live.v1.QueryRequest"

/// The proto type name of `loams.live.v1.LiveService/Mutate`'s request.
public let mutateRequestTypeName = "loams.live.v1.MutateRequest"

/// The proto type name of `loams.live.v1.LiveService/Deploy`'s request.
public let deployRequestTypeName = "loams.live.v1.DeployRequest"

/// The proto type name of `loams.live.v1.LiveService/ModifyQuerySet`'s request.
public let modifyQuerySetRequestTypeName = "loams.live.v1.ModifyQuerySetRequest"