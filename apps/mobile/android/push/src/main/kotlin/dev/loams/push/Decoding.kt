package dev.loams.push

import dev.loams.core.Loams
import dev.loams.core.push.Shown
import dev.loams.proto.loams.notifications.v1.Category
import dev.loams.proto.loams.notifications.v1.Notification

/** Pure mapping from an unsealed notification to what is displayed, and on which channel. */
object Decoding {
    /** The plaintext of a sealed push is one serialized loams.notifications.v1.Notification. */
    fun shown(plaintext: ByteArray): Shown = shown(Notification.parseFrom(plaintext))

    fun shown(n: Notification): Shown {
        val approval = if (n.refCase == Notification.RefCase.APPROVAL_ID) n.approvalId else null
        return Shown(n.title.ifEmpty { "Loams" }, n.body, n.id, approval)
    }

    fun channelFor(category: Category): String = when (category) {
        Category.CATEGORY_APPROVALS -> Loams.Channels.APPROVALS
        Category.CATEGORY_SECURITY -> Loams.Channels.SECURITY
        Category.CATEGORY_JOBS, Category.CATEGORY_RUNS -> Loams.Channels.JOBS
        else -> Loams.Channels.OPERATIONS
    }

    /** Approvals get a Review action that opens the app; there is no background Approve (AP2 Ruling 6). */
    fun hasReviewAction(shown: Shown): Boolean = shown.approvalId != null
}
