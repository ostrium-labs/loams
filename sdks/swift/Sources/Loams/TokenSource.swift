// Token sources (design §44 §7.4, D608; runtime contract R1).
//
// A `TokenSource` returns a bearer and can be asked for a new one. Tokens travel
// in `Authorization: Bearer` and **never** in a URL: a query string ends up in
// proxy logs, in browser history and in `Referer`. A `401` carrying `reason =
// token_expired` triggers one refresh and one retry; that logic lives in the call
// path (`CallInvoker.swift`), so a source stays a source.
//
// In Swift a source is a protocol with two requirements, and the refresh is
// optional-by-behaviour rather than optional-by-type: a source that cannot refresh
// — an API key, which does not expire — returns from ``TokenSource/refresh()``
// without minting anything, which is the no-op R1 describes.
//
// `refresh()` is `async throws` because fetching a token is a network call, and
// Swift will not let a call be cancelled without one.

import Foundation

/// Where a call's bearer comes from.
public protocol TokenSource: Sendable {
    /// The bearer to send, or `""` to send no credential at all.
    func token() async throws -> String

    /// Fetches a new token after the server reported the current one expired.
    ///
    /// Returning without minting anything — which is what ``APIKeyTokenSource``
    /// does — makes the runtime's refresh a no-op and the retry is skipped (R1).
    func refresh() async throws
}

// MARK: - API key

/// A Loams API key: what a script or a CI job has.
///
/// The key does not expire, so there is nothing to refresh and
/// ``TokenSource/refresh()`` is a no-op — R1's "a source that cannot refresh makes
/// the refresh a no-op", stated as code.
public struct APIKeyTokenSource: TokenSource, Sendable {
    private let key: String

    public init(_ key: String) {
        self.key = key
    }

    public func token() async throws -> String { key }

    public func refresh() async throws {}
}

// MARK: - Static token

/// A bearer that is already valid, for a caller that manages its own.
public struct StaticTokenSource: TokenSource, Sendable {
    private let token: String

    public init(_ token: String) {
        self.token = token
    }

    public func token() async throws -> String { token }

    /// A no-op: the caller owns this token's lifetime, and the SDK renewing it
    /// behind their back would be a surprise.
    public func refresh() async throws {}
}

// MARK: - Environment

/// Reads the environment. The variables are named `LOAMS_API_KEY` and
/// `LOAMS_TOKEN`, after design §44 §7.4.
public struct EnvTokenSource: TokenSource, Sendable {
    /// The variables read, in order.
    public static let names = ["LOAMS_API_KEY", "LOAMS_TOKEN"]

    private let lookup: @Sendable (String) -> String?

    /// `EnvTokenSource()` reads the process environment.
    ///
    /// `lookup` is a parameter so a test can supply an environment without
    /// touching the process's own, and so a caller who keeps its configuration
    /// somewhere other than the environment can say so.
    public init(lookup: @escaping @Sendable (String) -> String? = { ProcessInfo.processInfo.environment[$0] }) {
        self.lookup = lookup
    }

    /// The first of ``names`` that is set.
    ///
    /// The environment is read on **every** call rather than once at construction,
    /// so a process that receives its credentials after the client is built — a
    /// sidecar, a test — still authenticates.
    public func token() async throws -> String {
        for name in Self.names {
            if let value = lookup(name), !value.isEmpty { return value }
        }
        return ""
    }

    /// A no-op: an environment variable is not something the SDK can renew. A
    /// caller who wants a refreshed token points a ``RefreshingTokenSource`` at
    /// their own fetch.
    public func refresh() async throws {}
}

// MARK: - Refreshing

/// Caches a token and calls a fetch function when it is asked to refresh.
///
/// This is the shape every refreshing source has.
///
/// One in-flight refresh is shared by concurrent callers, so a burst of `401`s
/// produces **one** token exchange rather than one per request. That is not a
/// micro-optimisation: an instance rejecting every token because it is stale
/// would otherwise be hit with one exchange per in-flight call, which is how a
/// credential rotation turns into a self-inflicted denial of service.
///
/// # Why an actor
///
/// The sharing above needs mutual exclusion, and Swift's answer is an actor
/// rather than a lock: the cache and the in-flight continuation are both mutable
/// state touched by every attempt of every concurrent call, and an actor makes
/// that isolation checked instead of reviewed.
public actor RefreshingTokenSource: TokenSource {
    /// Mints a new token. It is called with the caller's task.
    public typealias Fetch = @Sendable () async throws -> String

    private let fetch: Fetch
    private var cached: String?
    private var inFlight: Task<String, any Error>?

    /// A source that caches a token and calls `fetch` when asked to refresh.
    public init(fetch: @escaping Fetch) {
        self.fetch = fetch
    }

    /// The cached token, fetching one first if the cache is empty.
    ///
    /// A source whose cache starts empty would send no credential at all, and an
    /// instance that requires one answers `unauthenticated` — which the call path
    /// treats as "the token expired" and retries, with still no credential. So the
    /// first `token()` fetches.
    public func token() async throws -> String {
        if let cached, !cached.isEmpty { return cached }
        try await refresh()
        return cached ?? ""
    }

    /// Mints a new token, sharing one in-flight fetch across callers.
    ///
    /// The fetch runs in an unstructured `Task` rather than in the calling task,
    /// because a `Task` inherits its creator's cancellation: if the first caller's
    /// task is cancelled, a fetch awaited directly would be torn down under every
    /// other caller waiting on it. Unstructured, one caller's cancellation does
    /// not take the shared fetch with it.
    public func refresh() async throws {
        if let existing = inFlight {
            cached = try await existing.value
            return
        }
        let task = Task<String, any Error> { try await fetch() }
        inFlight = task
        defer { inFlight = nil }
        cached = try await task.value
    }

    /// Forgets the cached token, so the next ``token()`` fetches.
    public func invalidate() {
        cached = nil
    }
}

// MARK: - OIDC token exchange

/// The RFC 8693 token exchange a person signed in through Authentik needs
/// (design §44 §7.4, D608; §19 §5.2).
///
/// The instance's `/oauth/token` endpoint takes the identity token and answers
/// with a Loams access token, which is then cached until the server says it
/// expired.
///
/// **Not exercised by the conformance suite.** The instance serves no OAuth
/// endpoint yet (the auth plan, MT, and API1 Task 7 build it), so this is written
/// to the documented request and response and cannot be run against a live
/// server. ``ConformanceTests/testTokenSourceRefresh`` covers the caching and the
/// refresh-once-and-retry behaviour that ``RefreshingTokenSource`` implements,
/// which is the part the SDK owns.
///
/// The `post` closure is the seam: it is injected rather than reaching for
/// `URLSession` directly, so a test can drive the exchange without a server and a
/// host with its own networking stack can supply its own.
public struct OIDCExchange: TokenSource, Sendable {
    /// Posts a form body and returns the response body.
    public typealias Post = @Sendable (_ endpoint: URL, _ body: Data) async throws -> Data

    /// The instance's `/oauth/token` endpoint.
    public let endpoint: URL
    /// The public OAuth client id; the gateway exchanges the token, so the client
    /// secret is never involved (D447/D449).
    public let clientID: String
    /// Mints the current identity token, from the host's OIDC session.
    public let subjectToken: @Sendable () async throws -> String
    /// Posts the request.
    public let post: Post

    private let inner: RefreshingTokenSource

    /// Builds the exchange and its caching source.
    public init(
        endpoint: URL,
        clientID: String,
        subjectToken: @escaping @Sendable () async throws -> String,
        post: @escaping Post
    ) {
        self.endpoint = endpoint
        self.clientID = clientID
        self.subjectToken = subjectToken
        self.post = post
        let inner = RefreshingTokenSource { [subjectToken, post, endpoint, clientID] in
            try await loamsOIDCRFC8693Exchange(
                endpoint: endpoint,
                clientID: clientID,
                subjectToken: subjectToken,
                post: post
            )
        }
        self.inner = inner
    }

    /// The cached Loams access token, exchanging one first if the cache is empty.
    public func token() async throws -> String { try await inner.token() }

    /// Exchanges a new access token.
    public func refresh() async throws { try await inner.refresh() }
}

/// Posts one RFC 8693 token exchange and returns the `access_token`.
///
/// Split out of ``OIDCExchange`` so the closure ``RefreshingTokenSource`` captures
/// does not form a retain cycle through `self`, which a method on the struct
/// would.
private func loamsOIDCRFC8693Exchange(
    endpoint: URL,
    clientID: String,
    subjectToken: @escaping @Sendable () async throws -> String,
    post: @Sendable (URL, Data) async throws -> Data
) async throws -> String {
    let subject = try await subjectToken()
    // `URLComponents.formURLEncoded()` rather than hand-built percent encoding:
    // an identity token is base64url and contains `-` and `_`, and hand-rolled
    // encoding is how a token with a `+` in it gets silently corrupted.
    var form: [(String, String)] = [
        ("grant_type", "urn:ietf:params:oauth:grant-type:token-exchange"),
        ("subject_token_type", "urn:ietf:params:oauth:token-type:id_token"),
        ("requested_token_type", "urn:ietf:params:oauth:token-type:access_token"),
        ("subject_token", subject),
        ("client_id", clientID),
        ("audience", endpoint.absoluteString),
    ]
    form.sort { $0.0 < $1.0 }
    let body = form
        .map { "\(loamsFormEscape($0.0))=\(loamsFormEscape($0.1))" }
        .joined(separator: "&")

    let payload = try await post(endpoint, Data(body.utf8))
    guard let parsed = JSONValue.decodedObject(from: payload),
          let accessToken = parsed["access_token"]?.stringValue,
          !accessToken.isEmpty
    else {
        throw LoamsError.internalError(
            "", "loams: the token exchange answered no access_token"
        )
    }
    return accessToken
}

/// `application/x-www-form-urlencoded` escaping.
///
/// `URLComponents` percent-encodes a query but not a form body, and the token
/// endpoint expects the latter, so the escaping is spelled out: space becomes
/// `+`, and everything outside the unreserved set is percent-encoded uppercase.
private func loamsFormEscape(_ value: String) -> String {
    var out = ""
    for byte in Array(value.utf8) {
        switch byte {
        case UInt8(ascii: "A")...UInt8(ascii: "Z"),
             UInt8(ascii: "a")...UInt8(ascii: "z"),
             UInt8(ascii: "0")...UInt8(ascii: "9"),
             UInt8(ascii: "-"), UInt8(ascii: "_"), UInt8(ascii: "."), UInt8(ascii: "~"):
            out.append(Character(UnicodeScalar(byte)))
        case UInt8(ascii: " "):
            out.append("+")
        default:
            out.append(String(format: "%%%02X", byte))
        }
    }
    return out
}