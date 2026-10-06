package dev.loams.data.keys

import android.os.Build
import androidx.biometric.BiometricManager.Authenticators.BIOMETRIC_STRONG
import androidx.biometric.BiometricManager.Authenticators.DEVICE_CREDENTIAL
import androidx.biometric.BiometricPrompt
import androidx.core.content.ContextCompat
import androidx.fragment.app.FragmentActivity
import java.security.Signature
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException
import kotlinx.coroutines.suspendCancellableCoroutine

/** The user dismissed or failed the prompt; nothing was signed. */
class SigningCancelledException(val code: Int, message: String) : Exception(message)

/**
 * Signs bytes with a user-presence key through BiometricPrompt and a CryptoObject, so the
 * Keystore releases the key only for this one authenticated operation (AP2 Review Focus 1).
 * Device credential (PIN, pattern) is allowed with a CryptoObject only on API 30+.
 */
class BiometricSigner(private val activity: FragmentActivity) {
    suspend fun sign(signature: Signature, data: ByteArray, title: String, subtitle: String): ByteArray =
        suspendCancellableCoroutine { cont ->
            val prompt = BiometricPrompt(
                activity,
                ContextCompat.getMainExecutor(activity),
                object : BiometricPrompt.AuthenticationCallback() {
                    override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                        val s = result.cryptoObject?.signature
                        if (s == null) {
                            cont.resumeWithException(IllegalStateException("no CryptoObject in the result"))
                            return
                        }
                        runCatching {
                            s.update(data)
                            s.sign()
                        }.onSuccess { cont.resume(it) }.onFailure { cont.resumeWithException(it) }
                    }

                    override fun onAuthenticationError(errorCode: Int, errString: CharSequence) {
                        cont.resumeWithException(SigningCancelledException(errorCode, errString.toString()))
                    }
                },
            )
            val info = BiometricPrompt.PromptInfo.Builder()
                .setTitle(title)
                .setSubtitle(subtitle)
                .setConfirmationRequired(true)
                .apply {
                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                        setAllowedAuthenticators(BIOMETRIC_STRONG or DEVICE_CREDENTIAL)
                    } else {
                        setAllowedAuthenticators(BIOMETRIC_STRONG)
                        setNegativeButtonText("Cancel")
                    }
                }
                .build()
            prompt.authenticate(info, BiometricPrompt.CryptoObject(signature))
            cont.invokeOnCancellation { prompt.cancelAuthentication() }
        }
}
