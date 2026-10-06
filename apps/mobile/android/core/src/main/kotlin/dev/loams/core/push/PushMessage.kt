package dev.loams.core.push

import java.util.Base64

/**
 * The data a push carries (design §37 §7.4, AP2 Task 8): `{"v":"1","i":instance_id,
 * "n":notification_id,"s":base64(sealed)}` and nothing else. The plaintext title and body are
 * inside `s`; Apple, Google and the push gateway never see them.
 */
data class PushMessage(val instanceId: String, val notificationId: String, val sealed: ByteArray) {
    override fun equals(other: Any?): Boolean =
        other is PushMessage && other.instanceId == instanceId && other.notificationId == notificationId && other.sealed.contentEquals(sealed)

    override fun hashCode(): Int = (instanceId.hashCode() * 31 + notificationId.hashCode()) * 31 + sealed.contentHashCode()

    companion object {
        val FIELDS = setOf("v", "i", "n", "s")

        /** Null when the map is not a v1 Loams push (another sender, or a future version). */
        fun parse(data: Map<String, String>): PushMessage? {
            if (data["v"] != "1") return null
            val i = data["i"]?.takeIf { it.isNotEmpty() } ?: return null
            val n = data["n"]?.takeIf { it.isNotEmpty() } ?: return null
            val s = data["s"]?.let { runCatching { Base64.getDecoder().decode(it) }.getOrNull() } ?: return null
            return PushMessage(i, n, s)
        }
    }
}

/** What the notification shows. */
data class Shown(val title: String, val body: String, val notificationId: String?, val approvalId: String?)

/**
 * Decides what to display for a push. Anything that cannot be opened (unknown instance, no key,
 * a payload that fails to unseal) shows the generic text and the app syncs its inbox, so a
 * failure never leaks or drops silently.
 */
class PushOpener(
    private val keyFor: (instanceId: String) -> ByteArray?,
    private val decode: (plaintext: ByteArray) -> Shown,
) {
    fun open(data: Map<String, String>): Shown {
        val msg = PushMessage.parse(data) ?: return GENERIC
        val key = keyFor(msg.instanceId) ?: return GENERIC
        return runCatching { decode(Unsealer.open(msg.sealed, msg.instanceId, msg.notificationId, key)) }
            .getOrDefault(GENERIC)
    }

    companion object {
        val GENERIC = Shown("Loams", "New activity in Loams", null, null)
    }
}
