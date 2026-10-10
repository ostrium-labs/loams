import Foundation
import LoamsCore

/// Tokens issued to this device. The access token stays in memory only.
public struct DeviceTokens: Sendable, Equatable, CustomStringConvertible {
    public let accessToken: String
    public let refreshToken: String
    public let expiresIn: Int
    public let deviceID: String

    public var description: String { "DeviceTokens(device: \(deviceID), expiresIn: \(expiresIn), tokens: <redacted>)" }
}

public struct DeviceRegistration: Sendable {
    public var deviceName: String
    public var decisionJWK: String
    public var model: String = ""
    public var appVersion: String = ""
    public var platform: String = "ios"
    public var clientID: String = Loams.clientID

    public init(deviceName: String, decisionJWK: String, model: String = "", appVersion: String = "") {
        self.deviceName = deviceName
        self.decisionJWK = decisionJWK
        self.model = model
        self.appVersion = appVersion
    }
}

public enum TokenResult: Sendable, Equatable {
    case ok(DeviceTokens)
    case refused(error: String, reason: Reason, description: String?)
}

/// The Loams gateway's token endpoint `POST {issuer}/api/v1/oauth/token` (design §37 §7.2.2).
/// TODO(auth plan, Q438): a DPoP proof on every call.
public struct TokenEndpoint: Sendable {
    let issuer: String
    let session: URLSession

    public init(issuer: String, session: URLSession = .shared) {
        self.issuer = issuer.hasSuffix("/") ? String(issuer.dropLast()) : issuer
        self.session = session
    }

    public func redeemPairing(code: String?, userCode: String?, device: DeviceRegistration) async throws -> TokenResult {
        precondition((code == nil) != (userCode == nil), "send exactly one of code or user_code")
        var f = fields(device) + [("grant_type", Loams.pairingGrant)]
        if let code { f.append(("code", code)) }
        if let userCode { f.append(("user_code", userCode)) }
        return try await post(f)
    }

    public func exchange(authentikAccessToken: String, device: DeviceRegistration) async throws -> TokenResult {
        try await post(fields(device) + [
            ("grant_type", Loams.tokenExchangeGrant),
            ("subject_token", authentikAccessToken),
            ("subject_token_type", "urn:ietf:params:oauth:token-type:access_token"),
        ])
    }

    public func refresh(_ refreshToken: String) async throws -> TokenResult {
        try await post([("grant_type", "refresh_token"), ("refresh_token", refreshToken), ("client_id", Loams.clientID)])
    }

    private func fields(_ d: DeviceRegistration) -> [(String, String)] {
        [("client_id", d.clientID), ("device_name", d.deviceName), ("platform", d.platform), ("model", d.model), ("app_version", d.appVersion), ("decision_jwk", d.decisionJWK)]
    }

    static func formEncode(_ fields: [(String, String)]) -> Data {
        var allowed = CharacterSet.alphanumerics
        allowed.insert(charactersIn: "-._~")
        let body = fields.map { k, v in
            "\(k.addingPercentEncoding(withAllowedCharacters: allowed)!)=\(v.addingPercentEncoding(withAllowedCharacters: allowed)!)"
        }.joined(separator: "&")
        return Data(body.utf8)
    }

    private func post(_ fields: [(String, String)]) async throws -> TokenResult {
        var req = URLRequest(url: URL(string: issuer + "/api/v1/oauth/token")!)
        req.httpMethod = "POST"
        req.setValue("application/x-www-form-urlencoded", forHTTPHeaderField: "Content-Type")
        req.httpBody = Self.formEncode(fields)
        let (data, response) = try await session.data(for: req)
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        return try Self.parse(status: status, body: data)
    }

    static func parse(status: Int, body: Data) throws -> TokenResult {
        guard let obj = try JSONSerialization.jsonObject(with: body) as? [String: Any] else {
            throw URLError(.cannotParseResponse)
        }
        if (200..<300).contains(status) {
            guard let access = obj["access_token"] as? String else { throw URLError(.cannotParseResponse) }
            return .ok(DeviceTokens(
                accessToken: access,
                refreshToken: obj["refresh_token"] as? String ?? "",
                expiresIn: obj["expires_in"] as? Int ?? 0,
                deviceID: obj["device_id"] as? String ?? ""
            ))
        }
        return .refused(
            error: obj["error"] as? String ?? "http_\(status)",
            // Only the machine field; error_description is free text.
            reason: Reason.fromWire(obj["loams_reason"] as? String),
            description: obj["error_description"] as? String
        )
    }
}
