import LoamsCore
import LoamsData
import SwiftUI

/// The instance and its operations, live over the `WatchOperations` server stream.
struct StatusView: View {
    @Environment(AppModel.self) private var model
    @State private var confirmCancel: LoamsOperation?

    var body: some View {
        VStack(spacing: 0) {
            StreamBanner(status: model.operationsStatus)
            List {
                Section("Instance") {
                    if let i = model.instance {
                        Text(i.issuer)
                        Text("\(i.edition) \(i.serverVersion) · \(i.instanceID)").font(.caption)
                        Text("Trust: \(i.trust)").font(.caption)
                        Text("Serves: \(i.apiVersions.joined(separator: ", "))").font(.caption)
                    } else {
                        Text("Loading…").font(.caption)
                    }
                    Text("Stream: \(streamText)").font(.caption).accessibilityIdentifier("stream-state")
                }
                Section("Operations") {
                    ForEach(model.operations.sorted { $0.updatedAt.seconds > $1.updatedAt.seconds }, id: \.id) { op in
                        VStack(alignment: .leading, spacing: 4) {
                            HStack {
                                Text("\(op.kind) · \(op.namespace)").font(.subheadline)
                                Spacer()
                                Text(String(describing: op.state)).font(.caption)
                            }
                            if op.state == .running {
                                if op.progress.hasFraction {
                                    ProgressView(value: op.progress.fraction)
                                } else {
                                    // Unknown progress: indeterminate, never a misleading 0 %.
                                    ProgressView()
                                }
                            }
                            if !op.progress.message.isEmpty { Text(op.progress.message).font(.caption) }
                            if op.hasError { Text("\(op.error.code): \(op.error.message)").font(.caption).foregroundStyle(.red) }
                            if op.state == .running || op.state == .pending {
                                Button("Cancel") { confirmCancel = op }.disabled(model.operationsStatus != .live)
                            }
                        }
                    }
                }
            }
        }
        .navigationTitle("Status")
        .task {
            await model.refreshInstance()
            await model.watchOperations()
        }
        .confirmationDialog("Cancel this operation?", isPresented: Binding(get: { confirmCancel != nil }, set: { if !$0 { confirmCancel = nil } })) {
            Button("Cancel operation", role: .destructive) {
                if let op = confirmCancel { Task { await model.cancel(op) } }
            }
        }
    }

    private var streamText: String {
        switch model.operationsStatus {
        case .live: return "live"
        case .connecting: return "connecting"
        case .reconnecting(_, let cause): return "reconnecting (\(cause ?? "ended"))"
        case .stopped(let cause): return "stopped (\(cause ?? ""))"
        }
    }
}
