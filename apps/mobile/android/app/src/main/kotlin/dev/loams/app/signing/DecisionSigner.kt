package dev.loams.app.signing

import android.security.keystore.KeyPermanentlyInvalidatedException
import androidx.fragment.app.FragmentActivity
import dev.loams.core.decision.EcdsaSignatures
import dev.loams.data.keys.BiometricSigner
import dev.loams.data.keys.DeviceKeys
import dev.loams.data.keys.KeyPolicy

/** Signs a decision's JWS signing input with the instance's decision key; returns raw r||s. */
fun interface DecisionSigner {
    suspend fun sign(instanceId: String, signingInput: ByteArray, title: String, subtitle: String): ByteArray
}

/** Why no signature was made, in words the approvals screen shows. */
class SigningUnavailableException(message: String, cause: Throwable? = null) : Exception(message, cause)

/**
 * The Keystore decision key behind BiometricPrompt. Demo mode uses the same path with a
 * `decide-demo` key, so the prompt is real even without a server.
 */
class KeystoreDecisionSigner(private val activity: FragmentActivity, private val keys: DeviceKeys) : DecisionSigner {
    override suspend fun sign(instanceId: String, signingInput: ByteArray, title: String, subtitle: String): ByteArray {
        val policy = KeyPolicy.decide(instanceId)
        try {
            keys.ensure(policy)
        } catch (e: Exception) {
            // Key generation with user authentication fails without a secure lock screen.
            throw SigningUnavailableException("Set a screen lock (PIN, pattern or fingerprint) to approve from this phone.", e)
        }
        val signature = try {
            keys.signature(policy)
        } catch (e: KeyPermanentlyInvalidatedException) {
            keys.delete(instanceId)
            throw SigningUnavailableException("Your fingerprints or face changed, so this phone's approval key was destroyed. Pair again.", e)
        }
        val der = BiometricSigner(activity).sign(signature, signingInput, title, subtitle)
        return EcdsaSignatures.derToRaw(der)
    }
}
