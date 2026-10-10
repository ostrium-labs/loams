// Feature detection and version reporting (design §44 §4, D600; runtime contract
// R5 and R9).
//
// Two halves of R5, and an SDK needs both.
//
//  1. **Without calling.** `GetInstance.services[]` says which packages this
//     binary carries. One call, no auth, cheap. ``available(_:)``,
//     ``served()``, ``unavailable()`` and ``guard(_:)`` wrap it; the catalogue is
//     cached for the life of the client and concurrent readers share one in-flight
//     fetch, so a hundred tasks asking at once cost one call.
//  2. **When the caller calls anyway.** Every RPC of an absent package answers
//     `unimplemented` with `reason = feature_not_in_variant` and the variant in
//     `metadata.variant`. The transport turns that into a
//     ``FeatureNotInVariantError`` (see `Errors.swift`), so the branch is a type
//     check and never the package name, which is a proto detail, and never the
//     message.
//
// ``guard(_:)`` raises the *same* error type, so one `catch` covers "the guard
// said no" and "the server refused", and the guard costs no request once the
// catalogue is cached.

import Foundation

/// The module catalogue, as one call reports it.
public struct Catalogue: Sendable, Equatable {
    /// The packages this binary serves.
    public let served: [String]
    /// The packages it knows about and does not serve.
    public let unavailable: [String]
    /// The packages this SDK speaks that the instance does not list **at all**.
    ///
    /// A package whose services have not been defined yet has no entry, so this is
    /// different from one that exists and is switched off — conflating the two
    /// would report a server bug as a missing feature.
    public let missing: [String]
    /// Every entry, in the server's order.
    public let services: [ServiceStatus]

    public init(served: [String], unavailable: [String], missing: [String], services: [ServiceStatus]) {
        self.served = served
        self.unavailable = unavailable
        self.missing = missing
        self.services = services
    }

    /// Whether the catalogue says a package is served.
    ///
    /// Three states collapse into a boolean: a package that is not listed at all
    /// is *not* available, which is the answer a caller wants even though it means
    /// something different.
    func serves(_ pkg: String) -> Bool {
        services.first { $0.package == pkg }?.available ?? false
    }
}

/// What the SDK speaks beside what the server serves (R9).
public struct VersionReport: Sendable, Equatable {
    /// The proto revision this SDK was generated from.
    public let protoRev: String
    /// The server's own semver, as `GetInstance` reports it.
    public let serverVersion: String
    /// The server's `GetInstance.apiVersions`: only what it serves.
    public let apiVersions: [String]
    /// Whether every package the SDK speaks is served.
    public let compatible: Bool
    /// The SDK's packages the server does not serve.
    public let missing: [String]

    public init(
        protoRev: String,
        serverVersion: String,
        apiVersions: [String],
        compatible: Bool,
        missing: [String]
    ) {
        self.protoRev = protoRev
        self.serverVersion = serverVersion
        self.apiVersions = apiVersions
        self.compatible = compatible
        self.missing = missing
    }
}

/// The module catalogue, feature detection and the version check.
///
/// An `actor` for R5's "concurrent readers share one in-flight fetch": the
/// catalogue is fetched once and every concurrent reader waits on that one fetch,
/// and Swift's actor is the synchronisation boundary that makes that checkable
/// rather than reviewed.
public actor LoamsSystem {
    /// What ``variantUnknown`` says when the SDK learned a package was absent from
    /// the catalogue rather than from a refusal.
    ///
    /// A plausible-looking variant in a support ticket is worse than an honest
    /// `unknown`.
    public static let variantUnknown = "unknown"

    private let invoker: CallInvoker
    private let endpoint: URL
    /// Every proto package this SDK speaks, which is what R9's report compares the
    /// server's `apiVersions` against.
    ///
    /// Computed once at construction: it is a property of the SDK, not of the
    /// server.
    private let speaks: [String]

    private var cached: Catalogue?
    private var inFlight: Task<Catalogue, any Error>?

    public init(invoker: CallInvoker, endpoint: URL) {
        self.invoker = invoker
        self.endpoint = endpoint
        // Filtered to the `loams.` packages, which is what
        // `GetInstance.apiVersions` names. `google.protobuf` is the well-known
        // types, and a client never asks an instance to serve them, so including
        // it would make every instance look incompatible.
        self.speaks = loamsProtoPackages.filter { $0.hasPrefix("loams.") }
    }

    /// The instance this client talks to.
    public nonisolated var instanceEndpoint: URL { endpoint }

    /// Forgets the cached catalogue, so the next check calls again.
    public func invalidate() {
        cached = nil
    }

    /// The module catalogue, fetching it once and sharing the result with every
    /// concurrent reader.
    ///
    /// `LOAMS_TEST_ENDPOINT` and a `loams dev` are the same shape here: one
    /// `GetInstance` call, no auth.
    public func catalogue() async throws -> Catalogue {
        if let cached { return cached }
        // One shared fetch. The `Task` is unstructured on purpose: inheriting the
        // first caller's cancellation would tear the catalogue down under every
        // other reader waiting on it.
        if let inFlight {
            return try await inFlight.value
        }
        let task = Task<Catalogue, any Error> { [invoker, speaks] in
            guard let binding = loamsBinding(module: "instance", call: "GetInstance") else {
                throw LoamsError.internalError("", "loams: the facade has no instance.GetInstance binding")
            }
            let info: GetInstanceResponse = try await invoker.unary(
                binding: binding,
                request: GetInstanceRequest()
            )
            return Self.catalogue(from: info, speaks: speaks)
        }
        inFlight = task
        defer { inFlight = nil }
        let result = try await task.value
        cached = result
        return result
    }

    /// The catalogue as `GetInstance` reports it.
    static func catalogue(from info: GetInstanceResponse, speaks: [String]) -> Catalogue {
        var served: [String] = []
        var unavailable: [String] = []
        var services: [ServiceStatus] = []

        for status in info.services {
            services.append(status)
            if status.available {
                served.append(status.package)
            } else {
                unavailable.append(status.package)
            }
        }
        let missing = speaks.filter { pkg in !services.contains { $0.package == pkg } }
        return Catalogue(served: served, unavailable: unavailable, missing: missing, services: services)
    }

    /// Whether this instance serves a proto package. It costs no RPC once the
    /// catalogue is cached.
    public func available(_ pkg: String) async throws -> Bool {
        try await catalogue().serves(pkg)
    }

    /// The packages this instance serves.
    public func served() async throws -> [String] {
        try await catalogue().served
    }

    /// The packages this instance knows about and does not serve.
    public func unavailable() async throws -> [String] {
        try await catalogue().unavailable
    }

    /// Whether an SDK module's package is served, taking a **module name or a
    /// proto package** so a caller holding `loams.live` and a caller reading a
    /// ``ServiceStatus`` can both ask.
    ///
    /// A module name is resolved through the facade table, so `loams.live` and
    /// `loams.tables` — two facade names for one package — answer the same thing.
    public func availableModule(_ moduleOrPackage: String) async throws -> Bool {
        try await available(try loamsPackage(of: moduleOrPackage))
    }

    /// The proto package behind a module name or a package name.
    public func loamsPackage(of moduleOrPackage: String) throws -> String {
        if moduleOrPackage.hasPrefix("loams.") { return moduleOrPackage }
        guard let module = loamsModule(moduleOrPackage) else {
            throw LoamsError.internalError("", "loams has no generated module \(moduleOrPackage)")
        }
        return module.package
    }

    /// Raises a ``FeatureNotInVariantError`` when a module's package is not served
    /// here, and returns normally when it is.
    ///
    /// The error is the **same type** a refused RPC produces, so one `catch`
    /// covers both, and it costs no request once the catalogue is cached:
    ///
    ///     do {
    ///         try await client.system.guard("live")
    ///     } catch let error as FeatureNotInVariantError {
    ///         // run without live sync
    ///     }
    ///
    /// The message names the package rather than the module, because the package
    /// is what the server knows about and what an operator will grep for.
    /// Backticked because `guard` is a Swift keyword: without them this cannot
    /// be declared at all. The name is worth keeping — it is what the doc
    /// examples and the conformance tests call, and renaming it would change the
    /// SDK's public surface to work around a language rule.
    public func `guard`(_ moduleOrPackage: String) async throws {
        let pkg = try loamsPackage(of: moduleOrPackage)
        if try await available(pkg) { return }
        // The guard learned this from the catalogue rather than from a refusal, so
        // the `rpc` is `GetInstance` — the call that actually answered — and the
        // variant is recorded as unknown rather than invented.
        let base = LoamsError.loams(
            code: .unimplemented,
            reason: .featureNotInVariant,
            unknownReason: nil,
            metadata: ["package": pkg, "variant": Self.variantUnknown],
            hint: "this build variant does not carry \(pkg)",
            rpc: "loams.instance.v1.InstanceService/GetInstance",
            message: "loams.\(moduleOrPackage) is not available on this instance: \(pkg) is not served"
        )
        throw FeatureNotInVariantError(base: base, variant: Self.variantUnknown)
    }

    /// Reports what this SDK speaks beside what the server serves (R9).
    ///
    /// A package the SDK speaks and the server does not serve is reported in
    /// `missing` and `compatible` is false; it is **not** an error. The SDK still
    /// works for the modules that are there, and what a missing one means is the
    /// caller's decision.
    public func version() async throws -> VersionReport {
        guard let binding = loamsBinding(module: "instance", call: "GetInstance") else {
            throw LoamsError.internalError("", "loams: the facade has no instance.GetInstance binding")
        }
        let info: GetInstanceResponse = try await invoker.unary(
            binding: binding,
            request: GetInstanceRequest()
        )
        var compatible = true
        var missing: [String] = []
        for pkg in speaks where !info.apiVersions.contains(pkg) {
            compatible = false
            missing.append(pkg)
        }
        return VersionReport(
            protoRev: loamsProtoRevision,
            serverVersion: info.serverVersion,
            apiVersions: info.apiVersions,
            compatible: compatible,
            missing: missing
        )
    }
}