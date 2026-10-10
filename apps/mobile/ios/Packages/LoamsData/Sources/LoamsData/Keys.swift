import CryptoKit
import Foundation
import LocalAuthentication
import LoamsCore

/// The user could not or did not authenticate; nothing was signed.
public struct SigningUnavailable: Error, LocalizedError {
    public let message: String
    public init(_ message: String) { self.message = message }
    public var errorDescription: String? { message }
}

/// The per-instance decision key (AP3 Ruling 2).
public protocol DecisionKeys: Sendable {
    /// Creates the key if needed and returns its public JWK (sent at pairing as `decision_jwk`).
    func publicJWK(instanceID: String) throws -> Jwk
    /// Asks for Face ID, Touch ID or the passcode, then signs; returns raw r||s (64 bytes).
    func sign(_ data: Data, instanceID: String, reason: String) async throws -> Data
    func delete(instanceID: String)
    var isHardwareBacked: Bool { get }
}

/// Secure Enclave P-256 keys whose every use needs the user: `.biometryCurrentSet` (a new
/// fingerprint or face voids the key) or `.userPresence` when there is no biometry, so the
/// passcode works. The enclave-wrapped key blob is kept in the keychain.
///
/// The simulator has no Secure Enclave: there the key is a software key in the same store, and
/// `isHardwareBacked` is false, so the flow can be exercised. A device always has an enclave.
public struct SecureEnclaveKeys: DecisionKeys {
    let store: SecretStore

    public init(store: SecretStore) {
        self.store = store
    }

    public var isHardwareBacked: Bool { SecureEnclave.isAvailable }

    private func account(_ id: String) -> String { "decide-\(id)" }

    static func accessControl() throws -> SecAccessControl {
        let hasBiometry = LAContext().canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: nil)
        let flags: SecAccessControlCreateFlags = hasBiometry ? [.privateKeyUsage, .biometryCurrentSet] : [.privateKeyUsage, .userPresence]
        guard let ac = SecAccessControlCreateWithFlags(nil, kSecAttrAccessibleWhenUnlockedThisDeviceOnly, flags, nil) else {
            throw SigningUnavailable("Could not create the key's access control.")
        }
        return ac
    }

    public func publicJWK(instanceID: String) throws -> Jwk {
        if SecureEnclave.isAvailable {
            if let blob = store.get(account(instanceID)) {
                return try Jwk.p256(x963: try SecureEnclave.P256.Signing.PrivateKey(dataRepresentation: blob).publicKey.x963Representation)
            }
            do {
                let key = try SecureEnclave.P256.Signing.PrivateKey(accessControl: Self.accessControl())
                try store.set(key.dataRepresentation, for: account(instanceID))
                try store.set(Self.domainState() ?? Data(), for: domainAccount(instanceID))
                return try Jwk.p256(x963: key.publicKey.x963Representation)
            } catch {
                throw SigningUnavailable("Set a passcode on this phone first: the approval key needs one.")
            }
        }
        let key: P256.Signing.PrivateKey
        if let raw = store.get(account(instanceID)) {
            key = try P256.Signing.PrivateKey(rawRepresentation: raw)
        } else {
            key = P256.Signing.PrivateKey()
            try store.set(key.rawRepresentation, for: account(instanceID))
        }
        return try Jwk.p256(x963: key.publicKey.x963Representation)
    }

    private func domainAccount(_ id: String) -> String { "decide-\(id)-domain" }

    /// The biometry enrolment fingerprint: it changes when a face or finger is added or removed.
    static func domainState() -> Data? {
        let context = LAContext()
        guard context.canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: nil) else { return nil }
        return context.evaluatedPolicyDomainState
    }

    public func sign(_ data: Data, instanceID: String, reason: String) async throws -> Data {
        guard let blob = store.get(account(instanceID)) else {
            throw SigningUnavailable("This phone has no approval key for the instance. Pair again.")
        }
        let context = LAContext()
        do {
            _ = try await context.evaluatePolicy(.deviceOwnerAuthentication, localizedReason: reason)
        } catch {
            throw SigningUnavailable("Not signed: \(error.localizedDescription)")
        }
        if SecureEnclave.isAvailable {
            do {
                let key = try SecureEnclave.P256.Signing.PrivateKey(dataRepresentation: blob, authenticationContext: context)
                return try key.signature(for: data).rawRepresentation
            } catch {
                // Only an enrolment change voids the key for good (.biometryCurrentSet); anything
                // else, such as an invalidated authentication context, is worth another try.
                let saved = store.get(domainAccount(instanceID))
                if let saved, !saved.isEmpty, let now = Self.domainState(), now != saved {
                    delete(instanceID: instanceID)
                    throw SigningUnavailable("Your Face ID or Touch ID changed, so this phone's approval key was destroyed. Pair again.")
                }
                throw SigningUnavailable("Not signed: \(error.localizedDescription). Try again.")
            }
        }
        return try P256.Signing.PrivateKey(rawRepresentation: blob).signature(for: data).rawRepresentation
    }

    public func delete(instanceID: String) {
        store.remove(account(instanceID))
        store.remove(domainAccount(instanceID))
    }
}

/// Signs without asking anyone; tests only.
public struct TestDecisionKeys: DecisionKeys {
    private let key = P256.Signing.PrivateKey()
    public init() {}
    public var isHardwareBacked: Bool { false }
    public func publicJWK(instanceID: String) throws -> Jwk { try Jwk.p256(x963: key.publicKey.x963Representation) }
    public func sign(_ data: Data, instanceID: String, reason: String) async throws -> Data { try key.signature(for: data).rawRepresentation }
    public func delete(instanceID: String) {}
}
