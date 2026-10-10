package dev.loams.app.ui

import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import dev.loams.core.watch.StreamStatus

/** A thin bar saying whether the live stream is up (cached data is shown read-only otherwise). */
@Composable
fun StreamBanner(status: StreamStatus) {
    val text = when (status) {
        StreamStatus.Live -> return
        StreamStatus.Connecting -> "Connecting…"
        is StreamStatus.Reconnecting -> "Offline: showing cached data. Reconnecting in ${(status.retryInMillis + 999) / 1000} s"
        is StreamStatus.Stopped -> "Disconnected: ${status.cause ?: "sign in again"}"
    }
    Surface(color = MaterialTheme.colorScheme.errorContainer, modifier = Modifier.fillMaxWidth().testTag("stream-banner")) {
        Text(text, Modifier.padding(horizontal = 16.dp, vertical = 6.dp), style = MaterialTheme.typography.bodySmall)
    }
}
