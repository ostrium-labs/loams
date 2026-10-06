package dev.loams.transport

import dev.loams.core.pairing.PairingPayload
import dev.loams.proto.loams.instance.v1.GetInstanceResponse

/** An instance this device is paired with. Tokens are kept by the caller's token store. */
data class PairedInstance(
    val issuer: String,
    val instanceId: String,
    val trust: TrustPolicy,
    val jkt: String,
    val tokens: DeviceTokens,
    val info: GetInstanceResponse,
)

sealed interface PairingOutcome {
    data class Paired(val instance: PairedInstance) : PairingOutcome

    data class Refused(val result: TokenResult.Refused) : PairingOutcome
}

/**
 * QR pairing (design §37 §7.2.2 path 1): pin [PairingPayload.spki] before the first byte, check
 * the instance id and key thumbprint, then redeem the code with the device's decision key.
 *
 * @param decisionKey creates (or loads) the hardware decision key for this instance and returns
 *   its public JWK; called only after the instance checks pass.
 */
object QrPairing {
    suspend fun pair(payload: PairingPayload, deviceName: String, decisionKey: suspend (instanceId: String) -> String): PairingOutcome {
        val trust = payload.spki?.let { TrustPolicy.Pinned(it.toSet()) } ?: TrustPolicy.System
        val http = Http.client(trust)
        val issuer = payload.issuer.toString().trimEnd('/')
        // GetInstance needs no token.
        val clients = Clients(issuer, http, { null })
        val info = InstanceCheck.verify(clients, http, payload.instanceId, payload.jkt)
        val jwk = decisionKey(payload.instanceId)
        return when (val r = TokenEndpoint(issuer, http).redeemPairing(payload.code, null, DeviceRegistration(deviceName, jwk))) {
            is TokenResult.Ok -> PairingOutcome.Paired(PairedInstance(issuer, payload.instanceId, trust, payload.jkt, r.tokens, info))
            is TokenResult.Refused -> PairingOutcome.Refused(r)
        }
    }
}

