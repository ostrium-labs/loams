import Foundation

#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

private final class HTTPResultBox: @unchecked Sendable {
    var responseData: Data?
    var statusCode: Int = 0
    var responseError: Error?
}

/// What an HTTP sender answers: the body and the status.
#if canImport(ObjectiveC)
@objcMembers
#endif
public final class LoamsHTTPResult: NSObject {
    public let data: Data
    public let status: Int

    public init(data: Data, status: Int) {
        self.data = data
        self.status = status
        super.init()
    }
}

/// Sends one HTTP request, in place of `URLSession` (a test's scripted server).
/// A sender that runs out of time throws `LoamsTransport.clientDeadline()`.
public protocol LoamsHTTPSender: AnyObject {
    func send(_ request: URLRequest) throws -> LoamsHTTPResult
}

#if canImport(ObjectiveC)
@objcMembers
#endif
public class LoamsTransport: NSObject {
    /// The client default for a unary call, in seconds (R12).
    public static let defaultTimeout: TimeInterval = 30

    public let endpoint: String
    public let tokenSource: LoamsTokenSource?
    public let maxRetries: Int
    /// A unary call's budget in seconds, retries included; 0 is none (R12).
    public let timeout: TimeInterval
    /// Replaces `URLSession` when set.
    public let sender: LoamsHTTPSender?

    public init(endpoint: String, tokenSource: LoamsTokenSource? = nil, maxRetries: Int = 3,
                timeout: TimeInterval = LoamsTransport.defaultTimeout, sender: LoamsHTTPSender? = nil) {
        self.endpoint = endpoint
        self.tokenSource = tokenSource
        self.maxRetries = maxRetries
        self.timeout = timeout
        self.sender = sender
        super.init()
    }

    /// R12: the error of a call whose budget ran out on the client. Never retried.
    public static func clientDeadline() -> LoamsError {
        return LoamsError(message: "the call ran out of time on the client",
                          codeString: "deadline_exceeded",
                          reason: "client_deadline_exceeded",
                          unknownReason: nil,
                          httpStatus: 504)
    }

    public func execute(path: String,
                        headers: [String: String],
                        body: Data) throws -> (Data, Int) {
        return try execute(path: path, headers: headers, body: body, timeout: 0)
    }

    /// One attempt; `timeout` is its budget in seconds, 0 for the transport default.
    public func execute(path: String,
                        headers: [String: String],
                        body: Data,
                        timeout: TimeInterval) throws -> (Data, Int) {
        let cleanEndpoint = endpoint.hasSuffix("/") ? String(endpoint.dropLast()) : endpoint
        let cleanPath = path.hasPrefix("/") ? String(path.dropFirst()) : path
        guard let url = URL(string: "\(cleanEndpoint)/\(cleanPath)") else {
            throw LoamsError(message: "Invalid URL",
                             codeString: "invalid_argument",
                             reason: nil,
                             unknownReason: nil,
                             httpStatus: 400)
        }

        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.httpBody = body
        if timeout > 0 {
            req.timeoutInterval = timeout
        }

        for (k, v) in headers {
            req.setValue(v, forHTTPHeaderField: k)
        }

        if let ts = tokenSource {
            let token = ts.token()
            if !token.isEmpty {
                req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
            }
        }

        if let sender = sender {
            let result = try sender.send(req)
            return (result.data, result.status)
        }

        let box = HTTPResultBox()
        let sem = DispatchSemaphore(value: 0)
        let task = URLSession.shared.dataTask(with: req) { data, resp, err in
            if let http = resp as? HTTPURLResponse {
                box.statusCode = http.statusCode
            }
            box.responseData = data
            box.responseError = err
            sem.signal()
        }
        task.resume()
        sem.wait()

        if let err = box.responseError {
            if let urlError = err as? URLError, urlError.code == .timedOut {
                throw LoamsTransport.clientDeadline()
            }
            throw err
        }

        return (box.responseData ?? Data(), box.statusCode)
    }

    public func callWithRetry(path: String,
                              headers: [String: String],
                              body: Data,
                              isMutation: Bool = false,
                              idempotencyKey: String? = nil) throws -> Data {
        return try callWithRetry(path: path, headers: headers, body: body, isMutation: isMutation,
                                 idempotencyKey: idempotencyKey, timeout: -1)
    }

    /// `timeout` is this call's budget in seconds: negative is the transport's,
    /// 0 is none (R12). The budget covers every retry, goes out as
    /// `connect-timeout-ms`, and a call that runs out is never retried.
    public func callWithRetry(path: String,
                              headers: [String: String],
                              body: Data,
                              isMutation: Bool,
                              idempotencyKey: String?,
                              timeout: TimeInterval) throws -> Data {
        var attempt = 0
        var refreshed = false
        let budget = timeout < 0 ? self.timeout : timeout
        let deadline = budget > 0 ? Date().addingTimeInterval(budget) : nil

        while true {
            attempt += 1
            var remaining: TimeInterval = 0
            if let deadline = deadline {
                remaining = deadline.timeIntervalSinceNow
                if remaining <= 0 {
                    throw LoamsTransport.clientDeadline()
                }
            }
            var curHeaders = headers
            if let key = idempotencyKey {
                curHeaders["idempotency-key"] = key
            }
            if deadline != nil {
                curHeaders["connect-timeout-ms"] = String(max(1, Int((remaining * 1000).rounded(.up))))
            }

            let (data, status) = try execute(path: path, headers: curHeaders, body: body, timeout: remaining)

            if status >= 200 && status < 300 {
                return data
            }

            let parsedErr = LoamsErrorParser.parse(withStatus: status, body: data)

            // R1: Token refresh on token_expired
            if parsedErr.reason == "token_expired" && !refreshed, let ts = tokenSource {
                ts.refreshToken()
                refreshed = true
                continue
            }

            // R2: Retriable status
            let isRetriable = (status == 503 || parsedErr.codeString == "unavailable")
            if isRetriable && (idempotencyKey != nil || !isMutation) && attempt < maxRetries {
                usleep(useconds_t(20000 * attempt))
                continue
            }

            throw parsedErr
        }
    }
}

#if canImport(ObjectiveC)
@objcMembers
#endif
public class LoamsInstanceService: NSObject {
    private let transport: LoamsTransport

    public init(transport: LoamsTransport) {
        self.transport = transport
        super.init()
    }

    public func getInstance() throws -> [String: Any] {
        let data = try transport.callWithRetry(path: "/loams.instance.v1.InstanceService/GetInstance",
                                               headers: ["Content-Type": "application/json"],
                                               body: "{}".data(using: .utf8)!)
        return (try? JSONSerialization.jsonObject(with: data) as? [String: Any]) ?? [:]
    }

    public func whoAmI() throws -> [String: Any] {
        let data = try transport.callWithRetry(path: "/loams.instance.v1.InstanceService/WhoAmI",
                                               headers: ["Content-Type": "application/json"],
                                               body: "{}".data(using: .utf8)!)
        return (try? JSONSerialization.jsonObject(with: data) as? [String: Any]) ?? [:]
    }
}

#if canImport(ObjectiveC)
@objcMembers
#endif
public class LoamsClientConfiguration: NSObject {
    public let endpoint: String
    public let tokenSource: LoamsTokenSource?
    public let maxRetries: Int
    /// The default namespace (R11), or nil.
    public var namespace: String?
    /// A unary call's budget in seconds; 0 is none (R12).
    public var timeout: TimeInterval = LoamsTransport.defaultTimeout
    /// The session store (R4); off by default.
    public var sessionConsistency = false
    /// Replaces `URLSession` when set.
    public var sender: LoamsHTTPSender?

    public init(endpoint: String, tokenSource: LoamsTokenSource? = nil, maxRetries: Int = 3) {
        self.endpoint = endpoint
        self.tokenSource = tokenSource
        self.maxRetries = maxRetries
        super.init()
    }

    /// A configuration from the environment (R1): `LOAMS_ENDPOINT`, `LOAMS_API_KEY`
    /// or `LOAMS_TOKEN`, and `LOAMS_NAMESPACE`. Nil without `LOAMS_ENDPOINT`.
    public static func fromEnvironment() -> LoamsClientConfiguration? {
        let env = ProcessInfo.processInfo.environment
        guard let endpoint = env["LOAMS_ENDPOINT"], !endpoint.isEmpty else { return nil }
        let key = env["LOAMS_API_KEY"].flatMap { $0.isEmpty ? nil : $0 } ?? env["LOAMS_TOKEN"].flatMap { $0.isEmpty ? nil : $0 }
        let config = LoamsClientConfiguration(endpoint: endpoint,
                                              tokenSource: key.map { LoamsStaticTokenSource(token: $0) })
        config.namespace = env["LOAMS_NAMESPACE"].flatMap { $0.isEmpty ? nil : $0 }
        return config
    }
}

#if canImport(ObjectiveC)
@objcMembers
#endif
public class LoamsClient: NSObject {
    public let transport: LoamsTransport
    public let instance: LoamsInstanceService
    /// The default namespace (R11).
    public let defaultNamespace: String?
    /// The session store when `sessionConsistency` is on (R4).
    public let session: LoamsSessionStore?

    // The app-building modules (SDKP §2).
    public private(set) var namespaces: LoamsNamespaces!
    public private(set) var collections: LoamsCollections!
    public private(set) var documents: LoamsDocuments!
    public private(set) var search: LoamsSearchModule!
    public private(set) var vector: LoamsVectorModule!
    /// Experimental: `loams.graph.v1` is unstable.
    public private(set) var graph: LoamsGraphModule!
    public private(set) var sql: LoamsSqlModule!
    public private(set) var streams: LoamsStreamsModule!
    private var raw: LoamsRawModule!

    public init(configuration: LoamsClientConfiguration) {
        self.transport = LoamsTransport(endpoint: configuration.endpoint,
                                        tokenSource: configuration.tokenSource,
                                        maxRetries: configuration.maxRetries,
                                        timeout: configuration.timeout,
                                        sender: configuration.sender)
        self.instance = LoamsInstanceService(transport: self.transport)
        self.defaultNamespace = configuration.namespace
        self.session = configuration.sessionConsistency ? LoamsSessionStore() : nil
        super.init()
        let context = LoamsAppContext(transport: transport, defaultNamespace: defaultNamespace, session: session)
        self.namespaces = LoamsNamespaces(context: context)
        self.collections = LoamsCollections(context: context)
        self.documents = LoamsDocuments(context: context)
        self.search = LoamsSearchModule(context: context)
        self.vector = LoamsVectorModule(context: context)
        self.graph = LoamsGraphModule(context: context)
        self.sql = LoamsSqlModule(context: context)
        self.streams = LoamsStreamsModule(context: context)
        self.raw = LoamsRawModule(context: context)
    }

    /// Any facade call by its facade-index name (`documents.write`,
    /// `instance.get_instance`) over its request's proto3 JSON.
    public func call(_ name: String, request: [String: Any], options: LoamsCallOptions? = nil) throws -> [String: Any] {
        if name == "instance.get_instance" || name == "instance.who_am_i" {
            let method = name == "instance.get_instance" ? "GetInstance" : "WhoAmI"
            let headers = ["Content-Type": "application/json", "Connect-Protocol-Version": "1"]
                .merging(options?.headers ?? [:]) { _, extra in extra }
            let body = try JSONSerialization.data(withJSONObject: request)
            let data = try transport.callWithRetry(path: "/loams.instance.v1.InstanceService/\(method)", headers: headers,
                                                   body: body, isMutation: false, idempotencyKey: nil,
                                                   timeout: options?.timeout ?? -1)
            return (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] ?? [:]
        }
        return try raw.call(name, request: request, options: options)
    }

    public convenience init(endpoint: String, tokenSource: LoamsTokenSource? = nil) {
        let config = LoamsClientConfiguration(endpoint: endpoint, tokenSource: tokenSource)
        self.init(configuration: config)
    }

    /// A handle on one collection (SDKP §2.8).
    public func collection(_ name: String, namespace: String? = nil) -> LoamsCollectionHandle {
        return LoamsCollectionHandle(client: self, name: name, namespace: namespace)
    }
}
