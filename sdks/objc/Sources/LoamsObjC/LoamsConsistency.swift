import Foundation

/// A consistency token (runtime contract R4): `v1:s<stream>/p<partition>@<offset>,…`,
/// the encoding `crates/loams-collection/src/token.rs` defines.
///
/// A write answers with one and a read accepts one as `atLeast`, so a caller that
/// just wrote can read its own write. Two tokens merge by keeping the largest
/// offset per `(stream, partition)`.
#if canImport(ObjectiveC)
@objcMembers
#endif
public final class LoamsConsistencyToken: NSObject, @unchecked Sendable {
    struct Item: Equatable {
        let stream: UInt64
        let partition: UInt32
        let offset: UInt64
    }

    let items: [Item]

    init(items: [Item]) {
        self.items = items
        super.init()
    }

    /// Parses `text`, keeping its order. Throws a `LoamsError` with code
    /// `invalid_argument` (client-side, no network call) when it is not a `v1:` token.
    public static func parse(_ text: String) throws -> LoamsConsistencyToken {
        guard text.hasPrefix("v1:") else { throw invalid(text) }
        let body = String(text.dropFirst(3))
        if body.isEmpty { return LoamsConsistencyToken(items: []) }
        var items: [Item] = []
        for part in body.split(separator: ",", omittingEmptySubsequences: false) {
            guard let item = parseItem(String(part)) else { throw invalid(text) }
            items.append(item)
        }
        return LoamsConsistencyToken(items: items)
    }

    /// Whether `text` parses.
    public static func isValid(_ text: String) -> Bool {
        return (try? parse(text)) != nil
    }

    /// The token's text.
    public var stringValue: String {
        return "v1:" + items.map { "s\($0.stream)/p\($0.partition)@\($0.offset)" }.joined(separator: ",")
    }

    public override var description: String { return stringValue }

    public var isEmpty: Bool { return items.isEmpty }

    /// The items as `[stream, partition, offset]`, decimal strings, in the token's order.
    public var itemStrings: [[String]] {
        return items.map { ["\($0.stream)", "\($0.partition)", "\($0.offset)"] }
    }

    /// Sorted by stream then partition, one item per pair (the largest offset).
    public func normalized() -> LoamsConsistencyToken {
        let sorted = items.sorted {
            ($0.stream, $0.partition, $0.offset) < ($1.stream, $1.partition, $1.offset)
        }
        var out: [Item] = []
        for item in sorted {
            if let last = out.last, last.stream == item.stream, last.partition == item.partition {
                out[out.count - 1] = item
            } else {
                out.append(item)
            }
        }
        return LoamsConsistencyToken(items: out)
    }

    /// A new token holding the larger offset of the two for every `(stream, partition)`.
    public func merge(_ other: LoamsConsistencyToken) -> LoamsConsistencyToken {
        return LoamsConsistencyToken(items: items + other.items).normalized()
    }

    /// The offset this token holds for one partition, or nil.
    public func offset(stream: UInt64, partition: UInt32) -> NSNumber? {
        let found = items.filter { $0.stream == stream && $0.partition == partition }.map { $0.offset }.max()
        return found.map { NSNumber(value: $0) }
    }

    public override func isEqual(_ object: Any?) -> Bool {
        guard let other = object as? LoamsConsistencyToken else { return false }
        return other.items == items
    }

    public override var hash: Int { return stringValue.hashValue }

    private static func parseItem(_ item: String) -> Item? {
        guard item.hasPrefix("s"),
              let p = item.range(of: "/p"),
              let at = item.range(of: "@", range: p.upperBound..<item.endIndex)
        else { return nil }
        let stream = canonical(String(item[item.index(after: item.startIndex)..<p.lowerBound]), max: UInt64.max)
        let partition = canonical(String(item[p.upperBound..<at.lowerBound]), max: UInt64(UInt32.max))
        let offset = canonical(String(item[at.upperBound...]), max: UInt64.max)
        guard let s = stream, let pt = partition, let o = offset else { return nil }
        return Item(stream: s, partition: UInt32(pt), offset: o)
    }

    /// A canonical decimal: digits only, no leading zero except `0`, at most `max`.
    private static func canonical(_ text: String, max: UInt64) -> UInt64? {
        guard !text.isEmpty, text.allSatisfy({ $0 >= "0" && $0 <= "9" }) else { return nil }
        if text.count > 1 && text.hasPrefix("0") { return nil }
        guard let value = UInt64(text), value <= max else { return nil }
        return value
    }

    private static func invalid(_ text: String) -> LoamsError {
        return LoamsError(message: "invalid consistency token \"\(text)\"",
                          codeString: "invalid_argument",
                          reason: "invalid_consistency_token",
                          unknownReason: nil,
                          httpStatus: 400)
    }
}

/// The session store (R4, `sessionConsistency`): every token an answer carries is
/// merged in, and every read that leaves `consistency` unset is sent with
/// `atLeast` set to the session's token. One token per proto package: a graph's
/// tokens name the graph's own stream and a graph read refuses another stream's.
#if canImport(ObjectiveC)
@objcMembers
#endif
public final class LoamsSessionStore: NSObject, @unchecked Sendable {
    /// The response fields a token is read from, in proto3 JSON.
    public static let tokenFields = ["token", "readToken", "consistencyToken"]

    private var tokens: [String: LoamsConsistencyToken] = [:]
    private let lock = NSLock()

    public func token(scope: String = "loams.collection.v1") -> LoamsConsistencyToken? {
        lock.lock()
        defer { lock.unlock() }
        return tokens[scope]
    }

    /// Merges `text` in; a token that does not parse is ignored.
    public func observe(scope: String, text: String?) {
        guard let text = text, !text.isEmpty, let parsed = try? LoamsConsistencyToken.parse(text) else { return }
        lock.lock()
        defer { lock.unlock() }
        tokens[scope] = tokens[scope].map { $0.merge(parsed) } ?? parsed.normalized()
    }

    public func observe(scope: String, answer: [String: Any]) {
        for field in LoamsSessionStore.tokenFields {
            observe(scope: scope, text: answer[field] as? String)
        }
    }

    /// Sets `consistency.atLeast` when the request leaves `consistency` unset.
    public func apply(scope: String, request: [String: Any]) -> [String: Any] {
        if request["consistency"] != nil { return request }
        guard let current = token(scope: scope), !current.isEmpty else { return request }
        var out = request
        out["consistency"] = ["atLeast": current.stringValue]
        return out
    }
}
