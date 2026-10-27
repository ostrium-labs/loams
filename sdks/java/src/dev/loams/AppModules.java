package dev.loams;

import com.google.protobuf.Message;
import com.google.protobuf.Parser;
import com.google.protobuf.Struct;
import dev.loams.facade.CallBinding;
import dev.loams.gen.loams.collection.v1.AddFieldsRequest;
import dev.loams.gen.loams.collection.v1.AddFieldsResponse;
import dev.loams.gen.loams.collection.v1.CollectionInfo;
import dev.loams.gen.loams.collection.v1.Consistency;
import dev.loams.gen.loams.collection.v1.CountDocumentsRequest;
import dev.loams.gen.loams.collection.v1.CountDocumentsResponse;
import dev.loams.gen.loams.collection.v1.CreateCollectionRequest;
import dev.loams.gen.loams.collection.v1.CreateNamespaceRequest;
import dev.loams.gen.loams.collection.v1.CreateNamespaceResponse;
import dev.loams.gen.loams.collection.v1.DeleteByFilterRequest;
import dev.loams.gen.loams.collection.v1.DeleteDocument;
import dev.loams.gen.loams.collection.v1.Document;
import dev.loams.gen.loams.collection.v1.DocumentId;
import dev.loams.gen.loams.collection.v1.DropCollectionRequest;
import dev.loams.gen.loams.collection.v1.DropCollectionResponse;
import dev.loams.gen.loams.collection.v1.FilterWriteResponse;
import dev.loams.gen.loams.collection.v1.GetCollectionRequest;
import dev.loams.gen.loams.collection.v1.GetDocumentsRequest;
import dev.loams.gen.loams.collection.v1.GetDocumentsResponse;
import dev.loams.gen.loams.collection.v1.Hit;
import dev.loams.gen.loams.collection.v1.ListCollectionsRequest;
import dev.loams.gen.loams.collection.v1.ListCollectionsResponse;
import dev.loams.gen.loams.collection.v1.ListNamespacesRequest;
import dev.loams.gen.loams.collection.v1.ListNamespacesResponse;
import dev.loams.gen.loams.collection.v1.ListVersionsRequest;
import dev.loams.gen.loams.collection.v1.ListVersionsResponse;
import dev.loams.gen.loams.collection.v1.NamespaceInfo;
import dev.loams.gen.loams.collection.v1.Patch;
import dev.loams.gen.loams.collection.v1.PatchByFilterRequest;
import dev.loams.gen.loams.collection.v1.Query;
import dev.loams.gen.loams.collection.v1.ScanPlan;
import dev.loams.gen.loams.collection.v1.ScanRequest;
import dev.loams.gen.loams.collection.v1.ScrollDocumentsRequest;
import dev.loams.gen.loams.collection.v1.ScrollDocumentsResponse;
import dev.loams.gen.loams.collection.v1.SearchRequest;
import dev.loams.gen.loams.collection.v1.SearchResponse;
import dev.loams.gen.loams.collection.v1.SetHotRequest;
import dev.loams.gen.loams.collection.v1.SetHotResponse;
import dev.loams.gen.loams.collection.v1.SortKey;
import dev.loams.gen.loams.collection.v1.UpdateAliasesRequest;
import dev.loams.gen.loams.collection.v1.UpdateAliasesResponse;
import dev.loams.gen.loams.collection.v1.WarmCollectionRequest;
import dev.loams.gen.loams.collection.v1.WarmCollectionResponse;
import dev.loams.gen.loams.collection.v1.WriteDocumentsRequest;
import dev.loams.gen.loams.collection.v1.WriteDocumentsResponse;
import dev.loams.gen.loams.collection.v1.WriteOp;
import dev.loams.gen.loams.graph.v1.CreateGraphRequest;
import dev.loams.gen.loams.graph.v1.DeleteGraphRequest;
import dev.loams.gen.loams.graph.v1.ExecuteBatchRequest;
import dev.loams.gen.loams.graph.v1.ExecuteBatchResponse;
import dev.loams.gen.loams.graph.v1.ExecuteRequest;
import dev.loams.gen.loams.graph.v1.ExecuteResponse;
import dev.loams.gen.loams.graph.v1.ExecuteStreamRequest;
import dev.loams.gen.loams.graph.v1.ExplainRequest;
import dev.loams.gen.loams.graph.v1.GetGraphRequest;
import dev.loams.gen.loams.graph.v1.GetSchemaRequest;
import dev.loams.gen.loams.graph.v1.Graph;
import dev.loams.gen.loams.graph.v1.GraphSchema;
import dev.loams.gen.loams.graph.v1.ListGraphsRequest;
import dev.loams.gen.loams.graph.v1.ListGraphsResponse;
import dev.loams.gen.loams.graph.v1.Plan;
import dev.loams.gen.loams.graph.v1.QueryLanguage;
import dev.loams.gen.loams.graph.v1.ResultChunk;
import dev.loams.gen.loams.graph.v1.UpdateGraphRequest;
import dev.loams.gen.loams.operations.v1.Operation;
import java.util.ArrayList;
import java.util.Iterator;
import java.util.List;
import java.util.Map;
import java.util.NoSuchElementException;

/**
 * The app-building modules (SDKP §2.2–§2.5): namespaces, collections, documents, search, vector and
 * graph. Hand-written under SDKP-4; every method resolves its binding by name from the facade table
 * and names no RPC path or retry class.
 *
 * <p>Each request-typed method has a shorthand beside it. A request whose {@code namespace} is
 * empty gets the client's default (R11); {@link CallOptions#withNamespace} overrides it per call.
 */
public final class AppModules {

    private AppModules() {}

    private abstract static class Base {
        final CallInvoker invoker;
        private final String module;

        Base(CallInvoker invoker, String module) {
            this.invoker = invoker;
            this.module = module;
        }

        /** The module's name, as the facade spells it. */
        public String module() {
            return module;
        }

        <Res extends Message> Res call(Parser<Res> parser, String name, Message request, CallOptions... options) {
            return invoker.unary(Modules.binding(module, name), request, CallOptions.merge(options), parser);
        }
    }

    /** {@code loams.namespaces}: the container every collection and stream lives in. */
    public static final class NamespacesModule extends Base {
        NamespacesModule(CallInvoker invoker) {
            super(invoker, "namespaces");
        }

        /** Creates a namespace. A taken name is {@code already_exists}. Retry-safe. */
        public CreateNamespaceResponse create(CreateNamespaceRequest request, CallOptions... options) {
            return call(CreateNamespaceResponse.parser(), "Create", request, options);
        }

        /** Creates the namespace {@code name}. */
        public CreateNamespaceResponse create(String name, CallOptions... options) {
            return create(CreateNamespaceRequest.newBuilder().setNamespace(name).build(), options);
        }

        /** One page of namespaces. */
        public ListNamespacesResponse list(ListNamespacesRequest request, CallOptions... options) {
            return call(ListNamespacesResponse.parser(), "List", request, options);
        }

        /** Every namespace, following the page tokens. */
        public PageIterator<NamespaceInfo> listAll(int pageSize, CallOptions... options) {
            return PageIterator.of(Modules.binding("namespaces", "List"),
                    token -> list(ListNamespacesRequest.newBuilder().setPageSize(pageSize).setPageToken(token).build(), options));
        }
    }

    /** {@code loams.collections}: collections and their schemas. */
    public static final class CollectionsModule extends Base {
        CollectionsModule(CallInvoker invoker) {
            super(invoker, "collections");
        }

        /** Creates a collection. Idempotent by name; a different schema under a taken name is {@code already_exists}. */
        public CollectionInfo create(CreateCollectionRequest request, CallOptions... options) {
            return call(CollectionInfo.parser(), "Create", request, options);
        }

        /** Creates {@code name} with a schema from {@link Schema}. */
        public CollectionInfo create(String name, Struct schema, CallOptions... options) {
            return create(CreateCollectionRequest.newBuilder().setName(name).setSchema(schema).build(), options);
        }

        /** Creates {@code name} with a schema from the builder. */
        public CollectionInfo create(String name, Schema schema, CallOptions... options) {
            return create(name, schema.build(), options);
        }

        /** One page of the namespace's collections. */
        public ListCollectionsResponse list(ListCollectionsRequest request, CallOptions... options) {
            return call(ListCollectionsResponse.parser(), "List", request, options);
        }

        /** Every collection in the namespace, following the page tokens. */
        public PageIterator<CollectionInfo> listAll(int pageSize, CallOptions... options) {
            return PageIterator.of(Modules.binding("collections", "List"),
                    token -> list(ListCollectionsRequest.newBuilder().setPageSize(pageSize).setPageToken(token).build(), options));
        }

        /** One collection, by name or alias. */
        public CollectionInfo get(GetCollectionRequest request, CallOptions... options) {
            return call(CollectionInfo.parser(), "Get", request, options);
        }

        /** One collection, by name or alias. */
        public CollectionInfo get(String collection, CallOptions... options) {
            return get(GetCollectionRequest.newBuilder().setCollection(collection).build(), options);
        }

        /** Drops a collection. */
        public DropCollectionResponse drop(DropCollectionRequest request, CallOptions... options) {
            return call(DropCollectionResponse.parser(), "Drop", request, options);
        }

        /** Drops a collection. */
        public DropCollectionResponse drop(String collection, CallOptions... options) {
            return drop(DropCollectionRequest.newBuilder().setCollection(collection).build(), options);
        }

        /** Adds fields and vectors to a collection's schema (additive). */
        public AddFieldsResponse addFields(AddFieldsRequest request, CallOptions... options) {
            return call(AddFieldsResponse.parser(), "AddFields", request, options);
        }

        /** One page of the collection's manifest versions. */
        public ListVersionsResponse listVersions(ListVersionsRequest request, CallOptions... options) {
            return call(ListVersionsResponse.parser(), "ListVersions", request, options);
        }

        /** Creates and deletes aliases, atomically. */
        public UpdateAliasesResponse updateAliases(UpdateAliasesRequest request, CallOptions... options) {
            return call(UpdateAliasesResponse.parser(), "UpdateAliases", request, options);
        }

        /** A scan plan over the collection's files. Never retried. */
        public ScanPlan scan(ScanRequest request, CallOptions... options) {
            return call(ScanPlan.parser(), "Scan", request, options);
        }

        /** Sets the collection's hot-tier configuration. */
        public SetHotResponse setHot(SetHotRequest request, CallOptions... options) {
            return call(SetHotResponse.parser(), "SetHot", request, options);
        }

        /** Warms the collection's hot tier. */
        public WarmCollectionResponse warm(WarmCollectionRequest request, CallOptions... options) {
            return call(WarmCollectionResponse.parser(), "Warm", request, options);
        }
    }

    /** {@code loams.documents}: write, read, scroll and count documents. */
    public static final class DocumentsModule extends Base {
        DocumentsModule(CallInvoker invoker) {
            super(invoker, "documents");
        }

        /** Raw write ops, as one write: keyed, the key reused on every retry (R3). Returns the write's token. */
        public WriteDocumentsResponse write(WriteDocumentsRequest request, CallOptions... options) {
            return call(WriteDocumentsResponse.parser(), "Write", request, options);
        }

        /** Upserts documents, as one write. */
        public WriteDocumentsResponse upsert(String collection, List<Doc> docs, CallOptions... options) {
            List<WriteOp> ops = new ArrayList<>();
            for (Doc doc : docs) {
                ops.add(doc.toOp());
            }
            return write(request(collection, ops), options);
        }

        /** Deletes documents by id (longs, strings, UUIDs or DocumentIds), as one write. */
        public WriteDocumentsResponse delete(String collection, List<?> ids, CallOptions... options) {
            List<WriteOp> ops = new ArrayList<>();
            for (Object id : ids) {
                ops.add(WriteOp.newBuilder().setDelete(DeleteDocument.newBuilder().setId(Values.toId(id))).build());
            }
            return write(request(collection, ops), options);
        }

        /**
         * Changes one document: {@code set} merges into its source (deep by default) and {@code unset}
         * removes keys. Either may be null; {@code mode} null takes the engine's default.
         */
        public WriteDocumentsResponse patch(String collection, Object id, Object set, List<String> unset, String mode,
                CallOptions... options) {
            Patch.Builder patch = Patch.newBuilder().setId(Values.toId(id));
            if (set != null) {
                patch.setSource(Values.structFrom(set));
            }
            if (unset != null) {
                patch.addAllDeleteKeys(unset);
            }
            if (mode != null) {
                patch.setMode(mode);
            }
            return write(request(collection, List.of(WriteOp.newBuilder().setPatch(patch).build())), options);
        }

        /** Writes any number of ops in chunks, one chunk in flight at a time (R13). See {@link WriteBatch}. */
        public WriteBatch.Result writeBatch(String collection, Iterable<WriteOp> ops, int chunkRows, int chunkBytes,
                CallOptions... options) {
            CallOptions merged = CallOptions.merge(options);
            return WriteBatch.run(ops, chunkRows, chunkBytes,
                    (chunk, key) -> write(request(collection, chunk), CallOptions.builder().from(merged).idempotencyKey(key).build()));
        }

        /** Upserts any number of documents in chunks of 10 000 ops and 4 MiB (R13). */
        public WriteBatch.Result writeBatch(String collection, List<Doc> docs, CallOptions... options) {
            List<WriteOp> ops = new ArrayList<>();
            for (Doc doc : docs) {
                ops.add(doc.toOp());
            }
            return writeBatch(collection, ops, WriteBatch.DEFAULT_CHUNK_ROWS, WriteBatch.DEFAULT_CHUNK_BYTES, options);
        }

        /** Documents by id. */
        public GetDocumentsResponse get(GetDocumentsRequest request, CallOptions... options) {
            return call(GetDocumentsResponse.parser(), "Get", request, options);
        }

        /** Documents by id (longs, strings, UUIDs or DocumentIds). */
        public GetDocumentsResponse get(String collection, List<?> ids, CallOptions... options) {
            return get(collection, ids, null, options);
        }

        /** Documents by id, at a consistency (null for the session's or the engine's default). */
        public GetDocumentsResponse get(String collection, List<?> ids, Consistency consistency, CallOptions... options) {
            GetDocumentsRequest.Builder request = GetDocumentsRequest.newBuilder().setCollection(collection);
            for (Object id : ids) {
                request.addIds(Values.toId(id));
            }
            if (consistency != null) {
                request.setConsistency(consistency);
            }
            return get(request.build(), options);
        }

        /** One page of documents in id order. */
        public ScrollDocumentsResponse scroll(ScrollDocumentsRequest request, CallOptions... options) {
            return call(ScrollDocumentsResponse.parser(), "Scroll", request, options);
        }

        /**
         * Every document matching {@code where} (null for all), in id order: pages of {@code limit} (0
         * for the engine's default), following {@code next} until it is unset.
         */
        public Iterable<Document> scrollAll(String collection, Query where, int limit, CallOptions... options) {
            ScrollDocumentsRequest.Builder request = ScrollDocumentsRequest.newBuilder().setCollection(collection);
            if (where != null) {
                request.setFilter(Filter.toIr(where));
            }
            if (limit > 0) {
                request.setLimit(limit);
            }
            return scrollAll(request.build(), options);
        }

        /** Every document a scroll request selects, following {@code next}. */
        public Iterable<Document> scrollAll(ScrollDocumentsRequest first, CallOptions... options) {
            return () -> new Iterator<>() {
                private ScrollDocumentsRequest next = first;
                private Iterator<Document> page = java.util.Collections.emptyIterator();
                private int pages;

                @Override
                public boolean hasNext() {
                    while (!page.hasNext()) {
                        if (next == null || pages >= 100_000) {
                            return false;
                        }
                        ScrollDocumentsResponse answer = scroll(next, options);
                        pages++;
                        page = answer.getDocumentsList().iterator();
                        next = answer.hasNext() ? next.toBuilder().setPageToken(answer.getNext()).build() : null;
                    }
                    return true;
                }

                @Override
                public Document next() {
                    if (!hasNext()) {
                        throw new NoSuchElementException();
                    }
                    return page.next();
                }
            };
        }

        /** How many documents match. */
        public CountDocumentsResponse count(CountDocumentsRequest request, CallOptions... options) {
            return call(CountDocumentsResponse.parser(), "Count", request, options);
        }

        /** How many documents match {@code where} (all of them when null). */
        public long count(String collection, Query where, CallOptions... options) {
            CountDocumentsRequest.Builder request = CountDocumentsRequest.newBuilder().setCollection(collection);
            if (where != null) {
                request.setFilter(Filter.toIr(where));
            }
            return count(request.build(), options).getCount();
        }

        /** Deletes every match of a filter. Never retried: resumable through its cursor instead. */
        public FilterWriteResponse deleteByFilter(DeleteByFilterRequest request, CallOptions... options) {
            return call(FilterWriteResponse.parser(), "DeleteByFilter", request, options);
        }

        /** Patches every match of a filter. Never retried: resumable through its cursor instead. */
        public FilterWriteResponse patchByFilter(PatchByFilterRequest request, CallOptions... options) {
            return call(FilterWriteResponse.parser(), "PatchByFilter", request, options);
        }

        private static WriteDocumentsRequest request(String collection, List<WriteOp> ops) {
            return WriteDocumentsRequest.newBuilder().setCollection(collection).addAllOps(ops).build();
        }
    }

    /** {@code loams.search}: text, vector and hybrid search over one collection. */
    public static final class SearchModule extends Base {
        SearchModule(CallInvoker invoker) {
            super(invoker, "search");
        }

        /** One search: the full request IR. Build one with {@link Search}. */
        public SearchResponse query(SearchRequest request, CallOptions... options) {
            return call(SearchResponse.parser(), "Query", request, options);
        }

        /** One search, from the builder. */
        public SearchResponse query(Search search, CallOptions... options) {
            return query(search.build(), options);
        }

        /** A text search: a match on one field, a multi-match on several. */
        public SearchResponse text(String collection, String text, int limit, String... fields) {
            return query(Search.on(collection).text(text, fields).limit(limit));
        }

        /** A hybrid search: text on {@code textField} plus a vector on {@code vectorField}, fused (RRF k=60 when null). */
        public SearchResponse hybrid(String collection, String text, String textField, String vectorField, float[] vector,
                dev.loams.gen.loams.collection.v1.Fusion fusion, int limit, CallOptions... options) {
            return query(Search.on(collection).text(text, textField).vector(vectorField, vector)
                    .fuse(fusion == null ? Fusion.rrf() : fusion).limit(limit), options);
        }

        /**
         * Every hit, page after page, following {@code search_after} from the last hit's sort values
         * until a page comes back short. The request must sort with a primary-key tiebreaker ({@link
         * Sort#pk()}), or pages can skip or repeat hits; without one this raises {@code
         * invalid_argument} before any call.
         */
        public Iterable<Hit> queryAll(SearchRequest request, CallOptions... options) {
            boolean tiebreak = false;
            for (SortKey key : request.getSortList()) {
                tiebreak |= key.getKeyCase() == SortKey.KeyCase.PK;
            }
            if (!tiebreak) {
                throw new InvalidArgumentException(Code.INVALID_ARGUMENT, null, "search_after_needs_tiebreak",
                        Map.of("field", "sort"), "", Modules.binding("search", "Query").rpc(),
                        new IllegalArgumentException("queryAll pages with search_after, which needs a sort with a"
                                + " primary-key tiebreaker: add Sort.pk() as the last sort key"));
            }
            int limit = request.hasLimit() ? request.getLimit() : 10;
            SearchRequest first = request.toBuilder().setLimit(limit).build();
            return () -> new Iterator<>() {
                private SearchRequest next = first;
                private Iterator<Hit> page = java.util.Collections.emptyIterator();
                private int pages;

                @Override
                public boolean hasNext() {
                    while (!page.hasNext()) {
                        if (next == null || pages >= 100_000) {
                            return false;
                        }
                        SearchResponse answer = query(next, options);
                        pages++;
                        List<Hit> hits = answer.getHitsList();
                        page = hits.iterator();
                        next = hits.size() < limit || hits.isEmpty()
                                ? null
                                : next.toBuilder().clearSearchAfter().addAllSearchAfter(hits.get(hits.size() - 1).getSortValuesList())
                                        .setOffset(0).build();
                    }
                    return true;
                }

                @Override
                public Hit next() {
                    if (!hasNext()) {
                        throw new NoSuchElementException();
                    }
                    return page.next();
                }
            };
        }

        /** Every hit of a builder's search; see {@link #queryAll(SearchRequest, CallOptions...)}. */
        public Iterable<Hit> queryAll(Search search, CallOptions... options) {
            return queryAll(search.build(), options);
        }
    }

    /** {@code loams.vector}: the search RPC with a vector's shape (Q613). */
    public static final class VectorModule extends Base {
        VectorModule(CallInvoker invoker) {
            super(invoker, "vector");
        }

        /** The nearest neighbours of {@code vector} in {@code field}; {@code filter} may be null. */
        public SearchResponse search(String collection, float[] vector, String field, int limit, Query filter,
                CallOptions... options) {
            Search search = Search.on(collection).vector(field, vector).limit(limit);
            if (filter != null) {
                search.filter(filter);
            }
            return search(search.build(), options);
        }

        /** A vector search, as the full request. */
        public SearchResponse search(SearchRequest request, CallOptions... options) {
            return call(SearchResponse.parser(), "Search", request, options);
        }
    }

    /**
     * {@code loams.graph}: stored property graphs, queried with GQL. <strong>Experimental</strong>:
     * the package is {@code unstable} (GR1e) and may change.
     */
    public static final class GraphModule extends Base {
        GraphModule(CallInvoker invoker) {
            super(invoker, "graph");
        }

        // `update` and `delete` are never retried (SDKP §2.5), so the SDK does not mint their key;
        // a caller who supplies one makes them retryable.
        private static CallOptions[] unkeyed(CallOptions... options) {
            CallOptions merged = CallOptions.merge(options);
            if (!merged.idempotencyKey().isEmpty()) {
                return new CallOptions[] {merged};
            }
            return new CallOptions[] {CallOptions.builder().from(merged).mintIdempotencyKey(false).build()};
        }

        /** Creates a graph. Keyed (R3). */
        public Graph create(CreateGraphRequest request, CallOptions... options) {
            return call(Graph.parser(), "Create", request, options);
        }

        /** Creates an owned graph named {@code name}. */
        public Graph create(String name, CallOptions... options) {
            return create(CreateGraphRequest.newBuilder().setName(name).build(), options);
        }

        /** One graph. */
        public Graph get(String name, CallOptions... options) {
            return call(Graph.parser(), "Get", GetGraphRequest.newBuilder().setName(name).build(), options);
        }

        /** One page of graphs. */
        public ListGraphsResponse list(ListGraphsRequest request, CallOptions... options) {
            return call(ListGraphsResponse.parser(), "List", request, options);
        }

        /** Updates a graph. Never retried unless the caller supplies a key. */
        public Graph update(UpdateGraphRequest request, CallOptions... options) {
            return call(Graph.parser(), "Update", request, unkeyed(options));
        }

        /** Deletes a graph; answers the deletion's operation. Never retried unless the caller supplies a key. */
        public Operation delete(String name, CallOptions... options) {
            return call(Operation.parser(), "Delete", DeleteGraphRequest.newBuilder().setName(name).build(), unkeyed(options));
        }

        /** The graph's labels, edge types, property keys and indexes. */
        public GraphSchema schema(String name, CallOptions... options) {
            return call(GraphSchema.parser(), "Schema", GetSchemaRequest.newBuilder().setName(name).build(), options);
        }

        /** Runs one statement. A read-only one is retried freely; a write is keyed (R3) and retried with its key. */
        public ExecuteResponse query(ExecuteRequest request, CallOptions... options) {
            CallOptions merged = CallOptions.merge(options);
            if (request.getReadOnly()) {
                CallOptions.Builder read = CallOptions.builder().from(merged);
                if (merged.retrySafe() == null) {
                    read.retrySafe(true);
                }
                if (merged.mintIdempotencyKey() == null) {
                    read.mintIdempotencyKey(false);
                }
                merged = read.build();
            }
            return call(ExecuteResponse.parser(), "Query", request, merged);
        }

        /** Runs one GQL statement with parameters (null for none). */
        public ExecuteResponse query(String graph, String statement, Map<String, ?> parameters, boolean readOnly,
                CallOptions... options) {
            ExecuteRequest.Builder request = ExecuteRequest.newBuilder()
                    .setGraph(graph)
                    .setStatement(statement)
                    .setLanguage(QueryLanguage.QUERY_LANGUAGE_GQL)
                    .setReadOnly(readOnly);
            if (parameters != null) {
                parameters.forEach((name, value) -> request.putParameters(name, GraphValues.of(value)));
            }
            return query(request.build(), options);
        }

        /** Runs several statements, atomically when asked. Keyed. */
        public ExecuteBatchResponse queryBatch(ExecuteBatchRequest request, CallOptions... options) {
            return call(ExecuteBatchResponse.parser(), "QueryBatch", request, options);
        }

        /** Runs one statement and streams its rows in chunks. */
        public Stream<ResultChunk> queryStream(ExecuteStreamRequest request, CallOptions... options) {
            ExecuteStreamRequest filled = request;
            if (request.getRequest().getNamespace().isEmpty() && invoker.defaultNamespace() != null) {
                filled = request.toBuilder()
                        .setRequest(request.getRequest().toBuilder().setNamespace(invoker.defaultNamespace()))
                        .build();
            }
            CallBinding binding = Modules.binding("graph", "QueryStream");
            CallOptions merged = CallOptions.merge(options);
            Receiver<ResultChunk> receiver = invoker.openServerStream(binding, filled, merged, bytes -> {
                try {
                    return ResultChunk.parseFrom(bytes);
                } catch (com.google.protobuf.InvalidProtocolBufferException e) {
                    throw Errors.internal(binding.rpc(), "a frame of " + binding.rpc() + " is not a ResultChunk", e);
                }
            }, filled);
            return new Stream<>(binding.rpc(), false, receiver, null, null, invoker.maxRetries());
        }

        /** The plan for a statement, or its profile. */
        public Plan explain(ExplainRequest request, CallOptions... options) {
            return call(Plan.parser(), "Explain", request, options);
        }
    }

    static DocumentId id(Object id) {
        return Values.toId(id);
    }
}
