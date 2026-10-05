// Server streams (design §44 §7.4, D610; runtime contract R7).
//
// The API has server streams only (D420): no client streaming, no bidi, because
// a browser cannot stream full duplex over `fetch` and half-duplex works through
// every proxy. A server stream here is an `AsyncSequence`, which is design §44
// §7.1's wording for the Swift SDK:
//
//     for try await transition in client.live.watch(request) {
//         apply(transition)
//     }
//
// # Why a stream is not just a retry
//
// A stream is the one call where "send it again" is not enough. The server hands
// out cursors, and a reconnect has to resume from the last one the client
// *applied*, or the client silently misses everything that changed in between —
// which is worse than an error, because a sync UI that is quietly stale looks
// exactly like a sync UI that works.
//
// So a stream with a resume tracks the cursor of every message, re-opens from it
// on a retryable failure, and does not re-yield what it already yielded. The
// cursor reader is **required**, with no default: a default would have to guess a
// field name, and the API's one server stream carries no `cursor` field at all —
// its cursor is a `StateVersion`. Guessing would be a method that silently
// resumes from nothing, which is the failure R7 is about.
//
// # Why the messages are `AsyncSequence` and not a callback
//
// An `AsyncSequence` composes with `for try await`, with `TaskGroup`, with
// cancellation, and with structured concurrency's promise that leaving the loop
// cancels the work. A callback stream has to re-implement all four, and getting
// the last one wrong leaks a request.

import Foundation

/// A cursor a server stream resumes from.
public struct StreamCursor: Sendable, Equatable {
    /// The cursor's value, opaque to the SDK.
    ///
    /// It is a `String` rather than bytes because the wire carries it as one and
    /// because the API's own cursor is a `StateVersion`, not an opaque blob. The
    /// SDK must not interpret it — see `Consistency.swift` for the same reasoning
    /// about consistency tokens.
    public let value: String

    public init(_ value: String) {
        self.value = value
    }

    public static let none = StreamCursor("")
}

/// How a server stream re-opens from a cursor (R7).
///
/// Generic over the request and response so the compiler checks that `resume`
/// returns the message the RPC takes. The invoker already knows both types, so
/// nothing here is type-erased.
public struct StreamResume<Request: LoamsMessage, Response: LoamsMessage>: Sendable {
    /// The request to re-open with, given the last cursor applied and the
    /// original request.
    ///
    /// Returning the original request re-opens from the beginning, which is
    /// correct — and loses nothing but time — for a stream whose snapshot is
    /// complete.
    public var resume: @Sendable (StreamCursor, Request) -> Request
    /// Reads the cursor off a message. **Required**, and there is no default: a
    /// stream's cursor is that stream's business.
    public var cursorOf: @Sendable (Response) -> StreamCursor
    /// The re-opens allowed in one **run** of disconnects.
    ///
    /// A negative value leaves the client's default in place, matching the
    /// client's own `maxRetries` convention.
    public var maxRetries: Int?
    /// Called after each message, with the cursor it carried.
    ///
    /// This is the hook an application persists a cursor in, and it is separate
    /// from ``cursorOf`` because "what does this message's cursor look like" and
    /// "what do I do with it" are different questions with different owners.
    public var onCursor: (@Sendable (StreamCursor, Response) async -> Void)?

    public init(
        resume: @escaping @Sendable (StreamCursor, Request) -> Request,
        cursorOf: @escaping @Sendable (Response) -> StreamCursor,
        maxRetries: Int? = nil,
        onCursor: (@Sendable (StreamCursor, Response) async -> Void)? = nil
    ) {
        self.resume = resume
        self.cursorOf = cursorOf
        self.maxRetries = maxRetries
        self.onCursor = onCursor
    }
}

/// A server stream that re-opens from the last cursor it applied.
///
/// It is a struct holding an `AsyncThrowingStream` rather than being one, so the
/// resume policy travels with the stream instead of being a closure the caller has
/// to close over — which is what keeps "did the caller configure a resume?" a
/// question this type answers rather than one it has to be told.
public struct ResumableServerStream<Request: LoamsMessage, Response: LoamsMessage>: Sendable {
    let source: WireStream
    let binding: CallBinding
    let original: Request
    let invoker: CallInvoker

    /// The re-opens allowed in one run of disconnects. A negative value means the
    /// client's default, which is why it is resolved here rather than in the
    /// initialiser.
    let maxRetries: Int

    /// The cursor state, shared by the iterator and by ``appliedCursor()``.
    ///
    /// An actor rather than captured `var`s because the sequence's `next()` and
    /// the resume loop are different call frames, and Swift 6 will not let both
    /// mutate one local without a synchronisation boundary. It has to be stored
    /// rather than created inside ``messages()`` so that a caller who holds the
    /// stream can ask for the applied cursor **after** iterating — which is the
    /// thing an application persists.
    ///
    /// `fileprivate`, not `private`: `CursorState` is a `private` type, and Swift
    /// requires a stored property's access to be no wider than its type's.
    fileprivate let state = CursorState()

    /// The resume policy, or `nil` when the caller passed none.
    ///
    /// A `nil` policy is what turns a broken stream into a **reported** error
    /// instead of a spin, and R7 asks for exactly that on a failure the retry
    /// class does not cover — notably an `unimplemented` stream, which is what
    /// every `loams.live.v1` RPC answers in the standard variant.
    var policy: StreamResume<Request, Response>?

    /// The last cursor applied, or ``StreamCursor/none``.
    public private(set) var cursor: StreamCursor = .none

    /// Opens a stream.
    ///
    /// The `resume` parameter is what turns on the re-open behaviour; a stream
    /// opened without one still yields every message and still reports a failure,
    /// it just does not reconnect.
    public init(
        source: WireStream,
        binding: CallBinding,
        original: Request,
        invoker: CallInvoker,
        maxRetries: Int,
        resume: StreamResume<Request, Response>? = nil
    ) {
        self.source = source
        self.binding = binding
        self.original = original
        self.invoker = invoker
        self.maxRetries = maxRetries
        self.policy = resume
    }

    /// Iterates the stream's messages, re-opening from the cursor on a retryable
    /// failure.
    ///
    /// The `for try await` is the whole interface: a failure throws out of the
    /// loop, and a clean end returns. There is no separate `Err()` to forget,
    /// which is the failure mode the Go SDK's `Stream.Receive()` / `Err()` split
    /// has to guard against in prose.
    public func messages() -> AsyncThrowingStream<Response, any Error> {
        // Bound to locals **before** the closure, so the escaping `Task` captures
        // the four values it needs rather than `self`. Capturing `self` would make
        // the stream retain itself for as long as its task lives, which is a leak
        // that only shows up as a long-lived stream that will not deallocate.
        let source = self.source
        let binding = self.binding
        let original = self.original
        let policy = self.policy
        let state = self.state
        let maxRetries = self.maxRetries
        let invoker = self.invoker

        return AsyncThrowingStream { continuation in
            let task = Task {
                var current = source
                var attempt = 0
                let budget = maxRetries

                while true {
                    do {
                        for try await message in current {
                            let response: Response
                            if let fields = message.fields,
                               let decoded = Response(loamsFields: fields) {
                                response = decoded
                            } else {
                                // A frame that does not decode is a broken
                                // agreement between the SDK and the server, which
                                // is `internal` (D611) and is reported rather than
                                // skipped: skipping would silently truncate the
                                // stream, and a truncated stream is the failure R7
                                // exists to prevent.
                                throw LoamsError.internalError(
                                    binding.rpc,
                                    "loams: could not decode a \(Response.protoTypeName) from a stream frame"
                                )
                            }

                            if let policy {
                                let next = policy.cursorOf(response)
                                if !next.value.isEmpty {
                                    await state.set(next)
                                    await policy.onCursor?(next, response)
                                }
                            }
                            continuation.yield(response)
                        }
                        // A clean end: the server finished. Not an error, and not
                        // something to resume from.
                        continuation.finish()
                        return
                    } catch {
                        let mapped = LoamsError.map(error, rpc: binding.rpc)

                        // No policy, or the class does not cover it, or the budget
                        // is spent: report. This is the `unimplemented` stream
                        // case — `loams.live.v1` is not served in the standard
                        // variant, and spinning on that would be a busy loop
                        // against a server that will never answer differently.
                        guard let policy,
                              attempt < budget,
                              loamsShouldRetry(
                                error: mapped,
                                retrySafe: true,
                                attempt: attempt,
                                maxRetries: budget
                              )
                        else {
                            continuation.finish(throwing: mapped)
                            return
                        }

                        let resumeRequest = policy.resume(await state.get(), original)
                        try await loamsSleep(milliseconds: loamsBackoff(attempt: attempt))
                        attempt += 1
                        do {
                            current = try await invoker.resumeSource(
                                binding: binding,
                                request: resumeRequest
                            )
                        } catch {
                            continuation.finish(throwing: LoamsError.map(error, rpc: binding.rpc))
                            return
                        }
                    }
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    /// The cursor the stream has applied so far.
    public func appliedCursor() async -> StreamCursor { await state.get() }
}

/// The cursor, in the one box the stream's escapes and its loop share.
private actor CursorState {
    private var value = StreamCursor.none
    func set(_ next: StreamCursor) { value = next }
    func get() -> StreamCursor { value }
}

extension CallInvoker {
    /// Re-opens a server stream from a cursor.
    ///
    /// Split out of ``CallInvoker/serverStream(binding:request:suppliedIdempotencyKey:)``
    /// so a resume does not re-apply the caller's idempotency key: the re-open is
    /// the **same logical call**, and R3 says one logical call gets one key. A
    /// resume that minted a second key would be the same duplicate-write bug in a
    /// different place, which is why this method takes the request as the caller
    /// already keyed it.
    ///
    /// Generic over `Request` only. The re-open returns a raw
    /// ``WireStream``, so a `Response` parameter would be unused — and an
    /// unused one cannot be inferred at the call site, which is what made
    /// ``messages()`` fail to compile.
    func resumeSource<Request: LoamsMessage>(
        binding: CallBinding,
        request: Request
    ) async throws -> WireStream {
        let bearer = try await currentBearer()
        return try await httpTransport.serverStream(
            WireRequest(rpc: binding.rpc, body: request.loamsFields(), serverStreaming: true),
            bearer: bearer
        )
    }
}