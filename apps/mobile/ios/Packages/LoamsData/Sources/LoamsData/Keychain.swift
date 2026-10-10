import Foundation
import Security

/// Where secrets live. The keychain in the app; memory in tests (an unsigned test host has no
/// keychain entitlement).
public protocol SecretStore: Sendable {
    func set(_ data: Data, for key: String) throws
    func get(_ key: String) -> Data?
    func remove(_ key: String)
}

/// Generic-password items, this device only, never synchronised to iCloud (AP3 constraints).
/// `AfterFirstUnlockThisDeviceOnly` so the Notification Service Extension can read push keys
/// while the phone is locked after first unlock.
public struct KeychainStore: SecretStore {
    let service: String
    let accessGroup: String?

    /// The store the app and the Notification Service Extension share. TODO(Q420): the access
    /// group `<TEAM>.dev.loams.app.shared` once the team id exists; until then each process has
    /// its own default group, so the extension cannot read push keys and shows the generic text.
    public static let shared = KeychainStore(accessGroup: nil)

    public init(service: String = "dev.loams.app", accessGroup: String? = nil) {
        self.service = service
        self.accessGroup = accessGroup
    }

    private func base(_ key: String) -> [CFString: Any] {
        var q: [CFString: Any] = [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: service,
            kSecAttrAccount: key,
            kSecAttrSynchronizable: false,
        ]
        if let accessGroup { q[kSecAttrAccessGroup] = accessGroup }
        return q
    }

    public func set(_ data: Data, for key: String) throws {
        remove(key)
        var q = base(key)
        q[kSecValueData] = data
        q[kSecAttrAccessible] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        let status = SecItemAdd(q as CFDictionary, nil)
        guard status == errSecSuccess else { throw NSError(domain: NSOSStatusErrorDomain, code: Int(status)) }
    }

    public func get(_ key: String) -> Data? {
        var q = base(key)
        q[kSecReturnData] = true
        q[kSecMatchLimit] = kSecMatchLimitOne
        var out: CFTypeRef?
        guard SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess else { return nil }
        return out as? Data
    }

    public func remove(_ key: String) {
        SecItemDelete(base(key) as CFDictionary)
    }
}

public final class MemoryStore: SecretStore, @unchecked Sendable {
    private var items: [String: Data] = [:]
    private let lock = NSLock()

    public init() {}

    public func set(_ data: Data, for key: String) throws {
        lock.lock()
        defer { lock.unlock() }
        items[key] = data
    }

    public func get(_ key: String) -> Data? {
        lock.lock()
        defer { lock.unlock() }
        return items[key]
    }

    public func remove(_ key: String) {
        lock.lock()
        defer { lock.unlock() }
        items[key] = nil
    }
}
