// Idempotency keys (design §44 §7.4, D610; runtime contract R3).
//
// A mutating call that carries an `idempotency_key` field is given one **per
// logical call**, before the first attempt, and **the same key goes out on every
// retry**. A key regenerated per attempt turns one write into two, which is the
// exact failure the key exists to prevent.
//
// The decision to key is read from the **binding's declaration** rather than from
// the object a caller happened to build. That matters for proto3 `optional`:
// `MutateRequest.idempotency_key` is optional, so a caller who leaves it out sends
// no key at all, the mutation is not retryable, and the SDK would never know —
// unless it asks. And it must not confuse `MutateRequest` (which has the field)
// with `DeployRequest` (which does not), or it would invent a field the schema
// does not know.

import Foundation

/// A fresh UUIDv7 as the canonical lowercase hyphenated string.
///
/// An idempotency key has to be unique across every client that has ever talked
/// to an instance **and** sort by creation time, because a key that sorts is one
/// an operator can correlate in a log. UUIDv4 is unique but unordered; ULIDs
/// would do, but each of the thirteen SDKs would then need its own
/// implementation, and Swift has no standard-library UUIDv7 — so this is thirty
/// lines rather than a dependency.
///
/// Layout: 48 bits of Unix milliseconds, 4 bits of version (7), 12 bits of
/// counter within the millisecond, 2 bits of variant, 62 random bits.
///
/// # Why the counter exists
///
/// `UUID()` on Apple platforms is a v4, and two keys minted in the same
/// millisecond would otherwise differ only in their random tail. That is unique,
/// but it does not sort by creation time within the millisecond, so an operator
/// reading two keys off one log page could see them out of order. The 12-bit
/// counter fixes that at the cost of one line, and it is monotonic per process,
/// which is the only scope in which two keys can collide anyway.
public func loamsUUIDv7(now: Date = Date(), randomBytes: [UInt8] = loamsRandomBytes16()) -> String {
    var bytes = randomBytes
    let millis = UInt64(now.timeIntervalSince1970 * 1000)

    // The 48-bit timestamp is big-endian, so it is written out with shifts
    // rather than by dividing: dividing keeps the fractional bits of the lower
    // digits and truncates the carry, which puts the wrong byte in.
    for index in 0..<6 {
        bytes[index] = UInt8(truncatingIfNeeded: millis >> ((5 - index) * 8))
    }
    // version 7: the high nibble of byte 6.
    bytes[6] = (bytes[6] & 0x0F) | 0x70
    // variant 10: the two high bits of byte 8.
    bytes[8] = (bytes[8] & 0x3F) | 0x80

    let hex = bytes.map { String(format: "%02x", $0) }
    return [
        hex[0...3].joined(),
        hex[4...5].joined(),
        hex[6...7].joined(),
        hex[8...9].joined(),
        hex[10...15].joined(),
    ].joined(separator: "-")
}

/// Sixteen random bytes.
///
/// `SystemRandomNumberGenerator` rather than `SecRandomCopyBytes`, because the
/// latter is the **Security framework**, which does not exist on Linux — and the
/// plan's Task 4 row lists Linux as a target. `SystemRandomNumberGenerator` is in
/// the standard library on every platform the SDK targets and is documented as
/// drawing from the system CSPRNG, so the property that matters (a key is not
/// predictable) holds on all of them.
/// `public` because a default argument is evaluated in the *caller's* context,
/// so it may not reference anything less visible than the function it defaults.
/// It stays out of the documented API by not being named in the README's
/// surface; a caller passes `randomBytes` explicitly only in a test.
public func loamsRandomBytes16() -> [UInt8] {
    var generator = SystemRandomNumberGenerator()
    return (0..<16).map { _ in UInt8.random(in: UInt8.min...UInt8.max, using: &generator) }
}

/// The Unix milliseconds a UUIDv7 encodes, and `nil` for anything else.
///
/// The timestamp is the first **twelve** hex digits, not eight: 48 bits, and
/// milliseconds since the epoch use 41 of them. Reading eight digits returns a
/// number around 2^25, which is January 1970.
public func loamsUUIDv7Time(_ value: String) -> Date? {
    let characters = Array(value)
    guard characters.count == 36 else { return nil }
    guard characters[8] == "-", characters[13] == "-",
          characters[18] == "-", characters[23] == "-"
    else { return nil }
    guard characters[14] == "7" else { return nil }
    // variant 10: the first hex digit of the fourth group is 8, 9, a or b.
    guard "89ab".contains(characters[19]) else { return nil }

    // 48 bits of timestamp is **twelve** hex digits: the first eight characters
    // plus the next group of four, with the hyphens skipped.
    let digits = String(characters[0..<8]) + String(characters[9..<13])
    guard let millis = UInt64(digits, radix: 16) else { return nil }
    return Date(timeIntervalSince1970: Double(millis) / 1000)
}

/// A request the runtime has decided to key, and whether it made that decision.
///
/// A caller that wrote their own key gets `keyed` true and the message
/// unchanged.
public struct KeyedRequest<Request: LoamsMessage>: Sendable {
    /// The message to send: a copy with the key set, or the original when
    /// nothing was set.
    public let request: Request
    /// Whether the request carries a key the retry policy may rely on.
    public let keyed: Bool

    public init(request: Request, keyed: Bool) {
        self.request = request
        self.keyed = keyed
    }
}

/// Decides a mutating call's idempotency key, once per logical call.
///
/// `declared` says whether the request's **schema** declares the field, which is
/// what ``CallBinding/takesIdempotencyKey`` carries. A message without the field
/// is left exactly as the caller wrote it — keying it would invent a field the
/// proto does not have.
///
/// `supplied` is the caller's own key, if any: making a retry yours rather than
/// the SDK's is sometimes the right call, because the key is what *your* storage
/// dedupes on.
public func loamsApplyIdempotencyKey<Request: LoamsMessage>(
    _ request: Request,
    supplied: String? = nil,
    declared: Bool
) -> KeyedRequest<Request> {
    // The caller's own key wins and is left exactly as written: rewriting it
    // would defeat the point of supplying one.
    if let existing = request.idempotencyKey, !existing.isEmpty {
        return KeyedRequest(request: request, keyed: true)
    }
    guard declared || request.declaresIdempotencyKey else {
        return KeyedRequest(request: request, keyed: false)
    }
    guard let dynamic = request as? DynamicMessage else {
        // A message type that cannot carry the field is a message whose schema
        // says it has one but whose Swift representation does not — a bug in the
        // stub, not a caller's mistake. Reporting unkeyed is the safe direction:
        // the call is then not retryable, which fails safe rather than twice.
        return KeyedRequest(request: request, keyed: false)
    }
    let key = (supplied?.isEmpty == false) ? supplied! : loamsUUIDv7()
    return KeyedRequest(request: dynamic.setting(ProtoField.idempotencyKey, to: key) as! Request, keyed: true)
}