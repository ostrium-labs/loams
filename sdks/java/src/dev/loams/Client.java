package dev.loams;

import dev.loams.connect.ConnectTransport;
import dev.loams.facade.CallBinding;
import dev.loams.facade.Facade;
import dev.loams.facade.ModuleBinding;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * One SDK client over one instance (design §44 §7.1).
 *
 * <pre>{@code
 * Client client = Client.builder()
 *         .endpoint("https://acme.loams.dev")
 *         .auth(TokenSources.apiKey(System.getenv("LOAMS_API_KEY")))
 *         .build();
 *
 * GetInstanceResponse info = client.instance().getInstance(GetInstanceRequest.getDefaultInstance());
 * if (client.system().guard("live") == null) {
 *     try (Stream<Transition> stream = client.live().watch(request, resume)) {
 *         while (stream.receive()) { apply(stream.message()); }
 *     }
 * }
 * }</pre>
 *
 * <h2>What is generated and what is hand-written</h2>
 *
 * <p>The <b>module surface</b> is the Q604 hand-written-facade fallback in
 * {@link Modules} — a transcription of the {@code loams.options.v1} annotations — because
 * {@code crates/loams-facade-gen} has no Java renderer and this task may not add one. The
 * <b>runtime</b> behind those methods is hand-written once, in this package: transport,
 * credentials, retry, errors, tokens, pagination, streams. This class is the thin join — it
 * builds the transport, wires the runtime and holds the client-wide defaults. It contains no RPC
 * path and no retry class, which is why annotating a proto is enough to add an SDK method.
 *
 * <p><b>Thread-safe.</b> Every field is either immutable or guarded, and the token source and
 * the session store are too. The one thing a caller must not do is close an {@link
 * java.net.http.HttpClient} they supplied while calls are in flight.
 */
public final class Client implements AutoCloseable {

    private final CallInvoker invoker;
    private final ConnectTransport transport;
    private final String endpoint;
    private final Protocol protocol;
    private final int maxRetries;
    private final ConsistencySession session;

    private final Modules.Instance instance;
    private final Modules.Live live;
    private final Modules.Tables tables;
    private final SystemModule system;

    /** Every module, by name: the catalogue a caller iterates when feature-detecting. */
    private final Map<String, Object> modules = new LinkedHashMap<>();

    private Client(Options options) {
        String endpoint =
                options.endpoint().endsWith("/")
                        ? options.endpoint().substring(0, options.endpoint().length() - 1)
                        : options.endpoint();
        this.endpoint = endpoint;
        this.protocol = options.protocol();
        this.transport =
                new ConnectTransport(endpoint, this.protocol, options.httpClient());
        this.session = options.sessionConsistency() ? new ConsistencySession() : null;
        // An unset budget means the design's number, not none. Java's `Integer` null is what
        // makes that expressible; see Options.maxRetries().
        int budget = Retry.DEFAULT_MAX_RETRIES;
        if (options.maxRetries() != null) {
            budget = Math.max(0, options.maxRetries());
        }
        if (options.noRetries()) {
            budget = 0;
        }
        this.maxRetries = budget;
        this.invoker = new CallInvoker(transport, options.auth(), budget, session);

        this.instance = new Modules.Instance(invoker);
        this.live = new Modules.Live(invoker);
        this.tables = new Modules.Tables(invoker);
        this.system = new SystemModule(instance);
        modules.put("instance", instance);
        modules.put("live", live);
        modules.put("tables", tables);
    }

    /** A builder for {@link Options}. */
    public static Options.Builder builder() {
        return Options.builder();
    }

    /**
     * Build a client.
     *
     * @throws IllegalArgumentException when no endpoint was given, or when the endpoint is not a
     *     URL. Both are usage errors and both are thrown here rather than turned into a
     *     mysterious failure on the first call.
     */
    public static Client of(Options options) {
        java.net.URI uri;
        try {
            uri = java.net.URI.create(
                    options.endpoint().endsWith("/")
                            ? options.endpoint()
                            : options.endpoint() + "/");
        } catch (IllegalArgumentException e) {
            throw new IllegalArgumentException(
                    "loams: Options.endpoint is not a URL: " + options.endpoint(), e);
        }
        if (uri.getHost() == null) {
            throw new IllegalArgumentException(
                    "loams: Options.endpoint has no host: " + options.endpoint());
        }
        return new Client(options);
    }

    /** {@code loams.instance}: what this instance is, and who the caller is. */
    public Modules.Instance instance() {
        return instance;
    }

    /**
     * {@code loams.live}: the live sync session half. Its package is unstable, so its wire
     * contract may still change (§44 §10.3).
     */
    public Modules.Live live() {
        return live;
    }

    /** {@code loams.tables}: the table half of the same service (design §44 §7.2). */
    public Modules.Tables tables() {
        return tables;
    }

    /** The module catalogue, feature detection and the version check. */
    public SystemModule system() {
        return system;
    }

    /**
     * A module by name, so a caller can feature-detect without a hard-coded accessor.
     *
     * <pre>{@code
     * Object module = client.module("live"); // null until the facade grows one
     * }</pre>
     */
    public Object module(String name) {
        return modules.get(name);
    }

    /** Every module, by name. */
    public Map<String, Object> modules() {
        return Map.copyOf(modules);
    }

    /** The binding table, as the SDK sees it. */
    public static List<ModuleBinding> bindings() {
        return Facade.MODULES;
    }

    /**
     * The binding a module and call name identify, or an internal error when it does not resolve.
     *
     * <p>{@code call} matches either the SDK's PascalCase name ({@code GetInstance}) or the
     * {@code FacadeOptions} proto name ({@code getInstance}).
     */
    public static CallBinding binding(String module, String call) {
        return Facade.binding(module, call)
                .orElseThrow(
                        () ->
                                Errors.internal(
                                        "", "loams." + module + " has no generated call " + call, null));
    }

    /**
     * The proto revision this SDK declares (§44 §10.3).
     *
     * <p>It is a constant rather than an instance field because it is a property of the generated
     * code, not of a client, and a client cannot disagree with itself.
     */
    public static String protoRev() {
        return Facade.PROTO_REV;
    }

    /** Every proto package in the module, as {@code GetInstance.api_versions} names them. */
    public static List<String> protoPackages() {
        return Facade.PROTO_PACKAGES;
    }

    /** The instance this client talks to. */
    public String endpoint() {
        return endpoint;
    }

    /** The wire protocol this client speaks. */
    public Protocol protocol() {
        return protocol;
    }

    /** The client's default retry budget, after {@code noRetries} and {@code maxRetries}. */
    public int maxRetries() {
        return maxRetries;
    }

    /**
     * The session consistency token store, or {@code null} when
     * {@link Options.Builder#sessionConsistency} was off.
     *
     * <p>{@code null} rather than an inert store, so "not on" and "on but empty" do not look the
     * same — a caller who checks it can tell whether read-your-writes is in play at all.
     */
    public ConsistencySession session() {
        return session;
    }

    /** Forget the cached service catalogue, so the next feature check calls again. */
    public void invalidateCatalogue() {
        system.invalidate();
    }

    /**
     * Every item of a paged call (R6), resolving the binding from a module and call name first.
     *
     * <p>It exists in this shape rather than as a method because Java cannot have a generic
     * method and this class is not generic.
     */
    public <Item> PageIterator<Item> paginate(String module, String call, PageIterator.PageFetcher<?> fetch) {
        return PageIterator.of(binding(module, call), fetch);
    }

    /**
     * Release the transport.
     *
     * <p>It does not close a caller-supplied {@link java.net.http.HttpClient}: that client
     * belongs to the caller and may be shared with something else. The JDK client has no close of
     * its own either, so there is nothing beyond releasing this SDK's reference.
     */
    @Override
    public void close() {
        transport.close();
    }

    /** The call path, for the module methods and for a caller building their own client. */
    public CallInvoker invoker() {
        return invoker;
    }

    /**
     * The modules this client exposes, in binding order.
     *
     * <p>It is here so a test can assert the client's surface and the binding table agree without
     * reaching into {@link #modules()}, whose values are the same objects in a different order.
     */
    List<String> moduleNames() {
        return new ArrayList<>(modules.keySet());
    }
}