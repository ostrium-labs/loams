#if canImport(CryptoKit)
@_exported import CryptoKit
#else
@_exported import Crypto
#endif
import Foundation

/// Unpadded base64url (RFC 7515 §2).
public enum Base64URL {
    public static func encode(_ data: Data) -> String {
        data.base64EncodedString()
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }

    public static func decode(_ text: String) -> Data? {
        var s = text.replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
        while s.count % 4 != 0 { s += "=" }
        return Data(base64Encoded: s)
    }
}

extension Data {
    var sha256: Data { Data(SHA256.hash(data: self)) }
    var hex: String { map { String(format: "%02x", $0) }.joined() }
}
