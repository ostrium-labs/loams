package dev.loams.app.ui.welcome

import android.app.Activity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import dev.loams.app.AppContainer
import dev.loams.app.BuildConfig
import dev.loams.app.session.AuthentikSignIn
import dev.loams.app.ui.SecureScreen
import kotlinx.coroutines.launch

@Composable
fun WelcomeScreen(container: AppContainer) {
    SecureScreen()
    val vm: WelcomeViewModel = viewModel { WelcomeViewModel(container.session) }
    val busy by vm.busy.collectAsStateWithLifecycle()
    val message by vm.message.collectAsStateWithLifecycle()
    val confirm by vm.confirm.collectAsStateWithLifecycle()
    val defaultServer by container.settings.serverUrl.collectAsStateWithLifecycle(initialValue = BuildConfig.DEFAULT_SERVER)
    var server by rememberSaveable { mutableStateOf<String?>(null) }
    val issuer = server ?: defaultServer
    var pasted by rememberSaveable { mutableStateOf("") }
    var userCode by rememberSaveable { mutableStateOf("") }
    val scope = rememberCoroutineScope()

    val scanner = rememberLauncherForActivityResult(ScanContract()) { result ->
        result.contents?.let(vm::pairFromPayload)
    }

    val context = LocalContext.current
    val signIn = remember { AuthentikSignIn(context) }
    DisposableEffect(signIn) { onDispose { signIn.dispose() } }
    val browser = rememberLauncherForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
        val p = vm.pendingSignIn ?: run {
            vm.say("Sign-in expired while the browser was open; try again.")
            return@rememberLauncherForActivityResult
        }
        vm.pendingSignIn = null
        val data = result.data
        if (result.resultCode != Activity.RESULT_OK && data == null) {
            vm.say("Sign-in was cancelled.")
            return@rememberLauncherForActivityResult
        }
        scope.launch {
            runCatching { signIn.complete(data!!) }
                .onSuccess { vm.finishBrowser(p.issuer, p.instanceId, p.jkt, it) }
                .onFailure { vm.say("Sign-in failed: ${it.message}") }
        }
    }
    LaunchedEffect(issuer) { if (server != null) container.settings.setServerUrl(issuer) }

    Column(
        Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text("Loams", style = MaterialTheme.typography.headlineLarge)
        Text("Approve operations and watch your Loams instance from this phone.", style = MaterialTheme.typography.bodyMedium)
        message?.let { Text(it, color = MaterialTheme.colorScheme.error) }

        Card(Modifier.fillMaxWidth()) {
            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("Pair with a QR code", style = MaterialTheme.typography.titleMedium)
                Text("In the console or desktop app, open Devices, then Pair a phone.", style = MaterialTheme.typography.bodySmall)
                Button(
                    onClick = {
                        scanner.launch(
                            ScanOptions().setDesiredBarcodeFormats(ScanOptions.QR_CODE).setPrompt("Scan the Loams pairing code")
                                .setBeepEnabled(false).setOrientationLocked(false),
                        )
                    },
                    enabled = !busy,
                ) { Text("Scan QR code") }
                OutlinedTextField(pasted, { pasted = it }, label = { Text("Or paste the pairing payload") }, modifier = Modifier.fillMaxWidth(), maxLines = 4)
                OutlinedButton(onClick = { vm.pairFromPayload(pasted) }, enabled = !busy && pasted.isNotBlank()) { Text("Pair") }
                if (BuildConfig.ALLOW_INSECURE_LOOPBACK) {
                    TextButton(onClick = { vm.pairWithMock(issuer) }, enabled = !busy) { Text("Debug: pair with the local mock at $issuer") }
                }
            }
        }

        OutlinedTextField(issuer, { server = it }, label = { Text("Instance address") }, singleLine = true, modifier = Modifier.fillMaxWidth())

        Card(Modifier.fillMaxWidth()) {
            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("Pair with a typed code", style = MaterialTheme.typography.titleMedium)
                Text("For a phone without a camera. You will compare a fingerprint with the console, because nothing is pinned in advance.", style = MaterialTheme.typography.bodySmall)
                OutlinedTextField(
                    userCode, { userCode = it.filter(Char::isDigit).take(8) }, label = { Text("8-digit code") }, singleLine = true,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.NumberPassword),
                )
                OutlinedButton(onClick = { vm.startTyped(issuer) }, enabled = !busy && userCode.length == 8) { Text("Continue") }
            }
        }

        Card(Modifier.fillMaxWidth()) {
            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("Sign in with Authentik", style = MaterialTheme.typography.titleMedium)
                Text("Opens your instance's sign-in page in the browser (OIDC with PKCE).", style = MaterialTheme.typography.bodySmall)
                OutlinedButton(
                    onClick = {
                        scope.launch {
                            runCatching { signIn.start(issuer) }
                                .onSuccess { (p, intent) ->
                                    vm.pendingSignIn = p
                                    browser.launch(intent)
                                }
                                .onFailure { vm.say(it.message ?: "Could not start sign-in.") }
                        }
                    },
                    enabled = !busy,
                ) { Text("Sign in") }
            }
        }

        TextButton(onClick = { container.session.startDemo() }) { Text("Try the demo (no server)") }
    }

    confirm?.let { c ->
        AlertDialog(
            onDismissRequest = vm::cancelTyped,
            title = { Text("Compare this fingerprint") },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text("The console shows the instance key's fingerprint. Continue only if it matches exactly:")
                    Text(c.thumbprint.chunked(4).joinToString(" "), fontFamily = FontFamily.Monospace)
                    Text("This pairing was not pinned in advance.", style = MaterialTheme.typography.bodySmall)
                }
            },
            confirmButton = { TextButton(onClick = { vm.confirmTyped(userCode) }) { Text("It matches") } },
            dismissButton = { TextButton(onClick = vm::cancelTyped) { Text("Cancel") } },
        )
    }
}
