import Connect
import SwiftProtobuf
import Foundation
import LoamsCore
import LoamsProto

public typealias Approval = Loams_Approvals_V1_Approval
public typealias LoamsOperation = Loams_Operations_V1_Operation

public struct InstanceSummary: Sendable, Equatable {
    public let issuer: String
    public let instanceID: String
    public let serverVersion: String
    public let edition: String
    public let apiVersions: [String]
    public let deviceID: String
    public let trust: String
}

public enum DecideOutcome: Sendable, Equatable {
    case decided(Approval)
    /// The server refused; `reason` is the stable ErrorInfo reason.
    case refused(Reason, String)
    /// It may not have reached the server; Retry reuses the idempotency key.
    case notSent(String)
}

/// Everything the screens need from an instance: [RemoteBackend] over Connect, [DemoBackend]
/// in memory (demo mode, previews, tests). Same shape as Android's `Backend`.
public protocol Backend: Sendable {
    var instanceID: String { get }
    var isDemo: Bool { get }
    /// Holds the stream open until the calling task is cancelled, reporting every change.
    func watchApprovals(_ onChange: @escaping @Sendable ([Approval], StreamStatus) async -> Void) async
    func watchOperations(_ onChange: @escaping @Sendable ([LoamsOperation], StreamStatus) async -> Void) async
    func instance() async throws -> InstanceSummary
    func decide(_ approval: Approval, decision: Decision, reason: String, proof: String, idempotencyKey: String) async -> DecideOutcome
    func cancel(operationID: String, idempotencyKey: String) async -> String?
    func registerPush(token: String, hpkePublicKey: Data) async throws -> String
    func sendTestNotification() async throws -> String
}

public final class RemoteBackend: Backend {
    public let clients: LoamsClients
    public let instanceID: String
    public let deviceID: String
    public var isDemo: Bool { false }

    public init(clients: LoamsClients, instanceID: String, deviceID: String) {
        self.clients = clients
        self.instanceID = instanceID
        self.deviceID = deviceID
    }

    private static let fatal: @Sendable (Error) -> Bool = { ($0 as? ConnectError)?.code == .unauthenticated }

    public func watchApprovals(_ onChange: @escaping @Sendable ([Approval], StreamStatus) async -> Void) async {
        let clients = self.clients
        let watch = ResumingWatch<Approval>(
            state: WatchState(id: { $0.id }),
            isFatal: Self.fatal,
            open: { clients.watchApprovals(cursor: $0) },
            observer: { state, status in await onChange(state.items, status) }
        )
        await watch.run()
    }

    public func watchOperations(_ onChange: @escaping @Sendable ([LoamsOperation], StreamStatus) async -> Void) async {
        let clients = self.clients
        let watch = ResumingWatch<LoamsOperation>(
            state: WatchState(id: { $0.id }),
            isFatal: Self.fatal,
            open: { clients.watchOperations(cursor: $0) },
            observer: { state, status in await onChange(state.items, status) }
        )
        await watch.run()
    }

    public func instance() async throws -> InstanceSummary {
        let i = try await clients.instance.getInstance(request: Loams_Instance_V1_GetInstanceRequest(), headers: clients.headers).get()
        return InstanceSummary(
            issuer: clients.baseURL, instanceID: i.instanceID, serverVersion: i.serverVersion,
            edition: String(describing: i.edition), apiVersions: i.apiVersions, deviceID: deviceID,
            trust: "System CAs + instance key"
        )
    }

    public func decide(_ approval: Approval, decision: Decision, reason: String, proof: String, idempotencyKey: String) async -> DecideOutcome {
        var req = Loams_Approvals_V1_DecideApprovalRequest()
        req.approvalID = approval.id
        req.revision = approval.revision
        req.decision = decision == .approve ? .approve : .reject
        req.reason = reason
        req.decisionProof = proof
        req.idempotencyKey = idempotencyKey
        let response = await clients.approvals.decideApproval(request: req, headers: clients.headers)
        switch response.result {
        case .success(let r):
            return .decided(r.approval)
        case .failure(let e):
            if e.code == .unavailable || e.code == .deadlineExceeded || e.exception is URLError {
                return .notSent("Not sent: \(e.message ?? "network error")")
            }
            return .refused(e.reason, e.message ?? "\(e.code)")
        }
    }

    public func cancel(operationID: String, idempotencyKey: String) async -> String? {
        var req = Loams_Operations_V1_CancelOperationRequest()
        req.operationID = operationID
        req.idempotencyKey = idempotencyKey
        let r = await clients.operations.cancelOperation(request: req, headers: clients.headers)
        return r.error.map { $0.message ?? "\($0.code)" }
    }

    public func registerPush(token: String, hpkePublicKey: Data) async throws -> String {
        var req = Loams_Devices_V1_RegisterPushTargetRequest()
        req.provider = .apns
        req.tokenOrEndpoint = token
        req.appID = Loams.bundleID
        #if DEBUG
        req.environment = .sandbox
        #else
        req.environment = .production
        #endif
        req.hpkePublicKey = hpkePublicKey
        req.idempotencyKey = UUID().uuidString
        return try await clients.devices.registerPushTarget(request: req, headers: clients.headers).get().pushTargetID
    }

    public func sendTestNotification() async throws -> String {
        var req = Loams_Devices_V1_SendTestNotificationRequest()
        req.idempotencyKey = UUID().uuidString
        return try await clients.devices.sendTestNotification(request: req, headers: clients.headers).get().notificationID
    }
}

/// In-memory data with no network. Decisions still go through the real signing path.
public actor DemoBackend: Backend {
    public nonisolated let instanceID = "demo"
    public nonisolated var isDemo: Bool { true }

    private var approvals: [Approval]
    private var operations: [LoamsOperation]
    private var approvalWatchers: [UUID: @Sendable ([Approval], StreamStatus) async -> Void] = [:]

    public init(now: Date = Date()) {
        approvals = DemoData.approvals(now: now)
        operations = DemoData.operations()
    }

    public func currentApprovals() -> [Approval] { approvals }

    public func watchApprovals(_ onChange: @escaping @Sendable ([Approval], StreamStatus) async -> Void) async {
        let id = UUID()
        approvalWatchers[id] = onChange
        await onChange(approvals, .live)
        while !Task.isCancelled {
            try? await Task.sleep(nanoseconds: 1_000_000_000)
        }
        approvalWatchers[id] = nil
    }

    public func watchOperations(_ onChange: @escaping @Sendable ([LoamsOperation], StreamStatus) async -> Void) async {
        await onChange(operations, .live)
        while !Task.isCancelled {
            try? await Task.sleep(nanoseconds: 2_000_000_000)
            operations = operations.map { op in
                guard op.state == .running else { return op }
                var next = op
                next.progress.done = min(op.progress.total, op.progress.done + 5)
                next.progress.fraction = Double(next.progress.done) / Double(max(1, op.progress.total))
                if next.progress.done >= next.progress.total { next.state = .succeeded }
                return next
            }
            await onChange(operations, .live)
        }
    }

    public func instance() async throws -> InstanceSummary {
        InstanceSummary(issuer: "demo (no network)", instanceID: instanceID, serverVersion: "demo", edition: "OSS",
                        apiVersions: ["loams.approvals.v1", "loams.operations.v1"], deviceID: "demo-device", trust: "none: in-memory demo data")
    }

    public func decide(_ approval: Approval, decision: Decision, reason: String, proof: String, idempotencyKey: String) async -> DecideOutcome {
        guard let current = approvals.first(where: { $0.id == approval.id }) else { return .refused(.approvalAlreadyDecided, "already decided") }
        guard current.revision == approval.revision else { return .refused(.unknown, "the approval changed; refresh it") }
        var decided = current
        decided.state = decision == .approve ? .approved : .rejected
        decided.revision += 1
        approvals.removeAll { $0.id == approval.id }
        for watcher in approvalWatchers.values { await watcher(approvals, .live) }
        return .decided(decided)
    }

    public func cancel(operationID: String, idempotencyKey: String) async -> String? {
        operations = operations.map { op in
            var o = op
            if o.id == operationID { o.state = .canceled }
            return o
        }
        return nil
    }

    public func registerPush(token: String, hpkePublicKey: Data) async throws -> String { "demo-push-target" }

    public func sendTestNotification() async throws -> String { throw SigningUnavailable("Demo mode has no server.") }
}

public enum DemoData {
    public static func approvals(now: Date) -> [Approval] {
        func ts(_ offset: TimeInterval) -> SwiftProtobuf.Google_Protobuf_Timestamp { .init(date: now.addingTimeInterval(offset)) }
        var bob = Loams_Instance_V1_Principal()
        bob.id = "usr_bob"; bob.kind = .user; bob.displayName = "Bob Example"
        var agent = Loams_Instance_V1_Principal()
        agent.id = "agt_reindexer"; agent.kind = .agent; agent.displayName = "reindex-agent"
        var prod = Loams_Instance_V1_Environment()
        prod.id = "env_prod"; prod.project = "search"; prod.name = "production"; prod.namespace = "prod"; prod.protected = true
        var staging = Loams_Instance_V1_Environment()
        staging.id = "env_staging"; staging.project = "search"; staging.name = "staging"; staging.namespace = "staging"
        var one = Loams_Approvals_V1_ApprovalPolicy()
        one.requiredApprovals = 1

        var drop = Approval()
        drop.id = "apr_drop_logs"; drop.revision = 1; drop.kind = "collection.drop"; drop.environment = prod; drop.requestedBy = bob
        drop.summary = "Drop collection logs-2026 in production"
        drop.detailLines = ["Namespace: prod", "Collection: logs-2026", "4.2 million documents, 18.3 GiB", "This cannot be undone."]
        drop.risk = .destructive; drop.policy = one; drop.state = .pending
        drop.createdAt = ts(-20 * 60); drop.expiresAt = ts(72 * 3600); drop.confirmText = "logs-2026"

        var reindex = Approval()
        reindex.id = "apr_agent_reindex"; reindex.revision = 1; reindex.kind = "agent.action"; reindex.environment = staging
        reindex.requestedBy = agent; reindex.actorChain = [agent, bob]
        reindex.summary = "reindex-agent wants to rebuild the products index in staging"
        reindex.detailLines = ["Acting for Bob Example", "Tool: collections.reindex", "Estimated 6 minutes"]
        reindex.risk = .medium; reindex.policy = one; reindex.state = .pending
        reindex.createdAt = ts(-5 * 60); reindex.expiresAt = ts(10 * 60)
        return [drop, reindex]
    }

    public static func operations() -> [LoamsOperation] {
        var reindex = LoamsOperation()
        reindex.id = "op_reindex"; reindex.kind = "collection.reindex"; reindex.namespace = "staging"; reindex.state = .running
        reindex.progress.total = 100; reindex.progress.done = 35; reindex.progress.fraction = 0.35; reindex.progress.message = "Rebuilding segments"
        var drop = LoamsOperation()
        drop.id = "op_drop_logs"; drop.kind = "collection.drop"; drop.namespace = "prod"; drop.state = .awaitingApproval; drop.approvalID = "apr_drop_logs"
        var backup = LoamsOperation()
        backup.id = "op_backup"; backup.kind = "namespace.backup"; backup.namespace = "prod"; backup.state = .succeeded
        return [reindex, drop, backup]
    }
}
