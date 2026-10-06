package dev.loams.data.keys

/**
 * The two Keystore keys per paired instance (AP2 Ruling 2), as plain data so the rules are unit
 * tested on the JVM; [DeviceKeys] maps them onto `KeyGenParameterSpec`.
 */
data class KeyPolicy(
    val alias: String,
    /** Every use needs the user (biometric or device credential), with no validity window. */
    val userPresenceEveryUse: Boolean,
    /** A new fingerprint or face enrolment destroys the key. */
    val invalidatedByEnrollment: Boolean,
    /** Allowed authenticators on API 30+: strong biometrics or the device credential. */
    val allowDeviceCredential: Boolean,
) {
    companion object {
        /** Signs DecisionClaims only, through BiometricPrompt with a CryptoObject. */
        fun decide(instanceId: String) = KeyPolicy("decide-$instanceId", userPresenceEveryUse = true, invalidatedByEnrollment = true, allowDeviceCredential = true)

        /** Signs DPoP proofs; needs no user, so background refresh works. */
        fun dpop(instanceId: String) = KeyPolicy("dpop-$instanceId", userPresenceEveryUse = false, invalidatedByEnrollment = false, allowDeviceCredential = false)
    }
}
