// A transport that answers from a script, for the tests that do not need a server.
//
// # Why this exists
//
// R2's retry loop, R3's key reuse and R7's resume are all about what the SDK does
// **between** attempts. A recorded fixture corpus pins what the SDK does with a
// *successful* response and with a *refusal*; it cannot pin "the first attempt
// failed with `unavailable` and the second carried the same idempotency key",
// because the corpus has no such case — the recorded server never failed that way
// on purpose.
//
// So those clauses are pinned against a transport that answers from a script, the
// same shape every other SDK's suite uses for this (``stream_resume_test.go``
// drives a Go stub, ``stream_resume_with_cursor`` drives a TypeScript one). The
// stub records what it was asked to send, which is what makes R3 checkable at all:
// the assertion is over the **bodies the transport saw**, not over the SDK's
// internals.

import Foundation
@testable import Loams

/// One scripted answer.
enum StubReply: Sendable {
    /// The call succeeded with this body.
    case success([String: JSONValue])
    /// The call failed with this code and `ErrorInfo`.
    case failure(code: Code, message: String, info: ErrorInfoShape?)
}

/// One thing the transport was asked to do.
struct StubCall: Sendable {
    /// The RPC's path, `package.Service/Method`.
    let rpc: String
    /// The request body the SDK actually sent.
    let body: [String: JSONValue]
    /// The bearer that was attached, or `nil`.
    let bearer: String?
}

/// A transport that walks a script and records every call.
final class StubTransport: HTTPTransport, @unchecked Sendable {
    let config: TransportConfig

    /// The answers, consumed in order; the last one repeats once exhausted.
    private let replies: [StubReply]
    private let lock = NSLock()
    private var recorded: [StubCall] = []
    private var index = 0

    /// The stream frames this transport serves, for R7's tests.
    ///
    /// A separate list rather than a `StubReply` case because a stream's failure
    /// arrives **after** some messages have been yielded, which a unary reply
    /// cannot express — and that ordering is the whole of R7.
    private let streamPlan: [[StubReply]]

    init(replies: [StubReply], streamPlan: [[StubReply]] = []) {
        self.replies = replies
        self.streamPlan = streamPlan
        self.config = TransportConfig(endpoint: URL(string: "http://stub.invalid")!)
    }

    /// Every call the transport saw, in order.
    var calls: [StubCall] {
        lock.lock()
        defer { lock.unlock() }
        return recorded
    }

    /// The idempotency keys the transport saw, one per call that carried one.
    ///
    /// R3's assertion is over this: the keys must be **one distinct value** across
    /// a retried call, not one per attempt.
    var idempotencyKeys: [String] {
        calls.compactMap { $0.body[ProtoField.idempotencyKey]?.stringValue }
    }

    func unary(_ request: WireRequest, bearer: String?) async throws -> WireResponse {
        let reply = next(StubCall(rpc: request.rpc, body: request.body, bearer: bearer))
        switch reply {
        case .success(let body):
            return .success(body: body, trailers: [:])
        case .failure(let code, let message, let info):
            return .failure(code: code, message: message, info: info, trailers: [:])
        }
    }

    func serverStream(_ request: WireRequest, bearer: String?) async throws -> WireStream {
        let call = StubCall(rpc: request.rpc, body: request.body, bearer: bearer)
        let reply = next(call)
        // Which leg of `streamPlan` this is: the count of stream opens so far.
        //
        // Read and bumped inside `withLock`, not with a bare `lock()`/`unlock()`
        // pair: this is an `async` function, and holding an `NSLock` across a
        // suspension point is a warning Swift is right to refuse — the lock
        // could still be held when the continuation resumes on another thread.
        // Nothing awaits inside the closure, so `withLock` returns immediately.
        let leg = lock.withLock { () -> Int in
            defer { streamOpens += 1 }
            return streamOpens
        }

        // A stream that was scripted to fail *after* messages uses `streamPlan`;
        // one scripted as a single reply fails immediately, which is the
        // refusal-at-open shape the corpus contains.
        let plan = leg < streamPlan.count ? streamPlan[leg] : [reply]
        return AsyncThrowingStream { continuation in
            for step in plan {
                switch step {
                case .success(let body):
                    continuation.yield(WireMessage(fields: body))
                case .failure(let code, let message, let info):
                    continuation.finish(
                        throwing: LoamsError.fromWire(code: code, info: info, rpc: request.rpc, message: message)
                    )
                    return
                }
            }
            continuation.finish()
        }
    }

    private var streamOpens = 0

    private func next(_ call: StubCall) -> StubReply {
        lock.lock()
        defer { lock.unlock() }
        recorded.append(call)
        guard !replies.isEmpty else { return .success([:]) }
        let reply = replies[min(index, replies.count - 1)]
        index += 1
        return reply
    }
}