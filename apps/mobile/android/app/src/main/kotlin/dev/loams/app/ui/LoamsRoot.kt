package dev.loams.app.ui

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.repeatOnLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import dev.loams.app.AppContainer
import dev.loams.app.R
import dev.loams.app.session.SessionState
import dev.loams.app.signing.DecisionSigner
import dev.loams.app.ui.approvals.ApprovalsScreen
import dev.loams.app.ui.approvals.ApprovalsViewModel
import dev.loams.app.ui.settings.SettingsScreen
import dev.loams.app.ui.status.StatusScreen
import dev.loams.app.ui.status.StatusViewModel
import dev.loams.app.ui.welcome.WelcomeScreen
import kotlinx.coroutines.flow.MutableStateFlow

private enum class Tab(val label: String, val icon: Int) {
    APPROVALS("Approvals", R.drawable.ic_tab_approvals),
    STATUS("Status", R.drawable.ic_tab_status),
    SETTINGS("Settings", R.drawable.ic_tab_settings),
}

@Composable
fun LoamsRoot(container: AppContainer, signer: DecisionSigner, openApproval: MutableStateFlow<String?>) {
    val state by container.session.state.collectAsStateWithLifecycle()
    when (val s = state) {
        SessionState.Loading -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
        SessionState.SignedOut -> WelcomeScreen(container)
        is SessionState.Active -> key(s.backend) { MainTabs(container, s, signer, openApproval) }
    }
}

@Composable
private fun MainTabs(container: AppContainer, session: SessionState.Active, signer: DecisionSigner, openApproval: MutableStateFlow<String?>) {
    val backend = session.backend
    var tab by rememberSaveable { mutableStateOf(Tab.APPROVALS) }
    val pending by openApproval.collectAsStateWithLifecycle()
    LaunchedEffect(pending) { if (pending != null) tab = Tab.APPROVALS }

    // Debug: show the mock's sealed pushes while the app is visible (stands in for FCM).
    val poll by container.settings.pollMockPush.collectAsStateWithLifecycle(initialValue = false)
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    val record = session.record
    if (poll && record != null && !backend.isDemo) {
        LaunchedEffect(record.issuer, lifecycle) {
            lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
                container.mockPushPoller.run(record.issuer, container.session.pushToken())
            }
        }
    }

    val approvals: ApprovalsViewModel = viewModel(key = "approvals-${backend.instanceId}-${System.identityHashCode(backend)}") { ApprovalsViewModel(backend, signer) }
    val status: StatusViewModel = viewModel(key = "status-${backend.instanceId}-${System.identityHashCode(backend)}") { StatusViewModel(backend) }

    Scaffold(
        bottomBar = {
            NavigationBar {
                Tab.entries.forEach { t ->
                    NavigationBarItem(
                        selected = tab == t,
                        onClick = { tab = t },
                        icon = { Icon(painterResource(t.icon), contentDescription = null) },
                        label = { Text(t.label) },
                    )
                }
            }
        },
    ) { padding ->
        Box(Modifier.padding(padding).fillMaxSize()) {
            when (tab) {
                Tab.APPROVALS -> ApprovalsScreen(approvals, pending) { openApproval.value = null }
                Tab.STATUS -> StatusScreen(status)
                Tab.SETTINGS -> SettingsScreen(container, session)
            }
        }
    }
}
