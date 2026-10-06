import Foundation

/// Sealed push payloads (design §37 §7.4): HPKE base mode, DHKEM(X25519, HKDF-SHA256),
/// HKDF-SHA256, ChaCha20-Poly1305. The instance and notification ids are bound through the HPKE
/// `info` ("loams-push-v1" 0x00 instance_id 0x00 notification_id) with empty associated data,
/// as on Android and in the mock. Sealed bytes are enc (32) || ciphertext.
public enum Unsealer {
    public static let suite = HPKE.Ciphersuite.Curve25519_SHA256_ChachaPoly

    public static func info(instanceID: String, notificationID: String) -> Data {
        var d = Data("loams-push-v1".utf8)
        d.append(0)
        d.append(contentsOf: Array(instanceID.utf8))
        d.append(0)
        d.append(contentsOf: Array(notificationID.utf8))
        return d
    }

    public static func open(_ sealed: Data, instanceID: String, notificationID: String, privateKey: Curve25519.KeyAgreement.PrivateKey) throws -> Data {
        guard sealed.count > 32 + 16 else { throw CryptoKitError.incorrectParameterSize }
        let enc = sealed.prefix(32)
        let ct = sealed.dropFirst(32)
        var recipient = try HPKE.Recipient(privateKey: privateKey, ciphersuite: suite, info: info(instanceID: instanceID, notificationID: notificationID), encapsulatedKey: Data(enc))
        return try recipient.open(Data(ct), authenticating: Data())
    }

    /// Seals like the server; for tests and the demo.
    public static func seal(_ plaintext: Data, instanceID: String, notificationID: String, publicKey: Curve25519.KeyAgreement.PublicKey) throws -> Data {
        var sender = try HPKE.Sender(recipientKey: publicKey, ciphersuite: suite, info: info(instanceID: instanceID, notificationID: notificationID))
        let ct = try sender.seal(plaintext, authenticating: Data())
        return sender.encapsulatedKey + ct
    }
}

/// The data an APNs push carries besides `aps`: `{"v":1,"i":…,"n":…,"s":base64}`.
public struct PushMessage: Sendable, Equatable {
    public let instanceID: String
    public let notificationID: String
    public let sealed: Data

    public static let fields: Set<String> = ["v", "i", "n", "s"]

    /// Accepts string or number `v` (APNs JSON keeps numbers; FCM data maps are strings).
    public static func parse(_ payload: [AnyHashable: Any]) -> PushMessage? {
        let v = (payload["v"] as? String) ?? (payload["v"] as? Int).map(String.init)
        guard v == "1",
              let i = payload["i"] as? String, !i.isEmpty,
              let n = payload["n"] as? String, !n.isEmpty,
              let s = payload["s"] as? String, let sealed = Data(base64Encoded: s)
        else { return nil }
        return PushMessage(instanceID: i, notificationID: n, sealed: sealed)
    }
}

/// What a notification shows.
public struct Shown: Sendable, Equatable {
    public let title: String
    public let body: String
    public let notificationID: String?
    public let approvalID: String?

    public init(title: String, body: String, notificationID: String?, approvalID: String?) {
        self.title = title
        self.body = body
        self.notificationID = notificationID
        self.approvalID = approvalID
    }

    /// Shown whenever a payload cannot be opened; Apple sees only this generic text anyway.
    public static let generic = Shown(title: "Loams", body: "New activity in Loams", notificationID: nil, approvalID: nil)
}

/// Opens a push or falls back to the generic text: the NSE fails closed (AP3 Review Focus 3).
public struct PushOpener: Sendable {
    let keyFor: @Sendable (String) -> Curve25519.KeyAgreement.PrivateKey?
    let decode: @Sendable (Data) throws -> Shown

    public init(keyFor: @escaping @Sendable (String) -> Curve25519.KeyAgreement.PrivateKey?, decode: @escaping @Sendable (Data) throws -> Shown) {
        self.keyFor = keyFor
        self.decode = decode
    }

    public func open(_ payload: [AnyHashable: Any]) -> Shown {
        guard let m = PushMessage.parse(payload), let key = keyFor(m.instanceID),
              let plain = try? Unsealer.open(m.sealed, instanceID: m.instanceID, notificationID: m.notificationID, privateKey: key),
              let shown = try? decode(plain)
        else { return .generic }
        return shown
    }
}
