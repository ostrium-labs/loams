import Foundation

/// One message of a `Watch*` stream (AP0 Ruling 3), independent of the generated types.
public enum WatchEvent<Item: Sendable>: Sendable {
    case snapshot([Item], cursor: String, reset: Bool)
    case upsert(Item, cursor: String)
    case remove(id: String, cursor: String)
    case heartbeat(cursor: String)

    public var cursor: String {
        switch self {
        case .snapshot(_, let c, _), .upsert(_, let c), .remove(_, let c), .heartbeat(let c): return c
        }
    }
}

/// Applies a watch stream to a list kept in arrival order and remembers the resume cursor.
public struct WatchState<Item: Sendable>: Sendable {
    public private(set) var items: [Item] = []
    public private(set) var cursor: String?
    public private(set) var lastMessageAt: Date?
    private let idOf: @Sendable (Item) -> String

    public static var heartbeat: TimeInterval { 15 }
    public static var deadAfter: TimeInterval { 45 }

    public init(id: @escaping @Sendable (Item) -> String) {
        idOf = id
    }

    public mutating func apply(_ event: WatchEvent<Item>, at now: Date) {
        // A local copy: closures over self.idOf while mutating self.items would overlap.
        let idOf = self.idOf
        lastMessageAt = now
        cursor = event.cursor
        switch event {
        case .snapshot(let all, _, _):
            items = all
        case .upsert(let item, _):
            let id = idOf(item)
            if let at = items.firstIndex(where: { idOf($0) == id }) { items[at] = item } else { items.append(item) }
        case .remove(let id, _):
            items.removeAll { idOf($0) == id }
        case .heartbeat:
            break
        }
    }

    /// Three missed 15 s heartbeats.
    public func isDead(at now: Date) -> Bool {
        guard let last = lastMessageAt else { return false }
        return now.timeIntervalSince(last) > Self.deadAfter
    }

    public mutating func clear() {
        items = []
        cursor = nil
        lastMessageAt = nil
    }
}

/// 500 ms doubling to 10 s, with jitter in [delay/2, delay] (the same numbers as Android).
public struct Backoff: Sendable {
    public var initial: TimeInterval
    public var maximum: TimeInterval
    private var attempt = 0

    public init(initial: TimeInterval = 0.5, maximum: TimeInterval = 10) {
        self.initial = initial
        self.maximum = maximum
    }

    public mutating func next<G: RandomNumberGenerator>(using rng: inout G) -> TimeInterval {
        let base = min(maximum, initial * pow(2, Double(min(attempt, 10))))
        attempt += 1
        return base / 2 + Double.random(in: 0...(base / 2), using: &rng)
    }

    public mutating func next() -> TimeInterval {
        var rng = SystemRandomNumberGenerator()
        return next(using: &rng)
    }

    public mutating func reset() {
        attempt = 0
    }
}

/// What a screen shows about its live stream.
public enum StreamStatus: Sendable, Equatable {
    case connecting
    case live
    case reconnecting(retryIn: TimeInterval, cause: String?)
    case stopped(cause: String?)
}
