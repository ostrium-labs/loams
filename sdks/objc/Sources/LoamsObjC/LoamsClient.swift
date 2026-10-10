import Foundation

#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

private final class HTTPResultBox: @unchecked Sendable {
    var responseData: Data?
    var statusCode: Int = 0
    var responseError: Error?
}

#if canImport(ObjectiveC)
@objcMembers
public class LoamsTransport: NSObject {
    public let endpoint: String
    public let tokenSource: LoamsTokenSource?
    public let maxRetries: Int

    public init(endpoint: String, tokenSource: LoamsTokenSource? = nil, maxRetries: Int = 3) {
        self.endpoint = endpoint
        self.tokenSource = tokenSource
        self.maxRetries = maxRetries
        super.init()
    }

    public func execute(path: String,
                        headers: [String: String],
                        body: Data) throws -> (Data, Int) {
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

        for (k, v) in headers {
            req.setValue(v, forHTTPHeaderField: k)
        }

        if let ts = tokenSource {
            let token = ts.token()
            if !token.isEmpty {
                req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
            }
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
            throw err
        }

        return (box.responseData ?? Data(), box.statusCode)
    }

    public func callWithRetry(path: String,
                              headers: [String: String],
                              body: Data,
                              isMutation: Bool = false,
                              idempotencyKey: String? = nil) throws -> Data {
        var attempt = 0
        var refreshed = false

        while true {
            attempt += 1
            var curHeaders = headers
            if let key = idempotencyKey {
                curHeaders["idempotency-key"] = key
            }

            let (data, status) = try execute(path: path, headers: curHeaders, body: body)

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

@objcMembers
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

@objcMembers
public class LoamsClientConfiguration: NSObject {
    public let endpoint: String
    public let tokenSource: LoamsTokenSource?
    public let maxRetries: Int

    public init(endpoint: String, tokenSource: LoamsTokenSource? = nil, maxRetries: Int = 3) {
        self.endpoint = endpoint
        self.tokenSource = tokenSource
        self.maxRetries = maxRetries
        super.init()
    }
}

@objcMembers
public class LoamsClient: NSObject {
    public let transport: LoamsTransport
    public let instance: LoamsInstanceService

    public init(configuration: LoamsClientConfiguration) {
        self.transport = LoamsTransport(endpoint: configuration.endpoint,
                                        tokenSource: configuration.tokenSource,
                                        maxRetries: configuration.maxRetries)
        self.instance = LoamsInstanceService(transport: self.transport)
        super.init()
    }

    public convenience init(endpoint: String, tokenSource: LoamsTokenSource? = nil) {
        let config = LoamsClientConfiguration(endpoint: endpoint, tokenSource: tokenSource)
        self.init(configuration: config)
    }
}
#else
public class LoamsTransport: NSObject {
    public let endpoint: String
    public let tokenSource: LoamsTokenSource?
    public let maxRetries: Int

    public init(endpoint: String, tokenSource: LoamsTokenSource? = nil, maxRetries: Int = 3) {
        self.endpoint = endpoint
        self.tokenSource = tokenSource
        self.maxRetries = maxRetries
        super.init()
    }

    public func execute(path: String,
                        headers: [String: String],
                        body: Data) throws -> (Data, Int) {
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

        for (k, v) in headers {
            req.setValue(v, forHTTPHeaderField: k)
        }

        if let ts = tokenSource {
            let token = ts.token()
            if !token.isEmpty {
                req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
            }
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
            throw err
        }

        return (box.responseData ?? Data(), box.statusCode)
    }

    public func callWithRetry(path: String,
                              headers: [String: String],
                              body: Data,
                              isMutation: Bool = false,
                              idempotencyKey: String? = nil) throws -> Data {
        var attempt = 0
        var refreshed = false

        while true {
            attempt += 1
            var curHeaders = headers
            if let key = idempotencyKey {
                curHeaders["idempotency-key"] = key
            }

            let (data, status) = try execute(path: path, headers: curHeaders, body: body)

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

public class LoamsClientConfiguration: NSObject {
    public let endpoint: String
    public let tokenSource: LoamsTokenSource?
    public let maxRetries: Int

    public init(endpoint: String, tokenSource: LoamsTokenSource? = nil, maxRetries: Int = 3) {
        self.endpoint = endpoint
        self.tokenSource = tokenSource
        self.maxRetries = maxRetries
        super.init()
    }
}

public class LoamsClient: NSObject {
    public let transport: LoamsTransport
    public let instance: LoamsInstanceService

    public init(configuration: LoamsClientConfiguration) {
        self.transport = LoamsTransport(endpoint: configuration.endpoint,
                                        tokenSource: configuration.tokenSource,
                                        maxRetries: configuration.maxRetries)
        self.instance = LoamsInstanceService(transport: self.transport)
        super.init()
    }

    public convenience init(endpoint: String, tokenSource: LoamsTokenSource? = nil) {
        let config = LoamsClientConfiguration(endpoint: endpoint, tokenSource: tokenSource)
        self.init(configuration: config)
    }
}
#endif
