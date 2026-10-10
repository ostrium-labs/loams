package dev.loams.core.push

import com.google.crypto.tink.InsecureSecretKeyAccess
import com.google.crypto.tink.hybrid.HpkeParameters
import com.google.crypto.tink.hybrid.HpkePrivateKey
import com.google.crypto.tink.hybrid.HpkePublicKey
import com.google.crypto.tink.hybrid.internal.HpkeDecrypt
import com.google.crypto.tink.hybrid.internal.HpkeEncrypt
import com.google.crypto.tink.subtle.X25519
import com.google.crypto.tink.util.Bytes
import com.google.crypto.tink.util.SecretBytes
import java.io.ByteArrayOutputStream
import java.security.GeneralSecurityException

/**
 * Opens sealed push payloads (design §37 §7.4): HPKE (RFC 9180) base mode with
 * DHKEM(X25519, HKDF-SHA256), HKDF-SHA256 and ChaCha20-Poly1305, through Tink.
 *
 * The instance id and notification id are bound through the HPKE `info`
 * ("loams-push-v1" 0x00 instance_id 0x00 notification_id) with empty associated data, which is
 * what Tink's HybridDecrypt exposes; a payload moved to another notification or instance fails to
 * open. The sealed bytes are enc (32) || ciphertext. Fixture: conformance/fixtures/push/sealed.json,
 * produced by the mock's Go implementation.
 */
object Unsealer {
    private val params: HpkeParameters = HpkeParameters.builder()
        .setKemId(HpkeParameters.KemId.DHKEM_X25519_HKDF_SHA256)
        .setKdfId(HpkeParameters.KdfId.HKDF_SHA256)
        .setAeadId(HpkeParameters.AeadId.CHACHA20_POLY1305)
        .setVariant(HpkeParameters.Variant.NO_PREFIX)
        .build()

    fun info(instanceId: String, notificationId: String): ByteArray {
        val out = ByteArrayOutputStream()
        out.write("loams-push-v1".toByteArray(Charsets.UTF_8))
        out.write(0)
        out.write(instanceId.toByteArray(Charsets.UTF_8))
        out.write(0)
        out.write(notificationId.toByteArray(Charsets.UTF_8))
        return out.toByteArray()
    }

    /** @throws GeneralSecurityException when the payload does not open with this key and binding. */
    fun open(sealed: ByteArray, instanceId: String, notificationId: String, privateKey: ByteArray): ByteArray {
        val pub = X25519.publicFromPrivate(privateKey)
        val key = HpkePrivateKey.create(
            HpkePublicKey.create(params, Bytes.copyFrom(pub), null),
            SecretBytes.copyFrom(privateKey, InsecureSecretKeyAccess.get()),
        )
        return HpkeDecrypt.create(key).decrypt(sealed, info(instanceId, notificationId))
    }

    /** Seals like the server does; used by tests and the in-app demo. */
    fun seal(plaintext: ByteArray, instanceId: String, notificationId: String, publicKey: ByteArray): ByteArray {
        val key = HpkePublicKey.create(params, Bytes.copyFrom(publicKey), null)
        return HpkeEncrypt.create(key).encrypt(plaintext, info(instanceId, notificationId))
    }
}

/** A raw X25519 key pair for push sealing (32-byte keys). */
class PushKeyPair(val publicKey: ByteArray, val privateKey: ByteArray) {
    companion object {
        fun publicFromPrivate(privateKey: ByteArray): ByteArray = X25519.publicFromPrivate(privateKey)

        fun generate(): PushKeyPair {
            val priv = X25519.generatePrivateKey()
            return PushKeyPair(X25519.publicFromPrivate(priv), priv)
        }
    }
}
