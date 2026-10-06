import Connect
import Foundation
import LoamsCore
import LoamsProto

/// Opens a server stream and maps it to core watch events; the stream is cancelled when the
/// consumer stops iterating.
func serverStream<Req: ProtobufMessage, Res: ProtobufMessage, Item: Sendable>(
    _ stream: any ServerOnlyAsyncStreamInterface<Req, Res>,
    request: Req,
    map: @escaping @Sendable (Res) -> WatchEvent<Item>?
) -> AsyncThrowingStream<WatchEvent<Item>, Error> {
    AsyncThrowingStream { continuation in
        let task = Task {
            do {
                try stream.send(request)
            } catch {
                continuation.finish(throwing: error)
                return
            }
            for await result in stream.results() {
                switch result {
                case .headers:
                    continue
                case .message(let m):
                    if let e = map(m) { continuation.yield(e) }
                case .complete(let code, let error, _):
                    if code == .ok {
                        continuation.finish()
                    } else {
                        continuation.finish(throwing: error ?? ConnectError(code: code, message: "stream ended with \(code)", exception: nil, details: [], metadata: [:]))
                    }
                    return
                }
            }
            continuation.finish()
        }
        continuation.onTermination = { _ in
            task.cancel()
            stream.cancel()
        }
    }
}

public extension LoamsClients {
    func watchApprovals(cursor: String?) -> AsyncThrowingStream<WatchEvent<Loams_Approvals_V1_Approval>, Error> {
        var req = Loams_Approvals_V1_WatchApprovalsRequest()
        req.resumeCursor = cursor ?? ""
        return serverStream(approvals.watchApprovals(headers: headers), request: req, map: WatchMapping.approvals)
    }

    func watchOperations(cursor: String?) -> AsyncThrowingStream<WatchEvent<Loams_Operations_V1_Operation>, Error> {
        var req = Loams_Operations_V1_WatchOperationsRequest()
        req.resumeCursor = cursor ?? ""
        return serverStream(operations.watchOperations(headers: headers), request: req, map: WatchMapping.operations)
    }
}

/// Pure mappings from Watch*Response to core events, tested without a server.
public enum WatchMapping {
    public static let approvals: @Sendable (Loams_Approvals_V1_WatchApprovalsResponse) -> WatchEvent<Loams_Approvals_V1_Approval>? = { m in
        switch m.event {
        case .snapshot(let s): return .snapshot(s.approvals, cursor: m.cursor, reset: m.snapshotReset)
        case .upsert(let a): return .upsert(a, cursor: m.cursor)
        case .remove(let id): return .remove(id: id, cursor: m.cursor)
        case .heartbeat: return .heartbeat(cursor: m.cursor)
        case nil: return nil
        }
    }

    public static let operations: @Sendable (Loams_Operations_V1_WatchOperationsResponse) -> WatchEvent<Loams_Operations_V1_Operation>? = { m in
        switch m.event {
        case .snapshot(let s): return .snapshot(s.operations, cursor: m.cursor, reset: m.snapshotReset)
        case .upsert(let o): return .upsert(o, cursor: m.cursor)
        case .remove(let id): return .remove(id: id, cursor: m.cursor)
        case .heartbeat: return .heartbeat(cursor: m.cursor)
        case nil: return nil
        }
    }
}
