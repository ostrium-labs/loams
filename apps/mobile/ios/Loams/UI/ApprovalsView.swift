import LoamsCore
import LoamsData
import LoamsProto
import SwiftUI

struct ApprovalsView: View {
    @Environment(AppModel.self) private var model
    @State private var selected: String?

    var body: some View {
        VStack(spacing: 0) {
            StreamBanner(status: model.approvalsStatus)
            if model.approvals.isEmpty {
                ContentUnavailableView("Nothing waits for your approval", systemImage: "checkmark.shield")
            } else {
                List {
                    // Grouped by environment, protected ones marked (AP3 Task 6).
                    ForEach(groups) { group in
                        Section(group.env.name + (group.env.protected ? " · protected" : "")) {
                            ForEach(group.items, id: \.id) { a in
                                NavigationLink(value: a.id) { ApprovalRow(approval: a) }
                                    .accessibilityIdentifier("approval-\(a.id)")
                            }
                        }
                    }
                }
            }
        }
        .navigationTitle("Approvals")
        .navigationDestination(for: String.self) { id in
            if let a = model.approvals.first(where: { $0.id == id }) {
                ApprovalDetail(approval: a)
            } else {
                Text("This approval is no longer pending.")
            }
        }
        .task { await model.watchApprovals() }
        .alert(model.message ?? "", isPresented: Binding(get: { model.message != nil }, set: { if !$0 { model.message = nil } })) {
            Button("OK", role: .cancel) {}
        }
    }

    private struct EnvGroup: Identifiable {
        let env: Loams_Instance_V1_Environment
        var items: [Approval]
        var id: String { env.id }
    }

    private var groups: [EnvGroup] {
        var order: [EnvGroup] = []
        for a in model.approvals {
            if let at = order.firstIndex(where: { $0.env.id == a.environment.id }) {
                order[at].items.append(a)
            } else {
                order.append(EnvGroup(env: a.environment, items: [a]))
            }
        }
        return order
    }
}

struct ApprovalRow: View {
    let approval: Approval

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(approval.summary).font(.headline)
            Text(Words.requester(approval)).font(.caption)
            Text("\(Words.risk(approval.risk)) · \(Words.progress(approval)) · \(Words.expires(approval, now: Date()))").font(.caption)
        }
    }
}

struct ApprovalDetail: View {
    @Environment(AppModel.self) private var model
    let approval: Approval
    @State private var typed = ""
    @State private var reason = ""
    @State private var result: String?
    /// Set synchronously on tap, so a second tap cannot start a second decision.
    @State private var inFlight = false

    private var live: Bool { model.approvalsStatus == .live }
    private var busy: Bool { inFlight || model.busyApprovalID == approval.id }
    private func allowed(_ d: Decision) -> Bool {
        !busy && DecisionRules.check(decision: d, pending: approval.state == .pending, destructive: approval.risk == .destructive,
                                     confirmText: approval.confirmText, typed: typed, reason: reason, live: live) == nil
    }

    var body: some View {
        Form {
            Section {
                Text(approval.summary).font(.title3).accessibilityIdentifier("detail-summary")
                Text("\(approval.environment.project) / \(approval.environment.name)\(approval.environment.protected ? " (protected)" : "")")
                Text(Words.requester(approval))
                Text("\(Words.risk(approval.risk)) · \(Words.progress(approval)) · \(Words.expires(approval, now: Date()))")
            }
            Section {
                ForEach(approval.detailLines, id: \.self) { Text($0) }
            }
            Section {
                if approval.risk == .destructive {
                    Text("This cannot be undone. Type \(approval.confirmText) to approve.").foregroundStyle(.red)
                    TextField(approval.confirmText, text: $typed)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .accessibilityIdentifier("confirm-field")
                }
                TextField("Reason (required to reject)", text: $reason).accessibilityIdentifier("reason-field")
                if !live { Text("Offline: decisions need a live connection.").foregroundStyle(.red) }
            }
            Section {
                Button(busy ? "Signing…" : "Approve") { decide(.approve) }
                    .disabled(!allowed(.approve))
                    .accessibilityIdentifier("approve-button")
                Button("Reject", role: .destructive) { decide(.reject) }
                    .disabled(!allowed(.reject))
                    .accessibilityIdentifier("reject-button")
                Text("Approving asks for Face ID, Touch ID or your passcode to sign with this phone's key.").font(.footnote)
            }
            if let result { Section { Text(result) } }
        }
        .navigationTitle(Words.risk(approval.risk))
    }

    private func decide(_ d: Decision) {
        guard !inFlight else { return }
        inFlight = true
        Task {
            result = await model.decide(approval, decision: d, reason: reason, typed: typed)
            inFlight = false
        }
    }
}

enum Words {
    static func requester(_ a: Approval) -> String {
        let chain = a.actorChain
        if chain.count >= 2, chain[0].kind == .agent {
            return "Requested by \(chain[0].displayName), acting for \(chain.dropFirst().map(\.displayName).joined(separator: ", "))"
        }
        return "Requested by \(a.requestedBy.displayName.isEmpty ? a.requestedBy.id : a.requestedBy.displayName)"
    }

    static func risk(_ r: Loams_Approvals_V1_Risk) -> String {
        switch r {
        case .destructive: return "Destructive"
        case .high: return "High risk"
        case .medium: return "Medium risk"
        case .low: return "Low risk"
        default: return "Risk unknown"
        }
    }

    static func progress(_ a: Approval) -> String {
        let have = a.decisions.filter { $0.decision == .approve }.count
        return "\(have) of \(max(1, a.policy.requiredApprovals)) approvals"
    }

    static func expires(_ a: Approval, now: Date) -> String {
        guard a.hasExpiresAt else { return "no expiry" }
        let left = Int(a.expiresAt.date.timeIntervalSince(now))
        if left <= 0 { return "expired" }
        if left < 3600 { return "expires in \(left / 60) min" }
        if left < 86_400 { return "expires in \(left / 3600) h \((left % 3600) / 60) min" }
        return "expires in \(left / 86_400) d"
    }
}
