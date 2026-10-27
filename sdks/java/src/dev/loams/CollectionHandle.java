package dev.loams;

import dev.loams.gen.loams.collection.v1.CollectionInfo;
import dev.loams.gen.loams.collection.v1.Consistency;
import dev.loams.gen.loams.collection.v1.Document;
import dev.loams.gen.loams.collection.v1.DropCollectionResponse;
import dev.loams.gen.loams.collection.v1.GetDocumentsResponse;
import dev.loams.gen.loams.collection.v1.Query;
import dev.loams.gen.loams.collection.v1.SearchRequest;
import dev.loams.gen.loams.collection.v1.SearchResponse;
import dev.loams.gen.loams.collection.v1.WriteDocumentsResponse;
import java.util.List;
import java.util.function.UnaryOperator;

/**
 * One collection's calls (SDKP §2.8): {@code client.collection("notes")} returns calls with the
 * collection (and namespace) filled in. It holds no state of its own; every call delegates to the
 * module call.
 */
public final class CollectionHandle {

    private final Client client;
    private final String name;
    private final String namespace;

    CollectionHandle(Client client, String name, String namespace) {
        this.client = client;
        this.name = name;
        this.namespace = namespace;
    }

    /** The collection's name or alias. */
    public String name() {
        return name;
    }

    /** The namespace, or null for the client's default. */
    public String namespace() {
        return namespace;
    }

    private CallOptions[] scoped(CallOptions... options) {
        CallOptions merged = CallOptions.merge(options);
        if (namespace == null || merged.namespace() != null) {
            return new CallOptions[] {merged};
        }
        return new CallOptions[] {CallOptions.builder().from(merged).namespace(namespace).build()};
    }

    public WriteDocumentsResponse upsert(List<Doc> docs, CallOptions... options) {
        return client.documents().upsert(name, docs, scoped(options));
    }

    public GetDocumentsResponse get(List<?> ids, CallOptions... options) {
        return client.documents().get(name, ids, scoped(options));
    }

    public GetDocumentsResponse get(List<?> ids, Consistency consistency, CallOptions... options) {
        return client.documents().get(name, ids, consistency, scoped(options));
    }

    public WriteDocumentsResponse delete(List<?> ids, CallOptions... options) {
        return client.documents().delete(name, ids, scoped(options));
    }

    public WriteDocumentsResponse patch(Object id, Object set, List<String> unset, CallOptions... options) {
        return client.documents().patch(name, id, set, unset, null, scoped(options));
    }

    public WriteBatch.Result writeBatch(List<Doc> docs, CallOptions... options) {
        return client.documents().writeBatch(name, docs, scoped(options));
    }

    public long count(Query where, CallOptions... options) {
        return client.documents().count(name, where, scoped(options));
    }

    public Iterable<Document> scrollAll(Query where, int limit, CallOptions... options) {
        return client.documents().scrollAll(name, where, limit, scoped(options));
    }

    /** A search over this collection; the builder's collection is replaced by the handle's. */
    public SearchResponse search(Search search, CallOptions... options) {
        SearchRequest request = search.build().toBuilder().setCollection(name).build();
        return client.search().query(request, scoped(options));
    }

    /** A search over this collection, starting from an empty builder. */
    public SearchResponse search(UnaryOperator<Search> build, CallOptions... options) {
        return search(build.apply(Search.on(name)), options);
    }

    public SearchResponse vectorSearch(float[] vector, String field, int limit, Query filter, CallOptions... options) {
        return client.vector().search(name, vector, field, limit, filter, scoped(options));
    }

    public DropCollectionResponse drop(CallOptions... options) {
        return client.collections().drop(name, scoped(options));
    }

    /** The collection's info ({@code collections.get}). */
    public CollectionInfo info(CallOptions... options) {
        return client.collections().get(name, scoped(options));
    }
}
