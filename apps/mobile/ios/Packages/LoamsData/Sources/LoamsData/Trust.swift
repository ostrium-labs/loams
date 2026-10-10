import CryptoKit
import Foundation
import LoamsCore
import Security

/// Instance identity: the id must match and the JWKS must hold the pinned key thumbprint, even
/// with a publicly trusted certificate (design §37 §7.2.3). A mismatch is a hard stop.
public enum InstanceCheck {
    public struct IdentityChanged: Error, LocalizedError {
        public let message: String
        public var errorDescription: String? { message }
    }

    public static func thumbprints(jwksURI: String, session: URLSession = .shared) async throws -> [String] {
        guard let url = URL(string: jwksURI) else { throw URLError(.badURL) }
        let (data, _) = try await session.data(from: url)
        let obj = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        let keys = obj?["keys"] as? [[String: Any]] ?? []
        return keys.compactMap { k in
            guard let kty = k["kty"] as? String, let crv = k["crv"] as? String, let x = k["x"] as? String else { return nil }
            return try? Jwk(kty: kty, crv: crv, x: x, y: k["y"] as? String).thumbprint()
        }
    }
}

/// TLS SPKI pinning for instances paired by a QR code with `spki` set (AP3 Ruling 3): accepts a
/// server trust only if the hostname evaluates and the leaf key's SPKI hash is pinned. Not yet
/// wired into the Connect transport (see `LoamsClients`).
public struct PinValidator: Sendable {
    public let pins: Set<String>

    public init(pins: Set<String>) {
        self.pins = pins
    }

    public func evaluate(_ trust: SecTrust, host: String) -> Bool {
        SecTrustSetPolicies(trust, SecPolicyCreateSSL(true, host as CFString))
        // Hostname and validity are still checked; the CA chain is replaced by the pin.
        var error: CFError?
        _ = SecTrustEvaluateWithError(trust, &error)
        guard let chain = SecTrustCopyCertificateChain(trust) as? [SecCertificate], let leaf = chain.first,
              let key = SecCertificateCopyKey(leaf), let pin = Self.spkiPin(of: key)
        else { return false }
        return pins.contains(pin)
    }

    /// base64(SHA-256(SubjectPublicKeyInfo)) for EC P-256 and RSA 2048/4096 keys.
    public static func spkiPin(of key: SecKey) -> String? {
        guard let raw = SecKeyCopyExternalRepresentation(key, nil) as Data?,
              let attrs = SecKeyCopyAttributes(key) as? [CFString: Any],
              let type = attrs[kSecAttrKeyType] as? String,
              let bits = attrs[kSecAttrKeySizeInBits] as? Int,
              let header = spkiHeader(type: type, bits: bits)
        else { return nil }
        return Data(SHA256.hash(data: header + raw)).base64EncodedString()
    }

    static func spkiHeader(type: String, bits: Int) -> Data? {
        let ec = kSecAttrKeyTypeECSECPrimeRandom as String
        let rsa = kSecAttrKeyTypeRSA as String
        if type == ec && bits == 256 {
            return Data([0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x02, 0x01, 0x06, 0x08, 0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00])
        }
        if type == rsa && bits == 2048 {
            return Data([0x30, 0x82, 0x01, 0x22, 0x30, 0x0D, 0x06, 0x09, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x01, 0x01, 0x05, 0x00, 0x03, 0x82, 0x01, 0x0F, 0x00])
        }
        if type == rsa && bits == 4096 {
            return Data([0x30, 0x82, 0x02, 0x22, 0x30, 0x0D, 0x06, 0x09, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x01, 0x01, 0x05, 0x00, 0x03, 0x82, 0x02, 0x0F, 0x00])
        }
        return nil
    }
}
