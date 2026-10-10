import Connect
import Foundation
import LoamsCore
import LoamsProto

/// Supplies the current access token; nil before sign-in.
public typealias TokenSource = @Sendable () -> String?

/// The generated Connect clients for one instance: Connect protocol, binary codec, reads as GET
/// where marked NO_SIDE_EFFECTS (design §37 §8.3).
///
/// TODO(AP3 Task 3): pinned instances. connect-swift's `URLSessionHTTPClient` keeps its session
/// delegate private and forwards no authentication challenges or redirects, so SPKI pins
/// (`PinValidator`) and redirect refusal need a small custom `HTTPClientInterface`. Until then
/// only system trust is wired, which is all the plain-http mock needs.
public final class LoamsClients: Sendable {
    public let baseURL: String
    public let instance: Loams_Instance_V1_InstanceServiceClient
    public let devices: Loams_Devices_V1_DeviceServiceClient
    public let approvals: Loams_Approvals_V1_ApprovalServiceClient
    public let operations: Loams_Operations_V1_OperationsServiceClient
    public let notifications: Loams_Notifications_V1_NotificationServiceClient
    private let token: TokenSource

    public struct InsecureEndpoint: Error, LocalizedError {
        public var errorDescription: String? { "The instance address must use https." }
    }

    /// Refuses a non-https address before any token can be attached; debug builds may reach
    /// the local mock over http on loopback only (`PairingPayloads.issuerAllowed`).
    public init(baseURL: String, allowInsecureLoopback: Bool, token: @escaping TokenSource) throws {
        guard let url = URL(string: baseURL), PairingPayloads.issuerAllowed(url, allowInsecureLoopback: allowInsecureLoopback) else {
            throw InsecureEndpoint()
        }
        self.baseURL = baseURL.hasSuffix("/") ? String(baseURL.dropLast()) : baseURL
        self.token = token
        let configuration = URLSessionConfiguration.ephemeral
        // An idle timeout longer than the 15 s heartbeat; ResumingWatch detects dead streams.
        configuration.timeoutIntervalForRequest = 60
        configuration.waitsForConnectivity = false
        configuration.httpCookieStorage = nil
        configuration.urlCache = nil
        let client = ProtocolClient(
            httpClient: URLSessionHTTPClient(configuration: configuration),
            config: ProtocolClientConfig(
                host: self.baseURL,
                networkProtocol: .connect,
                codec: ProtoCodec(),
                unaryGET: .enabledForLimitedPayloadSizes(maxBytes: 50_000)
            )
        )
        instance = Loams_Instance_V1_InstanceServiceClient(client: client)
        devices = Loams_Devices_V1_DeviceServiceClient(client: client)
        approvals = Loams_Approvals_V1_ApprovalServiceClient(client: client)
        operations = Loams_Operations_V1_OperationsServiceClient(client: client)
        notifications = Loams_Notifications_V1_NotificationServiceClient(client: client)
    }

    /// Headers for every call: the bearer token and Accept-Language (server-rendered text).
    /// TODO(auth plan, Q438): `DPoP <token>` and a DPoP proof from the dpop key.
    public var headers: Headers {
        var h: Headers = ["accept-language": [Locale.preferredLanguages.first ?? "en"]]
        if let t = token() { h["authorization"] = ["Bearer \(t)"] }
        return h
    }
}

/// A failed call's stable reason (AP0 Ruling 6).
public extension ConnectError {
    var reason: Reason {
        let infos: [Loams_Errors_V1_ErrorInfo] = unpackedDetails()
        return Reason.fromWire(infos.first?.reason)
    }
}

/// The message or the error of a unary call.
public extension ResponseMessage {
    func get() throws -> Output {
        switch result {
        case .success(let m): return m
        case .failure(let e): throw e
        }
    }
}

/// The in-memory access token, readable from any thread (never persisted).
public final class TokenBox: @unchecked Sendable {
    private let lock = NSLock()
    private var token: String?

    public init(_ token: String? = nil) {
        self.token = token
    }

    public var value: String? {
        get { lock.lock(); defer { lock.unlock() }; return token }
        set { lock.lock(); token = newValue; lock.unlock() }
    }
}
