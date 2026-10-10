// Decoding `loams.errors.v1.ErrorInfo` off the wire.
//
// # Why the SDK parses protobuf at all
//
// R8 says the **reason** is the stable branch, and it arrives inside an
// `ErrorInfo` detail, which is a protobuf message inside an `Any`. Reaching it
// needs a decoder, and the generated Swift message types do not exist yet (see
// `Facade.swift`'s header). So this file carries the one thing the runtime
// cannot delegate: a reader for the two shapes an `ErrorInfo` arrives in.
//
// It is deliberately **tiny and total** — six field numbers, no allocation beyond
// the strings it returns, and a hard refusal on anything it does not recognise
// rather than a guess. A hand-rolled protobuf parser that guesses is worse than
// no parser, because a wrong `reason` is a caller acting on a fiction.
//
// # What it reads
//
// `loams.errors.v1.ErrorInfo` (from `proto/loams/errors/v1/errors.proto`):
//
//   1: string reason
//   2: map<string, string> metadata   (repeated message { 1: key, 2: value })
//   3: string hint
//
// and `google.rpc.Status`, which is what `grpc-status-details-bin` carries:
//
//   1: int32 code
//   2: string message
//   3: repeated Any { 1: string type_url, 2: bytes value }
//
// # What it deliberately does not do
//
// It does not decode arbitrary nested messages and it does not implement groups
// (wire type 3/4), which proto3 does not emit. An unknown field is **skipped**,
// which is what makes this forward-compatible with a newer server that added a
// field — R8's "a reason from a newer server is surfaced, not dropped" depends on
// parsing succeeding in the first place.

import Foundation

/// A minimal, forward-compatible protobuf field reader.
///
/// A cursor over `Data` rather than a streaming parser, because the payloads are
/// error details: small, already fully buffered, and read exactly once.
struct ProtoReader {
    private let bytes: [UInt8]
    private var offset: Int

    init(_ data: Data) {
        self.bytes = Array(data)
        self.offset = 0
    }

    /// Whether any bytes remain.
    var hasMore: Bool { offset < bytes.count }

    /// The next field header, or `nil` at the end of the message.
    ///
    /// The header is a varint: field number in the top three bits, wire type in
    /// the bottom three. Returning `nil` rather than throwing is deliberate — a
    /// truncated detail is a malformed error, and the caller's answer for that is
    /// "no reason", which is reported, not thrown.
    mutating func nextField() -> (field: Int, wireType: UInt8)? {
        guard let key = readVarint() else { return nil }
        return (field: Int(key >> 3), wireType: UInt8(key & 0x7))
    }

    /// A length-delimited field's bytes, or `nil` when the field is truncated.
    mutating func readLengthDelimited() -> Data? {
        guard let length = readVarint() else { return nil }
        let start = offset
        let end = start + Int(length)
        guard end <= bytes.count else { return nil }
        offset = end
        return Data(bytes[start..<end])
    }

    /// A varint field's value, or `nil` when the field is truncated.
    mutating func readVarint() -> UInt64? {
        var result: UInt64 = 0
        var shift: UInt64 = 0
        while offset < bytes.count {
            let byte = bytes[offset]
            offset += 1
            result |= UInt64(byte & 0x7F) << shift
            if byte & 0x80 == 0 { return result }
            shift += 7
            // More than ten bytes is a malformed varint, not a large one: a
            // 64-bit value is at most ten bytes, and continuing would read
            // arbitrary bytes off the end of the payload.
            if shift > 63 { return nil }
        }
        return nil
    }

    /// Skips a field the reader does not know, which is what makes it
    /// forward-compatible.
    ///
    /// Wire type 3 (start group) and 4 (end group) are **not** skipped: proto3
    /// does not emit them, so their appearance means the payload is not a message
    /// this reader understands, and returning `false` says so rather than
    /// desynchronising the cursor and producing a plausible wrong answer.
    mutating func skip(wireType: UInt8) -> Bool {
        switch wireType {
        case 0: return readVarint() != nil
        case 1:
            guard offset + 8 <= bytes.count else { return false }
            offset += 8
            return true
        case 2: return readLengthDelimited() != nil
        case 5:
            guard offset + 4 <= bytes.count else { return false }
            offset += 4
            return true
        default: return false
        }
    }
}

// MARK: - ErrorInfo

/// Decodes base64 that may be unpadded.
///
/// The corpus ships unpadded base64 — `Cg9ub3RfaW1wbGVtZW50ZWQ`, 23 characters,
/// which is not a multiple of four — and `Data(base64Encoded:)` rejects it, so
/// every structured reason read as absent and every refusal lost its reason. The
/// pad first, then fall back to the strict decoder so a string that *is*
/// already padded still works.
func loamsBase64(_ text: String) -> Data? {
    if let data = Data(base64Encoded: text) { return data }
    let padding = (4 - text.count % 4) % 4
    guard padding > 0 else { return nil }
    return Data(base64Encoded: text + String(repeating: "=", count: padding))
}

extension ConnectTransport {
    /// The `ErrorInfo` in a Connect error's `details`, looked up **by type**.
    ///
    /// By type and never by position: a service that adds a detail of its own must
    /// not move `reason` out from under a caller (R8).
    ///
    /// A detail whose bytes do not parse is skipped rather than returned as a
    /// half-built value, and `nil` here means "no `ErrorInfo`" — which the
    /// caller reports as a Loams failure with no reason rather than dropping the
    /// whole error, because a Loams failure with no reason and no hint is the one
    /// thing R8 says must not happen.
    static func errorInfo(fromDetails details: [JSONValue]) -> ErrorInfoShape? {
        for detail in details {
            guard let object = detail.objectValue else { continue }
            let type = object["type"]?.stringValue ?? ""
            // Connect sends a bare type name; a `google.protobuf.Any` from a
            // server that used the fully-qualified prefix arrives as
            // `type.googleapis.com/loams.errors.v1.ErrorInfo`. Both name the same
            // message, so both match on the suffix rather than on equality.
            guard type == loamsErrorInfoTypeURL || type.hasSuffix("/" + loamsErrorInfoTypeURL) else {
                continue
            }
            guard let encoded = object["value"]?.stringValue,
                  let data = loamsBase64(encoded)
            else { continue }
            if let info = errorInfo(fromProto: data) { return info }
        }
        return nil
    }

    /// Decodes a serialised `loams.errors.v1.ErrorInfo`.
    static func errorInfo(fromProto data: Data) -> ErrorInfoShape? {
        var reader = ProtoReader(data)
        var reason = ""
        var metadata: [String: String] = [:]
        var hint = ""

        while let (field, wireType) = reader.nextField() {
            switch (field, wireType) {
            case (1, 2):
                guard let payload = reader.readLengthDelimited() else { return nil }
                reason = String(decoding: payload, as: UTF8.self)
            case (2, 2):
                guard let payload = reader.readLengthDelimited() else { return nil }
                // A `map<string, string>` entry is a message of two strings. An
                // entry that does not parse is skipped, not fatal: one malformed
                // metadata pair must not cost the caller the `reason` too.
                if let pair = metadataEntry(from: payload) { metadata[pair.key] = pair.value }
            case (3, 2):
                guard let payload = reader.readLengthDelimited() else { return nil }
                hint = String(decoding: payload, as: UTF8.self)
            default:
                guard reader.skip(wireType: wireType) else { return nil }
            }
        }
        return ErrorInfoShape(reason: reason, metadata: metadata, hint: hint)
    }

    /// One `map<string, string>` entry, or `nil` when it does not parse.
    private static func metadataEntry(from data: Data) -> (key: String, value: String)? {
        var reader = ProtoReader(data)
        var key = ""
        var value = ""
        while let (field, wireType) = reader.nextField() {
            switch (field, wireType) {
            case (1, 2):
                guard let payload = reader.readLengthDelimited() else { return nil }
                key = String(decoding: payload, as: UTF8.self)
            case (2, 2):
                guard let payload = reader.readLengthDelimited() else { return nil }
                value = String(decoding: payload, as: UTF8.self)
            default:
                guard reader.skip(wireType: wireType) else { return nil }
            }
        }
        return (key, value)
    }

    /// The `ErrorInfo` inside a `google.rpc.Status`, from
    /// `grpc-status-details-bin`.
    ///
    /// The `Any` entries are matched by type exactly as
    /// ``errorInfo(fromDetails:)`` matches a Connect detail, for the same reason.
    static func errorInfo(fromGoogleRPCStatus data: Data) -> ErrorInfoShape? {
        var reader = ProtoReader(data)
        while let (field, wireType) = reader.nextField() {
            guard field == 3, wireType == 2 else {
                guard reader.skip(wireType: wireType) else { return nil }
                continue
            }
            guard let payload = reader.readLengthDelimited() else { return nil }
            if let info = errorInfo(fromGoogleRPCAny: payload) { return info }
        }
        return nil
    }

    /// The serialised `ErrorInfo` inside a serialised `Any`.
    private static func errorInfo(fromGoogleRPCAny data: Data) -> ErrorInfoShape? {
        var reader = ProtoReader(data)
        var type = ""
        var value = Data()
        while let (field, wireType) = reader.nextField() {
            switch (field, wireType) {
            case (1, 2):
                guard let payload = reader.readLengthDelimited() else { return nil }
                type = String(decoding: payload, as: UTF8.self)
            case (2, 2):
                guard let payload = reader.readLengthDelimited() else { return nil }
                value = payload
            default:
                guard reader.skip(wireType: wireType) else { return nil }
            }
        }
        guard type == loamsErrorInfoTypeURL || type.hasSuffix("/" + loamsErrorInfoTypeURL) else {
            return nil
        }
        return errorInfo(fromProto: value)
    }
}

// MARK: - Encoding, for the conformance fixture server's benefit

/// Encodes an `ErrorInfo` the way a server would.
///
/// It exists for the **test fixture server**, which has to synthesise the
/// refusals the recorded corpus contains; without it the corpus could only be
/// replayed byte for byte and the SDK's decoding would never be exercised against
/// a message this repository built. It is `internal` rather than public for that
/// reason: the SDK never sends an `ErrorInfo`.
func loamsEncodeErrorInfo(_ info: ErrorInfoShape) -> Data {
    var out = Data()
    if !info.reason.isEmpty {
        out.append(loamsProtoTag(field: 1, wireType: 2))
        out.append(loamsProtoVarint(UInt64(info.reason.utf8.count)))
        out.append(contentsOf: Array(info.reason.utf8))
    }
    // Sorted, so a fixture is byte-stable across runs — the corpus is compared
    // byte for byte and an unordered map would make it flaky.
    for key in info.metadata.keys.sorted() {
        let value = info.metadata[key] ?? ""
        var entry = Data()
        entry.append(loamsProtoTag(field: 1, wireType: 2))
        entry.append(loamsProtoVarint(UInt64(key.utf8.count)))
        entry.append(contentsOf: Array(key.utf8))
        entry.append(loamsProtoTag(field: 2, wireType: 2))
        entry.append(loamsProtoVarint(UInt64(value.utf8.count)))
        entry.append(contentsOf: Array(value.utf8))
        out.append(loamsProtoTag(field: 2, wireType: 2))
        out.append(loamsProtoVarint(UInt64(entry.count)))
        out.append(entry)
    }
    if !info.hint.isEmpty {
        out.append(loamsProtoTag(field: 3, wireType: 2))
        out.append(loamsProtoVarint(UInt64(info.hint.utf8.count)))
        out.append(contentsOf: Array(info.hint.utf8))
    }
    return out
}

/// A protobuf field header.
func loamsProtoTag(field: Int, wireType: UInt8) -> Data {
    return loamsProtoVarint(UInt64(field << 3 | Int(wireType)))
}

/// A base-128 varint.
func loamsProtoVarint(_ value: UInt64) -> Data {
    var remaining = value
    var out = Data()
    repeat {
        var byte = UInt8(remaining & 0x7F)
        remaining >>= 7
        if remaining != 0 { byte |= 0x80 }
        out.append(byte)
    } while remaining != 0
    return out
}