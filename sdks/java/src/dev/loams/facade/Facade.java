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

    /** {@code loams.live.v1.LiveAdminService}, where Deploy moved (LV1 Task 10). */
    public static final String LIVE_ADMIN_SERVICE = "loams.live.v1.LiveAdminService";

    /**
     * The request header that selects one of a Live node's apps (LV1 Task 13). A request without
     * it addresses the server's own app ({@code --live-app}).
     */
    public static final String LIVE_APP_HEADER = "loams-live-app";

    /** The module catalogue, ordered by module name. */
    public static final List<ModuleBinding> MODULES = modules();

    /**
     * Every proto package in the module, as {@code GetInstance.api_versions} names them.
     */
    public static final List<String> PROTO_PACKAGES = List.of(
            "google.protobuf",
            "loams.approvals.v1",
            "loams.collection.v1",
            "loams.devices.v1",
            "loams.errors.v1",
            "loams.graph.v1",
            "loams.instance.v1",
            "loams.live.v1",
            "loams.notifications.v1",
            "loams.operations.v1",
            "loams.options.v1",
            "loams.sql.v1",
            "loams.stream.v1");

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
                                call("live", "Authenticate", "authenticate", LIVE_SERVICE,
                                        IdempotencyLevel.NONE, RetryClass.MANUAL,
                                        Streaming.UNARY, null, false),
                                call("live", "ModifyQuerySet", "modifyQuerySet", LIVE_SERVICE,
                                        IdempotencyLevel.NONE, RetryClass.MANUAL,
                                        Streaming.UNARY, null, false),
                                call("live", "Watch", "watch", LIVE_SERVICE,
                                        IdempotencyLevel.NONE, RetryClass.MANUAL,
                                        Streaming.SERVER, null, false))),
                new ModuleBinding(
                        "live.admin",
                        "Live administration: the app directory (LV1 Task 13).",
                        LIVE_ADMIN_SERVICE,
                        "loams.live.v1",
                        true,
                        false,
                        // The app-directory calls only; Deploy is still reached as
                        // `tables.deploy`.
                        List.of(
                                call("live.admin", "CreateApp", "createApp", LIVE_ADMIN_SERVICE,
                                        IdempotencyLevel.NONE, RetryClass.MANUAL,
                                        Streaming.UNARY, null, false),
                                call("live.admin", "DeleteApp", "deleteApp", LIVE_ADMIN_SERVICE,
                                        IdempotencyLevel.NONE, RetryClass.MANUAL,
                                        Streaming.UNARY, null, false),
                                call("live.admin", "GetApp", "getApp", LIVE_ADMIN_SERVICE,
                                        IdempotencyLevel.NONE, RetryClass.SAFE,
                                        Streaming.UNARY, null, false),
                                call("live.admin", "ListApps", "listApps", LIVE_ADMIN_SERVICE,
                                        IdempotencyLevel.NONE, RetryClass.SAFE,
                                        Streaming.UNARY, new Pagination("apps", "next_page_token", "page_size", "page_token"), false),
                                call("live.admin", "UpdateApp", "updateApp", LIVE_ADMIN_SERVICE,
                                        IdempotencyLevel.NONE, RetryClass.MANUAL,
                                        Streaming.UNARY, null, false))),
                new ModuleBinding(
                        "tables",
                        "",
                        LIVE_SERVICE,
                        "loams.live.v1",
                        true,
                        true,
                        List.of(
                                // Deploy moved to LiveAdminService (LV1 Task 10); `tables`
                                // keeps the call, now routed there.
                                call("tables", "Deploy", "deploy", LIVE_ADMIN_SERVICE,
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
                                        Streaming.UNARY, null, false))),
                // SDKP §2.2–§2.5: the app-building modules.
                appModule("namespaces", "Namespaces: the container every collection and stream lives in.",
                        NAMESPACE_SERVICE, false,
                        app("namespaces", "Create", "create", NAMESPACE_SERVICE, "CreateNamespace", IdempotencyLevel.NONE, RetryClass.SAFE, null, false),
                        app("namespaces", "List", "list", NAMESPACE_SERVICE, "ListNamespaces", IdempotencyLevel.NO_SIDE_EFFECTS, RetryClass.SAFE, page("namespaces"), false)),
                appModule("collections", "Collections: schema, aliases, versions, scan plans and the hot tier.",
                        COLLECTION_SERVICE, false,
                        app("collections", "Create", "create", COLLECTION_SERVICE, "CreateCollection", IdempotencyLevel.NONE, RetryClass.SAFE, null, false),
                        app("collections", "List", "list", COLLECTION_SERVICE, "ListCollections", IdempotencyLevel.NO_SIDE_EFFECTS, RetryClass.SAFE, page("collections"), false),
                        app("collections", "Get", "get", COLLECTION_SERVICE, "GetCollection", IdempotencyLevel.NO_SIDE_EFFECTS, RetryClass.SAFE, null, false),
                        app("collections", "Drop", "drop", COLLECTION_SERVICE, "DropCollection", IdempotencyLevel.NONE, RetryClass.SAFE, null, false),
                        app("collections", "AddFields", "addFields", COLLECTION_SERVICE, "AddFields", IdempotencyLevel.NONE, RetryClass.SAFE, null, false),
                        app("collections", "ListVersions", "listVersions", COLLECTION_SERVICE, "ListVersions", IdempotencyLevel.NO_SIDE_EFFECTS, RetryClass.SAFE, page("versions"), false),
                        app("collections", "UpdateAliases", "updateAliases", COLLECTION_SERVICE, "UpdateAliases", IdempotencyLevel.NONE, RetryClass.SAFE, null, false),
                        app("collections", "Scan", "scan", COLLECTION_SERVICE, "Scan", IdempotencyLevel.NONE, RetryClass.MANUAL, null, false),
                        app("collections", "SetHot", "setHot", COLLECTION_SERVICE, "SetHot", IdempotencyLevel.NONE, RetryClass.SAFE, null, false),
                        app("collections", "Warm", "warm", COLLECTION_SERVICE, "WarmCollection", IdempotencyLevel.NONE, RetryClass.SAFE, null, false)),
                appModule("documents", "Documents: write, get, scroll, count, delete by filter and patch by filter.",
                        DOCUMENT_SERVICE, false,
                        app("documents", "Write", "write", DOCUMENT_SERVICE, "WriteDocuments", IdempotencyLevel.NONE, RetryClass.MANUAL, null, true),
                        app("documents", "Get", "get", DOCUMENT_SERVICE, "GetDocuments", IdempotencyLevel.NO_SIDE_EFFECTS, RetryClass.SAFE, null, false),
                        app("documents", "Scroll", "scroll", DOCUMENT_SERVICE, "ScrollDocuments", IdempotencyLevel.NO_SIDE_EFFECTS, RetryClass.SAFE, null, false),
                        app("documents", "Count", "count", DOCUMENT_SERVICE, "CountDocuments", IdempotencyLevel.NO_SIDE_EFFECTS, RetryClass.SAFE, null, false),
                        app("documents", "DeleteByFilter", "deleteByFilter", DOCUMENT_SERVICE, "DeleteByFilter", IdempotencyLevel.NONE, RetryClass.MANUAL, null, false),
                        app("documents", "PatchByFilter", "patchByFilter", DOCUMENT_SERVICE, "PatchByFilter", IdempotencyLevel.NONE, RetryClass.MANUAL, null, false)),
                // Q613: one RPC, two facade entries.
                appModule("search", "Queries: one search over one collection, dense, sparse, text or hybrid.",
                        QUERY_SERVICE, false,
                        app("search", "Query", "query", QUERY_SERVICE, "Search", IdempotencyLevel.NO_SIDE_EFFECTS, RetryClass.SAFE, null, false)),
                appModule("vector", "Vector search: the search RPC with a vector's shape.",
                        QUERY_SERVICE, false,
                        app("vector", "Search", "search", QUERY_SERVICE, "Search", IdempotencyLevel.NO_SIDE_EFFECTS, RetryClass.SAFE, null, false)),
                appModule("graph", "Loams Graph: stored property graphs, queried with GQL.",
                        GRAPH_SERVICE, true,
                        app("graph", "Create", "create", GRAPH_ADMIN_SERVICE, "CreateGraph", IdempotencyLevel.NONE, RetryClass.MANUAL, null, true),
                        app("graph", "Get", "get", GRAPH_ADMIN_SERVICE, "GetGraph", IdempotencyLevel.NO_SIDE_EFFECTS, RetryClass.SAFE, null, false),
                        app("graph", "List", "list", GRAPH_ADMIN_SERVICE, "ListGraphs", IdempotencyLevel.NO_SIDE_EFFECTS, RetryClass.SAFE, page("graphs"), false),
                        app("graph", "Update", "update", GRAPH_ADMIN_SERVICE, "UpdateGraph", IdempotencyLevel.NONE, RetryClass.MANUAL, null, true),
                        app("graph", "Delete", "delete", GRAPH_ADMIN_SERVICE, "DeleteGraph", IdempotencyLevel.NONE, RetryClass.MANUAL, null, true),
                        app("graph", "Schema", "schema", GRAPH_ADMIN_SERVICE, "GetSchema", IdempotencyLevel.NO_SIDE_EFFECTS, RetryClass.SAFE, null, false),
                        app("graph", "Query", "query", GRAPH_SERVICE, "Execute", IdempotencyLevel.NONE, RetryClass.MANUAL, null, true),
                        app("graph", "QueryBatch", "queryBatch", GRAPH_SERVICE, "ExecuteBatch", IdempotencyLevel.NONE, RetryClass.MANUAL, null, true),
                        new CallBinding("graph", "QueryStream", "queryStream", "ExecuteStream",
                                GRAPH_SERVICE + "/ExecuteStream", GRAPH_SERVICE, "loams.graph.v1",
                                IdempotencyLevel.NONE, RetryClass.MANUAL, Streaming.SERVER, null, false),
                        app("graph", "Explain", "explain", GRAPH_SERVICE, "Explain", IdempotencyLevel.NONE, RetryClass.SAFE, null, false)),
                // Part C: loams.sql.v1 and loams.stream.v1 (SDKP P0b and P0c).
                appModule("sql", "SQL: one read-only statement over a namespace's collections, with parameters.",
                        SQL_SERVICE, false,
                        app("sql", "Query", "query", SQL_SERVICE, "Query", IdempotencyLevel.NO_SIDE_EFFECTS, RetryClass.SAFE, null, false),
                        app("sql", "QueryArrow", "queryArrow", SQL_SERVICE, "QueryArrow", IdempotencyLevel.NO_SIDE_EFFECTS, RetryClass.SAFE, null, false)),
                appModule("streams", "Streams: create, list and describe a namespace's logs; produce and fetch.",
                        STREAM_SERVICE, false,
                        app("streams", "Create", "create", STREAM_SERVICE, "CreateStream", IdempotencyLevel.NONE, RetryClass.SAFE, null, false),
                        app("streams", "Get", "get", STREAM_SERVICE, "DescribeStream", IdempotencyLevel.NO_SIDE_EFFECTS, RetryClass.SAFE, null, false),
                        app("streams", "List", "list", STREAM_SERVICE, "ListStreams", IdempotencyLevel.NO_SIDE_EFFECTS, RetryClass.SAFE, page("streams"), false),
                        app("streams", "Produce", "produce", STREAM_SERVICE, "Produce", IdempotencyLevel.NONE, RetryClass.MANUAL, null, false),
                        app("streams", "Fetch", "fetch", STREAM_SERVICE, "Fetch", IdempotencyLevel.NO_SIDE_EFFECTS, RetryClass.SAFE, null, false),
                        app("streams", "ProduceEvents", "produceEvents", STREAM_SERVICE, "ProduceCloudEvents", IdempotencyLevel.NONE, RetryClass.MANUAL, null, false),
                        app("streams", "FetchEvents", "fetchEvents", STREAM_SERVICE, "FetchCloudEvents", IdempotencyLevel.NO_SIDE_EFFECTS, RetryClass.SAFE, null, false)));
    }

    // ---- SDKP: the app-building API ----
    //
    // `loams.collection.v1` and `loams.graph.v1` carry no method-level `FacadeOptions` yet (SDKP
    // P0a adds them), so these rows are transcribed from the plan's tables (§2.2–§2.5): module and
    // call names, and the retry class each table gives. When P0a lands,
    // `java_facade_matches_index` checks every row against `sdks/fixtures/facade-index.json`.

    public static final String NAMESPACE_SERVICE = "loams.collection.v1.NamespaceService";
    public static final String COLLECTION_SERVICE = "loams.collection.v1.CollectionService";
    public static final String DOCUMENT_SERVICE = "loams.collection.v1.DocumentService";
    public static final String QUERY_SERVICE = "loams.collection.v1.QueryService";
    public static final String GRAPH_ADMIN_SERVICE = "loams.graph.v1.GraphAdminService";
    public static final String GRAPH_SERVICE = "loams.graph.v1.GraphService";
    public static final String SQL_SERVICE = "loams.sql.v1.SqlService";
    public static final String STREAM_SERVICE = "loams.stream.v1.StreamService";

    private static ModuleBinding appModule(
            String name, String summary, String service, boolean unstable, CallBinding... calls) {
        return new ModuleBinding(
                name, summary, service, service.substring(0, service.lastIndexOf('.')), unstable, false, List.of(calls));
    }

    private static Pagination page(String items) {
        return new Pagination(items, "next_page_token", "page_size", "page_token");
    }

    private static CallBinding app(
            String module,
            String name,
            String protoName,
            String service,
            String method,
            IdempotencyLevel idempotency,
            RetryClass retry,
            Pagination pagination,
            boolean takesIdempotencyKey) {
        return new CallBinding(
                module,
                name,
                protoName,
                method,
                service + "/" + method,
                service,
                service.substring(0, service.lastIndexOf('.')),
                idempotency,
                retry,
                Streaming.UNARY,
                pagination,
                takesIdempotencyKey);
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