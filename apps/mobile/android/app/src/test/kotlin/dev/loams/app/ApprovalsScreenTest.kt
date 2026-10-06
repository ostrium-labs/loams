package dev.loams.app

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollTo
import androidx.compose.ui.test.performTextInput
import dev.loams.app.backend.DemoBackend
import dev.loams.app.ui.approvals.ApprovalsContent
import dev.loams.core.decision.Decision
import dev.loams.core.watch.StreamStatus
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/** The approvals inbox on the JVM through Robolectric: list, detail, typed confirmation. */
@RunWith(RobolectricTestRunner::class)
// A plain Application: the real one opens the Android Keystore, which Robolectric lacks.
@Config(sdk = [35], application = android.app.Application::class)
class ApprovalsScreenTest {
    @get:Rule val compose = createComposeRule()

    @Test
    fun destructive_approval_needs_the_typed_name_before_approve_is_enabled() {
        val approvals = DemoBackend().approvals.value
        var decided: Pair<String, Decision>? = null
        compose.setContent {
            var selected by androidx.compose.runtime.remember { mutableStateOf<String?>(null) }
            ApprovalsContent(
                approvals = approvals,
                status = StreamStatus.Live,
                busyId = null,
                message = null,
                selectedId = selected,
                onSelect = { selected = it },
                onDecide = { a, d, _, _ -> decided = a.id to d },
                onDismissMessage = {},
            )
        }
        compose.onNodeWithText("production  ·  protected").assertIsDisplayed()
        compose.onNodeWithTag("approval-apr_drop_logs").performClick()
        compose.onNodeWithTag("detail-summary").assertIsDisplayed()
        compose.onNodeWithTag("approve-button").assertIsNotEnabled()
        compose.onNodeWithTag("reject-button").assertIsNotEnabled() // no reason yet
        compose.onNodeWithTag("confirm-field").performScrollTo().performTextInput("logs-2026")
        compose.onNodeWithTag("approve-button").performScrollTo().assertIsEnabled().performClick()
        assertEquals("apr_drop_logs" to Decision.APPROVE, decided)
    }

    @Test
    fun offline_shows_the_banner_and_disables_decisions() {
        val approvals = DemoBackend().approvals.value
        compose.setContent {
            ApprovalsContent(approvals, StreamStatus.Reconnecting(3_000, "network lost"), null, null, "apr_agent_reindex", {}, { _, _, _, _ -> }, {})
        }
        compose.onNodeWithTag("stream-banner").assertIsDisplayed()
        compose.onNodeWithTag("approve-button").assertIsNotEnabled()
    }
}
