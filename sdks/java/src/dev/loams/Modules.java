package dev.loams;

import com.google.protobuf.Message;
import com.google.protobuf.Parser;
import dev.loams.facade.CallBinding;
import dev.loams.facade.Facade;
import dev.loams.facade.ModuleBinding;
import dev.loams.gen.loams.instance.v1.GetInstanceRequest;
import dev.loams.gen.loams.instance.v1.GetInstanceResponse;
import dev.loams.gen.loams.instance.v1.WhoAmIRequest;
import dev.loams.gen.loams.instance.v1.WhoAmIResponse;
import dev.loams.gen.loams.live.v1.DeployRequest;
import dev.loams.gen.loams.live.v1.DeployResponse;
import dev.loams.gen.loams.live.v1.ModifyQuerySetRequest;
import dev.loams.gen.loams.live.v1.ModifyQuerySetResponse;
import dev.loams.gen.loams.live.v1.MutateRequest;
import dev.loams.gen.loams.live.v1.MutateResponse;
import dev.loams.gen.loams.live.v1.QueryRequest;
import dev.loams.gen.loams.live.v1.QueryResponse;
import dev.loams.gen.loams.live.v1.Transition;
import dev.loams.gen.loams.live.v1.WatchRequest;

/**
 * The module surface: one method per facade call (design §44 §7.1).
 *
 * <h2>Provenance</h2>
 *
 * <p>Design §44 §7.3 (D606) says the module and method surface is <b>generated</b> from the
 * {@code loams.options.v1} annotations, so thirteen languages cannot drift, and it allows a
 * hand-written facade for a language where the generator proves costly (Q604). The Java renderer
 * is not written — {@code crates/loams-facade-gen} ships TypeScript, Python, Go and Rust — and
 * this task may not add one, so this is that fallback, a transcription of the same annotations.
 *
 * <p>Every method below resolves its binding by module and call name and hands it to the shared
 * invoker. It holds no RPC path, no retry class and no message name of its own, so a wrong retry
 * class would have to be written here — and there is nowhere here to write one, because they all
 * come from {@link Facade}.
 *
 * <p>When {@code crates/loams-facade-gen/src/java.rs} lands, generation replaces these classes and
 * the hand-written ones are deleted. <b>Do not add a method without the annotation that generates
 * it.</b>
 *
 * <p>The generated stubs these methods send are real and <em>are</em> generated: they come from
 * {@code protoc-gen-java} over {@code proto/} via {@code sdks/java/buf.gen.yaml} (D604). The Java
 * SDK is generated from the protos, not a REST wrapper.
 */
public final class Modules {

    private Modules() {}

    /**
     * Resolve a module's call binding.
     *
     * <p>A name that does not resolve is a bug in this package rather than a caller's mistake,
     * which is why it is {@code internal} and not {@code not_found}.
     */
    static CallBinding binding(String module, String call) {
        return Facade.binding(module, call)
                .orElseThrow(
                        () ->
                                Errors.internal(
                                        "",
                                        "loams." + module + " has no generated call " + call,
                                        null));
    }

    /** Resolve a module's own binding. */
    static ModuleBinding module(String name) {
        return Facade.module(name)
                .orElseThrow(
                        () -> Errors.internal("", "loams has no generated module " + name, null));
    }

    /**
     * {@code loams.instance}: what this instance is, and who the caller is on it.
     *
     * <p>{@code getInstance} is the first call any client makes: it needs no credentials, and its
     * {@code services} field is the module catalogue an SDK feature-detects from (design §44 §4,
     * D600).
     */
    public static final class Instance {

        private final CallInvoker invoker;

        Instance(CallInvoker invoker) {
            this.invoker = invoker;
        }

        /** The SDK module's name, as it appears on the client. */
        public String module() {
            return "instance";
        }

        /** The service behind the module, from the binding table. */
        public String service() {
            return Modules.module("instance").service();
        }

        /** Whether the package's wire contract may still change. */
        public boolean unstable() {
            return Modules.module("instance").unstable();
        }

        /**
         * {@code loams.instance.v1.InstanceService/GetInstance}, retried: safe.
         *
         * <p>No auth: what this instance is and how to sign in to it.
         */
        public GetInstanceResponse getInstance(GetInstanceRequest request, CallOptions... options) {
            return call(GetInstanceResponse.parser(), "GetInstance", request, options);
        }

        /**
         * {@code loams.instance.v1.InstanceService/WhoAmI}, retried: safe.
         *
         * <p>The calling principal, its org, and the environments it can reach.
         */
        public WhoAmIResponse whoAmI(WhoAmIRequest request, CallOptions... options) {
            return call(WhoAmIResponse.parser(), "WhoAmI", request, options);
        }

        private <Res extends Message> Res call(
                Parser<Res> parser, String name, Message request, CallOptions... options) {
            return invoker.unary(binding("instance", name), request, CallOptions.merge(options), parser);
        }
    }

    /**
     * {@code loams.live}: the live sync session half.
     *
     * <p>Its wire contract may still change, so {@code buf breaking} skips the package and an SDK
     * marks the module experimental (§44 §10.3).
     */
    public static final class Live {

        private final CallInvoker invoker;

        Live(CallInvoker invoker) {
            this.invoker = invoker;
        }

        /** The SDK module's name, as it appears on the client. */
        public String module() {
            return "live";
        }

        /** The service behind the module, from the binding table. */
        public String service() {
            return Modules.module("live").service();
        }

        /** Whether the package's wire contract may still change. */
        public boolean unstable() {
            return Modules.module("live").unstable();
        }

        /**
         * {@code loams.live.v1.LiveService/ModifyQuerySet}, retried: manual.
         *
         * <p>Adds and removes queries in an open session; the next transition reflects the change.
         */
        public ModifyQuerySetResponse modifyQuerySet(
                ModifyQuerySetRequest request, CallOptions... options) {
            return invoker.unary(
                    binding("live", "ModifyQuerySet"),
                    request,
                    CallOptions.merge(options),
                    ModifyQuerySetResponse.parser());
        }

        /**
         * {@code loams.live.v1.LiveService/Watch}, retried: manual, and a server stream.
         *
         * <p>Pass {@link CallOptions#withStreamResume} to reconnect from the last cursor rather
         * than silently miss the changes in between (R7):
         *
         * <pre>{@code
         * StreamResume<Transition> resume = StreamResume.forMessages(
         *         t -> cursorOf(t),
         *         (cursor, original) -> reopenFrom(cursor, (WatchRequest) original),
         *         null,
         *         (cursor, t) -> lastSeen = cursor);
         * try (Stream<Transition> stream = client.live().watch(request, CallOptions.withStreamResume(resume))) {
         *     while (stream.receive()) { apply(stream.message()); }
         * }
         * }</pre>
         */
        public Stream<Transition> watch(WatchRequest request, CallOptions... options) {
            CallBinding binding = binding("live", "Watch");
            CallOptions merged = CallOptions.merge(options);
            @SuppressWarnings("unchecked")
            StreamResume<Transition> resume = (StreamResume<Transition>) merged.streamResume();
            Receiver<Transition> receiver =
                    invoker.openServerStream(
                            binding, request, merged, Modules.Live::parseTransition, request);
            return new Stream<>(
                    binding.rpc(),
                    binding.retrySafe(),
                    receiver,
                    resume,
                    reopener(binding, merged, request, resume),
                    invoker.maxRetries());
        }

        /**
         * How to re-open the stream from a cursor, or {@code null} when the caller passed no
         * resume.
         *
         * <p>A null reopener is what turns a broken stream into a reported error instead of a
         * spin, which is the difference between R7's two halves: with a resume the SDK
         * reconnects, without one it reports and stops.
         */
        private Stream.Reopener<Transition> reopener(
                CallBinding binding, CallOptions options, WatchRequest request, StreamResume<Transition> resume) {
            if (resume == null) {
                return null;
            }
            return cursor -> {
                Message reopened = resume.resumeFrom().apply(cursor, request);
                if (reopened == null) {
                    throw Errors.internal(
                            binding.rpc(), "the stream's resume function returned no request", null);
                }
                return invoker.openServerStream(
                        binding, request, options, Modules.Live::parseTransition, reopened);
            };
        }

        private static Transition parseTransition(byte[] bytes) {
            try {
                return Transition.parseFrom(bytes);
            } catch (java.io.IOException e) {
                throw Errors.internal(
                        "loams.live.v1.LiveService/Watch",
                        "a stream message is not a loams.live.v1.Transition",
                        e);
            }
        }
    }

    /**
     * {@code loams.tables}: the table half of the same service, a second facade name for
     * {@code loams.live.v1}'s unary RPCs (design §44 §7.2).
     */
    public static final class Tables {

        private final CallInvoker invoker;

        Tables(CallInvoker invoker) {
            this.invoker = invoker;
        }

        /** The SDK module's name, as it appears on the client. */
        public String module() {
            return "tables";
        }

        /** The service behind the module, from the binding table. */
        public String service() {
            return Modules.module("tables").service();
        }

        /** Whether the package's wire contract may still change. */
        public boolean unstable() {
            return Modules.module("tables").unstable();
        }

        /**
         * {@code loams.live.v1.LiveService/Query}, retried: manual.
         *
         * <p>A one-shot query, at the latest tick or at a given timestamp. This is the call the
         * conformance suite uses for the unavailable-service path, because
         * {@code loams.live.v1} is not carried by the standard variant.
         */
        public QueryResponse query(QueryRequest request, CallOptions... options) {
            return invoker.unary(
                    binding("tables", "Query"), request, CallOptions.merge(options), QueryResponse.parser());
        }

        /**
         * {@code loams.live.v1.LiveService/Mutate}, retried: manual, and the only keyed mutation
         * in the API today.
         *
         * <p>{@code MutateRequest.idempotency_key} is given a UUIDv7 before the first attempt and
         * reused on every retry, so a retried mutation is one write (R3). Pass
         * {@link CallOptions#withIdempotencyKey} to supply your own.
         */
        public MutateResponse mutate(MutateRequest request, CallOptions... options) {
            return invoker.unary(
                    binding("tables", "Mutate"), request, CallOptions.merge(options), MutateResponse.parser());
        }

        /**
         * {@code loams.live.v1.LiveService/Deploy}, retried: manual.
         *
         * <p>Admin: deploys a function bundle and a schema. Not keyed — {@code DeployRequest} has
         * no {@code idempotency_key} field, and inventing one would be the SDK guessing at the
         * schema.
         */
        public DeployResponse deploy(DeployRequest request, CallOptions... options) {
            return invoker.unary(
                    binding("tables", "Deploy"), request, CallOptions.merge(options), DeployResponse.parser());
        }
    }
}