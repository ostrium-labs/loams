package dev.loams.conformance

import com.connectrpc.Code
import com.connectrpc.ConnectException
import com.connectrpc.ResponseMessage
import com.connectrpc.getOrThrow
import dev.loams.core.decision.Decision
import dev.loams.core.decision.DecisionClaims
import dev.loams.core.decision.EcdsaSignatures
import dev.loams.core.decision.Jws
import dev.loams.core.errors.Reason
import dev.loams.core.jose.Jwk
import dev.loams.core.pairing.PairingPayloads
import dev.loams.core.pairing.PairingResult
import dev.loams.core.push.PushKeyPair
import dev.loams.core.push.PushOpener
import dev.loams.core.push.Shown
import dev.loams.core.watch.ResumingWatch
import dev.loams.core.watch.WatchState
import dev.loams.proto.loams.approvals.v1.Approval
import dev.loams.proto.loams.approvals.v1.ApprovalState
import dev.loams.proto.loams.approvals.v1.DecideApprovalRequest
import dev.loams.proto.loams.approvals.v1.DecisionKind
import dev.loams.proto.loams.approvals.v1.GetApprovalRequest
import dev.loams.proto.loams.approvals.v1.ListApprovalsRequest
import dev.loams.proto.loams.devices.v1.CreatePairingRequest
import dev.loams.proto.loams.devices.v1.GetNotificationPreferencesRequest
import dev.loams.proto.loams.devices.v1.ListDevicesRequest
import dev.loams.proto.loams.devices.v1.PushProvider
import dev.loams.proto.loams.devices.v1.RegisterPushTargetRequest
import dev.loams.proto.loams.devices.v1.RenameDeviceRequest
import dev.loams.proto.loams.devices.v1.RevokeDeviceRequest
import dev.loams.proto.loams.devices.v1.SendTestNotificationRequest
import dev.loams.proto.loams.devices.v1.SetNotificationPreferencesRequest
import dev.loams.proto.loams.devices.v1.UnregisterPushTargetRequest
import dev.loams.proto.loams.instance.v1.GetInstanceRequest
import dev.loams.proto.loams.instance.v1.WhoAmIRequest
import dev.loams.proto.loams.notifications.v1.ListNotificationsRequest
import dev.loams.proto.loams.notifications.v1.MarkReadRequest
import dev.loams.proto.loams.notifications.v1.Notification
import dev.loams.proto.loams.operations.v1.CancelOperationRequest
import dev.loams.proto.loams.operations.v1.GetOperationRequest
import dev.loams.proto.loams.operations.v1.ListOperationsRequest
import dev.loams.proto.loams.operations.v1.Operation
import dev.loams.transport.Clients
import dev.loams.transport.DeviceRegistration
import dev.loams.transport.Http
import dev.loams.transport.PairingOutcome
import dev.loams.transport.QrPairing
import dev.loams.transport.TokenEndpoint
import dev.loams.transport.TokenResult
import dev.loams.transport.TrustPolicy
import dev.loams.transport.reason
import dev.loams.transport.watchApprovals
import dev.loams.transport.watchOperations
import java.security.KeyPair
import java.security.KeyPairGenerator
import java.security.Signature
import java.security.interfaces.ECPublicKey
import java.security.spec.ECGenParameterSpec
import java.time.Instant
import java.util.UUID
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Before
import org.junit.Test

/**
 * The app's network layer against the mock, on the JVM (design §37 §12 layer 3). Each test pairs
 * its own device and creates its own approvals, so tests do not depend on each other.
 */
class MockConformanceTest {
    private val base = System.getenv("LOAMS_MOCK_URL").orEmpty().trimEnd('/')
    private val http by lazy { Http.client(TrustPolicy.System) }

    @Before
    fun needsMock() = assumeTrue("LOAMS_MOCK_URL is not set; start ../mock to run conformance", base.isNotEmpty())

    private class Device(val clients: Clients, val key: KeyPair, val deviceId: String)

    private fun mockGet(path: String): String =
        http.newCall(Request.Builder().url(base + path).build()).execute().use { it.body.string() }

    private fun mockPost(path: String): String =
        http.newCall(Request.Builder().url(base + path).post(ByteArray(0).toRequestBody()).build()).execute().use { it.body.string() }

    private fun newKey(): KeyPair = KeyPairGenerator.getInstance("EC").apply { initialize(ECGenParameterSpec("secp256r1")) }.generateKeyPair()

    private suspend fun pair(): Device {
        val text = mockGet("/mock/pairing?issuer=$base")
        val payload = (PairingPayloads.parse(text, Instant.now(), allowInsecureLoopback = true) as PairingResult.Ok).payload
        val key = newKey()
        val outcome = QrPairing.pair(payload, "conformance-${UUID.randomUUID()}") { Jwk.fromEcPublicKey(key.public as ECPublicKey).toJson() }
        val paired = (outcome as PairingOutcome.Paired).instance
        return Device(Clients(base, http, { paired.tokens.accessToken }), key, paired.tokens.deviceId)
    }

    private fun proof(key: KeyPair, claims: DecisionClaims): String {
        val input = Jws.signingInput(Jws.DECISION_HEADER, claims.canonicalJson())
        val der = Signature.getInstance("SHA256withECDSA").run {
            initSign(key.private)
            update(input.toByteArray())
            sign()
        }
        return Jws.compact(input, EcdsaSignatures.derToRaw(der))
    }

    private fun newApprovalId(): String =
        Json.parseToJsonElement(mockPost("/mock/approvals")).jsonObject["approval_id"]!!.jsonPrimitive.content

    @Test
    fun qr_pairing_round_trip_with_mock() = runBlocking {
        val d = pair()
        val who = d.clients.instance.whoAmI(WhoAmIRequest.getDefaultInstance()).getOrThrow()
        assertEquals(d.deviceId, who.device.id)
        assertEquals("usr_alice", who.principal.id)
    }

    @Test
    fun pairing_code_reuse_is_pairing_used() = runBlocking {
        val payload = (PairingPayloads.parse(mockGet("/mock/pairing?issuer=$base"), Instant.now(), true) as PairingResult.Ok).payload
        val tokens = TokenEndpoint(base, http)
        val reg = DeviceRegistration("x", Jwk.fromEcPublicKey(newKey().public as ECPublicKey).toJson())
        assertTrue(tokens.redeemPairing(payload.code, null, reg) is TokenResult.Ok)
        val again = tokens.redeemPairing(payload.code, null, reg) as TokenResult.Refused
        assertEquals(Reason.PAIRING_USED, again.reason)
    }

    @Test
    fun approve_with_a_signed_decision_reaches_the_watch() = runBlocking {
        val d = pair()
        val id = newApprovalId()
        val state = WatchState<Approval> { it.id }
        val watch = launch(Dispatchers.IO) { ResumingWatch(state, { d.clients.watchApprovals(it) }).run() }
        try {
            eventually { state.items.value.any { it.id == id } }
            val a = d.clients.approvals.getApproval(GetApprovalRequest.newBuilder().setApprovalId(id).build()).getOrThrow().approval
            val claims = DecisionClaims(id, a.revision, Decision.APPROVE, Instant.now().epochSecond, UUID.randomUUID().toString())
            val decided = d.clients.approvals.decideApproval(
                DecideApprovalRequest.newBuilder().setApprovalId(id).setRevision(a.revision).setDecision(DecisionKind.DECISION_KIND_APPROVE)
                    .setDecisionProof(proof(d.key, claims)).setIdempotencyKey(UUID.randomUUID().toString()).build(),
            ).getOrThrow().approval
            assertEquals(ApprovalState.APPROVAL_STATE_APPROVED, decided.state)
            // It leaves the pending set on the open stream.
            eventually { state.items.value.none { it.id == id } }
        } finally {
            watch.cancel()
        }
    }

    @Test
    fun decision_without_proof_is_step_up_required() = runBlocking {
        val d = pair()
        val id = newApprovalId()
        val r = d.clients.approvals.decideApproval(
            DecideApprovalRequest.newBuilder().setApprovalId(id).setRevision(1).setDecision(DecisionKind.DECISION_KIND_APPROVE).build(),
        )
        assertEquals(Reason.STEP_UP_REQUIRED, (r as ResponseMessage.Failure).cause.reason())
    }

    @Test
    fun stream_drop_and_resume() = runBlocking {
        val d = pair()
        val state = WatchState<Operation> { it.id }
        val watch = ResumingWatch(state, { d.clients.watchOperations(it) })
        val job = launch(Dispatchers.IO) { watch.run() }
        try {
            eventually { state.cursor != null }
            mockPost("/mock/drop-streams")
            val id = newApprovalId()
            val opId = "op_" + id.removePrefix("apr_")
            // Created while (or right after) the stream was down: delivered by the resume.
            eventually { state.items.value.any { it.id == opId } }
        } finally {
            job.cancel()
        }
    }

    @Test
    fun sealed_push_round_trip() = runBlocking {
        val d = pair()
        val keys = PushKeyPair.generate()
        val token = "conformance-fcm-${UUID.randomUUID()}"
        d.clients.devices.registerPushTarget(
            RegisterPushTargetRequest.newBuilder().setProvider(PushProvider.PUSH_PROVIDER_FCM).setTokenOrEndpoint(token)
                .setAppId("dev.loams.app").setHpkePublicKey(com.google.protobuf.ByteString.copyFrom(keys.publicKey)).build(),
        ).getOrThrow()
        val id = newApprovalId()
        val log = Json.parseToJsonElement(mockGet("/mock/push-log?token=$token")).jsonArray
        assertEquals(1, log.size)
        val data = log[0].jsonObject["data"]!!.jsonObject.mapValues { it.value.jsonPrimitive.content }
        assertEquals(setOf("v", "i", "n", "s"), data.keys)
        val shown = PushOpener({ keys.privateKey }) { plain ->
            val n = Notification.parseFrom(plain)
            Shown(n.title, n.body, n.id, n.approvalId)
        }.open(data)
        assertEquals("Approval needed", shown.title)
        assertEquals(id, shown.approvalId)
    }

    /** Every RPC answers with a message or a typed error: names and types match the server. */
    @Test
    fun endpoint_catalogue() = runBlocking {
        val d = pair()
        val c = d.clients
        val typed = setOf(Code.NOT_FOUND, Code.FAILED_PRECONDITION, Code.INVALID_ARGUMENT, Code.PERMISSION_DENIED)
        val calls: List<Pair<String, suspend () -> ResponseMessage<*>>> = listOf(
            "GetInstance" to { c.instance.getInstance(GetInstanceRequest.getDefaultInstance()) },
            "WhoAmI" to { c.instance.whoAmI(WhoAmIRequest.getDefaultInstance()) },
            "CreatePairing" to { c.devices.createPairing(CreatePairingRequest.getDefaultInstance()) },
            "ListDevices" to { c.devices.listDevices(ListDevicesRequest.getDefaultInstance()) },
            "RenameDevice" to { c.devices.renameDevice(RenameDeviceRequest.newBuilder().setDeviceId("dev_none").setName("x").build()) },
            "RegisterPushTarget" to { c.devices.registerPushTarget(RegisterPushTargetRequest.getDefaultInstance()) },
            "UnregisterPushTarget" to { c.devices.unregisterPushTarget(UnregisterPushTargetRequest.newBuilder().setPushTargetId("pt_none").build()) },
            "GetNotificationPreferences" to { c.devices.getNotificationPreferences(GetNotificationPreferencesRequest.getDefaultInstance()) },
            "SetNotificationPreferences" to { c.devices.setNotificationPreferences(SetNotificationPreferencesRequest.getDefaultInstance()) },
            "SendTestNotification" to { c.devices.sendTestNotification(SendTestNotificationRequest.getDefaultInstance()) },
            "ListApprovals" to { c.approvals.listApprovals(ListApprovalsRequest.getDefaultInstance()) },
            "GetApproval" to { c.approvals.getApproval(GetApprovalRequest.newBuilder().setApprovalId("apr_none").build()) },
            "DecideApproval" to { c.approvals.decideApproval(DecideApprovalRequest.newBuilder().setApprovalId("apr_none").build()) },
            "GetOperation" to { c.operations.getOperation(GetOperationRequest.newBuilder().setOperationId("op_none").build()) },
            "ListOperations" to { c.operations.listOperations(ListOperationsRequest.getDefaultInstance()) },
            "CancelOperation" to { c.operations.cancelOperation(CancelOperationRequest.newBuilder().setOperationId("op_none").build()) },
            "ListNotifications" to { c.notifications.listNotifications(ListNotificationsRequest.getDefaultInstance()) },
            "MarkRead" to { c.notifications.markRead(MarkReadRequest.getDefaultInstance()) },
            // Last: revokes this test's device.
            "RevokeDevice" to { c.devices.revokeDevice(RevokeDeviceRequest.newBuilder().setDeviceId(d.deviceId).build()) },
        )
        for ((name, call) in calls) {
            when (val r = call()) {
                is ResponseMessage.Success -> Unit
                is ResponseMessage.Failure -> assertTrue("$name: ${r.cause.code} ${r.cause.message}", r.cause.code in typed)
            }
        }
    }

    private suspend fun eventually(timeoutMillis: Long = 10_000, check: () -> Boolean) {
        withTimeout(timeoutMillis) {
            while (!check()) delay(50)
        }
    }

    @Suppress("unused")
    private fun ConnectException.describe() = "$code $message"
}
