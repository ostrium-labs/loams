import AuthenticationServices
import Foundation
import LoamsCore
import LoamsData
import Observation
import UIKit

/// App state for every screen: the session (paired, demo or signed out), the live lists and the
/// actions. Same flows as Android's SessionManager and ViewModels.
@MainActor
@Observable
final class AppModel {
    enum Phase: Equatable { case loading, signedOut, active }

    var phase: Phase = .loading
    private(set) var backend: (any Backend)?
    private(set) var record: SessionRecord?
    var approvals: [Approval] = []
    var approvalsStatus: StreamStatus = .connecting
    var operations: [LoamsOperation] = []
    var operationsStatus: StreamStatus = .connecting
    var instance: InstanceSummary?
    var message: String?
    var busy = false
    var busyApprovalID: String?
    var pendingApprovalID: String?
    /// Typed-code pairing waiting for the fingerprint comparison.
    var confirm: (issuer: String, instanceID: String, thumbprint: String)?

    var serverURL: String {
        didSet { UserDefaults.standard.set(serverURL, forKey: "loams.server") }
    }

    #if DEBUG
    static let allowInsecureLoopback = true
    #else
    static let allowInsecureLoopback = false
    #endif

    private let keys: DecisionKeys
    private let pushKeys: PushKeys
    private let sessions: SessionStore
    private let accessToken = TokenBox()
    private var attempts: [String: String] = [:]
    private let webAuth = WebAuth()

    init(secrets: SecretStore = KeychainStore.shared, keys: DecisionKeys? = nil) {
        self.keys = keys ?? SecureEnclaveKeys(store: secrets)
        pushKeys = PushKeys(store: secrets)
        sessions = SessionStore(secrets: secrets)
        serverURL = UserDefaults.standard.string(forKey: "loams.server") ?? (Self.allowInsecureLoopback ? "http://localhost:8084" : "")
    }

    private var deviceName: String { UIDevice.current.name }
    private var appVersion: String { Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "" }

    // MARK: Session

    func restore() async {
        guard let record = sessions.load(), let refresh = sessions.refreshToken() else {
            phase = .signedOut
            return
        }
        do {
            switch try await TokenEndpoint(issuer: record.issuer).refresh(refresh) {
            case .ok(let tokens):
                try sessions.saveRefreshToken(tokens.refreshToken)
                activate(record, tokens: tokens)
            case .refused:
                await signOut()
            }
        } catch {
            // Offline: show the instance read-only; streams retry and decisions stay disabled.
            activate(record, tokens: nil)
        }
    }

    func startDemo() {
        activate(backend: DemoBackend(), record: nil)
    }

    /// Used by tests and the demo.
    func activate(backend: any Backend, record: SessionRecord?) {
        self.backend = backend
        self.record = record
        approvals = []
        operations = []
        phase = .active
    }

    private func activate(_ record: SessionRecord, tokens: DeviceTokens?) {
        accessToken.value = tokens?.accessToken
        let box = accessToken
        guard let clients = try? LoamsClients(baseURL: record.issuer, allowInsecureLoopback: Self.allowInsecureLoopback, token: { box.value }) else {
            message = "The stored instance address is not https; pair again."
            phase = .signedOut
            return
        }
        activate(backend: RemoteBackend(clients: clients, instanceID: record.instanceID, deviceID: record.deviceID), record: record)
    }

    func signOut() async {
        if let r = record ?? sessions.load() {
            keys.delete(instanceID: r.instanceID)
            pushKeys.delete(instanceID: r.instanceID)
        }
        sessions.clear()
        accessToken.value = nil
        backend = nil
        record = nil
        phase = .signedOut
    }

    // MARK: Pairing (design §37 §7.2.2)

    func pair(payloadText: String) async {
        busy = true
        defer { busy = false }
        let payload: PairingPayload
        switch PairingPayloads.parse(payloadText, now: Date(), allowInsecureLoopback: Self.allowInsecureLoopback) {
        case .success(let p): payload = p
        case .failure(let e): message = Self.text(for: e); return
        }
        do {
            // TODO(AP3 Task 3): pin payload.spki in the transport; checked here only by id + jkt.
            let issuer = payload.issuer.absoluteString.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
            let clients = try LoamsClients(baseURL: issuer, allowInsecureLoopback: Self.allowInsecureLoopback, token: { nil })
            let info = try await clients.instance.getInstance(request: .init(), headers: clients.headers).get()
            guard info.instanceID == payload.instanceID else {
                message = "This server is a different Loams instance than the code you scanned."
                return
            }
            guard try await InstanceCheck.thumbprints(jwksURI: info.jwksUri).contains(payload.jkt) else {
                message = "This server's identity changed: its signing key is not the one the code named."
                return
            }
            let jwk = try keys.publicJWK(instanceID: payload.instanceID).json()
            let result = try await TokenEndpoint(issuer: issuer).redeemPairing(
                code: payload.code, userCode: nil, device: DeviceRegistration(deviceName: deviceName, decisionJWK: jwk, model: "iPhone", appVersion: appVersion))
            try finish(result, record: { SessionRecord(issuer: issuer, instanceID: payload.instanceID, jkt: payload.jkt, spki: payload.spki, deviceID: $0, method: "qr") })
        } catch {
            message = error.localizedDescription
        }
    }

    /// Debug builds only: ask the local mock for a fresh payload, as the console would show.
    func pairWithMock() async {
        guard let url = URL(string: serverURL + "/mock/pairing") else { return }
        do {
            let (data, _) = try await URLSession.shared.data(from: url)
            await pair(payloadText: String(decoding: data, as: UTF8.self))
        } catch {
            message = "Could not reach the mock at \(serverURL): \(error.localizedDescription)"
        }
    }

    /// Typed code, step 1: nothing is pinned, so the user compares the key fingerprint.
    func startTyped() async {
        guard let url = URL(string: serverURL), PairingPayloads.issuerAllowed(url, allowInsecureLoopback: Self.allowInsecureLoopback) else {
            message = "Enter the https address of your Loams instance."
            return
        }
        busy = true
        defer { busy = false }
        do {
            let clients = try LoamsClients(baseURL: serverURL, allowInsecureLoopback: Self.allowInsecureLoopback, token: { nil })
            let info = try await clients.instance.getInstance(request: .init(), headers: clients.headers).get()
            guard let thumb = try await InstanceCheck.thumbprints(jwksURI: info.jwksUri).first else {
                message = "This server publishes no instance key."
                return
            }
            confirm = (serverURL, info.instanceID, thumb)
        } catch {
            message = error.localizedDescription
        }
    }

    func finishTyped(userCode: String) async {
        guard let c = confirm else { return }
        confirm = nil
        guard userCode.count == 8, userCode.allSatisfy(\.isNumber) else {
            message = "The code is 8 digits."
            return
        }
        do {
            let jwk = try keys.publicJWK(instanceID: c.instanceID).json()
            let result = try await TokenEndpoint(issuer: c.issuer).redeemPairing(
                code: nil, userCode: userCode, device: DeviceRegistration(deviceName: deviceName, decisionJWK: jwk, model: "iPhone", appVersion: appVersion))
            try finish(result, record: { SessionRecord(issuer: c.issuer, instanceID: c.instanceID, jkt: c.thumbprint, spki: nil, deviceID: $0, method: "typed") })
        } catch {
            message = error.localizedDescription
        }
    }

    /// Browser sign-in at Authentik with PKCE in ASWebAuthenticationSession, then the RFC 8693
    /// exchange at the gateway (path 2). Against the mock, its fake Authentik redirects back.
    func signInWithBrowser() async {
        busy = true
        defer { busy = false }
        do {
            let clients = try LoamsClients(baseURL: serverURL, allowInsecureLoopback: Self.allowInsecureLoopback, token: { nil })
            let info = try await clients.instance.getInstance(request: .init(), headers: clients.headers).get()
            guard info.hasIdentityProvider else {
                message = "This instance has no browser sign-in configured."
                return
            }
            // Trust on first use: the key seen now is recorded and checked on later pairings.
            guard let jkt = try await InstanceCheck.thumbprints(jwksURI: info.jwksUri).first else {
                message = "This server publishes no instance key."
                return
            }
            let token = try await webAuth.signIn(idpIssuer: info.identityProvider.issuer, clientID: info.identityProvider.iosClientID.isEmpty ? Loams.clientID : info.identityProvider.iosClientID)
            let jwk = try keys.publicJWK(instanceID: info.instanceID).json()
            let result = try await TokenEndpoint(issuer: serverURL).exchange(
                authentikAccessToken: token, device: DeviceRegistration(deviceName: deviceName, decisionJWK: jwk, model: "iPhone", appVersion: appVersion))
            let issuer = serverURL
            try finish(result, record: { SessionRecord(issuer: issuer, instanceID: info.instanceID, jkt: jkt, spki: nil, deviceID: $0, method: "browser") })
        } catch {
            message = "Sign-in failed: \(error.localizedDescription)"
        }
    }

    private func finish(_ result: TokenResult, record: (String) -> SessionRecord) throws {
        switch result {
        case .ok(let tokens):
            let r = record(tokens.deviceID)
            try sessions.save(r, refreshToken: tokens.refreshToken)
            message = nil
            activate(r, tokens: tokens)
            Task { await registerPush() }
        case .refused(_, let reason, let description):
            message = Self.text(for: reason, description)
        }
    }

    // MARK: Live data (streams run while a screen shows them)

    func watchApprovals() async {
        guard let backend else { return }
        await backend.watchApprovals { [weak self] items, status in
            await MainActor.run {
                self?.approvals = items
                self?.approvalsStatus = status
            }
        }
    }

    func watchOperations() async {
        guard let backend else { return }
        await backend.watchOperations { [weak self] items, status in
            await MainActor.run {
                self?.operations = items
                self?.operationsStatus = status
            }
        }
    }

    func refreshInstance() async {
        instance = try? await backend?.instance()
    }

    // MARK: Decisions (design §37 §7.3)

    /// Checks, signs with the decision key (Face ID, Touch ID or passcode) and sends once.
    func decide(_ approval: Approval, decision: Decision, reason: String, typed: String) async -> String {
        guard let backend else { return "Not signed in." }
        if let problem = DecisionRules.check(
            decision: decision, pending: approval.state == .pending, destructive: approval.risk == .destructive,
            confirmText: approval.confirmText, typed: typed, reason: reason, live: approvalsStatus == .live
        ) {
            return Self.text(for: problem, approval)
        }
        busyApprovalID = approval.id
        defer { busyApprovalID = nil }
        let claims = DecisionClaims(approvalID: approval.id, revision: approval.revision, decision: decision, iat: Int64(Date().timeIntervalSince1970), jti: UUID().uuidString)
        let input = Jws.signingInput(payload: claims.canonicalJSON())
        let verb = decision == .approve ? "Approve" : "Reject"
        let raw: Data
        do {
            raw = try await keys.sign(Data(input.utf8), instanceID: backend.instanceID, reason: "\(verb) \(approval.kind) in \(approval.environment.name)")
        } catch {
            return error.localizedDescription
        }
        let key = "\(approval.id):\(approval.revision):\(decision.rawValue)"
        let idem = attempts[key] ?? UUID().uuidString
        attempts[key] = idem
        switch await backend.decide(approval, decision: decision, reason: reason.trimmingCharacters(in: .whitespaces), proof: Jws.compact(signingInput: input, rawSignature: raw), idempotencyKey: idem) {
        case .decided:
            attempts[key] = nil
            approvals.removeAll { $0.id == approval.id }
            return "\(decision == .approve ? "Approved" : "Rejected"): \(approval.summary)"
        case .notSent(let m):
            return "\(m). Tap \(verb.lowercased()) again to retry; it will not be applied twice."
        case .refused(let reason, let m):
            switch reason {
            case .approvalAlreadyDecided: return "Someone already decided this approval."
            case .approvalExpired: return "This approval expired."
            case .stepUpRequired, .decisionProofInvalid: return "The server did not accept this phone's signature. Pair the phone again."
            default: return "Refused: \(m)"
            }
        }
    }

    func cancel(_ op: LoamsOperation) async {
        message = await backend?.cancel(operationID: op.id, idempotencyKey: UUID().uuidString)
    }

    // MARK: Push (APNs stubbed)

    func registerPush() async {
        guard let backend else { return }
        do {
            let id = record?.instanceID ?? backend.instanceID
            let target = try await backend.registerPush(token: ApnsStub.token(), hpkePublicKey: pushKeys.ensure(instanceID: id).rawRepresentation)
            message = "Registered push target \(target)."
        } catch {
            message = "Push registration failed: \(error.localizedDescription)"
        }
    }

    func sendTestNotification() async {
        do {
            let id = try await backend?.sendTestNotification() ?? ""
            message = "Test notification \(id) sent."
        } catch {
            message = "Not sent: \(error.localizedDescription)"
        }
    }

    // MARK: Words

    static func text(for e: PairingError) -> String {
        switch e {
        case .unsupportedVersion: return "This pairing code needs a newer version of the app."
        case .wrongKind, .malformed: return "That is not a Loams pairing code."
        case .expired: return "This pairing code expired. Create a new one in the console."
        case .insecureIssuer: return "This pairing code points to a server without https."
        }
    }

    static func text(for r: Reason, _ description: String?) -> String {
        switch r {
        case .pairingExpired: return "This pairing code expired. Create a new one in the console."
        case .pairingUsed: return "This pairing code was already used. Create a new one in the console."
        case .deviceRevoked: return "This device was revoked."
        default: return description ?? "The server refused the pairing."
        }
    }

    static func text(for p: DecisionProblem, _ a: Approval) -> String {
        switch p {
        case .reasonRequired: return "Say why you are rejecting it."
        case .confirmationMismatch: return "Type \(a.confirmText) to confirm."
        case .offline: return "Offline: decisions need a live connection and are never queued."
        case .notPending: return "This approval is no longer pending."
        }
    }
}
