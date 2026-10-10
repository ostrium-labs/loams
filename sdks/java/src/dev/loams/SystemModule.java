package dev.loams;

import dev.loams.facade.Facade;
import dev.loams.facade.ModuleBinding;
import dev.loams.gen.loams.instance.v1.GetInstanceRequest;
import dev.loams.gen.loams.instance.v1.GetInstanceResponse;
import java.util.ArrayList;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Set;

/**
 * Feature detection and version reporting (design §44 §4, D600; runtime contract R5 and R9).
 *
 * <p>Two halves of R5, and an SDK needs both.
 *
 * <ol>
 *   <li><b>Without calling.</b> {@code GetInstance.services[]} says which packages this binary
 *       carries. One call, no auth, cheap. {@link #available}, {@link #served},
 *       {@link #unavailable} and {@link #guard} wrap it; the catalogue is cached for the life of
 *       the client and concurrent readers share one in-flight fetch, so a hundred threads asking
 *       at once cost one call.
 *   <li><b>When the caller calls anyway.</b> Every RPC of an absent package answers
 *       {@code unimplemented} with {@code reason = feature_not_in_variant} and the variant in
 *       {@code metadata.variant}. {@link Errors} turns that into a
 *       {@link FeatureNotInVariantException}, so the branch is a type check and never the package
 *       name, which is a proto detail, and never the message.
 * </ol>
 *
 * <p>{@link #guard} raises the <em>same</em> error type, so one {@code catch} covers "the guard
 * said no" and "the server refused", and the guard costs no RPC once the catalogue is cached.
 */
public final class SystemModule {

    /** The RPC the catalogue comes from, which is also what a guard's failure names. */
    private static final String INSTANCE_GET_INSTANCE = "loams.instance.v1.InstanceService/GetInstance";

    private final Modules.Instance instance;

    /**
     * Every proto package this SDK speaks, which is what R9's report compares the server's
     * {@code api_versions} against. Computed once at construction: it is a property of the SDK,
     * not of the server.
     */
    private final List<String> speaks;

    /** Guards {@link #catalogue} and {@link #inFlight}. */
    private final Object lock = new Object();

    private Catalogue catalogue;

    /**
     * The fetch currently running, so concurrent readers share it rather than each starting their
     * own {@code GetInstance}.
     *
     * <p>A plain monitor with a hand-off rather than a lock held across the network call: holding
     * the lock would make a hundred readers wait on the mutex for the length of a network round
     * trip, which is the wrong shape for a slow call.
     */
    private InFlight inFlight;

    SystemModule(Modules.Instance instance) {
        this.instance = instance;
        this.speaks = packagesSpoken();
    }

    /**
     * Every proto package this SDK speaks: {@code Facade.PROTO_PACKAGES} filtered to the
     * {@code loams.} ones, which is what {@code GetInstance.api_versions} names.
     *
     * <p>{@code google.protobuf} is the well-known types, and a client never asks an instance to
     * serve them, so including it would make every instance look incompatible.
     */
    private static List<String> packagesSpoken() {
        List<String> out = new ArrayList<>();
        for (String pkg : Facade.PROTO_PACKAGES) {
            if (pkg.startsWith("loams.")) {
                out.add(pkg);
            }
        }
        return List.copyOf(out);
    }

    /** The instance this client talks to. */
    public String endpoint() {
        return "";
    }

    /** Forget the cached catalogue, so the next check calls again. */
    public void invalidate() {
        synchronized (lock) {
            catalogue = null;
        }
    }

    /**
     * The module catalogue, fetching it once and sharing the result with every concurrent reader.
     *
     * <p>A live {@code loams dev} and a replayed fixture server are the same shape here: one
     * {@code GetInstance} call, no auth.
     */
    public Catalogue catalogue() {
        InFlight mine;
        boolean owns;
        synchronized (lock) {
            if (catalogue != null) {
                return catalogue;
            }
            if (inFlight != null) {
                mine = inFlight;
                owns = false;
            } else {
                mine = new InFlight();
                inFlight = mine;
                owns = true;
            }
        }
        if (!owns) {
            await(mine);
            return mine.catalogue;
        }
        try {
            GetInstanceResponse info = instance.getInstance(GetInstanceRequest.getDefaultInstance());
            Catalogue fetched = Catalogue.from(info, speaks);
            synchronized (lock) {
                catalogue = fetched;
                mine.catalogue = fetched;
                mine.done = true;
                inFlight = null;
                lock.notifyAll();
            }
            return fetched;
        } catch (RuntimeException failure) {
            synchronized (lock) {
                mine.done = true;
                mine.failure = failure;
                inFlight = null;
                lock.notifyAll();
            }
            throw failure;
        }
    }

    private void await(InFlight exchange) {
        synchronized (lock) {
            while (!exchange.done) {
                try {
                    lock.wait();
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                    throw Errors.internal(
                            INSTANCE_GET_INSTANCE,
                            "interrupted while waiting for another thread's catalogue fetch",
                            e);
                }
            }
        }
        if (exchange.failure != null) {
            throw exchange.failure;
        }
    }

    /**
     * Whether this instance serves a proto package. It costs no RPC once the catalogue is cached.
     *
     * <p>{@code client.system().available("loams.live.v1")} is the question; a caller who thinks
     * in module names uses {@link #availableModule}.
     */
    public boolean available(String protoPackage) {
        return catalogue().available(protoPackage);
    }

    /** The packages this instance serves. */
    public List<String> served() {
        return catalogue().served();
    }

    /** The packages this instance knows about and does not serve. */
    public List<String> unavailable() {
        return catalogue().unavailable();
    }

    /**
     * Whether an SDK module's package is served, taking a <b>module name or a proto package</b> so
     * a caller holding {@code loams.live} and a caller reading a {@link ServiceStatus} can both
     * ask.
     *
     * <p>A module name is resolved through the binding table, so {@code loams.live} and
     * {@code loams.tables} — two facade names for one package — answer the same thing.
     */
    public boolean availableModule(String moduleOrPackage) {
        return available(packageOf(moduleOrPackage));
    }

    /**
     * The proto package behind a module name or a package name.
     *
     * <p>The binding table is consulted <b>before</b> the {@code loams.} prefix, and that order is
     * the whole subtlety: a module name may itself be spelled {@code loams.live}, which also looks
     * like a proto package. Resolving the prefix first would hand {@code loams.live} back as if it
     * were a package name, and then {@code availableModule("loams.live")} would ask about a
     * package no instance has ever heard of and answer {@code false} for a module that is served.
     */
    static String packageOf(String moduleOrPackage) {
        java.util.Optional<ModuleBinding> generated = Facade.module(moduleOrPackage);
        if (generated.isPresent()) {
            return generated.get().protoPackage();
        }
        if (Facade.PROTO_PACKAGES.contains(moduleOrPackage)) {
            return moduleOrPackage;
        }
        // `loams.live` is the module's name spelled with the client's prefix rather than a proto
        // package. Resolving it as a package would ask about `loams.live`, which no instance
        // lists, and answer "not served" for a module that is.
        if (moduleOrPackage.startsWith("loams.")) {
            java.util.Optional<ModuleBinding> prefixed =
                    Facade.module(moduleOrPackage.substring("loams.".length()));
            if (prefixed.isPresent()) {
                return prefixed.get().protoPackage();
            }
            return moduleOrPackage;
        }
        throw Errors.internal(
                "", "loams has no generated module " + moduleOrPackage, null);
    }

    /**
     * A {@link FeatureNotInVariantException} when a module's package is not served here, and
     * {@code null} when it is.
     *
     * <p>The error is the <b>same type</b> a refused RPC produces, so one {@code catch} covers
     * both, and it costs no request once the catalogue is cached:
     *
     * <pre>{@code
     * try {
     *     client.system().guard("live");
     * } catch (FeatureNotInVariantException absent) {
     *     return runWithoutLiveSync();
     * }
     * }</pre>
     *
     * <p>The message names the package rather than the module, because the package is what the
     * server knows about and what an operator will grep for.
     */
    public FeatureNotInVariantException guard(String moduleOrPackage) {
        String pkg = packageOf(moduleOrPackage);
        if (available(pkg)) {
            return null;
        }
        // The guard learned this from the catalogue rather than from a refusal, so the `rpc` is
        // `GetInstance` — the call that actually answered — and the variant is recorded as
        // unknown rather than invented. A plausible-looking variant in a support ticket is worse
        // than an honest "unknown".
        return new FeatureNotInVariantException(
                Code.UNIMPLEMENTED,
                dev.loams.facade.Reason.FEATURE_NOT_IN_VARIANT,
                "",
                java.util.Map.of("package", pkg, "variant", FeatureNotInVariantException.VARIANT_UNKNOWN),
                "this build variant does not carry " + pkg,
                INSTANCE_GET_INSTANCE,
                new IllegalStateException(
                        "loams." + moduleOrPackage + " is not available on this instance: " + pkg + " is not served"),
                FeatureNotInVariantException.VARIANT_UNKNOWN);
    }

    /**
     * What this SDK speaks beside what the server serves (R9).
     *
     * <p>A package the SDK speaks and the server does not serve is reported in {@code missing} and
     * {@code compatible} is false; it is <b>not</b> an error. The SDK still works for the modules
     * that are there, and what a missing one means is the caller's decision.
     */
    public VersionReport version() {
        GetInstanceResponse info = instance.getInstance(GetInstanceRequest.getDefaultInstance());
        Set<String> served = new LinkedHashSet<>(info.getApiVersionsList());
        List<String> missing = new ArrayList<>();
        for (String pkg : speaks) {
            if (!served.contains(pkg)) {
                missing.add(pkg);
            }
        }
        return new VersionReport(
                Facade.PROTO_REV, info.getServerVersion(), info.getApiVersionsList(), missing.isEmpty(), missing);
    }

    /** One shared catalogue fetch. */
    private static final class InFlight {

        private boolean done;

        private Catalogue catalogue;

        private RuntimeException failure;
    }
}