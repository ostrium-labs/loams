package dev.loams.test.Support

import com.google.protobuf.ByteString
import com.google.protobuf.DynamicMessage
import dev.loams.CallBinding
import dev.loams.CallInvoker
import dev.loams.CallOptions
import dev.loams.Codec
import dev.loams.ContentTypes
import dev.loams.Descriptors
import dev.loams.EnvelopeFlags
import dev.loams.Envelopes
import dev.loams.Idempotency
import dev.loams.Json
import dev.loams.LoamsException
import dev.loams.MessageCodec
import dev.loams.Protocol
import dev.loams.Streaming
import java.io.File

/** What one fixture's replay found. */
data class FixtureOutcome(
    /** The fixture's name, as the manifest spells it. */
    val name: String,
    /** Whether the driver replayed it **and** every assertion held. */
    val ran: Boolean,
    /** What failed, with the fixture's name in each message. */
    val failures: List<String>,
)

/**
 * The whole run's result, and the report written from it.
 *
 * [ok] is what `kotlin_conformance_all_required_fixtures` asserts, and it is two
 * conditions rather than one: every required fixture ran, and nothing failed
 * anywhere. A run that replayed everything and disagreed with the corpus somewhere
 * is not a pass.
 */
data class CorpusRun(
    val outcomes: List<FixtureOutcome>,
    /** Where it ran, for the report and for a failure message. */
    val endpoint: String,
    /** Whether that endpoint was a real server rather than a replay. Always false here. */
    val live: Boolean,
) {
    /** The fixture names the driver replayed, which is what `ran` is. */
    val ran: List<String> get() = outcomes.filter { it.ran }.map { it.name }

    /** Every failure, across every fixture. */
    val failures: List<String> get() = outcomes.flatMap { it.failures }

    /** Whether every required fixture ran and every assertion held. */
    val ok: Boolean get() = failures.isEmpty() && outcomes.all { it.ran }
}

/**
 * The corpus driver: it replays `sdks/fixtures` through the SDK and reports what
 * it actually ran.
 *
 * # Why the driver derives its coverage rather than listing it
 *
 * `sdks/conformance/required.mjs` is explicit that `ran` is the only thing that
 * counts: "a test that passes without touching a required fixture has not run
 * it." A hand-written list of fixture names would be a second source of truth that
 * could name a fixture the suite stopped replaying, and the gate would pass on it —
 * so the list here is read from `manifest.json` at run time, the fixture is
 * **replayed**, and the name goes into `ran` only once the recorded response and
 * the recording's own `expect` block have both been checked. A fixture the driver
 * could not reach is therefore absent from `ran` and the gate says so by name.
 *
 * The same rule governs every other decision in here. Which call a step is driven
 * through, whether its idempotency key is minted, and where a named response field
 * lives are all asked of the **schema** — the RPC path, the request and response
 * descriptors — and never of a fixture's name. A rule written as a list of names
 * stops tracking what the driver did the moment the corpus grows.
 *
 * # Why the mock fixtures need no facade
 *
 * The 13 `mock_*` fixtures sit on `ApprovalService` and `DeviceService`, which
 * carry **no** `loams.options.v1.module` annotation — so the corpus has no facade
 * for them, and `Facade.modules` does not list them. They are driven through the
 * same generic `CallInvoker.unary` every facade method delegates to, with the
 * binding resolved from the **service descriptor** the RPC path names. There is no
 * table here to add a fixture to, and nothing to fall out of step with the corpus.
 *
 * # What a fixture run asserts
 *
 * Everything the recording states about itself, and nothing it does not:
 *
 *  - the **request** is byte-identical to the recording, because
 *    `fixture-server.mjs` answers 400 otherwise — which is what proves the SDK
 *    encoded what the corpus recorded rather than something equivalent;
 *  - the **outcome** matches `expect`: the HTTP status as the SDK saw it, the
 *    `reason`, the `grpcStatus`, and the response fields the recording names;
 *  - `identicalToStep` means the **SDK's own** two answers are the same bytes. It
 *    is not a comparison of the recording against itself, which would hold for a
 *    driver that never looked at either answer.
 */
class CorpusDriver(private val corpusDir: File) {
    private val clients = mutableMapOf<Pair<Protocol, Codec>, CallInvoker>()

    private val manifest: Json.JsonObject by lazy {
        val file = File(corpusDir, "manifest.json")
        require(file.isFile) { "there is no ${file.path}; this is not a Loams fixture corpus" }
        Json.parse(file.readText()).asObject()
            ?: throw IllegalArgumentException("${file.path} is not a JSON object")
    }

    /**
     * Every required fixture, read from the manifest and nothing else.
     *
     * This is the anti-vacuity property, stated as code: the list of fixtures the
     * run is judged on is the manifest's, so a fixture the manifest newly requires
     * is in the next run without anybody editing this file, and a fixture that
     * stops being reachable is named in the failure rather than quietly dropped
     * from a hand-written list.
     */
    val requiredFixtures: List<RequiredFixture> by lazy {
        CorpusRecording.requiredFixtures(manifest as dev.loams.JsonValue)
    }

    /** Runs every required fixture against the fixture server. */
    fun run(): CorpusRun {
        val fixtures = corpusDir.absoluteFile
        val outcomes = mutableListOf<FixtureOutcome>()
        FixtureServer.start(fixtures).use { server ->
            for (fixture in requiredFixtures) {
                outcomes.add(runFixture(server.endpoint, fixture))
            }
            return@use CorpusRun(outcomes, server.endpoint, server.live)
        }
    }

    private fun runFixture(endpoint: String, fixture: RequiredFixture): FixtureOutcome {
        val file = File(corpusDir, fixture.file)
        require(file.isFile) {
            // A manifest entry with no file behind it is a corpus bug, and saying
            // so by name is what makes it legible rather than a missing-file error
            // pointing at a path nobody wrote.
            return FixtureOutcome(fixture.name, ran = false, listOf("${fixture.name}: ${file.path} does not exist"))
        }
        val steps = CorpusRecording.readSteps(fixture.name, Json.parse(file.readText()))
        if (steps.isEmpty()) {
            return FixtureOutcome(fixture.name, ran = false, listOf("${fixture.name}: the recording has no steps"))
        }

        // The binding is resolved from the RPC path, not looked up in a table: a
        // fixture whose RPC this SDK's descriptor set does not carry is a fixture
        // this suite cannot run, and saying so by name is what makes the gate's
        // gap legible.
        val rpc = steps[0].request.path.removePrefix("/")
        val binding = Descriptors.bindingForRpc(rpc)
            ?: return FixtureOutcome(
                fixture.name,
                ran = false,
                listOf("$rpc: the committed descriptor set declares no service with that RPC, so this suite cannot run it"),
            )

        val failures = mutableListOf<String>()

        // The SDK's **own** answer per step, in step order, for `identicalToStep`.
        // The recording's bytes are deliberately not collected: comparing them to
        // each other is a check that holds for a driver that never looked at what
        // the SDK made of them.
        val answers = mutableListOf<ByteArray?>()
        for (step in steps) {
            val outcome = runStep(endpoint, fixture.name, step)
            failures.addAll(outcome.problems)
            answers.add(outcome.answer)

            val identical = step.expect?.asObject()?.get("identicalToStep") ?: continue
            val against = identical.asLong()?.toInt() ?: continue
            if (against < 0 || against > answers.size - 2) {
                failures.add(
                    "${fixture.name} step ${step.step}: expect.identicalToStep names step $against, and this is " +
                        "step ${answers.size - 1}, so there is no earlier step for it to be identical to"
                )
                continue
            }
            val mine = outcome.answer
            val theirs = answers[against]
            if (theirs == null || mine == null || !mine.contentEquals(theirs)) {
                failures.add(
                    "${fixture.name} step ${step.step}: expect.identicalToStep says its response bytes are " +
                        "step $against's, and the SDK's two answers are not the same"
                )
            }
        }

        if (failures.isNotEmpty()) {
            return FixtureOutcome(fixture.name, ran = false, failures)
        }
        return FixtureOutcome(fixture.name, ran = true, emptyList())
    }

    /**
     * One step's problems, and the answer the SDK made of it.
     *
     * [answer] is the SDK's own re-encoding in **binary protobuf** whatever the
     * step's encoding is, for the reason `dev.loams.CompactJson` gives: it
     * deliberately refuses a message holding a well-known type, and
     * `DecideApprovalResponse.approval.created_at` is exactly such a message, so
     * the JSON re-encode would be unavailable on precisely the fixture that needs
     * it. Binary protobuf has no such gap and is the stricter comparison anyway: it
     * covers every field of both messages, including those proto3 JSON omits at
     * their defaults.
     */
    private class StepOutcome(val problems: List<String>, val answer: ByteArray?)

    private fun runStep(endpoint: String, fixtureName: String, step: RecordedStep): StepOutcome {
        val problems = mutableListOf<String>()
        val rpc = step.request.path.removePrefix("/")
        val binding = Descriptors.bindingForRpc(rpc)
            ?: return StepOutcome(listOf("$fixtureName step ${step.step}: no descriptor for $rpc"), null)

        val (protocol, codec) = transportOf(step.request.contentType)
        val invoker = invokerFor(endpoint, protocol, codec)

        val request = try {
            decodeRequest(binding, step, codec)
        } catch (failure: IllegalArgumentException) {
            return StepOutcome(
                listOf("$fixtureName step ${step.step}: the recorded request does not decode as a ${binding.request.name} (${failure.message})"),
                null,
            )
        }

        val options = CallOptions(
            // The two headers the harness defines: which fixture answers, and
            // which step of it. Both are needed — six recorded scenarios share the
            // `WatchApprovals` Connect-JSON key, and the idempotency replay sends
            // the same request twice.
            headers = mapOf(
                "loams-fixture-name" to fixtureName,
                "loams-fixture-step" to step.step.toString(),
            ),
            // R3 deliberately **not** applied to a recorded keyless mutation. See
            // `keylessMutation` for why that is decided from the schema and not
            // from a list of fixture names (D655).
            mintIdempotencyKey = !keylessMutation(request),
        )

        if (binding.streaming == Streaming.SERVER) {
            return runStreamStep(invoker, binding, request, options, step, fixtureName)
        }

        val response = try {
            invoker.unary(binding, request, options)
        } catch (error: LoamsException) {
            return StepOutcome(Expectations.checkError(fixtureName, step.step, step.expect, error), null)
        }

        problems.addAll(
            Expectations.checkSuccess(
                fixtureName, step.step, step.status, step.expect, response, expected = step.status == 200,
            )
        )
        return StepOutcome(problems, MessageCodec.serialize(response, Codec.PROTO))
    }

    /**
     * Whether this step is one the SDK's own keyed path cannot reproduce.
     *
     * **D610 gives every mutation an idempotency key**, and six app-mock
     * mutations were recorded *without* one — the client that recorded them did not
     * send a key, so there was none to record. Putting one on the wire changes the
     * request, and `fixture-server.mjs` — correctly — refuses a request that is
     * not the recorded one, answering 400 with its own JSON error body. That answer
     * is not a Connect error envelope, so the SDK reported `code=unknown` with no
     * reason against a server that had said exactly which reason it meant: 18
     * approvals and 10 devices calls, every one of them a harness disagreement
     * dressed up as an error-mapping bug.
     *
     * Decided from the **request schema** and the decoded message — the same two
     * questions `Idempotency.apply` asks — so a corpus that grows a keyed mutation
     * is handled by the rule rather than by somebody remembering to move a name. A
     * request whose schema declares no key field is never keyless here, because
     * there was nothing to mint; and a request that already carries one keeps it,
     * so `mock_state_idempotent_decide` and the two `mock_state_stream_resume*`
     * mutations are still driven through R3's keyed path, which is the only place
     * that clause is exercised end to end.
     */
    private fun keylessMutation(request: DynamicMessage): Boolean {
        val field = request.descriptorForType.findFieldByName("idempotency_key") ?: return false
        if (field.type != com.google.protobuf.Descriptors.FieldDescriptor.Type.STRING) return false
        return (request.getField(field) as? String).isNullOrEmpty()
    }

    private fun runStreamStep(
        invoker: CallInvoker,
        binding: CallBinding,
        request: DynamicMessage,
        options: CallOptions,
        step: RecordedStep,
        fixtureName: String,
    ): StepOutcome {
        val problems = mutableListOf<String>()
        val handle = try {
            invoker.serverStream(binding, request, options, resume = null)
        } catch (error: LoamsException) {
            return StepOutcome(Expectations.checkError(fixtureName, step.step, step.expect, error), null)
        }

        val messages = mutableListOf<DynamicMessage>()
        val answer = java.io.ByteArrayOutputStream()
        try {
            handle.messages.forEach { message ->
                messages.add(message)
                // The answer is the SDK's own re-encoding of the messages it
                // yielded, which is what `identicalToStep` compares. A stream
                // recording never uses `identicalToStep`, so this costs one
                // re-encode per message and keeps the one comparison honest.
                answer.write(MessageCodec.serialize(message, Codec.PROTO))
            }
        } catch (error: LoamsException) {
            return StepOutcome(Expectations.checkError(fixtureName, step.step, step.expect, error), null)
        }

        val expect = step.expect?.asObject()
        if (expect == null) {
            return StepOutcome(problems, answer.toByteArray())
        }

        // `frames` counts every frame on the wire and `frameKinds` names them in
        // order, so the heartbeat fixture's two frames and one message is checked as
        // two frames whose kinds are `snapshot`, `heartbeat` — and the messages the
        // caller saw are one.
        expect["frames"]?.asLong()?.let { want ->
            if (handle.frameKinds.size.toLong() != want) {
                problems.add(
                    "$fixtureName step ${step.step}: expect.frames is $want, and ${handle.frameKinds.size} " +
                        "frame(s) arrived (${handle.frameKinds})"
                )
            }
        }
        expect["frameKinds"]?.asArray()?.let { want ->
            val wanted = want.mapNotNull { it.asString() }
            if (handle.frameKinds != wanted) {
                problems.add(
                    "$fixtureName step ${step.step}: expect.frameKinds is [$wanted], and " +
                        "[${handle.frameKinds}] arrived"
                )
            }
        }

        for (message in messages) {
            problems.addAll(Expectations.checkMessage(fixtureName, step.step, step.expect, message))
        }
        if (messages.isEmpty() && expect["frameKinds"] != null) {
            problems.add("$fixtureName step ${step.step}: expect.frameKinds is set and no message arrived")
        }
        return StepOutcome(problems, answer.toByteArray())
    }

    /**
     * The recorded request, decoded into the binding's request type.
     *
     * Decoded rather than sent verbatim: the point of a fixture is that the SDK
     * **encodes** what the corpus recorded, so handing the server the recording's
     * own bytes would prove nothing. The re-encoded request goes out and
     * `fixture-server.mjs` compares it byte for byte, which is where an SDK whose
     * JSON writer emits a space fails.
     */
    private fun decodeRequest(binding: CallBinding, step: RecordedStep, codec: Codec): DynamicMessage {
        // A stream's request body is one framed message; a unary's is the message
        // itself. Either way the payload is what has to decode.
        val payload = if (step.request.contentType.startsWith("application/connect") ||
            step.request.contentType.startsWith("application/grpc-web")
        ) {
            Envelopes.split(step.request.body).firstOrNull()?.payload
                ?: throw IllegalArgumentException("the recorded request carries no frame")
        } else {
            step.request.body
        }
        return MessageCodec.deserialize(binding.request, payload, codec)
    }

    /** The protocol and codec a recorded content type implies. */
    private fun transportOf(contentType: String): Pair<Protocol, Codec> = when (contentType.split(";")[0].trim()) {
        ContentTypes.CONNECT_UNARY_JSON, ContentTypes.CONNECT_STREAM_JSON -> Protocol.CONNECT to Codec.JSON
        ContentTypes.CONNECT_UNARY_PROTO, ContentTypes.CONNECT_STREAM_PROTO -> Protocol.CONNECT to Codec.PROTO
        ContentTypes.GRPC_PROTO -> Protocol.GRPC_WEB to Codec.PROTO
        ContentTypes.GRPC_JSON -> Protocol.GRPC_WEB to Codec.JSON
        else -> throw IllegalArgumentException(
            "the corpus records a content type this SDK does not speak: '$contentType'"
        )
    }

    /**
     * One invoker per (protocol, codec) pair, built lazily.
     *
     * The corpus records four encodings of the same RPC and a client that picked
     * one would fail the other three, which is the point of them.
     */
    private fun invokerFor(endpoint: String, protocol: Protocol, codec: Codec): CallInvoker =
        clients.getOrPut(protocol to codec) {
            // No retries: the harness serves one recorded response per request, and
            // a retry would ask it for a second step's bytes and get a 400. The
            // retry policy is pinned by `kotlin_retry_reuses_idempotency_key`
            // against a stub instead.
            dev.loams.LoamsClient(
                endpoint = endpoint,
                protocol = protocol,
                codec = codec,
                maxRetries = 0,
            ).invoker
        }

    /** The recorded step's own frame kinds, for a test that wants them. */
    fun recordedFrameKinds(step: RecordedStep): List<EnvelopeFlags> =
        step.frames.orEmpty().map { EnvelopeFlags.of(it.flags) }

    /** Re-exported so the report can name the codec family without a second import. */
    fun familyOf(contentType: String): String = when (transportOf(contentType).second) {
        Codec.JSON -> "json"
        Codec.PROTO -> "proto"
    }

    @Suppress("unused")
    private fun unusedImports(): ByteArray? = null
}