// Consistency tokens (runtime contract R4 as amended by SDKP §2.1).
//
// The encoding is `crates/loams-collection/src/token.rs`'s:
//
//     v1:s<stream>/p<partition>@<offset>,…
//
// one entry per (stream, partition), sorted, with `merge` keeping the max offset
// per pair. `v1:` alone is the empty token. Every number is canonical decimal (no
// sign, no leading zero), a partition fits in 32 bits and a stream and offset in
// 64, and anything else is `invalid_argument` raised here, before the network.

import Foundation

/// A parsed consistency token: the log positions a write reached, which a read
/// can wait for (`Consistency.at_least`).
public struct ConsistencyToken: Sendable, Equatable, Hashable, CustomStringConvertible {
    /// One (stream, partition, offset) entry.
    public struct Position: Sendable, Equatable, Hashable, Comparable {
        public let stream: UInt64
        public let partition: UInt32
        public let offset: UInt64

        public init(stream: UInt64, partition: UInt32, offset: UInt64) {
            self.stream = stream
            self.partition = partition
            self.offset = offset
        }

        public static func < (a: Position, b: Position) -> Bool {
            (a.stream, a.partition, a.offset) < (b.stream, b.partition, b.offset)
        }
    }

    /// The entries, sorted by stream then partition, one per pair.
    public let positions: [Position]

    /// The empty token, `v1:`.
    public static let empty = ConsistencyToken(normalized: [])

    private init(normalized: [Position]) {
        positions = normalized
    }

    /// A token over these positions, normalised: sorted, the max offset per pair.
    public init(_ positions: [Position]) {
        var normal: [Position] = []
        for position in positions.sorted() {
            if let last = normal.last, last.stream == position.stream, last.partition == position.partition {
                normal[normal.count - 1] = position
            } else {
                normal.append(position)
            }
        }
        self.positions = normal
    }

    /// Parses a token, throwing `invalid_argument` when it is not a `v1:` token.
    public static func parse(_ text: String) throws -> ConsistencyToken {
        guard let token = tryParse(text) else {
            throw LoamsError.loams(
                code: .invalidArgument,
                reason: .invalidArgument,
                unknownReason: "invalid_consistency_token",
                metadata: ["token": text],
                hint: "",
                rpc: "",
                message: "invalid consistency token \"\(text)\": expected v1:s<stream>/p<partition>@<offset>,…"
            )
        }
        return token
    }

    /// Parses a token, or answers nil.
    public static func tryParse(_ text: String) -> ConsistencyToken? {
        guard text.hasPrefix("v1:") else { return nil }
        let body = text.dropFirst(3)
        if body.isEmpty { return .empty }
        var positions: [Position] = []
        for item in body.split(separator: ",", omittingEmptySubsequences: false) {
            guard let position = parseItem(Substring(item)) else { return nil }
            positions.append(position)
        }
        return ConsistencyToken(normalized: positions)
    }

    private static func parseItem(_ item: Substring) -> Position? {
        guard item.first == "s", let slash = item.range(of: "/p") else { return nil }
        let afterSlash = item[slash.upperBound...]
        guard let at = afterSlash.firstIndex(of: "@") else { return nil }
        guard let stream = canonical(item[item.index(after: item.startIndex)..<slash.lowerBound]),
              let partition = canonical(afterSlash[afterSlash.startIndex..<at]),
              let offset = canonical(afterSlash[afterSlash.index(after: at)...]),
              partition <= UInt64(UInt32.max)
        else { return nil }
        return Position(stream: stream, partition: UInt32(partition), offset: offset)
    }

    private static func canonical(_ digits: Substring) -> UInt64? {
        guard !digits.isEmpty, digits.allSatisfy({ $0 >= "0" && $0 <= "9" }) else { return nil }
        if digits.count > 1, digits.first == "0" { return nil }
        return UInt64(digits)
    }

    /// The union of two tokens, keeping the max offset per (stream, partition).
    public func merge(_ other: ConsistencyToken) -> ConsistencyToken {
        ConsistencyToken(positions + other.positions)
    }

    /// The offset this token holds for a pair, or nil.
    public func offset(stream: UInt64, partition: UInt32) -> UInt64? {
        positions.filter { $0.stream == stream && $0.partition == partition }.map(\.offset).max()
    }

    /// The wire form.
    public var description: String {
        "v1:" + positions.map { "s\($0.stream)/p\($0.partition)@\($0.offset)" }.joined(separator: ",")
    }
}
