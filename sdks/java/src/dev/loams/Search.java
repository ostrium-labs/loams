package dev.loams;

import com.google.protobuf.ListValue;
import com.google.protobuf.Struct;
import com.google.protobuf.Value;
import dev.loams.gen.loams.collection.v1.AnnParams;
import dev.loams.gen.loams.collection.v1.Consistency;
import dev.loams.gen.loams.collection.v1.ExpandDirection;
import dev.loams.gen.loams.collection.v1.ExpandSpec;
import dev.loams.gen.loams.collection.v1.GroupBy;
import dev.loams.gen.loams.collection.v1.Highlight;
import dev.loams.gen.loams.collection.v1.HighlightField;
import dev.loams.gen.loams.collection.v1.Query;
import dev.loams.gen.loams.collection.v1.Retriever;
import dev.loams.gen.loams.collection.v1.SearchRequest;
import dev.loams.gen.loams.collection.v1.SortKey;
import dev.loams.gen.loams.collection.v1.SparseQuery;
import dev.loams.gen.loams.collection.v1.TextQuery;
import dev.loams.gen.loams.collection.v1.VectorQuery;
import java.util.List;
import java.util.Map;

/**
 * The search builder (SDKP §2.4): the same verbs in every SDK, each a pure function onto {@code
 * loams.collection.v1.SearchRequest}.
 *
 * <pre>{@code
 * SearchRequest request = Search.on("notes")
 *     .text("refund", "body")
 *     .vector("embedding", new float[] {0.1f, 0.2f, 0.3f, 0.4f})
 *     .fuse(Fusion.rrf(60))
 *     .filter(Filter.eq("tags", "policy"))
 *     .limit(10)
 *     .build();
 * }</pre>
 */
public final class Search {

    private final SearchRequest.Builder request = SearchRequest.newBuilder();

    private Search(String collection) {
        request.setCollection(collection);
    }

    /** A search over one collection. */
    public static Search on(String collection) {
        if (collection == null) {
            throw new IllegalArgumentException("loams: Search.on needs a collection");
        }
        return new Search(collection);
    }

    /** The namespace, when it is not the client's default. */
    public Search namespace(String ns) {
        request.setNamespace(ns);
        return this;
    }

    /**
     * A text retriever: a {@code match} on one field, a {@code multi_match} on several, or a
     * query-string query when no field is named.
     */
    public Search text(String text, String... fields) {
        return text(switch (fields.length) {
            case 0 -> Filter.queryString(text);
            case 1 -> Filter.match(fields[0], text);
            default -> Filter.multiMatch(text, fields);
        }, 0);
    }

    /** A text retriever over any scoring query; {@code k} 0 takes the default. */
    public Search text(Query query, int k) {
        request.addRetrievers(Retriever.newBuilder().setText(TextQuery.newBuilder().setQuery(query).setK(k)));
        return this;
    }

    /** A dense vector retriever (kNN). */
    public Search vector(String field, float[] values) {
        return vector(field, values, 0, null, null);
    }

    /** A dense vector retriever with a candidate count, ANN parameters and a pre-filter (each may be 0 or null). */
    public Search vector(String field, float[] values, int k, AnnParams ann, Query filter) {
        VectorQuery.Builder vector = VectorQuery.newBuilder().setField(field).setK(k);
        for (float v : values) {
            vector.addQuery(v);
        }
        if (ann != null) {
            vector.setParams(ann);
        }
        if (filter != null) {
            vector.setFilter(filter);
        }
        request.addRetrievers(Retriever.newBuilder().setVector(vector));
        return this;
    }

    /** A sparse vector retriever over term weights. */
    public Search sparse(String field, Map<String, Float> terms, int k) {
        Struct.Builder query = Struct.newBuilder();
        terms.forEach((term, weight) -> query.putFields(term, Value.newBuilder().setNumberValue(weight).build()));
        request.addRetrievers(Retriever.newBuilder().setSparse(SparseQuery.newBuilder().setField(field).setQuery(query).setK(k)));
        return this;
    }

    /** How several retrievers' results combine; see {@link Fusion}. */
    public Search fuse(dev.loams.gen.loams.collection.v1.Fusion fusion) {
        request.setFusion(fusion);
        return this;
    }

    /** A non-scoring filter every hit must pass. */
    public Search filter(Query filter) {
        request.setFilter(filter);
        return this;
    }

    /** The sort keys, in order; see {@link Sort}. */
    public Search sort(SortKey... keys) {
        request.clearSort();
        request.addAllSort(List.of(keys));
        return this;
    }

    /** At most this many hits. */
    public Search limit(int limit) {
        request.setLimit(limit);
        return this;
    }

    /** Skips this many hits. */
    public Search offset(int offset) {
        request.setOffset(offset);
        return this;
    }

    /** The page after the hit with these sort values ({@code search_after}). */
    public Search after(List<Value> sortValues) {
        request.clearSearchAfter();
        request.addAllSearchAfter(sortValues);
        return this;
    }

    /** Drops hits scoring below this. */
    public Search minScore(float score) {
        request.setScoreThreshold(score);
        return this;
    }

    /** The projection: which source paths each hit carries. */
    public Search select(List<String> include, List<String> exclude) {
        return select(include, exclude, null, null);
    }

    /** The projection: source paths, vectors and typed fields; any may be null. */
    public Search select(List<String> include, List<String> exclude, List<String> vectors, List<String> fields) {
        Struct.Builder select = Struct.newBuilder();
        if (include != null || exclude != null) {
            // Only the lists the caller gave, as the builder corpus spells it.
            Struct.Builder source = Struct.newBuilder();
            if (include != null) {
                source.putFields("include", strings(include));
            }
            if (exclude != null) {
                source.putFields("exclude", strings(exclude));
            }
            select.putFields("source", Value.newBuilder().setStructValue(source).build());
        }
        if (vectors != null) {
            select.putFields("vectors", strings(vectors));
        }
        if (fields != null) {
            select.putFields("fields", strings(fields));
        }
        request.setSelect(select);
        return this;
    }

    /** Highlights matches in a field with the engine's tags. */
    public Search highlight(String field) {
        return highlight(field, null, null, null, null);
    }

    /** Highlights matches in a field; each setting may be null. */
    public Search highlight(String field, String pre, String post, Integer fragmentSize, Integer fragments) {
        HighlightField.Builder h = HighlightField.newBuilder().setField(field);
        if (pre != null) {
            h.setPreTag(pre);
        }
        if (post != null) {
            h.setPostTag(post);
        }
        if (fragmentSize != null) {
            h.setFragmentSize(fragmentSize);
        }
        if (fragments != null) {
            h.setNumberOfFragments(fragments);
        }
        Highlight.Builder highlight = request.hasHighlight() ? request.getHighlight().toBuilder() : Highlight.newBuilder();
        request.setHighlight(highlight.addFields(h));
        return this;
    }

    /** Groups hits by a field's value; size and limit may be null. */
    public Search groupBy(String field, Integer size, Integer limit) {
        GroupBy.Builder g = GroupBy.newBuilder().setField(field);
        if (size != null) {
            g.setGroupSize(size);
        }
        if (limit != null) {
            g.setLimit(limit);
        }
        request.setGroupBy(g);
        return this;
    }

    /** Expands each hit one hop through a graph ({@code expand_spec}). */
    public Search expand(String graph, String fromField) {
        return expand(graph, fromField, 1, ExpandDirection.EXPAND_DIRECTION_UNSPECIFIED, List.of(), 0, 0);
    }

    /** Expands each hit through a graph ({@code expand_spec}). */
    public Search expand(String graph, String fromField, int hops, ExpandDirection direction, List<String> edgeTypes,
            int limitPerSeed, int limit) {
        request.setExpandSpec(ExpandSpec.newBuilder()
                .setGraph(graph)
                .setFromField(fromField)
                .setHops(hops)
                .setDirection(direction)
                .addAllEdgeTypes(edgeTypes)
                .setLimitPerSeed(limitPerSeed)
                .setLimit(limit));
        return this;
    }

    /** The read's consistency; see {@link Consistent}. */
    public Search consistency(Consistency consistency) {
        request.setConsistency(consistency);
        return this;
    }

    /**
     * The request. A retriever's {@code k} is sent only when the caller set it; left unset, the
     * engine fills in {@code max(10, offset + limit)}.
     */
    public SearchRequest build() {
        return request.build();
    }

    private static Value strings(List<String> values) {
        ListValue.Builder list = ListValue.newBuilder();
        if (values != null) {
            for (String v : values) {
                list.addValues(Value.newBuilder().setStringValue(v));
            }
        }
        return Value.newBuilder().setListValue(list).build();
    }
}
