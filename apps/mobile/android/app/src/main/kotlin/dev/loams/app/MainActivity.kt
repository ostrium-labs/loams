package dev.loams.app

import android.content.Intent
import android.os.Bundle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.fragment.app.FragmentActivity
import dev.loams.app.signing.KeystoreDecisionSigner
import dev.loams.app.ui.LoamsRoot
import dev.loams.app.ui.theme.LoamsTheme
import dev.loams.push.Notifier
import kotlinx.coroutines.flow.MutableStateFlow

/** A FragmentActivity because BiometricPrompt needs one; Compose draws everything. */
class MainActivity : FragmentActivity() {
    /** The approval a notification's Review action opened, if any. */
    private val openApproval = MutableStateFlow<String?>(null)

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        openApproval.value = intent?.getStringExtra(Notifier.EXTRA_APPROVAL_ID)
        val container = (application as LoamsApp).container
        val signer = KeystoreDecisionSigner(this, container.keys)
        setContent {
            LoamsTheme {
                LoamsRoot(container, signer, openApproval)
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        intent.getStringExtra(Notifier.EXTRA_APPROVAL_ID)?.let { openApproval.value = it }
    }
}
