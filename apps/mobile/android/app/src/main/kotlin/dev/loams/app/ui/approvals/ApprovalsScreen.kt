package dev.loams.app.ui.approvals

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.repeatOnLifecycle
import dev.loams.app.ui.SecureScreen
import dev.loams.app.ui.StreamBanner
import dev.loams.core.decision.Decision
import dev.loams.core.decision.DecisionRules
import dev.loams.core.watch.StreamStatus
import dev.loams.proto.loams.approvals.v1.Approval
import dev.loams.proto.loams.approvals.v1.ApprovalState
import dev.loams.proto.loams.approvals.v1.Risk
import dev.loams.proto.loams.instance.v1.PrincipalKind
import kotlinx.coroutines.delay

@Composable
fun ApprovalsScreen(vm: ApprovalsViewModel, openId: String?, onOpened: () -> Unit) {
    val approvals by vm.approvals.collectAsStateWithLifecycle()
    val status by vm.status.collectAsStateWithLifecycle()
    val busy by vm.busy.collectAsStateWithLifecycle()
    val message by vm.message.collectAsStateWithLifecycle()
    var selected by rememberSaveable { mutableStateOf<String?>(null) }

    // Streams run only while the screen is visible (AP2 Ruling 5).
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    LaunchedEffect(lifecycle) { lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) { vm.watch() } }
    LaunchedEffect(openId) {
        if (openId != null) {
            selected = openId
            onOpened()
        }
    }

    ApprovalsContent(
        approvals = approvals,
        status = status,
        busyId = busy,
        message = message,
        selectedId = selected,
        onSelect = { selected = it },
        onDecide = { a, d, reason, typed -> vm.decide(a, d, reason, typed) },
        onDismissMessage = vm::dismissMessage,
    )
}

/** Stateless, for previews and the Compose UI test. */
@Composable
fun ApprovalsContent(
    approvals: List<Approval>,
    status: StreamStatus,
    busyId: String?,
    message: String?,
    selectedId: String?,
    onSelect: (String?) -> Unit,
    onDecide: (Approval, Decision, String, String) -> Unit,
    onDismissMessage: () -> Unit,
) {
    var nowMillis by remember { mutableLongStateOf(System.currentTimeMillis()) }
    LaunchedEffect(Unit) {
        while (true) {
            delay(30_000)
            nowMillis = System.currentTimeMillis()
        }
    }
    val selected = approvals.firstOrNull { it.id == selectedId }
    Column(Modifier.fillMaxSize()) {
        StreamBanner(status)
        if (message != null) {
            Surface(color = MaterialTheme.colorScheme.secondaryContainer, modifier = Modifier.fillMaxWidth()) {
                Row(Modifier.padding(horizontal = 16.dp, vertical = 8.dp)) {
                    Text(message, Modifier.weight(1f).testTag("message"))
                    TextButton(onClick = onDismissMessage) { Text("OK") }
                }
            }
        }
        if (selected != null) {
            ApprovalDetail(selected, live = status == StreamStatus.Live, busy = busyId == selected.id, nowMillis = nowMillis, onDecide = onDecide, onClose = { onSelect(null) })
        } else {
            ApprovalList(approvals, nowMillis, onSelect)
        }
    }
}

@Composable
private fun ApprovalList(approvals: List<Approval>, nowMillis: Long, onSelect: (String) -> Unit) {
    if (approvals.isEmpty()) {
        Text("Nothing waits for your approval.", Modifier.padding(24.dp), style = MaterialTheme.typography.bodyLarge)
        return
    }
    // Grouped by environment, protected ones marked (AP2 Task 6).
    val groups = approvals.groupBy { it.environment }
    LazyColumn(Modifier.fillMaxSize().padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        groups.forEach { (env, list) ->
            item(key = "env-${env.id}") {
                Text(
                    env.name + if (env.protected) "  ·  protected" else "",
                    style = MaterialTheme.typography.titleSmall,
                    color = if (env.protected) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(top = 16.dp),
                )
            }
            items(list, key = { it.id }) { a ->
                Card(Modifier.fillMaxWidth().clickable { onSelect(a.id) }.testTag("approval-${a.id}")) {
                    Column(Modifier.padding(16.dp)) {
                        Text(a.summary, style = MaterialTheme.typography.titleMedium)
                        Spacer(Modifier.height(4.dp))
                        Text(requesterLine(a), style = MaterialTheme.typography.bodySmall)
                        Text("${riskLabel(a.risk)} · ${progressLine(a)} · ${expiresLine(a, nowMillis)}", style = MaterialTheme.typography.bodySmall)
                    }
                }
            }
        }
    }
}

@Composable
private fun ApprovalDetail(a: Approval, live: Boolean, busy: Boolean, nowMillis: Long, onDecide: (Approval, Decision, String, String) -> Unit, onClose: () -> Unit) {
    SecureScreen()
    var typed by rememberSaveable(a.id) { mutableStateOf("") }
    var reason by rememberSaveable(a.id) { mutableStateOf("") }
    val pending = a.state == ApprovalState.APPROVAL_STATE_PENDING
    val destructive = a.risk == Risk.RISK_DESTRUCTIVE
    val canApprove = !busy && DecisionRules.check(Decision.APPROVE, pending, destructive, a.confirmText, typed, reason, live) == null
    val canReject = !busy && DecisionRules.check(Decision.REJECT, pending, destructive, a.confirmText, typed, reason, live) == null

    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        TextButton(onClick = onClose) { Text("Back") }
        Text(a.summary, style = MaterialTheme.typography.headlineSmall, modifier = Modifier.testTag("detail-summary"))
        Text("${a.environment.project} / ${a.environment.name}" + if (a.environment.protected) " (protected)" else "", style = MaterialTheme.typography.bodyMedium)
        Text(requesterLine(a), style = MaterialTheme.typography.bodyMedium)
        Text("${riskLabel(a.risk)} · ${progressLine(a)} · ${expiresLine(a, nowMillis)}", style = MaterialTheme.typography.bodyMedium)
        a.detailLinesList.forEach { Text("• $it", style = MaterialTheme.typography.bodyMedium) }
        Spacer(Modifier.height(8.dp))
        if (destructive) {
            Text("This cannot be undone. Type ${a.confirmText} to approve.", fontWeight = FontWeight.SemiBold, color = MaterialTheme.colorScheme.error)
            OutlinedTextField(typed, { typed = it }, label = { Text(a.confirmText) }, singleLine = true, modifier = Modifier.fillMaxWidth().testTag("confirm-field"))
        }
        OutlinedTextField(reason, { reason = it }, label = { Text("Reason (required to reject)") }, modifier = Modifier.fillMaxWidth().testTag("reason-field"))
        if (!live) Text("Offline: decisions need a live connection.", color = MaterialTheme.colorScheme.error)
        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            OutlinedButton(onClick = { onDecide(a, Decision.REJECT, reason, typed) }, enabled = canReject, modifier = Modifier.testTag("reject-button")) { Text("Reject") }
            Spacer(Modifier.width(4.dp))
            Button(
                onClick = { onDecide(a, Decision.APPROVE, reason, typed) },
                enabled = canApprove,
                colors = if (destructive) ButtonDefaults.buttonColors(containerColor = MaterialTheme.colorScheme.error) else ButtonDefaults.buttonColors(),
                modifier = Modifier.testTag("approve-button"),
            ) { Text(if (busy) "Signing…" else "Approve") }
        }
        Text("Approving asks for your fingerprint, face or screen lock to sign with this phone's key.", style = MaterialTheme.typography.bodySmall)
    }
}

private fun requesterLine(a: Approval): String {
    val chain = a.actorChainList
    return if (chain.size >= 2 && chain.first().kind == PrincipalKind.PRINCIPAL_KIND_AGENT) {
        "Requested by ${chain.first().displayName}, acting for ${chain.drop(1).joinToString { it.displayName }}"
    } else {
        "Requested by ${a.requestedBy.displayName.ifEmpty { a.requestedBy.id }}"
    }
}

private fun riskLabel(r: Risk) = when (r) {
    Risk.RISK_DESTRUCTIVE -> "Destructive"
    Risk.RISK_HIGH -> "High risk"
    Risk.RISK_MEDIUM -> "Medium risk"
    Risk.RISK_LOW -> "Low risk"
    else -> "Risk unknown"
}

private fun progressLine(a: Approval): String {
    val need = maxOf(1, a.policy.requiredApprovals)
    val have = a.decisionsList.count { it.decision == dev.loams.proto.loams.approvals.v1.DecisionKind.DECISION_KIND_APPROVE }
    return "$have of $need approvals"
}

private fun expiresLine(a: Approval, nowMillis: Long): String {
    if (!a.hasExpiresAt()) return "no expiry"
    val left = a.expiresAt.seconds - nowMillis / 1000
    return when {
        left <= 0 -> "expired"
        left < 3600 -> "expires in ${left / 60} min"
        left < 86_400 -> "expires in ${left / 3600} h ${(left % 3600) / 60} min"
        else -> "expires in ${left / 86_400} d"
    }
}
