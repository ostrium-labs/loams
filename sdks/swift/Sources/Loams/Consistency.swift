// Consistency tokens (design §44 §7.4, D609; runtime contract R4).
//
// A write answers with a `consistency_token`; a read accepts one, so a caller that
// just wrote can read its own write. Threading those by hand is the caller's job
// today. The session store is the alternative §44 §7.4 asks for: **off by
// default**, and when a call opts in, every response's token is folded into the
// session and attached to later reads.
//
// **The token's encoding is not in the protos yet.** §05 §5 defines the semantics
// (offsets per stream and partition, `STRONG` as the default, `EVENTUAL`,
// `AT_LEAST{token}`) and API1's write paths carry it as an opaque `v1:` string;
// §44 §7.4 says it merges by "max offset per stream and partition", which needs the
// encoding to be parsed. Until that lands this store keeps the token it was given,
// refuses to merge two different tokens into a wrong one, and says so — a
// silently-wrong consistency token reads stale data, which is worse than an error.

import Foundation

/// The prefix every consistency token carries (§44 §7.4).
public let loamsTokenPrefix = "v1:"

/// The request header a read's token travels in.
///
/// §44 §7.4 names the response header `loams-consistency-token`; the request side
/// is the `consistency` field, and until a proto carries it the header is how the
/// token gets there.
public let loamsConsistencyHeader = "Loams-Consistency-Token"

/// Whether a string looks like a consistency token.
public func loamsIsConsistencyToken(_ value: String) -> Bool {
    value.hasPrefix(loamsTokenPrefix) && value.count > loamsTokenPrefix.count
}

/// A session's consistency token store (D609).
///
/// It is a protocol so a caller who has real offset arithmetic to do can supply
/// their own, and so a session can be shared between two clients without either
/// owning it.
public protocol ConsistencyTokenStore: Sendable {
    /// The token to attach to the next read, or `nil` for none.
    func current() async -> String?

    /// Folds a token the server returned into the session's.
    func record(_ token: String) async throws

    /// How many unmergeable pairs the session has seen.
    ///
    /// Surfaced so the limitation in this file's header is **visible** rather than
    /// silent.
    func conflicts() async -> Int
}

/// The default store: it keeps the token it was given and counts the ones it could
/// not merge.
///
/// An `actor` because a session is shared by every concurrent call in a process
/// and R4's whole value is that those calls agree on one token.
public actor ConsistencySession: ConsistencyTokenStore {
    private var token: ConsistencyToken?
    private var conflictCount = 0

    public init() {}

    /// The merged token later reads go out with, or nil when none has been recorded.
    public func current() -> String? { token?.description }

    /// The merged token, parsed.
    public func parsed() -> ConsistencyToken? { token }

    /// Folds a response's token in (R4 as amended by SDKP §2.1): the max offset
    /// per stream and partition. Empty is ignored. A token that is not a `v1:`
    /// token is counted in `conflicts()` and refused, and the session keeps what
    /// it had.
    public func record(_ incoming: String) async throws {
        guard !incoming.isEmpty else { return }
        guard let parsed = ConsistencyToken.tryParse(incoming) else {
            conflictCount += 1
            throw LoamsError.internalError("", "loams: not a consistency token: \(incoming)")
        }
        token = token.map { $0.merge(parsed) } ?? parsed
    }

    public func conflicts() -> Int { conflictCount }

    public func clear() {
        token = nil
        conflictCount = 0
    }
}


/// Folds the token a response carried into a session.
///
/// A failure here is **counted, not thrown**: the RPC already succeeded, and a
/// caller that retried on that error would perform the write twice. The conflict
/// count is the only honest report available until the encoding can be merged.
public func loamsRecordConsistency(
    _ store: (any ConsistencyTokenStore)?,
    response: (some LoamsMessage)?
) async {
    guard let store, let response else { return }
    try? await store.record(response.consistencyToken ?? "")
}