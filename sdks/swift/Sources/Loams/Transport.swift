// Transports (design §44 §4, D612; runtime contract R10).
//
// One port serves the Connect protocol, gRPC and gRPC-Web (D600), so the only
// question a Swift client has is which of those it would rather speak. This file
// speaks two of them directly and leaves gRPC to `connect-swift`'s `ConnectNIO`,
// because gRPC needs HTTP trailers that `URLSession` cannot expose:
//
//   - **Connect**, unary and server-streaming. Connect's streaming is a plain
//     chunked HTTP response, so it works over HTTP/1.1, and so does everything a
//     TLS terminator, an ingress or a corporate proxy in front of the instance
//     understands. This is the default, because it is the one that works
//     everywhere.
//   - **gRPC-Web**, unary and server-streaming. A browser can only do gRPC-Web,
//     and a caller behind a proxy that only speaks gRPC-Web needs it. The
//     difference from Connect is that gRPC-Web puts its trailers in the body
//     rather than in HTTP trailers, which is why the whole protocol works over
//     `URLSession`.
//   - **gRPC** is **not** spoken by this file: it needs real trailers, which
//     `URLSession` does not expose. It arrives with `connect-swift`'s `ConnectNIO`
//     product (D612) — see the note in `Package.swift`.
//
// R10 ("the browser is a first-class target") is restated the Swift way rather
// than skipped: `Foundation`'s `URLSession` is available on every Apple platform
// the SDK targets and is the **only** networking API in the SDK, so there is no
// entry point that has to avoid a platform-specific socket call, and the default
// path is one that a `WKWebView` and a server-side process can both take.
//
// # The seam
//
// ``HTTPTransport`` is a protocol, and ``CallInvoker`` is written against it and
// nothing else. `connect-swift` conforms to its own `HTTPClientInterface`, so
// adopting it is a conformance plus a change of one factory call — not a rewrite
// of the runtime. That boundary is the reason the protocol is written directly
// rather than against a library whose surface could not be compiled here.

import Foundation

#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

/// The wire protocol a client speaks. All of them are served on one port (D600).
public enum WireProtocol: String, Sendable, Equatable {
    /// The Connect protocol: the default, and the only one that works
    /// identically over HTTP/1.1 and HTTP/2.
    case connect
    /// gRPC-Web. A browser cannot do gRPC; this is what it can do.
    case grpcWeb
    /// gRPC over HTTP/2. **Not implemented** by ``ConnectTransport`` — it needs
    /// HTTP trailers, which `URLSession` does not expose. See the file header.
    case grpc
}

/// Whether a body is binary protobuf or JSON.
///
/// Four of the thirteen recorded fixtures are JSON and four are binary protobuf,
/// per protocol, so this is not a preference: an SDK that always sent one of them
/// would fail half the corpus. The default is `.json`, matching the Connect
/// protocol's own default and the Go SDK's fixtures, which were recorded over
/// Connect+JSON.
public enum Codec: String, Sendable, Equatable {
    case proto
    case json
}

/// How a caller configures the transport.
public struct TransportConfig: Sendable {
    /// The instance's base URL, for example `https://acme.loams.dev`. A loopback
    /// stack is `http://127.0.0.1:8080`.
    public var endpoint: URL
    /// The wire protocol. Unset means Connect, which is what the design asks
    /// every language to default to (D612).
    public var protocol_: WireProtocol
    /// Whether bodies are binary protobuf or JSON.
    public var codec: Codec
    /// Headers added to every request, for a gateway or a proxy.
    public var extraHeaders: [String: String]

    public init(
        endpoint: URL,
        protocol_: WireProtocol = .connect,
        codec: Codec = .json,
        extraHeaders: [String: String] = [:]
    ) {
        self.endpoint = endpoint
        self.protocol_ = protocol_
        self.codec = codec
        self.extraHeaders = extraHeaders
    }
}

/// The request half of a call, as the runtime hands it to the wire.
public struct WireRequest: Sendable {
    /// `package.Service/Method`, which is the URL path.
    public let rpc: String
    /// The request message's fields, keyed by proto field name.
    public let body: [String: JSONValue]
    /// Whether this is a server-streaming call, which changes the framing.
    public let serverStreaming: Bool

    public init(rpc: String, body: [String: JSONValue], serverStreaming: Bool) {
        self.rpc = rpc
        self.body = body
        self.serverStreaming = serverStreaming
    }
}

/// One message off the wire.
public struct WireMessage: Sendable {
    /// The message's fields, or `nil` when the frame did not parse.
    public let fields: [String: JSONValue]?
}

/// A whole response: either it succeeded, or it failed with a typed failure.
public enum WireResponse: Sendable {
    /// The call succeeded and carried this response body.
    case success(body: [String: JSONValue], trailers: [String: String])
    /// The call failed. `info` is the `ErrorInfo` detail when one arrived.
    case failure(code: Code, message: String, info: ErrorInfoShape?, trailers: [String: String])
}

/// A server stream, as the transport sees it: a sequence of messages and a
/// terminal failure or success.
///
/// An `AsyncThrowingStream` rather than a callback, because R7 asks for an
/// `AsyncSequence` and because a stream's failure arrives *after* some messages
/// have already been yielded — which a callback API cannot express without a
/// second, differently-shaped error channel.
public typealias WireStream = AsyncThrowingStream<WireMessage, any Error>

/// The seam a transport is written against.
///
/// ``CallInvoker`` holds one of these and knows nothing about `URLSession`, which
/// is what makes `connect-swift` adoptable without touching the runtime.
public protocol HTTPTransport: Sendable {
    /// The configuration in force, for a failure message.
    var config: TransportConfig { get }

    /// Performs a unary call.
    func unary(_ request: WireRequest, bearer: String?) async throws -> WireResponse

    /// Opens a server stream.
    func serverStream(_ request: WireRequest, bearer: String?) async throws -> WireStream
}

// MARK: - The Connect transport

/// The Connect and gRPC-Web transport, over `URLSession`.
///
/// Both protocols share their framing — the same five-byte envelope — and differ
/// only in the content type, in whether a unary body is enveloped, and in where
/// the trailers end up. Keeping them in one type rather than two is why the
/// conformance corpus's thirteen cases are thirteen cases rather than two
/// transports' worth of near-duplicates.
public final class ConnectTransport: HTTPTransport, @unchecked Sendable {
    public let config: TransportConfig

    private let session: URLSession
    /// The `Content-Type` this transport sends.
    private let contentType: String

    /// Builds a transport.
    ///
    /// `session` is a parameter so a caller with its own TLS configuration,
    /// proxy or timeout supplies it, and so a test can point one at a fixture
    /// server.
    public init(config: TransportConfig, session: URLSession = .shared) {
        self.config = config
        self.session = session
        self.contentType = ConnectTransport.contentType(
            protocol: config.protocol_, codec: config.codec, streaming: false
        )
    }

    /// The `Content-Type` a protocol and codec pair sends.
    ///
    /// The gRPC-Web spelling is `application/grpc-web+json` /
    /// `application/grpc-web+proto` and the Connect spelling is
    /// `application/json` / `application/proto` for unary, `application/connect+json`
    /// / `application/connect+proto` for a stream. All four appear in the recorded
    /// corpus, which is why this is a function of both flags rather than a
    /// constant.
    static func contentType(protocol proto: WireProtocol, codec: Codec, streaming: Bool) -> String {
        let suffix = codec == .json ? "json" : "proto"
        switch proto {
        case .connect:
            return streaming ? "application/connect+\(suffix)" : "application/\(suffix)"
        case .grpcWeb:
            return "application/grpc-web+\(suffix)"
        case .grpc:
            return "application/grpc+\(suffix)"
        }
    }

    // MARK: Unary

    public func unary(_ request: WireRequest, bearer: String?) async throws -> WireResponse {
        let (data, response) = try await perform(
            request,
            bearer: bearer,
            streaming: false
        )
        let trailers = ConnectTransport.trailers(from: response)

        // A Connect unary failure arrives as an HTTP status with a JSON body
        // describing it. gRPC-Web instead answers 200 and puts the status in a
        // trailer, so both paths have to be read, and reading only one is how a
        // client ends up treating a 501 as a successful empty response.
        if let failure = ConnectTransport.failure(from: data, http: response, trailers: trailers) {
            return failure
        }
        guard let body = JSONValue.decoded(from: data)?.objectValue else {
            // A body that is not JSON is not a Loams response. It is reported with
            // the bytes in the message rather than turned into an empty success:
            // a proxy's HTML 502 page must be visible, not swallowed.
            let preview = String(data: data.prefix(512), encoding: .utf8) ?? "<\(data.count) bytes>"
            return .failure(
                code: .internal,
                message: "loams: the response was neither a Loams message nor a Loams error: \(preview)",
                info: nil,
                trailers: trailers
            )
        }
        return .success(body: body, trailers: trailers)
    }

    // MARK: Server stream

    public func serverStream(_ request: WireRequest, bearer: String?) async throws -> WireStream {
        let (data, response) = try await perform(request, bearer: bearer, streaming: true)
        let frames = ConnectTransport.parseEnvelopes(data)

        // The gRPC-Web trailers frame carries `grpc-status` **in the body**, so the
        // body has to be consulted before the HTTP status can be trusted. Merging
        // it over the HTTP headers (rather than replacing them) matters for the
        // Connect case, where the end frame is the authority and the headers are
        // empty — and for a gRPC-Web response where both are present.
        var trailers = ConnectTransport.trailers(from: response)
        for frame in frames {
            if case .trailers(let payload) = frame {
                trailers.merge(ConnectTransport.trailers(fromFrame: payload)) { _, body in body }
            }
        }

        return AsyncThrowingStream { continuation in
            var ended = false
            for frame in frames {
                switch frame {
                case .message(let payload):
                    continuation.yield(WireMessage(fields: JSONValue.decoded(from: payload)?.objectValue))
                case .trailers:
                    // Read above; nothing to do here.
                    break
                case .end(let payload):
                    ended = true
                    if let failure = ConnectTransport.endStreamFailure(payload) {
                        continuation.finish(throwing: failure)
                    } else {
                        continuation.finish()
                    }
                }
            }
            if !ended {
                // A gRPC-Web refusal is a 200 with a `grpc-status` in the body and
                // no end frame, so it is reported here. Reading only the HTTP status
                // would end the stream **cleanly** on a refusal, which is the
                // silently-wrong outcome R7 exists to prevent.
                if let failure = ConnectTransport.failure(from: data, http: response, trailers: trailers) {
                    continuation.finish(throwing: failure)
                } else {
                    continuation.finish()
                }
            }
        }
    }

    // MARK: The request itself

    private func perform(
        _ request: WireRequest,
        bearer: String?,
        streaming: Bool
    ) async throws -> (Data, HTTPURLResponse) {
        let body = try ConnectTransport.encodeBody(request.body, codec: config.codec)

        // gRPC-Web and a Connect **stream** both envelope the request. A Connect
        // **unary** does not: its body is the bare message, which is what makes
        // Connect unary something `curl` can send (design §44 §4) and what the
        // recorded `application/json` fixture bodies are.
        let payload: Data
        if streaming || config.protocol_ == .grpcWeb {
            payload = ConnectTransport.envelope(payload: body, flags: 0)
        } else {
            payload = body
        }

        var urlRequest = URLRequest(url: ConnectTransport.url(for: config.endpoint, rpc: request.rpc))
        urlRequest.httpMethod = "POST"
        urlRequest.httpBody = payload
        urlRequest.setValue(contentType(protocol: config.protocol_, codec: config.codec, streaming: streaming),
                            forHTTPHeaderField: "Content-Type")
        urlRequest.setValue(String(payload.count), forHTTPHeaderField: "Content-Length")
        // The bearer travels in a header and **never** in the URL: a query string
        // ends up in proxy logs, in browser history and in `Referer` (R1).
        if let bearer, !bearer.isEmpty {
            urlRequest.setValue("Bearer \(bearer)", forHTTPHeaderField: "Authorization")
        }
        for (name, value) in config.extraHeaders {
            urlRequest.setValue(value, forHTTPHeaderField: name)
        }

        let (data, response) = try await session.data(for: urlRequest)
        guard let http = response as? HTTPURLResponse else {
            throw LoamsError.transport(code: .unknown, rpc: request.rpc, message: "loams: the response was not HTTP")
        }
        return (data, http)
    }

    // MARK: Encoding

    /// The request body bytes for a codec.
    ///
    /// A `.proto` codec is **not** implemented, and saying so loudly beats
    /// silently sending JSON under a `application/proto` content type: a server
    /// would answer a parse error, and the caller would see a content-type
    /// mismatch rather than "this SDK cannot encode protobuf yet". Binary protobuf
    /// arrives with the generated Swift stubs, which is where the descriptor-driven
    /// encoder comes from.
    static func encodeBody(_ fields: [String: JSONValue], codec: Codec) throws -> Data {
        switch codec {
        case .json:
            return try JSONValue.object(fields).encoded()
        case .proto:
            throw LoamsError.internalError(
                "",
                "loams: the .proto codec is not implemented; it needs the generated Swift message types, which arrive with the protoc-gen-swift stubs. Use Codec.json, which is the Connect protocol's own default."
            )
        }
    }

    /// One enveloped frame: a flags byte, a big-endian length, then the payload.
    ///
    /// The five-byte header is the whole of Connect's and gRPC-Web's framing.
    /// `flags` 0x02 marks the Connect end-of-stream frame and 0x80 marks the
    /// gRPC-Web trailers frame; both are handled in ``parseEnvelopes(_:)``.
    static func envelope(payload: Data, flags: UInt8) -> Data {
        var out = Data([flags])
        let length = UInt32(payload.count).bigEndian
        withUnsafeBytes(of: length) { out.append(contentsOf: $0) }
        out.append(payload)
        return out
    }

    /// Every frame in a response body, in order.
    ///
    /// A trailing partial frame is dropped rather than throwing: a response cut
    /// short by a dropped connection has no complete frame to read, and the
    /// caller's ``WireStream`` finishing early is the honest report. Throwing here
    /// would replace "the stream stopped" with "the SDK could not parse", which
    /// sends a reader looking in the wrong place.
    static func parseEnvelopes(_ data: Data) -> [Envelope] {
        var frames: [Envelope] = []
        var offset = 0
        while offset + 5 <= data.count {
            let flags = data[data.startIndex + offset]
            let lengthBytes = data[(data.startIndex + offset + 1)..<(data.startIndex + offset + 5)]
            var length: UInt32 = 0
            for byte in lengthBytes {
                length = (length << 8) | UInt32(byte)
            }
            let start = offset + 5
            let end = start + Int(length)
            guard end <= data.count else { break }
            let payload = data[(data.startIndex + start)..<(data.startIndex + end)]
            if flags & 0x02 != 0 {
                frames.append(.end(Data(payload)))
            } else if flags & 0x80 != 0 {
                frames.append(.trailers(Data(payload)))
            } else {
                frames.append(.message(Data(payload)))
            }
            offset = end
        }
        return frames
    }

    /// One decoded frame.
    enum Envelope {
        /// A message frame.
        case message(Data)
        /// A Connect end-of-stream frame, carrying either success (`{}`) or an
        /// error description.
        case end(Data)
        /// A gRPC-Web trailers frame, carrying HTTP-style `name: value` lines.
        case trailers(Data)
    }

    // MARK: Decoding failures

    /// The failure a response describes, or `nil` when it succeeded.
    ///
    /// Three shapes are read, and all three are needed:
    ///
    ///  1. a Connect unary error — an HTTP status outside 2xx with a JSON body of
    ///     `{"code","message","details"}`;
    ///  2. a gRPC-Web error — HTTP 200 with `grpc-status` in the trailers frame,
    ///     where the status is *not* an HTTP status at all;
    ///  3. `grpc-status-details-bin`, a base64 `google.rpc.Status` whose `details`
    ///     carry the `ErrorInfo`.
    static func failure(
        from data: Data,
        http: HTTPURLResponse,
        trailers: [String: String]
    ) -> WireResponse? {
        let parsed = JSONValue.decoded(from: data)?.objectValue

        // (1) Connect unary. The body's `code` wins over the HTTP status because
        // the status is only a mapping of it, and a proxy's 502 in front of a
        // `not_found` would otherwise be reported as the proxy's fault.
        if let code = parsed?["code"]?.stringValue,
           let resolved = Code(rawValue: code) {
            let message = parsed?["message"]?.stringValue ?? ""
            return .failure(
                code: resolved,
                message: message,
                info: errorInfo(fromDetails: parsed?["details"]?.arrayValue ?? []),
                trailers: trailers
            )
        }

        // (2) gRPC-Web.
        if let status = trailers["grpc-status"], let code = code(grpcStatus: status) {
            var message = trailers["grpc-message"] ?? ""
            message = message.removingPercentEncoding ?? message
            var info: ErrorInfoShape?
            if let binary = trailers["grpc-status-details-bin"], let data = Data(base64Encoded: binary) {
                info = errorInfo(fromGoogleRPCStatus: data)
            }
            return .failure(code: code, message: message, info: info, trailers: trailers)
        }

        // A bare HTTP failure with no protocol body: a gateway, or a TLS
        // terminator. Reported with the status rather than dropped.
        guard (200..<300).contains(http.statusCode) == false else { return nil }
        let preview = String(data: data.prefix(512), encoding: .utf8) ?? ""
        return .failure(
            code: loamsCode(fromHTTPStatus: http.statusCode),
            message: preview.isEmpty ? "HTTP \(http.statusCode)" : preview,
            info: nil,
            trailers: trailers
        )
    }

    /// The failure a Connect end-of-stream frame describes, or `nil` for a clean
    /// end.
    ///
    /// A clean end is the literal `{}` (or an empty frame): the server finished,
    /// which is **not** an error and must not be reported as one.
    static func endStreamFailure(_ payload: Data) -> (any Error)? {
        guard let parsed = JSONValue.decoded(from: payload)?.objectValue else { return nil }
        guard let error = parsed["error"]?.objectValue else { return nil }
        let codeText = error["code"]?.stringValue ?? ""
        let code = Code(rawValue: codeText) ?? .unknown
        let message = error["message"]?.stringValue ?? ""
        let info = errorInfo(fromDetails: error["details"]?.arrayValue ?? [])
        return LoamsError.fromWire(code: code, info: info, rpc: "", message: message)
    }

    /// The trailers a response carries.
    ///
    /// gRPC-Web puts them in the **body** as a 0x80 frame, so the body has to be
    /// parsed for them; a Connect stream puts them in HTTP trailers, which
    /// `URLSession` does not expose, so those are absent and the end frame is the
    /// authority instead. An empty dictionary is the normal result and callers
    /// treat it as such.
    static func trailers(from response: HTTPURLResponse) -> [String: String] {
        var out: [String: String] = [:]
        for (key, value) in response.allHeaderFields {
            if let name = key as? String, let text = value as? String {
                out[name.lowercased()] = text
            }
        }
        return out
    }

    /// The gRPC-Web trailers frame, parsed as HTTP-style `name: value` lines.
    ///
    /// Lowercased keys, because the frame's names are lowercase by convention and
    /// the runtime looks them up lowercase.
    static func trailers(fromFrame payload: Data) -> [String: String] {
        var out: [String: String] = [:]
        guard let text = String(data: payload, encoding: .utf8) else { return out }
        for line in text.split(separator: "\r\n") {
            guard let colon = line.firstIndex(of: ":") else { continue }
            let name = line[line.startIndex..<colon]
                .trimmingCharacters(in: .whitespaces)
                .lowercased()
            let value = line[line.index(after: colon)...]
                .trimmingCharacters(in: .whitespaces)
            out[name] = value
        }
        return out
    }

    /// The absolute URL of one RPC on the instance.
///
/// Built by string concatenation rather than `URL.appendingPathComponent` or
/// `URL.appending(path:)`, for two reasons that both bite here:
///
///  - `appending(path:)` is **iOS 16 / macOS 13**, and this SDK's floor is iOS 15 /
///    macOS 12 (the plan's Task 4 row). Using it would raise the floor past what
///    the design states without saying so — which is the mistake the Go SDK's
///    `DEPENDENCIES.md` records as an owner's decision (Q732).
///  - `appendingPathComponent` percent-encodes a component, and the RPC's slashes
///    are **path separators**, not data. A component built from
///    `loams.instance.v1.InstanceService/GetInstance` must keep them.
static func url(for endpoint: URL, rpc: String) -> URL {
    var base = endpoint.absoluteString
    while base.hasSuffix("/") { base.removeLast() }
    return URL(string: base + "/" + rpc) ?? endpoint
}

/// The `Code` a `grpc-status` number names.
    static func code(grpcStatus: String) -> Code? {
        switch Int(grpcStatus.trimmingCharacters(in: .whitespaces)) {
        case 0: return nil
        case 1: return .canceled
        case 2: return .unknown
        case 3: return .invalidArgument
        case 4: return .deadlineExceeded
        case 5: return .notFound
        case 6: return .alreadyExists
        case 7: return .permissionDenied
        case 8: return .resourceExhausted
        case 9: return .failedPrecondition
        case 10: return .aborted
        case 11: return .outOfRange
        case 12: return .unimplemented
        case 13: return .internal
        case 14: return .unavailable
        case 15: return .dataLoss
        case 16: return .unauthenticated
        default: return .unknown
        }
    }

    /// The `Code` an HTTP status maps to, per the Connect protocol's own table.
    ///
    /// Used only where there is no protocol body, so a gateway's status is still
    /// classified rather than collapsing to `unknown`.
    static func loamsCode(fromHTTPStatus status: Int) -> Code {
        switch status {
        case 400: return .invalidArgument
        case 401: return .unauthenticated
        case 403: return .permissionDenied
        case 404: return .notFound
        case 409: return .alreadyExists
        case 412: return .failedPrecondition
        case 429: return .resourceExhausted
        case 499: return .canceled
        case 501: return .unimplemented
        case 503: return .unavailable
        case 504: return .deadlineExceeded
        default: return .unknown
        }
    }
}