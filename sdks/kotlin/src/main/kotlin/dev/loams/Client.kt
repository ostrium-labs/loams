package dev.loams

import com.google.protobuf.DynamicMessage

/**
 * The client, and the module surface a caller reaches it through.
 *
 * ## Why there is no `loams.kotlin` facade renderer
 *
 * Design §44 §7.3 has `protoc-gen-loams-facade` render one facade per language from the
 * `loams.options.v1` annotations, so thirteen SDKs cannot drift. **There is no Kotlin
 * renderer** in `crates/loams-facade-gen` (it ships `typescript.rs` only). This SDK
 * therefore takes §7.3's **Q604 fallback**, and takes it one step further than the C# and
 * C++ SDKs did: rather than a hand-written *table* of bindings transcribed from the
 * annotations, [Facade] reads the annotations out of the committed descriptor set at run
 * time. See [Descriptors] for why a table is the thing to avoid here.
 *
 * ## What that costs, stated plainly
 *
 * A descriptor-driven facade is **reflective**, where a generated one is not: a call
 * resolves its binding through a map lookup and encodes through `DynamicMessage` rather
 * than through a generated class. That is a real cost on a hot path, and it is the
 * trade this SDK makes in exchange for a facade that cannot drift from `proto/`. The
 * generated path arrives with the renderer, and nothing under `dev.loams` has to move
 * for it: [CallBinding] is already the same shape a renderer would emit, and
 * [Descriptors.bindingForRpc] is what a generated registry would replace.
 *
 * ## Blocking, deliberately
 *
 * Every call here is synchronous. Kotlin's own idiom for this is `suspend`, which would
 * require `kotlinx-coroutines` as a dependency — and a dependency every Loams Android
 * app would then have to reconcile with its own coroutine version. A blocking SDK is
 * callable from `suspend` functions without anything but `withContext`, and a
 * `suspend` SDK is not callable from a blocking caller at all. See `DEPENDENCIES.md`.
 */
class LoamsClient(
    /**
     * The base URL, for example `https://api.loams.dev`.
     *
     * Ignored when [transport] is supplied, which is how a caller points the SDK at a
     * proxy, a loopback server or an in-process stub.
     */
    endpoint: String? = null,
    /** The credentials, or null for an unauthenticated client — which is what `GetInstance` needs. */
    val tokenSource: TokenSource? = null,
    /** The protocol this client speaks. Connect by default; see [Protocol]. */
    val protocol: Protocol = Protocol.CONNECT,
    /** The codec this client sends. Binary protobuf by default (D612). */
    val codec: Codec = Codec.PROTO,
    /** The client's default retry budget; a call overrides it. */
    maxRetries: Int = RetryPolicy.DEFAULT_MAX_RETRIES,
    /** A transport of the caller's own, or null to build an [HttpTransport] from [endpoint]. */
    transport: Transport? = null,
    /** A consistency-token session, or null. Inert today; see [ConsistencySession]. */
    session: ConsistencySession? = null,
    /** How long one attempt may take. Only used when this client builds its own transport. */
    timeout: java.time.Duration = java.time.Duration.ofSeconds(30),
) {
    /** The transport calls go over. */
    val transport: Transport = resolveTransport(endpoint, transport)

    /** The call path every module method and the generic `invoke` delegate to. */
    val invoker: CallInvoker = CallInvoker(
        transport = this.transport,
        tokenSource = tokenSource,
        maxRetries = maxRetries,
        protocol = protocol,
        codec = codec,
        session = session,
    )

    /** The caller's default retry budget. */
    val retryBudget: Int get() = invoker.maxRetries

    /**
     * The module surface, derived from the committed descriptor set.
     *
     * A module with no annotation is absent here rather than present and empty — so
     * `client.approvals` does not compile, and the reason is a proto that has to say
     * which module it belongs to rather than a name this SDK invented.
     */
    val modules: Modules = Modules(this)

    /**
     * One RPC by its path, through the same path a facade method uses.
     *
     * The generic entry point, and the reason the conformance driver can replay the 13
     * `mock_*` fixtures that sit on `ApprovalService` and `DeviceService` with **no
     * module annotation**: there is no facade call for them, and this is the path a
     * facade call takes anyway.
     *
     * @throws NoSuchElementException the descriptor set declares no such RPC, naming the
     *   RPCs it does declare for that service.
     */
    fun binding(rpc: String): CallBinding = requireNotNull(Descriptors.bindingForRpc(rpc)) {
        val service = rpc.substringBefore('/', missingDelimiterValue = "")
        val declared = Descriptors.bindingsByRpc.keys.filter { it.startsWith("$service/") }
        "the committed descriptor set declares no RPC '$rpc'; $service declares " +
            (declared.ifEmpty { listOf("no RPCs at all") })
    }

    /** Makes one unary call by RPC path. The generic path a facade method delegates to. */
    fun invoke(
        rpc: String,
        request: DynamicMessage,
        options: CallOptions? = null,
    ): DynamicMessage = invoker.unary(binding(rpc), request, options)

    /** Opens one server stream by RPC path, reconnecting as [resume] says. */
    fun openStream(
        rpc: String,
        request: DynamicMessage,
        options: CallOptions? = null,
        resume: StreamResume? = null,
    ): ServerStreamHandle = invoker.serverStream(binding(rpc), request, options, resume)

    /**
     * What this instance is: its API packages, its sign-in methods, and which of its
     * services this binary actually serves.
     *
     * No credentials needed — an app calls this first, before sign-in — which is what
     * keeps a bearer out of a cold start entirely.
     */
    fun getInstance(): DynamicMessage =
        invoker.unary(binding("loams.instance.v1.InstanceService/GetInstance"), empty("loams.instance.v1.GetInstanceRequest"))

    /**
     * The API packages this instance serves, or an empty list when the answer did not
     * carry any.
     *
     * The list an SDK **feature-detects** from: a package that is not served answers
     * `unimplemented` with reason `feature_not_in_variant`, and reading that list is one
     * call rather than a call per module. §44 §4.
     */
    fun apiVersions(): List<String> {
        val response = getInstance()
        val field = response.descriptorForType.findFieldByName("api_versions") ?: return emptyList()
        @Suppress("UNCHECKED_CAST")
        return (response.getField(field) as List<String>)
    }

    /**
     * Whether this instance serves [module], from the same call.
     *
     * Returns true for a module whose package the instance did not list at all: a server
     * that serves a package it does not advertise is not a failure to refuse, and a
     * guard that refused here would break every instance that omits the list.
     */
    fun serves(module: String): Boolean {
        val binding = Facade.module(module)
        val served = apiVersions()
        if (served.isEmpty()) {
            return true
        }
        return binding.packageName in served
    }

    /** An empty request message of the named type. */
    private fun empty(fullName: String): DynamicMessage {
        val descriptor = requireNotNull(Descriptors.message(fullName)) {
            "the committed descriptor set carries no $fullName"
        }
        return DynamicMessage.getDefaultInstance(descriptor)
    }
}

/**
 * The module surface: one property per module the descriptor set annotates.
 *
 * Each property resolves its binding by name through [Facade], which reads the
 * annotation — so a call that the proto does not expose on that module does not exist
 * here, and one it does expose is here without anybody editing this file.
 *
 * The properties are on one class rather than generated per module because there is no
 * Kotlin renderer to generate them (see [LoamsClient]); the *names* are still derived,
 * which is the property that matters, since a module that the descriptor set gains is one
 * property away rather than one source edit away.
 */
class Modules(private val client: LoamsClient) {
    /** `loams.instance` — what this instance is, and who the caller is on it. */
    val instance: ModuleFacade get() = ModuleFacade(client, "instance")

    /** `loams.live` — live sync: watch a query set over a server stream. Experimental. */
    val live: ModuleFacade get() = ModuleFacade(client, "live")

    /**
     * `loams.tables` — the table half of `loams.live.v1`: query, mutate, deploy.
     *
     * A **separate** module from [live] because §44 §7.2 splits the service in two, and
     * the split is written on the proto as `FacadeOptions.module = "tables"` — so this
     * is a name the annotation gave, not one this SDK chose.
     */
    val tables: ModuleFacade get() = ModuleFacade(client, "tables")
}

/**
 * One module's calls, resolved by name.
 *
 * Every call goes through [LoamsClient.invoke] or [LoamsClient.openStream], which is the
 * same path the conformance driver uses for the RPCs that carry no module annotation —
 * so there is one implementation of a call and one set of conformance claims to hold up.
 */
class ModuleFacade(private val client: LoamsClient, val module: String) {
    /** The module's catalogue entry, as the descriptor set states it. */
    val binding: ModuleBinding get() = Facade.module(module)

    /** The calls on this module, in name order. */
    val calls: List<CallBinding> get() = binding.calls

    /** The binding a call's `snake_case` proto name identifies. */
    fun call(name: String): CallBinding = Facade.binding(module, name)

    /** Makes one unary call on this module. */
    fun invoke(name: String, request: DynamicMessage, options: CallOptions? = null): DynamicMessage =
        client.invoke(call(name).rpc, request, options)

    /** Opens one server stream on this module. */
    fun openStream(
        name: String,
        request: DynamicMessage,
        options: CallOptions? = null,
        resume: StreamResume? = null,
    ): ServerStreamHandle = client.openStream(call(name).rpc, request, options, resume)

    /**
     * Calls this module only if the instance serves it, and raises the same
     * [FeatureNotInVariantException] the server would otherwise raise (design §44 §4).
     *
     * One `catch` therefore covers "the guard said no" and "the server refused", which is
     * the whole point of the guard raising the same type — and it costs no request
     * against a server that would have said no anyway.
     */
    fun guard(action: () -> DynamicMessage): DynamicMessage {
        if (client.serves(module)) {
            return action()
        }
        throw FeatureNotInVariantException(
            rpc = binding.service,
            metadata = mapOf("variant" to "standard", "package" to binding.packageName),
            hint = "this instance does not serve ${binding.packageName}; read GetInstance.services[] for what it does",
        )
    }
}