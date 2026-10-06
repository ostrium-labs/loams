package dev.loams.push

import dev.loams.core.Loams
import dev.loams.core.push.PushMessage
import dev.loams.core.push.PushOpener
import dev.loams.core.push.Unsealer
import dev.loams.proto.loams.notifications.v1.Notification

/**
 * What `FirebaseMessagingService.onMessageReceived` (or the debug mock poller) calls with a
 * message's data map: unseal with the instance's HPKE key and post it on its channel. Anything
 * that does not open shows the generic text; the app then syncs its inbox (design §37 §7.4).
 */
class PushHandler(private val keys: PushKeys, private val notifier: Notifier) {
    fun onMessage(data: Map<String, String>) {
        val msg = PushMessage.parse(data)
        val key = msg?.let { keys.privateKey(it.instanceId) }
        val n = if (msg != null && key != null) {
            runCatching { Notification.parseFrom(Unsealer.open(msg.sealed, msg.instanceId, msg.notificationId, key)) }.getOrNull()
        } else {
            null
        }
        if (n == null) {
            notifier.show(PushOpener.GENERIC, Loams.Channels.OPERATIONS)
        } else {
            notifier.show(Decoding.shown(n), Decoding.channelFor(n.category))
        }
    }
}
