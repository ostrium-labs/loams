import Foundation

/// What survives a restart about the paired instance. Not secret: kept in UserDefaults. The
/// refresh token is in the keychain.
public struct SessionRecord: Codable, Sendable, Equatable {
    public var issuer: String
    public var instanceID: String
    public var jkt: String
    public var spki: [String]?
    public var deviceID: String
    public var method: String

    public init(issuer: String, instanceID: String, jkt: String, spki: [String]?, deviceID: String, method: String) {
        self.issuer = issuer
        self.instanceID = instanceID
        self.jkt = jkt
        self.spki = spki
        self.deviceID = deviceID
        self.method = method
    }
}

public struct SessionStore: Sendable {
    let secrets: SecretStore
    let defaultsKey = "loams.session"

    public init(secrets: SecretStore) {
        self.secrets = secrets
    }

    public func load() -> SessionRecord? {
        guard let data = UserDefaults.standard.data(forKey: defaultsKey) else { return nil }
        return try? JSONDecoder().decode(SessionRecord.self, from: data)
    }

    public func refreshToken() -> String? { secrets.get("refresh").map { String(decoding: $0, as: UTF8.self) } }

    public func save(_ record: SessionRecord, refreshToken: String) throws {
        UserDefaults.standard.set(try JSONEncoder().encode(record), forKey: defaultsKey)
        try secrets.set(Data(refreshToken.utf8), for: "refresh")
    }

    public func saveRefreshToken(_ token: String) throws {
        try secrets.set(Data(token.utf8), for: "refresh")
    }

    public func clear() {
        UserDefaults.standard.removeObject(forKey: defaultsKey)
        secrets.remove("refresh")
    }
}
