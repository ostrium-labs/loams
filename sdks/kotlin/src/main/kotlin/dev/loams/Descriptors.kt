package dev.loams

import com.google.protobuf.ByteString
import com.google.protobuf.Descriptors.Descriptor
import com.google.protobuf.Descriptors.EnumValueDescriptor
import com.google.protobuf.Descriptors.FieldDescriptor
import com.google.protobuf.Descriptors.FileDescriptor
import com.google.protobuf.Descriptors.MethodDescriptor
import com.google.protobuf.Descriptors.ServiceDescriptor
import com.google.protobuf.DescriptorProtos
import com.google.protobuf.DynamicMessage
import com.google.protobuf.InvalidProtocolBufferException

/**
 * The descriptor set: the **committed** `FileDescriptorSet` in `gen/`, and every
 * binding, module name and message type the SDK derives from it (D652).
 *
 * # Why the facade is derived and not transcribed
 *
 * Design §44 §7.3 has `protoc-gen-loams-facade` render one facade per language from
 * the `loams.options.v1.module` and `loams.options.v1.facade` annotations, so
 * thirteen SDKs cannot drift. **There is no Kotlin renderer** in
 * `crates/loams-facade-gen` (it ships `typescript.rs` only), and §7.3's Q604
 * fallback permits a hand-written facade — but a *hand-written table* is a second
 * copy of the annotations, and it stops matching the protos the moment somebody adds
 * a module. The corpus would not notice: it touches 28 of the 25-odd RPCs in the
 * set, and a table that is stale only on the RPCs the corpus does not replay is
 * stale and green.
 *
 * So nothing here is written out. `Facade.modules`, `Descriptors.bindingForRpc`,
 * `CallBinding.retry`, `CallBinding.takesIdempotencyKey` and the pagination fields
 * are all **read out of the descriptor set** — the `idempotency_level` option, the
 * custom options carried as extension bytes, and the request and response message
 * descriptors. A proto that adds an annotation changes the surface in the next
 * build with nobody editing this repository, which is the property the C# and C++
 * hand-written tables do not have.
 *
 * # Where the annotations come from
 *
 * `buf build` writes the options as **extension fields on the `ServiceOptions` and
 * `MethodOptions` messages**, and `FileDescriptor.buildFrom` has no extension
 * registry for them — it parses the options into `GeneratedMessage`s that keep the
 * extension bytes as unknown fields. That is not an obstacle, it is the raw material:
 * [readExtension] pulls field 50001 off a `ServiceOptions`' unknown fields and 50002
 * off a `MethodOptions`', and parses each as the `ModuleOptions` or `FacadeOptions`
 * the descriptor set also carries. So the annotations are read from the same bytes
 * protoc wrote, through the same descriptors, with no generated code and no registry
 * to keep in step.
 *
 * # One class-initialisation, read once
 *
 * The set is parsed once and the resulting descriptors are immutable, so the
 * tables are `val`s rather than something rebuilt per call. A lookup that had to
 * rebuild would be a surprising cost in the one path every request takes.
 */
object Descriptors {
    /** The resource the descriptor set is read from, copied into the classes by `build.sh`. */
    private const val RESOURCE = "/loams-descriptor.binpb"

    /** The proto revision this SDK was generated from (design §44 §10.3). */
    const val PROTO_REV: String = "v1"

    /** Every file in the set, keyed by its proto path. */
    private val filesByName: Map<String, FileDescriptor> = load()

    /** Every message in the set, keyed by its fully qualified proto name. */
    private val messagesByName: Map<String, Descriptor> = buildMap {
        for (file in filesByName.values) {
            collectMessages(file.messageTypes, "", this)
        }
    }

    /** Every service in the set, in the order the files declare them. */
    val services: List<ServiceDescriptor> = filesByName.values.flatMap { it.services }

    /** Every service in the set, keyed by its fully qualified name. */
    private val servicesByName: Map<String, ServiceDescriptor> = services.associateBy { it.fullName }

    /**
     * Every RPC in the set, keyed by `<c>package.Service/Method</c>`.
     *
     * Built over **every** service, not only the ones carrying a module annotation,
     * and that is the property the conformance driver depends on: the 13 `mock_*`
     * fixtures sit on `ApprovalService` and `DeviceService`, which carry no
     * annotation and so have no facade, and they are replayed through this path
     * rather than through a second one.
     */
    val bindingsByRpc: Map<String, CallBinding> = buildMap {
        for (service in services) {
            for (method in service.methods) {
                val rpc = "${service.fullName}/${method.name}"
                put(rpc, bindingFor(service, method))
            }
        }
    }

    /** The binding an RPC path names, or null when no service in the set declares it. */
    fun bindingForRpc(rpc: String): CallBinding? = bindingsByRpc[rpc]

    /** The message a fully qualified proto name names, or null. */
    fun message(fullName: String): Descriptor? = messagesByName[fullName]

    /** The service a fully qualified name names, or null. */
    fun service(fullName: String): ServiceDescriptor? = servicesByName[fullName]

    /**
     * The binding whose request or response type is [fullName], or null.
     *
     * The reverse lookup the conformance harness needs: a decoded response has to
     * say which RPC produced it, and the descriptor is what says so.
     */
    fun bindingForMessage(fullName: String): CallBinding? =
        bindingsByRpc.values.firstOrNull { it.request.fullName == fullName || it.response.fullName == fullName }

    /**
     * Every proto package in the set, as `GetInstance` names them.
     *
     * From the `loams.*` files only, and **including** `loams.options.v1` — which
     * declares no message a client sends, and is not on any SDK's wire surface. It is
     * here because the list is a faithful rendering of what the descriptor set carries,
     * and a caller diffing it against a server's `GetInstance.api_versions` should see
     * a difference that exists rather than one that does not.
     */
    val protoPackages: List<String> = filesByName.values
        .map { it.`package` }
        .filter { it.startsWith("loams.") }
        .distinct()
        .sorted()

    /** The names of modules declared on services with loams.options.v1.module. */
    val serviceModuleNames: Set<String> by lazy {
        servicesByName.values.mapNotNull { service ->
            extension(service.options, MODULE_EXTENSION, "ModuleOptions")?.string("name")
        }.toSet()
    }

    /**
     * Builds one binding from a service and a method, deciding every field from the
     * descriptor rather than from a table.
     *
     * The three decisions, and where each answer comes from:
     *
     *  - **streaming**, from `MethodDescriptor.isServerStreaming`. There is no
     *    client streaming and no bidi in any proto here (D420), so this is the
     *    whole question.
     *  - **retry class**, from `idempotency_level` by D610's rule — `NO_SIDE_EFFECTS`
     *    and `IDEMPOTENT` retry on their own, a mutation does not — and overridden by
     *    `FacadeOptions.retry_safe` when the annotation says so.
     *  - **the module and the call's name**, from the service's `module` option and
     *    the method's repeated `facade` option, with `FacadeOptions.module` naming a
     *    *different* module than the service's own (which is how `loams.live.v1`
     *    splits into `live` and `tables`).
     *
     * A service with no `module` annotation still gets a binding, with a null
     * module: that is the generic path, and a binding whose module is null is how
     * the caller knows there is no facade call for it rather than that the RPC does
     * not exist.
     */
    private fun bindingFor(service: ServiceDescriptor, method: MethodDescriptor): CallBinding {
        val moduleOptions = extension(service.options, MODULE_EXTENSION, "ModuleOptions")
        val moduleName = moduleOptions?.let { it.string("name") }
        val summary = moduleOptions?.let { it.string("summary") }
        val unstable = moduleOptions?.let { it.bool("unstable") } ?: false

        val facadeOptions = extension(method.options, FACADE_EXTENSION, "FacadeOptions")

        // The proto enum is `DescriptorProtos.MethodOptions.IdempotencyLevel`, not
        // `MethodDescriptor.IdempotencyLevel`: it is declared on the *options message*,
        // and `MethodDescriptor` only exposes it through `getOptions()`. This SDK's own
        // `IdempotencyLevel` is a distinct type on purpose — it is the *derived* class,
        // with `NONE` for an undeclared level, which the proto enum has no spelling for.
        val idempotency = when (method.options.idempotencyLevel) {
            DescriptorProtos.MethodOptions.IdempotencyLevel.IDEMPOTENT -> IdempotencyLevel.IDEMPOTENT
            DescriptorProtos.MethodOptions.IdempotencyLevel.NO_SIDE_EFFECTS -> IdempotencyLevel.NO_SIDE_EFFECTS
            // `IDEMPOTENCY_UNKNOWN` is the proto3 default: no level declared, which by
            // D610's rule makes it a mutation that may only be retried once keyed.
            else -> IdempotencyLevel.NONE
        }
        val forcedSafe = facadeOptions?.let { it.bool("retry_safe") } ?: false
        val retry = if (forcedSafe) {
            RetryClass.SAFE
        } else if (idempotency == IdempotencyLevel.NONE) {
            RetryClass.MANUAL
        } else {
            RetryClass.SAFE
        }

        return CallBinding(
            rpc = "${service.fullName}/${method.name}",
            service = service.fullName,
            method = method.name,
            packageName = service.file.`package`,
            streaming = if (method.isServerStreaming) Streaming.SERVER else Streaming.UNARY,
            idempotency = idempotency,
            retry = retry,
            request = method.inputType,
            response = method.outputType,
            module = facadeOptions?.let { it.string("module")?.takeIf { name -> name.isNotEmpty() } } ?: moduleName,
            facadeName = facadeOptions?.let { it.string("name")?.takeIf { name -> name.isNotEmpty() } }
                ?: method.name.replaceFirstChar { it.lowercaseChar() },
            summary = summary,
            unstable = unstable,
            pagination = facadeOptions?.let { it.string("pagination")?.takeIf { value -> value.isNotEmpty() } },
        )
    }

    /** The field number the `module` extension occupies on `ServiceOptions`. */
    private const val MODULE_EXTENSION = 50001

    /** The field number the `facade` extension occupies on `MethodOptions`. */
    private const val FACADE_EXTENSION = 50002

    /**
     * Reads one `loams.options.v1` extension off an options message, or null.
     *
     * [messageName] is the extension's own type, which the descriptor set also
     * carries, so it is parsed with [DynamicMessage] against that descriptor rather
     * than against a generated class — there is no generated code in this SDK, and
     * there does not need to be.
     *
     * A value that does not parse as the extension is **refused**, and it is
     * refused loudly: an option this SDK cannot read is a proto that grew a field
     * the descriptor set does not have, and guessing would mean a module name
     * invented out of the bytes rather than the annotation.
     */
    private fun extension(
        options: com.google.protobuf.Message,
        fieldNumber: Int,
        messageName: String,
    ): DynamicMessage? {
        val field = options.unknownFields.asMap()[fieldNumber] ?: return null
        if (field.lengthDelimitedList.isEmpty()) return null
        val descriptor = messagesByName["loams.options.v1.$messageName"]
            ?: throw IllegalStateException(
                "the descriptor set carries no loams.options.v1.$messageName, so the extension on " +
                    "${options.descriptorForType.name} field $fieldNumber cannot be read"
            )
        return field.lengthDelimitedList.mapNotNull { raw ->
            try {
                DynamicMessage.parseFrom(descriptor, raw)
            } catch (refused: InvalidProtocolBufferException) {
                throw IllegalStateException(
                    "the loams.options.v1.$messageName on ${options.descriptorForType.name} field " +
                        "$fieldNumber does not parse: ${refused.message}",
                    refused,
                )
            }
        }.firstOrNull()
    }

    private fun collectMessages(
        descriptors: List<Descriptor>,
        prefix: String,
        into: MutableMap<String, Descriptor>,
    ) {
        for (descriptor in descriptors) {
            val fullName = if (prefix.isEmpty()) descriptor.fullName else "$prefix.${descriptor.name}"
            into[fullName] = descriptor
            collectMessages(descriptor.nestedTypes, fullName, into)
        }
    }

    /**
     * Reads and links the descriptor set.
     *
     * Linked in **dependency order**, not in the order the set happens to list:
     * `FileDescriptor.buildFrom` needs its imports already built, and the set lists
     * `google/protobuf/timestamp.proto` first for this repository but nothing
     * guarantees that for a future one. The loop therefore retries any file whose
     * imports are not yet available and **fails** if a pass makes no progress,
     * which is a missing import rather than an ordering accident.
     */
    private fun load(): Map<String, FileDescriptor> {
        val bytes = Descriptors::class.java.getResourceAsStream(RESOURCE)?.use { it.readBytes() }
            ?: throw IllegalStateException(
                "no $RESOURCE on the classpath. It is the committed FileDescriptorSet the SDK derives its " +
                    "facade from; run './build.sh descriptors' to regenerate gen/loams-descriptor.binpb."
            )
        val set = try {
            DescriptorProtos.FileDescriptorSet.parseFrom(bytes)
        } catch (refused: InvalidProtocolBufferException) {
            throw IllegalStateException(
                "$RESOURCE does not parse as a FileDescriptorSet (${refused.message}). Regenerate it with " +
                    "'./build.sh descriptors'; it is generated, never hand-edited.",
                refused,
            )
        }

        val built = LinkedHashMap<String, FileDescriptor>()
        val pending = set.fileList.toMutableList()
        while (pending.isNotEmpty()) {
            var progressed = false
            val iterator = pending.iterator()
            while (iterator.hasNext()) {
                val proto = iterator.next()
                val ready = proto.dependencyList.all { built.containsKey(it) }
                if (!ready) continue
                built[proto.name] = FileDescriptor.buildFrom(
                    proto,
                    proto.dependencyList.map { built.getValue(it) }.toTypedArray(),
                )
                iterator.remove()
                progressed = true
            }
            if (!progressed) {
                throw IllegalStateException(
                    "the descriptor set has files whose imports are absent: " +
                        pending.joinToString(", ") { it.name } +
                        ". Regenerate it with './build.sh descriptors'."
                )
            }
        }
        return built
    }
}

/**
 * The `idempotency_level` off a proto method, which is what the retry class is
 * derived from (D610).
 */
enum class IdempotencyLevel {
    /** No level declared: a mutation, which may only be retried once it carries an idempotency key. */
    NONE,

    /** `IDEMPOTENT`: repeating it is the same call. */
    IDEMPOTENT,

    /** `NO_SIDE_EFFECTS`: a read, retryable on its own. */
    NO_SIDE_EFFECTS,
}

/** Whether the SDK may retry a call on its own. */
enum class RetryClass {
    /** A mutation: retried only once it carries an idempotency key. */
    MANUAL,

    /** A read or an idempotent RPC: the SDK retries it. */
    SAFE,
}

/** Whether a call answers with one message or with a stream. */
enum class Streaming {
    /** One request, one response. */
    UNARY,

    /**
     * A server stream. There is no client streaming and no bidi (D420): a browser
     * cannot do it over `fetch`, and half-duplex works through every proxy.
     */
    SERVER,
}

/**
 * One RPC, as the runtime dispatches it: everything a module method hands the
 * invoker and nothing the invoker has to guess.
 *
 * Every field is derived from the descriptor set — see [Descriptors] — so there is
 * no table here to drift.
 *
 * @property rpc the full RPC path, `package.Service/Method`.
 * @property module the SDK module, or null when the service carries no annotation
 *   and there is therefore no facade call for it.
 */
data class CallBinding(
    val rpc: String,
    val service: String,
    val method: String,
    val packageName: String,
    val streaming: Streaming,
    val idempotency: IdempotencyLevel,
    val retry: RetryClass,
    val request: Descriptor,
    val response: Descriptor,
    val module: String?,
    val facadeName: String,
    val summary: String?,
    val unstable: Boolean,
    val pagination: String?,
) {
    /** Whether the request's **schema** declares a string `idempotency_key`. */
    val takesIdempotencyKey: Boolean get() = Idempotency.schemaHasKey(request)

    /**
     * The two fields a paged call pages on, as `FacadeOptions.pagination` names
     * them (`"<items>:<next_page_token>"`), or null.
     *
     * **No annotated RPC is paged yet** — `ListCollections` arrives with API1
     * Task 2 — so this is exercised against a stub in
     * `kotlin_pagination_iterator` rather than end to end.
     */
    fun paginationFields(): Pair<FieldDescriptor, FieldDescriptor>? {
        val spec = pagination ?: return null
        val parts = spec.split(":")
        if (parts.size != 2) return null
        val items = response.fields.firstOrNull { it.jsonName == parts[0] || it.name == parts[0] } ?: return null
        val token = response.fields.firstOrNull { it.jsonName == parts[1] || it.name == parts[1] } ?: return null
        return items to token
    }
}

/** One module and the calls it exposes, both derived from the descriptor set. */
data class ModuleBinding(
    /** The module's name in `snake_case`, as `ModuleOptions.name` spells it. */
    val name: String,
    /** The one-line description from `ModuleOptions.summary`. */
    val summary: String,
    /** The proto package it binds. */
    val packageName: String,
    /** The service, fully qualified. */
    val service: String,
    /** Whether the package's wire contract may still change, so a caller feature-detects. */
    val unstable: Boolean,
    /** The calls exposed on this module, sorted by name. */
    val calls: List<CallBinding>,
)

/**
 * The module catalogue, derived from the descriptor set and nothing else (D652).
 *
 * ## Provenance — read this before editing
 *
 * Nothing in this object is written out. `modules` is every service in the
 * committed descriptor set that carries a `loams.options.v1.module` annotation, and
 * each module's `calls` is every RPC on that service whose `facade` annotation names
 * it — **plus** every RPC whose `FacadeOptions.module` names it, which is how
 * `loams.live.v1`'s `query`, `mutate` and `deploy` end up on `tables` rather than on
 * `live` (§44 §7.2 splits the service into the session half and the table half).
 *
 * There is therefore no `Add(...)` row to add, and no possibility of this file
 * disagreeing with `proto/`: if a call is missing, the proto is missing the
 * `loams.options.v1.facade` annotation. See Q604 in design §44 §7.3 for the
 * hand-written fallback this replaces, and note the difference: a fallback is a
 * *table*, and a table is a second copy of the annotations.
 */
object Facade {
    /**
     * Every module the descriptor set annotates, sorted by name.
     *
     * **Two, not seven**: `InstanceService` and `LiveService` are the only two
     * services in the set carrying a `module` annotation. `ApprovalService`,
     * `DeviceService`, `NotificationService` and `OperationsService` carry none, so
     * the corpus has no facade for them — which is exactly why the conformance
     * driver replays their fixtures through the generic `Descriptors.bindingForRpc`
     * path rather than through one of these.
     */
    private val allModules: List<ModuleBinding> = buildList {
        val byName = linkedMapOf<String, ModuleBinding>()
        for (binding in Descriptors.bindingsByRpc.values) {
            val module = binding.module ?: continue
            val existing = byName[module]
            if (existing == null) {
                byName[module] = ModuleBinding(
                    name = module,
                    summary = binding.summary ?: "",
                    packageName = binding.packageName,
                    service = binding.service,
                    unstable = binding.unstable,
                    calls = listOf(binding),
                )
            } else {
                byName[module] = existing.copy(calls = existing.calls + binding)
            }
        }
        addAll(byName.values.map { it.copy(calls = it.calls.sortedBy { binding -> binding.facadeName }) })
    }

    val modules: List<ModuleBinding> = allModules.filter { it.name in Descriptors.serviceModuleNames }

    private val modulesByName: Map<String, ModuleBinding> = allModules.associateBy { it.name }

    /** The module a name identifies, or a failure naming the modules that exist. */
    fun module(name: String): ModuleBinding = modulesByName[name]
        ?: throw NoSuchElementException(
            "loams.$name is not a module in the committed descriptor set; the modules are " +
                modules.map { it.name }.joinToString(", ")
        )

    /** The binding a module and a call name identify. */
    fun binding(module: String, name: String): CallBinding =
        module(module).calls.firstOrNull { it.facadeName == name }
            ?: throw NoSuchElementException(
                "loams.$module has no bound call $name; its calls are " +
                    module(module).calls.joinToString(", ") { it.facadeName }
            )
}

/** Reads a string field off a `ModuleOptions` or `FacadeOptions`. */
private fun DynamicMessage.string(field: String): String? =
    descriptorForType.findFieldByName(field)?.let { getField(it) as? String }

/** Reads a boolean field off a `ModuleOptions` or `FacadeOptions`. */
private fun DynamicMessage.bool(field: String): Boolean =
    descriptorForType.findFieldByName(field)?.let { getField(it) as? Boolean } ?: false

/**
 * An enum field's value as the name the wire spells it.
 *
 * Through the descriptor's `enumType` and never by casting the value, because
 * `DynamicMessage` boxes an enum as its **number** rather than as an
 * `EnumValueDescriptor`. A number the enum does not declare — a value from a server
 * newer than this SDK was generated from — is rendered as its decimal text, which is
 * what the proto3 JSON mapping permits and what keeps an unknown value from reading as
 * a missing field.
 */
internal fun enumNameOf(field: FieldDescriptor, number: Int): String =
    field.enumType?.findValueByNumber(number)?.name ?: number.toString()