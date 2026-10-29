import Foundation

/// A v1 pairing payload as a QR code carries it (design §37 §7.2.1). Same rules as Android's
/// `PairingPayloads`; both are pinned by conformance/fixtures/pairing/cases.json.
public struct PairingPayload: Sendable, Equatable {
    /// Always 1: a reader refuses any other version.
    public let v: Int
    public let issuer: URL
    public let instanceID: String
    /// Base64 SHA-256 SPKI pins; nil for publicly trusted certificates.
    public let spki: [String]?
    /// The instance key thumbprint that anchors the instance's identity.
    public let jkt: String
    public let code: String
    public let userCode: String
    public let exp: Date
    /// §37 §7.2.4's optional private-network hint; never an auth key.
    public let net: PairingNetwork?

    public init(v: Int = PairingPayloads.version, issuer: URL, instanceID: String, spki: [String]?, jkt: String, code: String, userCode: String, exp: Date, net: PairingNetwork? = nil) {
        self.v = v
        self.issuer = issuer
        self.instanceID = instanceID
        self.spki = spki
        self.jkt = jkt
        self.code = code
        self.userCode = userCode
        self.exp = exp
        self.net = net
    }

    /// Pins that are actually set; an empty list counts as none.
    public var pins: Set<String>? {
        guard let spki, !spki.isEmpty else { return nil }
        return Set(spki)
    }
}

/// `"net": {"kind": "tailnet", "provider": "tailscale|headscale", "login_server": "https://..."}`:
/// the instance is reachable on the user's tailnet, so the app asks them to connect to it first
/// instead of timing out (design §37 §7.2.4).
public struct PairingNetwork: Sendable, Equatable {
    public let kind: String
    public let provider: String?
    public let loginServer: URL?

    public init(kind: String, provider: String?, loginServer: URL?) {
        self.kind = kind
        self.provider = provider
        self.loginServer = loginServer
    }

    public var isTailnet: Bool { kind == "tailnet" }
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
        struct Net: Decodable {
            let kind: String?
            let provider: String?
            let login_server: String?
        }

        let v: Int?
        let kind: String?
        let issuer: String?
        let instance_id: String?
        let spki: [String]?
        let jkt: String?
        let code: String?
        let user_code: String?
        let exp: Int64?
        let net: Net?
    }

    /// The plan's entry point (AP3 Task 2): the JSON bytes a QR code carries. Unknown fields are
    /// ignored, but `v`, `kind`, `exp` and the issuer scheme are checked strictly.
    public static func parse(_ json: Data, now: Date, allowInsecureLoopback: Bool = false) throws(PairingError) -> PairingPayload {
        let w: Wire
        do {
            w = try JSONDecoder().decode(Wire.self, from: json)
        } catch {
            throw PairingError.malformed
        }
        guard let kind = w.kind, !kind.isEmpty else { throw PairingError.malformed }
        guard kind == Self.kind else { throw PairingError.wrongKind }
        guard let v = w.v else { throw PairingError.malformed }
        guard v == version else { throw PairingError.unsupportedVersion }
        guard let issuerText = w.issuer.nonEmpty, let instanceID = w.instance_id.nonEmpty, let jkt = w.jkt.nonEmpty,
              let code = w.code.nonEmpty, let userCode = w.user_code.nonEmpty, let exp = w.exp,
              let issuer = URL(string: issuerText), issuer.host != nil
        else { throw PairingError.malformed }
        guard issuerAllowed(issuer, allowInsecureLoopback: allowInsecureLoopback) else { throw PairingError.insecureIssuer }
        let expiry = Date(timeIntervalSince1970: TimeInterval(exp))
        guard now < expiry else { throw PairingError.expired }
        let net = w.net.flatMap { n -> PairingNetwork? in
            guard let kind = n.kind.nonEmpty else { return nil }
            return PairingNetwork(kind: kind, provider: n.provider.nonEmpty, loginServer: n.login_server.nonEmpty.flatMap(URL.init(string:)))
        }
        return PairingPayload(v: v, issuer: issuer, instanceID: instanceID, spki: w.spki, jkt: jkt, code: code, userCode: userCode, exp: expiry, net: net)
    }

    /// Reads a scanned QR code or pasted text (surrounding whitespace is ignored).
    public static func parse(_ text: String, now: Date, allowInsecureLoopback: Bool = false) -> Result<PairingPayload, PairingError> {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        do throws(PairingError) {
            return .success(try parse(Data(trimmed.utf8), now: now, allowInsecureLoopback: allowInsecureLoopback))
        } catch {
            return .failure(error)
        }
    }

    /// Whether the app may talk to an issuer at all; typed addresses use the same rule.
    public static func issuerAllowed(_ issuer: URL, allowInsecureLoopback: Bool) -> Bool {
        switch issuer.scheme?.lowercased() {
        case "https": return issuer.host != nil
        case "http": return allowInsecureLoopback && loopbackHosts.contains(issuer.host?.lowercased() ?? "")
        default: return false
        }
    }

    /// The issuer without a trailing slash, as tokens and clients expect it.
    public static func normalized(_ issuer: URL) -> String {
        var s = issuer.absoluteString
        while s.hasSuffix("/") { s.removeLast() }
        return s
    }
}

private extension Optional where Wrapped == String {
    var nonEmpty: String? {
        guard let s = self, !s.isEmpty else { return nil }
        return s
    }
}
