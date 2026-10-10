import Foundation

/// The JWK shapes the app handles: EC P-256 (device keys) and OKP Ed25519 (instance keys).
public struct Jwk: Sendable, Equatable, Codable {
    public let kty: String
    public let crv: String
    public let x: String
    public let y: String?

    public init(kty: String, crv: String, x: String, y: String? = nil) {
        self.kty = kty
        self.crv = crv
        self.x = x
        self.y = y
    }

    /// RFC 7638 thumbprint, base64url.
    public func thumbprint() throws -> String {
        let members: String
        switch kty {
        case "EC":
            guard let y else { throw CryptoKitError.incorrectParameterSize }
            members = #"{"crv":"\#(crv)","kty":"EC","x":"\#(x)","y":"\#(y)"}"#
        case "OKP":
            members = #"{"crv":"\#(crv)","kty":"OKP","x":"\#(x)"}"#
        default:
            throw CryptoKitError.incorrectParameterSize
        }
        return Base64URL.encode(Data(members.utf8).sha256)
    }

    /// The public JWK of a P-256 key, from its x9.63 representation (04 || x || y).
    public static func p256(x963 raw: Data) throws -> Jwk {
        let bytes = Array(raw)
        guard bytes.count == 65, bytes[0] == 0x04 else { throw CryptoKitError.incorrectParameterSize }
        return Jwk(kty: "EC", crv: "P-256", x: Base64URL.encode(Data(bytes[1..<33])), y: Base64URL.encode(Data(bytes[33..<65])))
    }

    public func json() -> String {
        if let y { return #"{"kty":"\#(kty)","crv":"\#(crv)","x":"\#(x)","y":"\#(y)"}"# }
        return #"{"kty":"\#(kty)","crv":"\#(crv)","x":"\#(x)"}"#
    }
}

/// PKCE S256 (RFC 7636) for ASWebAuthenticationSession, which does no PKCE of its own.
public struct Pkce: Sendable, Equatable {
    public let verifier: String
    public let challenge: String
    public var method: String { "S256" }

    public static func generate() -> Pkce {
        var rng = SystemRandomNumberGenerator()
        let bytes = Data((0..<32).map { _ in UInt8.random(in: 0...255, using: &rng) })
        return from(verifier: Base64URL.encode(bytes))
    }

    public static func from(verifier: String) -> Pkce {
        Pkce(verifier: verifier, challenge: Base64URL.encode(Data(verifier.utf8).sha256))
    }
}

/// Identifiers fixed by AP3 Ruling 1.
public enum Loams {
    public static let bundleID = "dev.loams.app"
    public static let clientID = "loams-ios"
    public static let pairingGrant = "urn:loams:params:oauth:grant-type:pairing"
    public static let tokenExchangeGrant = "urn:ietf:params:oauth:grant-type:token-exchange"
    public static let callbackScheme = "dev.loams.app"
    public static let redirectURI = "dev.loams.app:/oauth2redirect"

    public enum Category: String, Sendable {
        case approval = "APPROVAL", operation = "OPERATION", job = "JOB", security = "SECURITY"
    }

    public static let reviewAction = "REVIEW"
}
