package dev.loams.app.ui.settings

import android.Manifest
import android.os.Build
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.loams.app.AppContainer
import dev.loams.app.BuildConfig
import dev.loams.app.session.SessionState
import kotlinx.coroutines.launch

@Composable
fun SettingsScreen(container: AppContainer, session: SessionState.Active) {
    val scope = rememberCoroutineScope()
    val backend = session.backend
    val record = session.record
    var note by remember { mutableStateOf<String?>(null) }
    var confirmSignOut by remember { mutableStateOf(false) }
    val poll by container.settings.pollMockPush.collectAsStateWithLifecycle(initialValue = false)
    val askNotifications = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        note = if (granted) "Notifications allowed." else "Notifications are off; approvals still show in the app."
    }

    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text("Settings", style = MaterialTheme.typography.headlineSmall)
        note?.let { Text(it, color = MaterialTheme.colorScheme.secondary) }

        Card(Modifier.fillMaxWidth()) {
            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text("This phone", style = MaterialTheme.typography.titleMedium)
                if (record == null) {
                    Text("Demo mode: in-memory data, nothing leaves the phone.")
                } else {
                    Text("Instance: ${record.issuer}")
                    Text("Instance id: ${record.instanceId}", style = MaterialTheme.typography.bodySmall)
                    Text("Device id: ${record.deviceId} (paired by ${record.method})", style = MaterialTheme.typography.bodySmall)
                    Text("Trust: " + if (record.spki != null) "pinned TLS key + instance key" else "system CAs + instance key", style = MaterialTheme.typography.bodySmall)
                    Text("Instance key: ${record.jkt.take(16)}…", style = MaterialTheme.typography.bodySmall)
                }
            }
        }

        Card(Modifier.fillMaxWidth()) {
            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("Notifications", style = MaterialTheme.typography.titleMedium)
                Text(
                    "Pushes are sealed to this phone's key, so Google and the push gateway see no content. " +
                        "Firebase is a stub in this build: no store accounts yet.",
                    style = MaterialTheme.typography.bodySmall,
                )
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                    OutlinedButton(onClick = { askNotifications.launch(Manifest.permission.POST_NOTIFICATIONS) }) { Text("Allow notifications") }
                }
                OutlinedButton(onClick = {
                    scope.launch {
                        val id = record?.instanceId ?: backend.instanceId
                        note = backend.registerPush(container.session.pushToken(), container.pushKeys.ensure(id))
                            .fold({ "Registered push target $it." }, { "Push registration failed: ${it.message}" })
                    }
                }) { Text("Register for push") }
                OutlinedButton(onClick = {
                    scope.launch {
                        note = backend.sendTestNotification().fold({ "Test notification $it sent." }, { "Not sent: ${it.message}" })
                    }
                }, enabled = !backend.isDemo) { Text("Send a test notification") }
                if (BuildConfig.ALLOW_INSECURE_LOOPBACK) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text("Debug: show the mock's pushes while the app is open", Modifier.weight(1f), style = MaterialTheme.typography.bodySmall)
                        Switch(checked = poll, onCheckedChange = { on -> scope.launch { container.settings.setPollMockPush(on) } })
                    }
                }
            }
        }

        Button(
            onClick = { confirmSignOut = true },
            colors = ButtonDefaults.buttonColors(containerColor = MaterialTheme.colorScheme.error),
        ) { Text(if (record == null) "Leave demo mode" else "Sign out and forget this instance") }

        Text("Loams ${BuildConfig.VERSION_NAME} · Apache License 2.0", style = MaterialTheme.typography.bodySmall)
    }

    if (confirmSignOut) {
        AlertDialog(
            onDismissRequest = { confirmSignOut = false },
            title = { Text("Sign out?") },
            text = { Text("This deletes this phone's keys and tokens for the instance. You will need to pair again.") },
            confirmButton = {
                TextButton(onClick = {
                    confirmSignOut = false
                    scope.launch { container.session.signOut() }
                }) { Text("Sign out") }
            },
            dismissButton = { TextButton(onClick = { confirmSignOut = false }) { Text("Cancel") } },
        )
    }
}
