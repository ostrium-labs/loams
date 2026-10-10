// The `Loams` object: one client with namespaced modules (design §44 §7.1).
//
//     let client = try Loams(
//         Options(
//             endpoint: URL(string: ProcessInfo.processInfo.environment["LOAMS_ENDPOINT"] ?? "")!,
//             auth: APIKeyTokenSource(ProcessInfo.processInfo.environment["LOAMS_API_KEY"] ?? "")
//         )
//     )
//     let info = try await client.instance.getInstance()
//
// # What is generated and what is hand-written, once more
//
// The **module surface** is generated from the `loams.options.v1` annotations on
// the protos: `Facade.swift` holds the module catalogue and the binding table, and
// `Modules.swift` is one method per binding that hands it to the same invoker.
// The **runtime** behind those methods is hand-written, once, in this package:
// transport, credentials, retry, errors, tokens, pagination, streams. This file
// is the thin join — it builds the transport, wires the runtime, and holds the
// client-wide defaults. It contains no RPC path and no retry class, which is why
// annotating a proto is enough to add an SDK method.
//
// **For Swift the module surface is the Q604 hand-written fallback** rather than
// generated; see `Facade.swift`'s header.

import Foundation

#if canImport(FoundationNetworking)
// `URLSession` lives in FoundationNetworking on Linux and Foundation on Apple
// platforms. The plan's Task 4 row lists Linux as a target, so every file that
// names `URLSession` or `URLSessionConfiguration` needs this, not just the one
// that makes the request.
import FoundationNetworking
#endif

/// How to build a ``Loams`` client.
public struct Options: Sendable {
    /// The instance's base URL, for example `https://acme.loams.dev`. A loopback
    /// stack is `http://127.0.0.1:8080`.
    public var endpoint: URL
    /// The bearer source: ``APIKeyTokenSource`` for a script or a CI job,
    /// ``StaticTokenSource``, ``EnvTokenSource``, ``OIDCExchange``, or any
    /// `TokenSource` of your own.
    ///
    /// Omitted means an unauthenticated client, which is what
    /// `client.instance.getInstance()` needs anyway.
    public var auth: (any TokenSource)?
    /// Configures the wire. The defaults are Connect over HTTP/1.1 with the JSON
    /// codec, which works through every proxy an instance is likely to sit behind.
    public var transport: TransportConfig?
    /// The `URLSession` the transport uses. It wins over `transport`, so a caller
    /// with its own TLS config, proxy or timeout supplies it here.
    public var session: URLSession?
    /// The retries after the first attempt, for every call.
    ///
    /// **`nil` means ``loamsDefaultMaxRetries`` (3), not none.** The design's
    /// number is the client's number: an SDK that silently did not retry unless it
    /// was configured to would be an SDK whose safe-by-default behaviour is a
    /// sharp edge, and a caller writing `Options(endpoint: …)` would get something
    /// other than what every other SDK does.
    ///
    /// Swift's `nil`-means-default is the opposite of Go's zero-value problem, so
    /// the opt-out is spelled:
    ///
    ///     let client = try Loams(Options(endpoint: url, noRetries: true))
    ///
    /// A **call** may still pass `maxRetries: 0`, which unambiguously means none,
    /// because writing the number there is an explicit choice.
    public var maxRetries: Int?
    /// Turns off automatic retries for the whole client.
    public var noRetries: Bool
    /// Holds a session consistency token across calls (D609).
    ///
    /// **Off by default**: every read is then `STRONG` on its own, which is
    /// correct but does not give read-your-writes across processes. Turn it on when
    /// one process is both writing and reading, and remember it keeps the token it
    /// was given rather than merging — see `Consistency.swift`.
    public var sessionConsistency: Bool

    public init(
        endpoint: URL,
        auth: (any TokenSource)? = nil,
        transport: TransportConfig? = nil,
        session: URLSession? = nil,
        maxRetries: Int? = nil,
        noRetries: Bool = false,
        sessionConsistency: Bool = false
    ) {
        self.endpoint = endpoint
        self.auth = auth
        self.transport = transport
        self.session = session
        self.maxRetries = maxRetries
        self.noRetries = noRetries
        self.sessionConsistency = sessionConsistency
    }
}

/// One SDK over one instance.
///
/// It is safe for concurrent use: the invoker, the token source and the session
/// are all actors, and every other field is immutable. The one thing a caller must
/// not do is tear down the `URLSession` it supplied while calls are in flight.
public final class Loams: Sendable {
    /// The invoker every module method goes through.
    public let invoker: CallInvoker
    /// `loams.instance` — what this instance is, and who the caller is.
    public let instance: InstanceModule
    /// `loams.live` — the live sync session half. Its package is unstable, so its
    /// wire contract may still change (§44 §10.3).
    public let live: LiveModule
    /// `loams.tables` — the table half of the same service (design §44 §7.2).
    public let tables: TablesModule
    /// The module catalogue, feature detection and the version check.
    public let system: LoamsSystem

    /// The transport configuration, for a failure message.
    public let config: TransportConfig

    /// Builds a client.
    ///
    /// An empty or schemeless endpoint is a usage error rather than a request to
    /// localhost: a client that quietly talks to the wrong instance is worse than
    /// one that does not start.
    public init(_ options: Options) throws {
        guard let scheme = options.endpoint.scheme,
              scheme == "https" || scheme == "http",
              options.endpoint.host != nil
        else {
            throw LoamsError.internalError(
                "",
                "loams: Options.endpoint is not an absolute http(s) URL; it is the instance's base URL, "
                    + "for example https://acme.loams.dev"
            )
        }

        var config = options.transport ?? TransportConfig(endpoint: options.endpoint)
        config.endpoint = options.endpoint

        let transport = ConnectTransport(
            config: config,
            session: options.session ?? .shared
        )
        self.config = config

        var resolved = options.maxRetries ?? loamsDefaultMaxRetries
        if resolved < 0 { resolved = loamsDefaultMaxRetries }
        if options.noRetries { resolved = 0 }

        let session: ConsistencyTokenStore? = options.sessionConsistency ? ConsistencySession() : nil
        self.invoker = CallInvoker(
            transport: transport,
            auth: options.auth,
            maxRetries: resolved,
            session: session
        )
        self.instance = InstanceModule(invoker: invoker)
        self.live = LiveModule(invoker: invoker)
        self.tables = TablesModule(invoker: invoker)
        self.system = LoamsSystem(invoker: invoker, endpoint: options.endpoint)
    }

    /// The proto revision this SDK declares (§44 §10.3).
    public var protoRev: String { loamsProtoRevision }

    /// Every proto package in the module, as `GetInstance` names them.
    public var protoPackages: [String] { loamsProtoPackages }

    /// The generated binding table, as the SDK sees it.
    public var modules: [ModuleBinding] { loamsModules }

    /// The session consistency token store, or `nil` when
    /// `Options.sessionConsistency` was off.
    ///
    /// `nil` rather than an inert store, so "not on" and "on but empty" do not look
    /// the same.
    public var session: (any ConsistencyTokenStore)? {
        get async { await invoker.sessionStore() }
    }

    /// Forgets the cached service catalogue, so the next feature check calls again.
    public func invalidateCatalogue() async {
        await system.invalidate()
    }
}

extension CallInvoker {
    /// The session store, or `nil` when R4's session was off.
    func sessionStore() async -> (any ConsistencyTokenStore)? {
        sessionStoreValue
    }
}