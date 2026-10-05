package dev.loams.facade;

import dev.loams.gen.loams.errors.v1.ErrorInfo;
import java.util.ArrayList;
import java.util.List;
import java.util.Optional;

/**
 * The SDK surface of design §44 §7.3 (D606): the module catalogue, the binding table the
 * runtime dispatches through, and the reason registry.
 *
 * <h2>Provenance — read this before editing</h2>
 *
 * <p>{@code crates/loams-facade-gen} ships renderers for TypeScript, Python, Go and Rust and
 * <b>none for Java</b>, and this task may not add one: {@code crates/**} and
 * {@code scripts/sdk/gen.sh} are both outside the paths SDK2 Task 6 may write. So this file
 * is the <b>Q604 hand-written-facade fallback</b>, which design §44 §7.3 explicitly allows:
 *
 * <blockquote>If the plugin proves too costly for a language, that language falls back to a
 * hand-written facade checked by the same conformance suite (D606, Q604).</blockquote>
 *
 * <p>It is a transcription of the {@code loams.options.v1} annotations in {@code proto/}, field
 * for field, and the conformance suite checks it the same way it checks the generated facades.
 * {@code sdks/go/gen/facade/facade.go} is the same fallback in Go and says the same thing at
 * the top of its file.
 *
 * <p><b>Do not grow it.</b> If a call is missing, the proto is missing the
 * {@code loams.options.v1} annotation — see {@code docs/design/13-decision-log.md} Q604. When
 * {@code crates/loams-facade-gen/src/java.rs} lands, generation replaces this class and the
 * hand-written one is deleted.
 *
 * <p>The generated stubs the runtime sends are real and <em>are</em> generated: they come from
 * {@code protoc-gen-java} over {@code proto/} via {@code sdks/java/buf.gen.yaml} (D604).
 */
public final class Facade {

    private Facade() {}

    /**
     * The proto revision this SDK was generated from ({@code LOAMS_PROTO_REV}, design §44
     * §10.3). {@code loams.system().version()} checks it against the server's
     * {@code GetInstance.api_versions}. Pre-1.0, so it is the API major rather than a release
     * tag.
     */
    public static final String PROTO_REV = "v1";

    /**
     * The {@code type} a server puts in an error detail, which is how the runtime finds the
     * {@code ErrorInfo} among whatever other details a service chose to send (R8).
     */
    public static final String ERROR_INFO_TYPE = "loams.errors.v1.ErrorInfo";

    /** The fully qualified names of the services the SDK's modules call. */
    public static final String INSTANCE_SERVICE = "loams.instance.v1.InstanceService";

    public static final String LIVE_SERVICE = "loams.live.v1.LiveService";

    /** The module catalogue, ordered by module name. */
    public static final List<ModuleBinding> MODULES = modules();

    /**
     * Every proto package in the module, as {@code GetInstance.api_versions} names them.
     */
    public static final List<String> PROTO_PACKAGES = List.of(
            "google.protobuf",
            "loams.approvals.v1",
            "loams.devices.v1",
            "loams.errors.v1",
            "loams.instance.v1",
            "loams.live.v1",
            "loams.notifications.v1",
            "loams.operations.v1",
            "loams.options.v1");

    private static List<ModuleBinding> modules() {
        return List.of(
                new ModuleBinding(
                        "instance",
                        "What this instance is, and who the caller is on it.",
                        INSTANCE_SERVICE,
                        "loams.instance.v1",
                        false,
                        false,
                        List.of(
                                call("instance", "GetInstance", "getInstance", INSTANCE_SERVICE,
                                        IdempotencyLevel.NO_SIDE_EFFECTS, RetryClass.SAFE,
                                        Streaming.UNARY, null, false),
                                call("instance", "WhoAmI", "whoAmI", INSTANCE_SERVICE,
                                        IdempotencyLevel.NO_SIDE_EFFECTS, RetryClass.SAFE,
                                        Streaming.UNARY, null, false))),
                new ModuleBinding(
                        "live",
                        "Live sync: watch a query set over a server stream.",
                        LIVE_SERVICE,
                        "loams.live.v1",
                        true,
                        false,
                        List.of(
                                call("live", "ModifyQuerySet", "modifyQuerySet", LIVE_SERVICE,
                                        IdempotencyLevel.NONE, RetryClass.MANUAL,
                                        Streaming.UNARY, null, false),
                                call("live", "Watch", "watch", LIVE_SERVICE,
                                        IdempotencyLevel.NONE, RetryClass.MANUAL,
                                        Streaming.SERVER, null, false))),
                new ModuleBinding(
                        "tables",
                        "",
                        LIVE_SERVICE,
                        "loams.live.v1",
                        true,
                        true,
                        List.of(
                                call("tables", "Deploy", "deploy", LIVE_SERVICE,
                                        IdempotencyLevel.NONE, RetryClass.MANUAL,
                                        Streaming.UNARY, null, false),
                                // `MutateRequest.idempotency_key` (live.proto). Read off
                                // the generated message, not guessed: `DeployRequest` has no
                                // such field and must not be keyed.
                                call("tables", "Mutate", "mutate", LIVE_SERVICE,
                                        IdempotencyLevel.NONE, RetryClass.MANUAL,
                                        Streaming.UNARY, null, true),
                                call("tables", "Query", "query", LIVE_SERVICE,
                                        IdempotencyLevel.NONE, RetryClass.MANUAL,
                                        Streaming.UNARY, null, false))));
    }

    private static CallBinding call(
            String module,
            String name,
            String protoName,
            String service,
            IdempotencyLevel idempotency,
            RetryClass retry,
            Streaming streaming,
            Pagination pagination,
            boolean takesIdempotencyKey) {
        return new CallBinding(
                module,
                name,
                protoName,
                name,
                service + "/" + name,
                service,
                service.substring(0, service.lastIndexOf('.')),
                idempotency,
                retry,
                streaming,
                pagination,
                takesIdempotencyKey);
    }

    /**
     * The binding a module and call name identify, or empty. {@code call} matches either the
     * SDK's PascalCase name or the {@code FacadeOptions} proto name, so a caller holding
     * either can look a call up.
     */
    public static Optional<CallBinding> binding(String module, String call) {
        for (ModuleBinding entry : MODULES) {
            if (!entry.name().equals(module)) {
                continue;
            }
            for (CallBinding candidate : entry.calls()) {
                if (candidate.name().equals(call) || candidate.protoName().equals(call)) {
                    return Optional.of(candidate);
                }
            }
        }
        return Optional.empty();
    }

    /** The module binding by name. */
    public static Optional<ModuleBinding> module(String name) {
        for (ModuleBinding entry : MODULES) {
            if (entry.name().equals(name)) {
                return Optional.of(entry);
            }
        }
        return Optional.empty();
    }

    /**
     * The module that owns a proto package. {@code loams.live} and {@code loams.tables} are
     * two names for one package, so the first module in registration order that claims it wins;
     * use {@link #MODULES} when the whole list of facade names matters.
     */
    public static Optional<String> moduleOfPackage(String protoPackage) {
        for (ModuleBinding entry : MODULES) {
            if (entry.protoPackage().equals(protoPackage)) {
                return Optional.of(entry.name());
            }
        }
        return Optional.empty();
    }

    /**
     * Every module name that is a facade for one proto package.
     *
     * <p>It is what a guard consults, because a guard on {@code live} must also cover
     * {@code tables}: they are the same service, and a refusal on either means the engine is
     * absent.
     */
    public static List<String> modulesForPackage(String protoPackage) {
        List<String> names = new ArrayList<>();
        for (ModuleBinding entry : MODULES) {
            if (entry.protoPackage().equals(protoPackage)) {
                names.add(entry.name());
            }
        }
        names.sort(String::compareTo);
        return List.copyOf(names);
    }

    /** The {@code ErrorInfo} message class, so a caller who wants the detail need not import
     * the stub package by hand. */
    public static Class<ErrorInfo> errorInfoType() {
        return ErrorInfo.class;
    }
}