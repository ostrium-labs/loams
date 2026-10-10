import Foundation

/// Keeps one `Watch*` stream open while the calling task runs: opens it from the last cursor,
/// applies each message, treats 45 s of silence as a dead stream, and reconnects with [Backoff].
/// Same behaviour as Android's `ResumingWatch` (AP0 Ruling 3, AP3 Task 2).
public actor ResumingWatch<Item: Sendable> {
    public typealias Open = @Sendable (_ cursor: String?) -> AsyncThrowingStream<WatchEvent<Item>, Error>
    public typealias Observer = @Sendable (_ state: WatchState<Item>, _ status: StreamStatus) async -> Void

    public private(set) var state: WatchState<Item>
    public private(set) var status: StreamStatus = .connecting
    private let open: Open
    private let observer: Observer
    private let isFatal: @Sendable (Error) -> Bool
    private let deadAfter: TimeInterval
    private var backoff: Backoff
    private var lastMessage = Date()

    public init(
        state: WatchState<Item>,
        backoff: Backoff = Backoff(),
        deadAfter: TimeInterval = WatchState<Item>.deadAfter,
        isFatal: @escaping @Sendable (Error) -> Bool = { _ in false },
        open: @escaping Open,
        observer: @escaping Observer
    ) {
        self.state = state
        self.backoff = backoff
        self.deadAfter = deadAfter
        self.isFatal = isFatal
        self.open = open
        self.observer = observer
    }

    /// Runs until the calling task is cancelled or a fatal error stops it.
    public func run() async {
        while !Task.isCancelled {
            await set(.connecting)
            do {
                let stalled = try await collectOnce()
                await retry(stalled ? "no message for \(Int(deadAfter)) s" : nil)
            } catch is CancellationError {
                return
            } catch {
                if isFatal(error) {
                    await set(.stopped(cause: String(describing: error)))
                    return
                }
                await retry(String(describing: error))
            }
        }
    }

    /// Consumes one stream until it ends; returns true when it was cut for being silent.
    private func collectOnce() async throws -> Bool {
        lastMessage = Date()
        let stream = open(state.cursor)
        let consumer = Task { try await self.consume(stream) }
        let watchdog = Task {
            while !Task.isCancelled {
                try await Task.sleep(nanoseconds: 1_000_000_000)
                if await self.silentTooLong() {
                    consumer.cancel()
                    return true
                }
            }
            return false
        }
        defer { watchdog.cancel() }
        do {
            try await withTaskCancellationHandler {
                try await consumer.value
            } onCancel: {
                consumer.cancel()
            }
        } catch is CancellationError {
            // The watchdog cancelled a silent stream; only our own cancellation ends the loop.
            if Task.isCancelled { throw CancellationError() }
        }
        if Task.isCancelled { throw CancellationError() }
        watchdog.cancel()
        return (try? await watchdog.value) ?? false
    }

    private func consume(_ stream: AsyncThrowingStream<WatchEvent<Item>, Error>) async throws {
        for try await event in stream {
            state.apply(event, at: Date())
            lastMessage = Date()
            if status != .live {
                backoff.reset()
                await set(.live)
            } else {
                await observer(state, status)
            }
        }
    }

    private func silentTooLong() -> Bool { Date().timeIntervalSince(lastMessage) > deadAfter }

    private func retry(_ cause: String?) async {
        let wait = backoff.next()
        await set(.reconnecting(retryIn: wait, cause: cause))
        try? await Task.sleep(nanoseconds: UInt64(wait * 1_000_000_000))
    }

    private func set(_ s: StreamStatus) async {
        status = s
        await observer(state, s)
    }
}
