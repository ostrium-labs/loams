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
        session: ConsistencyTokenStore? = nil
    ) {
        self.transport = transport
        self.auth = auth
        self.maxRetries = maxRetries
        self.session = session
    }

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
        // R3: decided ONCE, before the first attempt, and never recomputed. The
        // retry loop below sends `wire` verbatim every time.
        let keyed = loamsApplyIdempotencyKey(
            request,
            supplied: suppliedIdempotencyKey,
            declared: binding.takesIdempotencyKey
        )

        // A mutation is retryable only once it carries a key: that is what makes
        // the repeat the same write rather than two. `keyed.request` is the
        // message, so this reads R3's answer rather than re-deciding it.
        let retrySafe = binding.isRetrySafe || keyed.keyed
        let retries = overrides.maxRetries ?? maxRetries

        let wire = WireRequest(
            rpc: binding.rpc,
            body: keyed.request.loamsFields(),
            serverStreaming: false
        )

        // R1: the refresh budget is a counter that lives **outside** the loop, so
        // "exactly one refresh" is enforced by the counter rather than by a flag
        // someone has to remember to check. A second expiry finds the budget
        // spent and falls through to the report below.
        let refreshBudget = overrides.maxTokenRefreshes ?? 1

        var attempt = 0
        var tokenRefreshes = 0

        while true {
            let bearer = try await auth?.token()

            let response: WireResponse
            do {
                response = try await transport.unary(wire, bearer: bearer)
            } catch {
                let mapped = LoamsError.map(error, rpc: binding.rpc)
                // A failure from below the API retries on the same terms as a wire
                // one: a socket that dropped mid-request is indistinguishable from
                // a `unavailable` as far as the caller is concerned.
                if attempt < retries,
                   retrySafe,
                   loamsShouldRetry(error: mapped, retrySafe: true, attempt: attempt, maxRetries: retries) {
                    try await loamsSleep(milliseconds: loamsBackoff(attempt: attempt))
                    attempt += 1
                    continue
                }
                throw mapped
            }

            switch response {
            case .success(let body, _):
                // R4: fold the response's token into the session, when one is on.
                // A failure to fold is counted, not thrown — the RPC already
                // succeeded and a caller retrying on that error writes twice.
                if let store = session {
                    let message = DynamicMessage(protoTypeName: "", fields: body)
                    try? await store.record(message.consistencyToken ?? "")
                }
                return try Self.decode(Response.self, from: body, rpc: binding.rpc)

            case .failure(let code, let message, let info, _):
                let mapped = LoamsError.fromWire(code: code, info: info, rpc: binding.rpc, message: message)

                // R1: refresh once and retry once, and only for `token_expired`.
                // `unauthenticated` with any other reason is a real refusal — a
                // caller with no credential at all — and retrying it would spend
                // the call's budget on a request that cannot succeed.
                let isExpired = mapped is TokenExpiredError
                if isExpired, tokenRefreshes < refreshBudget {
                    tokenRefreshes += 1
                    try await auth?.refresh()
                    // No `attempt += 1`: the credential refresh is **not** a
                    // transport retry and must not be charged to the retry budget
                    // R2 bounds. A call whose first attempt found a stale token
                    // should still get its three transport retries.
                    continue
                }

                if attempt < retries,
                   loamsShouldRetry(error: mapped, retrySafe: retrySafe, attempt: attempt, maxRetries: retries) {
                    try await loamsSleep(milliseconds: loamsBackoff(attempt: attempt))
                    attempt += 1
                    continue
                }
                throw mapped
            }
        }
    }

    // MARK: Server streams

    /// Opens a server stream.
    ///
    /// The stream's **resume** half lives in ``ResumableServerStream``, because
    /// re-opening is a decision made between messages rather than during the
    /// attempt that opened it. What is here is the open, the bearer, and the
    /// R3 key decision, which the re-opens reuse — see
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

    public init(
        maxRetries: Int? = nil,
        maxTokenRefreshes: Int? = nil,
        idempotencyKey: String? = nil,
        headers: [String: String] = [:]
    ) {
        self.maxRetries = maxRetries
        self.maxTokenRefreshes = maxTokenRefreshes
        self.idempotencyKey = idempotencyKey
        self.headers = headers
    }
}