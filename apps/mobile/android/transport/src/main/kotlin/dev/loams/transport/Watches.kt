package dev.loams.transport

import com.connectrpc.ServerOnlyStreamInterface
import com.connectrpc.getOrThrow
import dev.loams.core.watch.WatchEvent
import dev.loams.proto.loams.approvals.v1.Approval
import dev.loams.proto.loams.approvals.v1.WatchApprovalsRequest
import dev.loams.proto.loams.approvals.v1.WatchApprovalsResponse
import dev.loams.proto.loams.notifications.v1.Notification
import dev.loams.proto.loams.notifications.v1.WatchNotificationsRequest
import dev.loams.proto.loams.notifications.v1.WatchNotificationsResponse
import dev.loams.proto.loams.operations.v1.Operation
import dev.loams.proto.loams.operations.v1.WatchOperationsRequest
import dev.loams.proto.loams.operations.v1.WatchOperationsResponse
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.flow

/** Opens a server stream, sends the one request, and emits every response until the stream ends. */
private fun <Req : Any, Resp : Any, T> serverStream(
    open: suspend () -> ServerOnlyStreamInterface<Req, Resp>,
    request: Req,
    map: (Resp) -> WatchEvent<T>?,
): Flow<WatchEvent<T>> = flow {
    val stream = open()
    try {
        stream.sendAndClose(request).getOrThrow()
        // The channel closes normally at a clean end, or with a ConnectException on error.
        for (message in stream.responseChannel()) {
            map(message)?.let { emit(it) }
        }
    } finally {
        stream.receiveClose()
    }
}

fun Clients.watchApprovals(cursor: String?): Flow<WatchEvent<Approval>> = serverStream(
    { approvals.watchApprovals() },
    WatchApprovalsRequest.newBuilder().setResumeCursor(cursor.orEmpty()).build(),
) { m: WatchApprovalsResponse ->
    when (m.eventCase) {
        WatchApprovalsResponse.EventCase.SNAPSHOT -> WatchEvent.Snapshot(m.snapshot.approvalsList, m.cursor, m.snapshotReset)
        WatchApprovalsResponse.EventCase.UPSERT -> WatchEvent.Upsert(m.upsert, m.cursor)
        WatchApprovalsResponse.EventCase.REMOVE -> WatchEvent.Remove(m.remove, m.cursor)
        WatchApprovalsResponse.EventCase.HEARTBEAT -> WatchEvent.Heartbeat(m.cursor)
        else -> null // a newer server's event kind: ignore it
    }
}

fun Clients.watchOperations(cursor: String?): Flow<WatchEvent<Operation>> = serverStream(
    { operations.watchOperations() },
    WatchOperationsRequest.newBuilder().setResumeCursor(cursor.orEmpty()).build(),
) { m: WatchOperationsResponse ->
    when (m.eventCase) {
        WatchOperationsResponse.EventCase.SNAPSHOT -> WatchEvent.Snapshot(m.snapshot.operationsList, m.cursor, m.snapshotReset)
        WatchOperationsResponse.EventCase.UPSERT -> WatchEvent.Upsert(m.upsert, m.cursor)
        WatchOperationsResponse.EventCase.REMOVE -> WatchEvent.Remove(m.remove, m.cursor)
        WatchOperationsResponse.EventCase.HEARTBEAT -> WatchEvent.Heartbeat(m.cursor)
        else -> null
    }
}

fun Clients.watchNotifications(cursor: String?): Flow<WatchEvent<Notification>> = serverStream(
    { notifications.watchNotifications() },
    WatchNotificationsRequest.newBuilder().setResumeCursor(cursor.orEmpty()).build(),
) { m: WatchNotificationsResponse ->
    when (m.eventCase) {
        WatchNotificationsResponse.EventCase.SNAPSHOT -> WatchEvent.Snapshot(m.snapshot.notificationsList, m.cursor, m.snapshotReset)
        WatchNotificationsResponse.EventCase.UPSERT -> WatchEvent.Upsert(m.upsert, m.cursor)
        WatchNotificationsResponse.EventCase.REMOVE -> WatchEvent.Remove(m.remove, m.cursor)
        WatchNotificationsResponse.EventCase.HEARTBEAT -> WatchEvent.Heartbeat(m.cursor)
        else -> null
    }
}
