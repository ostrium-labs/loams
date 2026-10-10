package dev.loams.app.ui.status

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Card
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.repeatOnLifecycle
import dev.loams.app.ui.StreamBanner
import dev.loams.core.watch.StreamStatus
import dev.loams.proto.loams.operations.v1.Operation
import dev.loams.proto.loams.operations.v1.OperationState

@Composable
fun StatusScreen(vm: StatusViewModel) {
    val ops by vm.operations.collectAsStateWithLifecycle()
    val status by vm.status.collectAsStateWithLifecycle()
    val instance by vm.instance.collectAsStateWithLifecycle()
    val error by vm.error.collectAsStateWithLifecycle()
    var confirmCancel by remember { mutableStateOf<Operation?>(null) }

    val lifecycle = LocalLifecycleOwner.current.lifecycle
    LaunchedEffect(lifecycle) {
        vm.refresh()
        lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) { vm.watch() }
    }

    Column(Modifier.fillMaxSize()) {
        StreamBanner(status)
        LazyColumn(Modifier.fillMaxSize().padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            item {
                Card(Modifier.fillMaxWidth().padding(top = 16.dp)) {
                    Column(Modifier.padding(16.dp)) {
                        Text("Instance", style = MaterialTheme.typography.titleMedium)
                        val i = instance
                        error?.let { Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall) }
                        if (i == null) {
                            if (error == null) Text("Loading…", style = MaterialTheme.typography.bodySmall)
                        } else {
                            Text(i.issuer, style = MaterialTheme.typography.bodyMedium)
                            Text("${i.edition} ${i.serverVersion} · ${i.instanceId}", style = MaterialTheme.typography.bodySmall)
                            Text("Trust: ${i.trust}", style = MaterialTheme.typography.bodySmall)
                            Text("Serves: ${i.apiVersions.joinToString()}", style = MaterialTheme.typography.bodySmall)
                        }
                        Text(
                            "Stream: " + when (val s = status) {
                                StreamStatus.Live -> "live"
                                StreamStatus.Connecting -> "connecting"
                                is StreamStatus.Reconnecting -> "reconnecting (${s.cause ?: "ended"})"
                                is StreamStatus.Stopped -> "stopped (${s.cause})"
                            },
                            style = MaterialTheme.typography.bodySmall,
                            modifier = Modifier.testTag("stream-state"),
                        )
                    }
                }
            }
            item { Text("Operations", style = MaterialTheme.typography.titleSmall, modifier = Modifier.padding(top = 8.dp)) }
            items(ops.sortedByDescending { it.updatedAt.seconds }, key = { it.id }) { op ->
                Card(Modifier.fillMaxWidth()) {
                    Column(Modifier.padding(16.dp)) {
                        Row {
                            Text("${op.kind} · ${op.namespace}", style = MaterialTheme.typography.titleSmall, modifier = Modifier.weight(1f))
                            Text(op.state.name.removePrefix("OPERATION_STATE_").lowercase().replace('_', ' '), style = MaterialTheme.typography.labelMedium)
                        }
                        if (op.state == OperationState.OPERATION_STATE_RUNNING) {
                            if (op.progress.hasFraction()) {
                                LinearProgressIndicator(progress = { op.progress.fraction.toFloat() }, modifier = Modifier.fillMaxWidth().padding(top = 8.dp))
                            } else {
                                // Unknown progress: indeterminate, never a misleading 0 %.
                                LinearProgressIndicator(modifier = Modifier.fillMaxWidth().padding(top = 8.dp))
                            }
                        }
                        if (op.progress.message.isNotEmpty()) Text(op.progress.message, style = MaterialTheme.typography.bodySmall)
                        if (op.hasError()) Text("${op.error.code}: ${op.error.message}", color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
                        if (op.state == OperationState.OPERATION_STATE_RUNNING || op.state == OperationState.OPERATION_STATE_PENDING) {
                            Row {
                                Spacer(Modifier.width(0.dp).weight(1f))
                                TextButton(onClick = { confirmCancel = op }, enabled = status == StreamStatus.Live) { Text("Cancel") }
                            }
                        }
                    }
                }
            }
        }
    }

    confirmCancel?.let { op ->
        AlertDialog(
            onDismissRequest = { confirmCancel = null },
            title = { Text("Cancel ${op.kind}?") },
            text = { Text("The operation stops where it is.") },
            confirmButton = { TextButton(onClick = { vm.cancel(op); confirmCancel = null }) { Text("Cancel operation") } },
            dismissButton = { TextButton(onClick = { confirmCancel = null }) { Text("Keep running") } },
        )
    }
}
