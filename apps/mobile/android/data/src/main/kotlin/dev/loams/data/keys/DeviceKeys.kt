package dev.loams.data.keys

import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.PrivateKey
import java.security.Signature
import java.security.interfaces.ECPublicKey
import java.security.spec.ECGenParameterSpec

/**
 * Hardware-backed EC P-256 keys in the Android Keystore, StrongBox when the device has one and
 * the TEE otherwise (AP2 Ruling 2). Private keys never leave the Keystore.
 */
class DeviceKeys(context: Context) {
    private val strongBox = context.packageManager.hasSystemFeature(PackageManager.FEATURE_STRONGBOX_KEYSTORE)
    private val keyStore: KeyStore by lazy { KeyStore.getInstance(ANDROID_KEYSTORE).apply { load(null) } }

    /** Creates the key if needed and returns its public half. */
    fun ensure(policy: KeyPolicy): ECPublicKey {
        (keyStore.getCertificate(policy.alias)?.publicKey as? ECPublicKey)?.let { return it }
        if (!strongBox) return generate(policy, false)
        return try {
            generate(policy, true)
        } catch (e: Exception) {
            // Some devices advertise StrongBox but refuse a given spec (StrongBoxUnavailable,
            // ProviderException, InvalidAlgorithmParameter): fall back to the TEE, which
            // rethrows if the cause was something else, such as no secure lock screen.
            generate(policy, false)
        }
    }

    private fun generate(policy: KeyPolicy, inStrongBox: Boolean): ECPublicKey {
        val spec = KeyGenParameterSpec.Builder(policy.alias, KeyProperties.PURPOSE_SIGN)
            .setAlgorithmParameterSpec(ECGenParameterSpec("secp256r1"))
            .setDigests(KeyProperties.DIGEST_SHA256)
            .apply {
                if (policy.userPresenceEveryUse) {
                    setUserAuthenticationRequired(true)
                    setInvalidatedByBiometricEnrollment(policy.invalidatedByEnrollment)
                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                        val types = KeyProperties.AUTH_BIOMETRIC_STRONG or
                            (if (policy.allowDeviceCredential) KeyProperties.AUTH_DEVICE_CREDENTIAL else 0)
                        // Timeout 0: every signature needs its own authentication.
                        setUserAuthenticationParameters(0, types)
                    }
                }
                if (inStrongBox) setIsStrongBoxBacked(true)
            }
            .build()
        val kpg = KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, ANDROID_KEYSTORE)
        kpg.initialize(spec)
        return kpg.generateKeyPair().public as ECPublicKey
    }

    /**
     * A Signature initialised with the key, ready to hand to BiometricPrompt as a CryptoObject.
     * Throws KeyPermanentlyInvalidatedException after an enrolment change: the caller deletes the
     * key and the device must pair again.
     */
    fun signature(policy: KeyPolicy): Signature {
        val key = keyStore.getKey(policy.alias, null) as? PrivateKey ?: error("no key ${policy.alias}")
        return Signature.getInstance("SHA256withECDSA").apply { initSign(key) }
    }

    fun has(policy: KeyPolicy): Boolean = keyStore.containsAlias(policy.alias)

    /** Deletes both keys of an instance (sign-out, revocation). */
    fun delete(instanceId: String) {
        for (p in listOf(KeyPolicy.decide(instanceId), KeyPolicy.dpop(instanceId))) {
            if (keyStore.containsAlias(p.alias)) keyStore.deleteEntry(p.alias)
        }
    }

    companion object {
        const val ANDROID_KEYSTORE = "AndroidKeyStore"
    }
}
