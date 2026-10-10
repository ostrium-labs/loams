// The message and JSON foundation the runtime is written against.
//
// # Why this file exists at all
//
// Every other SDK's runtime sits on **generated** protobuf message types:
// `protoc-gen-go` writes `*loams.GetInstanceRequest`, `protoc-gen-swift` writes
// `Loams_Instance_V1_GetInstanceRequest`. Design §44 §7.3 and D606 say the
// module surface is generated and **only the runtime is hand-written**.
//
// For Swift neither of those exists yet. There is no `swift.rs` in
// `crates/loams-facade-gen` (it ships `typescript.rs`, `python.rs`, `go.rs` and
// `rust.rs`), there is no `sdks/templates/swift/`, there is no `swift` case in
// `scripts/sdk/gen.sh`, and no `protoc-gen-swift` output for `proto/` is checked
// in anywhere in this repository. So this file defines the seam those types will
// plug into rather than pretending they exist.
//
// The seam is `LoamsMessage`: a protocol, not a concrete struct, and every piece
// of the runtime that touches a request or a response is generic over it. The
// one concrete implementation shipped today is `DynamicMessage`, which is what
// the tests and the example use. When the Swift protobuf stubs land,
// `DynamicMessage` is deleted, real message types conform to `LoamsMessage`, and
// **nothing in the runtime changes** — which is the whole point of making it a
// protocol rather than a struct.
//
// The protocol is deliberately tiny: the runtime only ever needs to read and write
// five fields by name, and read a repeated field for the pagination iterator.
// Everything else is opaque bytes to it. That is the same shape as the Go SDK,
// which reaches the same fields by generated Go field name (`IdempotencyKey`,
// `PageToken`, `NextPageToken`, `ConsistencyToken`) — Go does it with
// reflection, Swift does it through this protocol, and both are reading the *proto
// field name* rather than a guess.

import Foundation

/// A JSON value, because the Connect protocol's default codec is JSON and the
/// recorded conformance corpus is four-fifths JSON encodings.
///
/// Protobuf's own `Value` type is not vendored here: adding `swift-protobuf` for
/// one enum, before the Swift stubs that would need it exist, would be a
/// dependency bought before the thing that needs it.
public enum JSONValue: Sendable, Equatable, Hashable {
    case null
    case bool(Bool)
    case number(Double)
    case int(Int)
    case string(String)
    case array([JSONValue])
    case object([String: JSONValue])

    /// The value's string form, or `nil` when it is not a string.
    ///
    /// proto3 encodes a string field as a JSON string and **omits** it when it is
    /// empty, so "absent" and "empty" are the same state on the wire. A caller
    /// asking for a page token therefore gets `nil` for the last page, which is
    /// exactly the stop condition `Pagination` wants.
    public var stringValue: String? {
        if case .string(let value) = self { return value }
        return nil
    }

    /// The value's boolean form, or `nil` when it is not a boolean.
    public var boolValue: Bool? {
        if case .bool(let value) = self { return value }
        return nil
    }

    /// The value's integer form, or `nil` when it is not a number.
    public var intValue: Int? {
        switch self {
        case .int(let value): return value
        case .number(let value) where value.rounded() == value: return Int(value)
        default: return nil
        }
    }

    /// The value's array form, or `nil` when it is not an array.
    public var arrayValue: [JSONValue]? {
        if case .array(let value) = self { return value }
        return nil
    }

    /// The value's object form, or `nil` when it is not an object.
    public var objectValue: [String: JSONValue]? {
        if case .object(let value) = self { return value }
        return nil
    }

    /// Whether the value is `null`, which proto3 uses for an unset message field.
    public var isNull: Bool { self == .null }
}

// MARK: - Encoding and decoding

extension JSONValue {
    /// The value as canonical JSON bytes.
    ///
    /// `.sortedKeys` is set so the same field dictionary always produces the same
    /// bytes. The conformance corpus records **exact request bodies** and the
    /// fixture server compares them, so a JSON encoder that emitted keys in hash
    /// order would fail a test that has nothing to do with the SDK.
    public func encoded() throws -> Data {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        return try encoder.encode(self)
    }

    /// The value decoded from JSON bytes.
    public static func decoded(from data: Data) throws -> JSONValue {
        try JSONDecoder().decode(JSONValue.self, from: data)
    }

    /// The value decoded from a UTF-8 string, or `nil` when it is not JSON.
    ///
    /// The `nil` case matters for error paths: a server that answers HTML (a
    /// proxy's 502 page, say) must produce a *reported* failure with the body in
    /// the message, not a thrown decoding error that hides what the server said.
    public static func decoded(fromUTF8 string: String) -> JSONValue? {
        guard let data = string.data(using: .utf8) else { return nil }
        return try? decoded(from: data)
    }

    /// The object decoded from JSON bytes, or `nil` when the bytes are not JSON
    /// or not a JSON object.
    ///
    /// Every wire path asks this question, and the answer has to be `nil` rather
    /// than a throw in all of them: a body that is not a Loams response is a
    /// failure to *report* with the bytes in the message, not a decoding error
    /// that hides what the server actually said.
    public static func decodedObject(from data: Data) -> [String: JSONValue]? {
        guard let value = try? decoded(from: data) else { return nil }
        return value.objectValue
    }
}

// `JSONValue` is `Codable` through a hand-written implementation rather than
// synthesis, because the synthesized version cannot round-trip a JSON integer
// without turning it into a `Double` — and `page_size` coming back as
// `1000.0` would be a silent type change on a field the iterator writes.
extension JSONValue: Codable {
    public init(from decoder: any Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let value = try? container.decode(Bool.self) {
            self = .bool(value)
        } else if let value = try? container.decode(Int.self) {
            self = .int(value)
        } else if let value = try? container.decode(Double.self) {
            self = .number(value)
        } else if let value = try? container.decode(String.self) {
            self = .string(value)
        } else if let value = try? container.decode([JSONValue].self) {
            self = .array(value)
        } else if let value = try? container.decode([String: JSONValue].self) {
            self = .object(value)
        } else {
            throw DecodingError.dataCorruptedError(
                in: container,
                debugDescription: "not a JSON value"
            )
        }
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .null: try container.encodeNil()
        case .bool(let value): try container.encode(value)
        case .int(let value): try container.encode(value)
        case .number(let value): try container.encode(value)
        case .string(let value): try container.encode(value)
        case .array(let value): try container.encode(value)
        case .object(let value): try container.encode(value)
        }
    }
}

// MARK: - The proto field names the runtime reads

/// The proto field names the runtime reads by name, in one place.
///
/// These are the names in `proto/`, lower_snake_case, which is what the Connect
/// JSON codec uses and what `loams.options.v1` annotates. Centralising them means
/// a proto rename is one edit here rather than a hunt, and it keeps the runtime
/// from inventing a field name that no schema declares.
public enum ProtoField {
    /// `idempotency_key` (R3).
    public static let idempotencyKey = "idempotency_key"
    /// `page_size`, the AIP-158 request field (R6).
    public static let pageSize = "page_size"
    /// `page_token`, the AIP-158 request field (R6).
    public static let pageToken = "page_token"
    /// `next_page_token`, the AIP-158 response field (R6).
    public static let nextPageToken = "next_page_token"
    /// `consistency_token`, the response field R4's session store folds in.
    public static let consistencyToken = "consistency_token"
}

// MARK: - LoamsMessage

/// A request or a response, as the runtime sees it.
///
/// The protocol exists so the runtime is message-library-agnostic. A generated
/// Swift message type conforms by mapping `loamsFields()` onto its own storage;
/// `DynamicMessage` conforms directly.
public protocol LoamsMessage: Sendable {
    /// The fully qualified proto message name, for example
    /// `loams.instance.v1.GetInstanceRequest`.
    ///
    /// It goes on the wire in the Connect unary request URL (`…/pkg.Service/Type`)
    /// and it is what a caller reads in a failure, so it is part of the contract
    /// rather than a debug string.
    static var protoTypeName: String { get }

    /// A new, empty message.
    init()

    /// The message's fields, keyed by proto field name.
    func loamsFields() -> [String: JSONValue]

    /// A message rebuilt from `fields`, or `nil` when the shape is not this type.
    ///
    /// Returning `nil` rather than throwing keeps the runtime's decoding path
    /// total: a server that answers a `GetInstance` where a `Mutate` was asked
    /// for is a failure to *report*, not a crash.
    init?(loamsFields fields: [String: JSONValue])
}

// MARK: - Field access, shared by every message

extension LoamsMessage {
    /// The message's `idempotency_key`, or `nil` when it carries none.
    ///
    /// `nil` and `""` are the same state on the wire, so both answer `nil`: a
    /// request without a key is a request the retry policy must not treat as
    /// retryable (R3).
    public var idempotencyKey: String? {
        guard let value = loamsFields()[ProtoField.idempotencyKey]?.stringValue,
              !value.isEmpty else { return nil }
        return value
    }

    /// The message's `next_page_token`, or `nil` when there is no next page.
    public var nextPageToken: String? {
        guard let value = loamsFields()[ProtoField.nextPageToken]?.stringValue,
              !value.isEmpty else { return nil }
        return value
    }

    /// The message's `consistency_token`, or `nil` when it carries none.
    public var consistencyToken: String? {
        guard let value = loamsFields()[ProtoField.consistencyToken]?.stringValue,
              !value.isEmpty else { return nil }
        return value
    }

    /// Whether the message declares `idempotency_key` **at all**.
    ///
    /// This is R3's distinction and it is why the check is not "is the field
    /// non-empty": `DeployRequest` has no `idempotency_key`, so keying it would
    /// invent a field the schema does not declare. A caller-built message
    /// therefore has to say so, which is what the generated stubs will do from
    /// their descriptor.
    public var declaresIdempotencyKey: Bool {
        loamsFields()[ProtoField.idempotencyKey] != nil
    }
}

// MARK: - DynamicMessage

/// A schema-free Loams message.
///
/// This is what the SDK uses until `protoc-gen-swift` output exists for `proto/`
/// (see this file's header). It is a real message in every way the runtime cares
/// about — it round-trips through the Connect JSON codec, it carries the fields
/// the runtime reads by name, and it is `Sendable` — but it does not fail to
/// compile when a caller sets a field no proto declares.
///
/// **It is not a substitute for generated types and is not offered as one.** A
/// typo in a field name here is a runtime failure rather than a compile error,
/// which is a real loss of safety and the reason this type is deliberately
/// `public` but undocumented as a stable API. When the Swift stubs land it goes.
public struct DynamicMessage: LoamsMessage, Equatable, Hashable {
    /// The fully qualified proto message name.
    public var protoTypeName: String
    /// The message's fields, keyed by proto field name.
    public var fields: [String: JSONValue]

    public init(protoTypeName: String, fields: [String: JSONValue] = [:]) {
        self.protoTypeName = protoTypeName
        self.fields = fields
    }

    public init() {
        self.protoTypeName = ""
        self.fields = [:]
    }

    public func loamsFields() -> [String: JSONValue] { fields }

    /// Empty, always.
    ///
    /// `LoamsMessage` requires this as a **static** member because a generated
    /// type has one name for all its instances. A schema-free message does not:
    /// its name is whatever the caller put in the instance property of the same
    /// name, which varies per message. So the static form reports the only thing
    /// true of every instance of this type — that there is no single type here.
    ///
    /// The wire never reads it. The Connect URL is built from the RPC name
    /// (`Transport.url(for:rpc:)`), and the only other reader is a decode
    /// failure's message text.
    public static var protoTypeName: String { "" }

    public init?(loamsFields fields: [String: JSONValue]) {
        self.init(protoTypeName: "", fields: fields)
    }

    // MARK: Field mutators, for the runtime and for callers

    /// Sets a field, replacing whatever was there.
    public mutating func set(_ name: String, _ value: JSONValue) {
        fields[name] = value
    }

    /// Removes a field entirely, which is how proto3 "unset" is expressed.
    public mutating func clear(_ name: String) {
        fields.removeValue(forKey: name)
    }

    /// The value of a field, or `nil` when it is absent.
    public func value(_ name: String) -> JSONValue? { fields[name] }

    /// A copy with `name` set, which is what the idempotency and pagination paths
    /// use so they never mutate a caller's message.
    public func setting(_ name: String, to value: JSONValue) -> DynamicMessage {
        var copy = self
        copy.fields[name] = value
        return copy
    }

    /// A copy with a string field set, or the field removed when `value` is
    /// `nil`.
    ///
    /// Removing rather than writing an empty string is deliberate: proto3 omits
    /// an empty string, so writing `""` and removing are the same message on the
    /// wire, and removing is what keeps the recorded request bodies byte-exact.
    public func setting(_ name: String, to value: String?) -> DynamicMessage {
        var copy = self
        if let value {
            copy.fields[name] = .string(value)
        } else {
            copy.fields.removeValue(forKey: name)
        }
        return copy
    }
}

extension DynamicMessage: ExpressibleByDictionaryLiteral {
    /// `let request: DynamicMessage = ["page_size": .int(10)]`
    ///
    /// The proto type name has to be supplied separately — a dictionary literal
    /// cannot carry it — so a message built this way is a *field set* until
    /// `typed(_:)` names it. That is an intentional bit of friction: the type
    /// name is on the wire.
    public init(dictionaryLiteral elements: (String, JSONValue)...) {
        self.init(protoTypeName: "", fields: Dictionary(elements) { _, last in last })
    }

    /// A copy of this message carrying `protoTypeName`.
    public func typed(_ name: String) -> DynamicMessage {
        var copy = self
        copy.protoTypeName = name
        return copy
    }
}