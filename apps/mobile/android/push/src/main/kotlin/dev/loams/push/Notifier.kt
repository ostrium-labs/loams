package dev.loams.push

import android.Manifest
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import dev.loams.core.Loams
import dev.loams.core.push.Shown

/** Posts unsealed notifications. Channel ids are permanent once released (AP2 Ruling 1). */
class Notifier(private val context: Context, private val launchIntent: () -> Intent) {
    fun createChannels() {
        val nm = context.getSystemService(NotificationManager::class.java)
        nm.createNotificationChannels(
            listOf(
                NotificationChannel(Loams.Channels.APPROVALS, "Approvals", NotificationManager.IMPORTANCE_HIGH),
                NotificationChannel(Loams.Channels.OPERATIONS, "Operations", NotificationManager.IMPORTANCE_DEFAULT),
                NotificationChannel(Loams.Channels.JOBS, "Jobs and runs", NotificationManager.IMPORTANCE_DEFAULT),
                NotificationChannel(Loams.Channels.SECURITY, "Security", NotificationManager.IMPORTANCE_HIGH),
                NotificationChannel(Loams.Channels.CONNECTION, "Connection", NotificationManager.IMPORTANCE_LOW),
            ),
        )
    }

    fun canPost(): Boolean =
        ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED

    fun show(shown: Shown, channel: String) {
        if (!canPost()) return
        // A data URI per notification keeps PendingIntents distinct (request codes can collide),
        // so Review always opens the approval it was posted for.
        val open = PendingIntent.getActivity(
            context,
            0,
            launchIntent()
                .setData(android.net.Uri.fromParts("loams-notification", shown.notificationId ?: "generic", null))
                .putExtra(EXTRA_APPROVAL_ID, shown.approvalId),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val builder = NotificationCompat.Builder(context, channel)
            .setSmallIcon(android.R.drawable.stat_notify_more)
            .setContentTitle(shown.title)
            .setContentText(shown.body)
            .setAutoCancel(true)
            .setContentIntent(open)
            // Hide the text on the lock screen; the server-rendered text may name environments.
            .setVisibility(NotificationCompat.VISIBILITY_PRIVATE)
            .setPublicVersion(
                NotificationCompat.Builder(context, channel)
                    .setSmallIcon(android.R.drawable.stat_notify_more)
                    .setContentTitle("Loams")
                    .setContentText("New activity in Loams")
                    .build(),
            )
        if (Decoding.hasReviewAction(shown)) {
            // Review opens the approval and asks for biometrics there; never a background approve.
            builder.addAction(0, "Review", open)
        }
        try {
            NotificationManagerCompat.from(context).notify(shown.notificationId ?: "generic", 0, builder.build())
        } catch (e: SecurityException) {
            // Permission revoked between the check and the post.
        }
    }

    companion object {
        const val EXTRA_APPROVAL_ID = "dev.loams.app.APPROVAL_ID"
    }
}
