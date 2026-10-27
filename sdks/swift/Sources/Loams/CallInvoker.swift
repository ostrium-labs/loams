// The call path (design §44 §7.4; runtime contract R1, R2 and R3 all land here).
//
// One function runs every RPC, and the three clauses it has to satisfy at once
// are:
//
//   - **R1** — a `401` carrying `reason = token_expired` triggers **exactly one**
//     refresh and **one** retry; a second expiry is reported.
//   - **R2** — the retry class comes from the generated binding, the backoff is
//     full-jittered, and a call the class does not cover is not retried.
//   - **R3** — the idempotency key is decided **once per logical call**, before the
//     first attempt, and the *same* key goes out on every retry.
//
// # Why they are in one place
//
// Separating them is how R3 gets broken: a retry loop that re-derives its request
// per attempt will mint a second key, and one write becomes two — the exact
// failure the key exists to prevent. So `request` is computed **once**, before the
// loop, and every attempt sends the very same message. The key is part of that
// message, which makes key reuse structural rather than a rule someone has to
// remember.
//
// R1's "exactly one" is likewise structural: ``refreshed`` is a `let` binding
// outside the loop, and a second expiry cannot refresh again because the flag is
// already set and the only remaining branch is the one that gives up.

import Foundation

/// Runs one RPC, with the runtime contract's retry, credential and idempotency
/// behaviour.
///
/// It is generic over the request and response so the module methods can be
/// typed, and it is the **only** place in the SDK that knows a call is happening.
public actor CallInvoker {
    private let transport: any HTTPTransport
    private let auth: (any TokenSource)?
    private let maxRetries: Int
    private let session: ConsistencyTokenStore?
    private let defaultNamespace: String?
    private let timeout: Double

    /// The session store, or `nil` when R4's session was off.
    ///
    /// Exposed so ``Loams`` can answer `session` without holding a second
    /// reference to it, which is what keeps "off" and "on but empty"
    /// distinguishable.
    nonisolated var sessionStoreValue: (any ConsistencyTokenStore)? { session }

    /// Builds an invoker.
    ///
    /// `maxRetries` is the client's resolved default, which is already
    /// ``loamsDefaultMaxRetries`` unless the caller opted out — see `Options` for
    /// why zero does not mean "none".
    public init(
        transport: any HTTPTransport,
        auth: (any TokenSource)?,
        maxRetries: Int,
        session: ConsistencyTokenStore? = nil,
        defaultNamespace: String? = nil,
        timeout: Double = loamsDefaultTimeout
    ) {
        self.transport = transport
        self.auth = auth
        self.maxRetries = maxRetries
        self.session = session
        self.defaultNamespace = (defaultNamespace?.isEmpty ?? true) ? nil : defaultNamespace
        self.timeout = timeout
    }

    /// The namespace an app request with an empty `namespace` goes to (R11).
    public var namespace: String? { defaultNamespace }

    /// The client's resolved retry budget, for a per-call override.
    public var defaultMaxRetries: Int { maxRetries }

    /// The transport, so a resume can re-open without the runtime knowing how a
    /// transport is built.
    ///
    /// Named for its role rather than repeating `transport`: a same-named
    /// computed property over the same member is a redeclaration, not an
    /// accessor, and the initialiser already stores the value.
    var httpTransport: any HTTPTransport { transport }


    /// The bearer a call would send right now.
    ///
    /// A resume asks for a **fresh** bearer rather than reusing the one the first
    /// open used: a stream that reconnects minutes later must not re-present a
    /// token that expired while it was open, which is the failure R1's refresh
    /// exists to prevent.
    func currentBearer() async throws -> String? {
        try await auth?.token()
    }

    // MARK: Unary

    /// Runs a unary RPC.
    ///
    /// - Parameters:
    ///   - binding: the call's generated binding, which carries the retry class.
    ///   - request: the caller's message.
    ///   - suppliedIdempotencyKey: the caller's own key, if any.
    ///   - overrides: per-call options.
    public func unary<Request: LoamsMessage, Response: LoamsMessage>(
        binding: CallBinding,
        request: Request,
        suppliedIdempotencyKey: String? = nil,
        overrides: CallOptions = .init()
    ) async throws -> Response {
        let prepared = try await prepare(binding: binding, request: request, overrides: overrides)
        let keyed: KeyedRequest<Request>
        if overrides.mintIdempotencyKey == false, (prepared.idempotencyKey ?? "").isEmpty {
            keyed = KeyedRequest(request: prepared, keyed: false)
        } else {
            keyed = loamsApplyIdempotencyKey(
                prepared,
                supplied: suppliedIdempotencyKey ?? overrides.idempotencyKey,
                declared: binding.takesIdempotencyKey
            )
        }
        let retrySafe = overrides.retrySafe ?? (binding.isRetrySafe || keyed.keyed)
        let retries = overrides.maxRetries ?? maxRetries
        let budget = overrides.timeout ?? timeout
        let started = DispatchTime.now().uptimeNanoseconds
        func remainingMs() -> Int? {
            guard budget > 0 else { return nil }
            let spent = Double(DispatchTime.now().uptimeNanoseconds - started) / 1_000_000_000
            return max(1, Int(((budget - spent) * 1000).rounded(.up)))
        }
        func spent() -> Bool {
            budget > 0 && Double(DispatchTime.now().uptimeNanoseconds - started) / 1_000_000_000 >= budget
        }
        let session = sessionFor(binding: binding)
        var headers = overrides.headers
        if let token = overrides.consistencyToken, !token.isEmpty {
            headers[loamsConsistencyHeader] = token
        }
        let body = keyed.request.loamsFields()
        let refreshBudget = overrides.maxTokenRefreshes ?? 1
        var attempt = 0
        var tokenRefreshes = 0
        while true {
            if spent() { throw loamsDeadlineExceeded(binding.rpc, budget: budget) }
            var attemptHeaders = headers
            if let left = remainingMs() {
                attemptHeaders["connect-timeout-ms"] = String(left)
            }
            let wire = WireRequest(rpc: binding.rpc, body: body, serverStreaming: false, headers: attemptHeaders)
            let bearer = try await auth?.token()
            let response: WireResponse
            do {
                response = try await withDeadline(remainingMs(), rpc: binding.rpc, budget: budget) {
                    try await self.transport.unary(wire, bearer: bearer)
                }
            } catch let error as LoamsError where error.unknownReason == loamsClientDeadlineExceeded {
                throw error
            } catch {
                let mapped = LoamsError.map(error, rpc: binding.rpc)
                if attempt < retries,
                   retrySafe,
                   loamsShouldRetry(error: mapped, retrySafe: true, attempt: attempt, maxRetries: retries) {
                    try await sleepWithin(loamsBackoff(attempt: attempt), remainingMs(), rpc: binding.rpc, budget: budget)
                    attempt += 1
                    continue
                }
                throw mapped
            }
            switch response {
            case .success(let body, let trailers):
                if let store = session {
                    // A store that cannot merge a token counts it: the RPC succeeded.
                    for name in ["consistencyToken", "consistency_token", "token", "readToken", "read_token"] {
                        if let token = body[name]?.stringValue, !token.isEmpty {
                            try? await store.record(token)
                        }
                    }
                    if let header = trailers[loamsConsistencyHeader.lowercased()], !header.isEmpty {
                        try? await store.record(header)
                    }
                }
                return try Self.decode(Response.self, from: body, rpc: binding.rpc)
            case .failure(let code, let message, let info, _):
                let mapped = LoamsError.fromWire(code: code, info: info, rpc: binding.rpc, message: message)
                let isExpired = mapped is TokenExpiredError
                if isExpired, tokenRefreshes < refreshBudget {
                    tokenRefreshes += 1
                    try await auth?.refresh()
                    continue
                }
                if attempt < retries,
                   loamsShouldRetry(error: mapped, retrySafe: retrySafe, attempt: attempt, maxRetries: retries) {
                    try await sleepWithin(loamsBackoff(attempt: attempt), remainingMs(), rpc: binding.rpc, budget: budget)
                    attempt += 1
                    continue
                }
                throw mapped
            }
        }
    }

    /// R12: runs `body`, failing with `client_deadline_exceeded` when `ms` passes first.
    private func withDeadline<T: Sendable>(
        _ ms: Int?,
        rpc: String,
        budget: Double,
        _ body: @escaping @Sendable () async throws -> T
    ) async throws -> T {
        guard let ms else { return try await body() }
        return try await withThrowingTaskGroup(of: T?.self) { group in
            group.addTask { try await body() }
            group.addTask {
                try await Task.sleep(nanoseconds: UInt64(ms) * 1_000_000)
                return nil
            }
            defer { group.cancelAll() }
            guard let first = try await group.next(), let value = first else {
                throw loamsDeadlineExceeded(rpc, budget: budget)
            }
            return value
        }
    }

    /// A backoff that does not outlive the budget: when it would, the call is spent.
    private func sleepWithin(_ backoff: Int, _ left: Int?, rpc: String, budget: Double) async throws {
        if let left, backoff >= left {
            throw loamsDeadlineExceeded(rpc, budget: budget)
        }
        try await loamsSleep(milliseconds: backoff)
    }

    /// R4's session for this binding: the collection reads and writes only.
    private func sessionFor(binding: CallBinding) -> (any ConsistencyTokenStore)? {
        guard let session, loamsSessionPackages.contains(binding.package) else { return nil }
        return session
    }

    /// R11's namespace and R4's read half, on a copy of the request.
    private func prepare<Request: LoamsMessage>(
        binding: CallBinding,
        request: Request,
        overrides: CallOptions
    ) async throws -> Request {
        guard var dynamic = request as? DynamicMessage else { return request }
        if loamsNamespacedPackages.contains(binding.package),
           !loamsNoTopLevelNamespace.contains(binding.rpc),
           (dynamic.fields["namespace"]?.stringValue ?? "").isEmpty {
            guard let fill = overrides.namespace ?? defaultNamespace, !fill.isEmpty else {
                throw LoamsError.loams(
                    code: .invalidArgument,
                    reason: .invalidArgument,
                    unknownReason: "namespace_required",
                    metadata: ["field": "namespace"],
                    hint: "",
                    rpc: binding.rpc,
                    message: "\(binding.rpc): no namespace: pass one, or give the client a default "
                        + "(Options.namespace or LOAMS_NAMESPACE)"
                )
            }
            dynamic.fields["namespace"] = .string(fill)
        }
        if let session = sessionFor(binding: binding),
           loamsSessionReads.contains(binding.rpc),
           (overrides.consistencyToken ?? "").isEmpty,
           dynamic.fields["consistency"] == nil,
           let current = await session.current() {
            dynamic.fields["consistency"] = .object(["atLeast": .string(current)])
        }
        return (dynamic as? Request) ?? request
    }

    /// ``ResumableServerStream`` for why that matters.
    public func serverStream<Request: LoamsMessage, Response: LoamsMessage>(
        binding: CallBinding,
        request: Request,
        suppliedIdempotencyKey: String? = nil
    ) async throws -> ResumableServerStream<Request, Response> {
        let keyed = loamsApplyIdempotencyKey(
            request,
            supplied: suppliedIdempotencyKey,
            declared: binding.takesIdempotencyKey
        )
        let bearer = try await auth?.token()
        let wire = WireRequest(
            rpc: binding.rpc,
            body: keyed.request.loamsFields(),
            serverStreaming: true
        )
        let source = try await transport.serverStream(wire, bearer: bearer)
        return ResumableServerStream(
            source: source,
            binding: binding,
            original: keyed.request,
            invoker: self,
            maxRetries: maxRetries
        )
    }

    // MARK: Decoding

    /// Rebuilds a typed response from a field dictionary.
    ///
    /// A message that will not decode is an `internal` failure naming the type,
    /// because it means the SDK and the server disagree about a shape — which is a
    /// bug in one of them, not a caller error, and `internal` is what D611 says
    /// `internal` means.
    static func decode<Response: LoamsMessage>(
        _ type: Response.Type,
        from body: [String: JSONValue],
        rpc: String
    ) throws -> Response {
        guard let message = Response(loamsFields: body) else {
            throw LoamsError.internalError(
                rpc,
                "loams: could not decode a \(Response.protoTypeName) from the response body"
            )
        }
        return message
    }
}

// MARK: - Per-call options

/// The request header that selects one of a Live node's apps
/// (`loams.live.v1.LiveAdminService`, LV1 Task 13). A request without it
/// addresses the server's own app (`--live-app`).
public let loamsLiveAppHeader = "loams-live-app"

/// Per-call overrides.
///
/// They are a struct rather than trailing closures because Swift has no default
/// arguments in a variadic position, and a `Set<Option>` would allocate on every
/// call for options almost nobody sets.
public struct CallOptions: Sendable {
    /// The retries after the first attempt for **this** call, overriding the
    /// client's default.
    ///
    /// Unlike the client's `maxRetries`, `0` here unambiguously means none:
    /// writing the number on a call is an explicit choice, so there is no
    /// "did they mean zero or did they not ask?" question to resolve.
    public var maxRetries: Int?
    /// The credential refreshes allowed for this call. `0` disables R1's refresh
    /// entirely, which is what a caller asserting "a 401 must reach me" sets.
    public var maxTokenRefreshes: Int?
    /// A per-call idempotency key, overriding the SDK's.
    public var idempotencyKey: String?
    /// Headers for this call only, for a gateway or a trace.
    public var headers: [String: String]
    /// This call's namespace (R11), over the client's default; a request's own
    /// `namespace` wins over both.
    public var namespace: String?
    /// This call's deadline in seconds (R12); nil takes the client's, 0 means none.
    public var timeout: Double?
    /// Reads at this consistency token (`Loams-Consistency-Token`).
    public var consistencyToken: String?
    /// Forces the retry class: true retries a call the SDK would not.
    public var retrySafe: Bool?
    /// False sends a keyable request unkeyed, and so never retried unless safe.
    public var mintIdempotencyKey: Bool?

    public init(
        maxRetries: Int? = nil,
        maxTokenRefreshes: Int? = nil,
        idempotencyKey: String? = nil,
        headers: [String: String] = [:],
        namespace: String? = nil,
        timeout: Double? = nil,
        consistencyToken: String? = nil,
        retrySafe: Bool? = nil,
        mintIdempotencyKey: Bool? = nil
    ) {
        self.namespace = namespace
        self.timeout = timeout
        self.consistencyToken = consistencyToken
        self.retrySafe = retrySafe
        self.mintIdempotencyKey = mintIdempotencyKey
        self.maxRetries = maxRetries
        self.maxTokenRefreshes = maxTokenRefreshes
        self.idempotencyKey = idempotencyKey
        self.headers = headers
    }

    /// Options that address the Live app `name` (the ``loamsLiveAppHeader``
    /// header). Set the same header in `extraHeaders` to address one app on
    /// every call a client makes.
    public static func liveApp(_ name: String, _ base: CallOptions = .init()) -> CallOptions {
        var options = base
        options.headers[loamsLiveAppHeader] = name
        return options
    }
}
