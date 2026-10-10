import CryptoKit
import Foundation
import LoamsCore
import LoamsProto

/// Per-instance X25519 HPKE keys for sealed pushes (AP3 Task 8). The private key lives in the
/// keychain, in the access group shared with the Notification Service Extension.
public struct PushKeys: Sendable {
    let store: SecretStore

    public init(store: SecretStore) {
        self.store = store
    }

    public func ensure(instanceID: String) throws -> Curve25519.KeyAgreement.PublicKey {
        if let key = privateKey(instanceID: instanceID) { return key.publicKey }
        let key = Curve25519.KeyAgreement.PrivateKey()
        try store.set(key.rawRepresentation, for: "push-\(instanceID)")
        return key.publicKey
    }

    public func privateKey(instanceID: String) -> Curve25519.KeyAgreement.PrivateKey? {
        store.get("push-\(instanceID)").flatMap { try? Curve25519.KeyAgreement.PrivateKey(rawRepresentation: $0) }
    }

    public func delete(instanceID: String) {
        store.remove("push-\(instanceID)")
    }
}

/// Stub for APNs: a stable fake device token, so registration, sealing and display run against
/// the mock. The real token comes from `didRegisterForRemoteNotificationsWithDeviceToken`, which
/// needs the `aps-environment` entitlement and a signed build (TODO(Q420)).
public enum ApnsStub {
    public static func token(defaults: UserDefaults = .standard) -> String {
        if let t = defaults.string(forKey: "loams.apns.stub") { return t }
        let t = "stub-apns-" + UUID().uuidString
        defaults.set(t, forKey: "loams.apns.stub")
        return t
    }
}

/// Decodes an unsealed payload (a serialized loams.notifications.v1.Notification).
public enum NotificationDecoding {
    public static func shown(_ plaintext: Data) throws -> Shown {
        let n = try Loams_Notifications_V1_Notification(serializedBytes: plaintext)
        let approval: String? = {
            if case .approvalID(let id)? = n.ref { return id }
            return nil
        }()
        return Shown(title: n.title.isEmpty ? "Loams" : n.title, body: n.body, notificationID: n.id, approvalID: approval)
    }

    /// The notification category, and with it the REVIEW action for approvals.
    public static func category(_ plaintext: Data) -> Loams.Category {
        guard let n = try? Loams_Notifications_V1_Notification(serializedBytes: plaintext) else { return .operation }
        switch n.category {
        case .approvals: return .approval
        case .security: return .security
        case .jobs, .runs: return .job
        default: return .operation
        }
    }
}
