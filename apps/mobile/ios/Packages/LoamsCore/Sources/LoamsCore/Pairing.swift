import Foundation

/// A v1 pairing payload as a QR code carries it (design §37 §7.2.1). Same rules as Android's
/// `PairingPayloads`; both are pinned by conformance/fixtures/pairing/cases.json.
public struct PairingPayload: Sendable, Equatable {
    public let issuer: URL
    public let instanceID: String
    /// Base64 SHA-256 SPKI pins; nil for publicly trusted certificates.
    public let spki: [String]?
    /// The instance key thumbprint that anchors the instance's identity.
    public let jkt: String
    public let code: String
    public let userCode: String
    public let exp: Date
}

public enum PairingError: String, Error, Sendable {
    case unsupportedVersion = "unsupported_version"
    case wrongKind = "wrong_kind"
    case expired
    case insecureIssuer = "insecure_issuer"
    case malformed
}

public enum PairingPayloads {
    public static let kind = "loams-pair"
    public static let version = 1
    static let loopbackHosts: Set<String> = ["10.0.2.2", "127.0.0.1", "localhost", "::1"]

    private struct Wire: Decodable {
        let v: Int?
        let kind: String?
        let issuer: String?
        let instance_id: String?
        let spki: [String]?
        let jkt: String?
        let code: String?
        let user_code: String?
        let exp: Int64?
    }

    /// Reads a scanned QR code or pasted text: unknown fields are ignored, but `v`, `kind`,
    /// `exp` and the issuer scheme are checked strictly.
    public static func parse(_ text: String, now: Date, allowInsecureLoopback: Bool = false) -> Result<PairingPayload, PairingError> {
        guard let data = text.trimmingCharacters(in: .whitespacesAndNewlines).data(using: .utf8),
              let w = try? JSONDecoder().decode(Wire.self, from: data)
        else { return .failure(.malformed) }
        guard let kind = w.kind, !kind.isEmpty else { return .failure(.malformed) }
        guard kind == Self.kind else { return .failure(.wrongKind) }
        guard let v = w.v else { return .failure(.malformed) }
        guard v == version else { return .failure(.unsupportedVersion) }
        guard let issuerText = w.issuer.nonEmpty, let instanceID = w.instance_id.nonEmpty, let jkt = w.jkt.nonEmpty,
              let code = w.code.nonEmpty, let userCode = w.user_code.nonEmpty, let exp = w.exp,
              let issuer = URL(string: issuerText), issuer.host != nil
        else { return .failure(.malformed) }
        guard issuerAllowed(issuer, allowInsecureLoopback: allowInsecureLoopback) else { return .failure(.insecureIssuer) }
        let expiry = Date(timeIntervalSince1970: TimeInterval(exp))
        guard now < expiry else { return .failure(.expired) }
        return .success(PairingPayload(issuer: issuer, instanceID: instanceID, spki: w.spki, jkt: jkt, code: code, userCode: userCode, exp: expiry))
    }

    /// Whether the app may talk to an issuer at all; typed addresses use the same rule.
    public static func issuerAllowed(_ issuer: URL, allowInsecureLoopback: Bool) -> Bool {
        switch issuer.scheme?.lowercased() {
        case "https": return true
        case "http": return allowInsecureLoopback && loopbackHosts.contains(issuer.host?.lowercased() ?? "")
        default: return false
        }
    }
}

private extension Optional where Wrapped == String {
    var nonEmpty: String? {
        guard let s = self, !s.isEmpty else { return nil }
        return s
    }
}
