// `streams.subscribe` (runtime contract R14, SDKP §2.5, SDKP-2).
//
// A client loop over unary `Fetch`, so it works on every transport: fetch from
// the next offset, yield what came back, and move the offset to the last
// record's offset + 1. An empty fetch backs off 100 ms, doubling to 1 s, and
// the first non-empty one resets it. `offset` is always the next offset to
// fetch, so a caller that stops can resume from it. Cancelling the iteration
// stops the loop.
//
// `client.streams.subscribe` wires this to `loams.stream.v1.StreamService/Fetch`
// when that package is on the main port (SDKP P0c); until then the loop is
// public so a caller can drive it over any fetch.

import Foundation

/// One record a subscription yields.
public struct StreamRecord: Sendable, Equatable {
    public let partition: Int
    public let offset: UInt64
    public let value: Data
    public let timestampMs: Int64
    /// The record's key, when it was produced with one.
    public let key: Data?

    public init(partition: Int, offset: UInt64, value: Data, timestampMs: Int64 = 0, key: Data? = nil) {
        self.partition = partition
        self.offset = offset
        self.value = value
        self.timestampMs = timestampMs
        self.key = key
    }

    /// The value as UTF-8 text.
    public var text: String { String(decoding: value, as: UTF8.self) }
}

/// A resumable subscription to one partition.
public final class Subscription: AsyncSequence, @unchecked Sendable {
    public typealias Element = StreamRecord

    public static let initialBackoffMs = 100
    public static let maxBackoffMs = 1000

    private let fetch: @Sendable (UInt64) async throws -> [StreamRecord]
    private let sleep: @Sendable (Int) async throws -> Void
    private let lock = NSLock()
    private var next: UInt64

    /// A subscription from `fromOffset`. `sleep` waits between empty fetches
    /// (milliseconds); `Task.sleep` unless a test replaces it.
    public init(
        from fromOffset: UInt64,
        sleep: @escaping @Sendable (Int) async throws -> Void = { try await loamsSleep(milliseconds: $0) },
        fetch: @escaping @Sendable (UInt64) async throws -> [StreamRecord]
    ) {
        self.next = fromOffset
        self.fetch = fetch
        self.sleep = sleep
    }

    /// The next offset to fetch: the last yielded record's offset + 1. Resume from it.
    public var offset: UInt64 { lock.withLock { next } }

    public struct AsyncIterator: AsyncIteratorProtocol {
        let owner: Subscription
        var page: [StreamRecord] = []
        var backoff = Subscription.initialBackoffMs

        public mutating func next() async throws -> StreamRecord? {
            while !Task.isCancelled {
                while !page.isEmpty {
                    let record = page.removeFirst()
                    if record.offset < owner.offset { continue }
                    owner.lock.withLock { owner.next = record.offset + 1 }
                    return record
                }
                let from = owner.offset
                let records = try await owner.fetch(from)
                // A page with nothing at or past the offset is an empty one: back off rather
                // than spin on records already yielded.
                if !records.contains(where: { $0.offset >= from }) {
                    do {
                        try await owner.sleep(backoff)
                    } catch is CancellationError {
                        return nil
                    }
                    backoff = Swift.min(backoff * 2, Subscription.maxBackoffMs)
                    continue
                }
                backoff = Subscription.initialBackoffMs
                page = records
            }
            return nil
        }
    }

    public func makeAsyncIterator() -> AsyncIterator { AsyncIterator(owner: self) }
}
