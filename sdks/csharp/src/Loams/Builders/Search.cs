// The search builder (SDKP §2.4): the same verbs in every SDK, each a pure
// function onto `loams.collection.v1.SearchRequest`.
//
//     var request = Search.On("notes")
//         .Text("refund", "body")
//         .Vector("embedding", [0.1f, 0.2f, 0.3f, 0.4f])
//         .Fuse(Fusion.Rrf(60))
//         .Filter(Filter.Eq("tags", "policy"))
//         .Limit(10);
//     var hits = await client.Search.QueryAsync(request);

using Google.Protobuf.WellKnownTypes;
using Loams.Collection.V1;

namespace Loams;

/// <summary>Fusion methods for a hybrid search.</summary>
public static class Fusion
{
    /// <summary>Reciprocal rank fusion with constant <paramref name="k"/> (60 by default).</summary>
    public static Loams.Collection.V1.Fusion Rrf(uint? k = 60)
    {
        var rrf = new Rrf();
        if (k is { } value)
        {
            rrf.K = value;
        }
        return new Loams.Collection.V1.Fusion { Rrf = rrf };
    }

    /// <summary>Distribution-based score fusion.</summary>
    public static Loams.Collection.V1.Fusion Dbsf() => new() { Dbsf = new Empty() };

    /// <summary>A weighted sum of the retrievers' normalised scores, one weight per retriever.</summary>
    public static Loams.Collection.V1.Fusion Weighted(params float[] weights)
    {
        var sum = new WeightedSum();
        sum.Weights.AddRange(weights);
        return new Loams.Collection.V1.Fusion { WeightedSum = sum };
    }
}

/// <summary>Sort keys.</summary>
public static class Sort
{
    /// <summary>By score, best first unless <paramref name="ascending"/>. The default order is
    /// sent unset (the IR's default for a score, descending), as the builder corpus spells it.</summary>
    public static SortKey Score(bool ascending = false) =>
        new() { Score = new ScoreSort { Order = ascending ? SortOrder.Asc : SortOrder.Unspecified } };

    /// <summary>By primary key, the tiebreaker <c>QueryAllAsync</c> needs.</summary>
    public static SortKey Pk(bool descending = false) =>
        new() { Pk = new PrimaryKeySort { Order = descending ? SortOrder.Desc : SortOrder.Unspecified } };

    /// <summary>By a fast field.</summary>
    public static SortKey Field(string name, bool descending = false, bool? missingFirst = null) =>
        new()
        {
            Field = new FieldSort
            {
                Field = name,
                Order = descending ? SortOrder.Desc : SortOrder.Asc,
                Missing = missingFirst switch
                {
                    true => MissingOrder.First,
                    false => MissingOrder.Last,
                    null => MissingOrder.Unspecified,
                },
            },
        };
}

/// <summary>Consistency levels for a read.</summary>
public static class Consistent
{
    /// <summary>Read everything acknowledged so far.</summary>
    public static Consistency Strong() => new() { Freshness = Freshness.Strong };

    /// <summary>Read whatever the node has; fastest.</summary>
    public static Consistency Eventual() => new() { Freshness = Freshness.Eventual };

    /// <summary>Read at least what a write's token reached.</summary>
    public static Consistency AtLeast(string token) => new() { AtLeast = token };

    /// <summary>Read at least what a write's token reached.</summary>
    public static Consistency AtLeast(ConsistencyToken token) => new() { AtLeast = token.ToString() };
}

/// <summary>Builds a <see cref="SearchRequest"/>.</summary>
public sealed class Search
{
    private readonly SearchRequest _request = new();

    private Search(string collection) => _request.Collection = collection;

    /// <summary>A search over one collection.</summary>
    public static Search On(string collection) => new(collection ?? throw new ArgumentNullException(nameof(collection)));

    /// <summary>The namespace, when it is not the client's default.</summary>
    public Search Namespace(string ns)
    {
        _request.Namespace = ns;
        return this;
    }

    /// <summary>
    /// A text retriever: a <c>match</c> on one field, a <c>multi_match</c> on several,
    /// or a query-string query when no field is named.
    /// </summary>
    public Search Text(string text, params string[] fields) => Text(fields.Length switch
    {
        0 => Loams.Filter.QueryString(text),
        1 => Loams.Filter.Match(fields[0], text),
        _ => Loams.Filter.MultiMatch(text, fields),
    });

    /// <summary>A text retriever over any scoring query.</summary>
    public Search Text(Query query, uint k = 0)
    {
        _request.Retrievers.Add(new Retriever { Text = new TextQuery { Query = query, K = k } });
        return this;
    }

    /// <summary>A dense vector retriever (kNN).</summary>
    public Search Vector(string field, IEnumerable<float> values, uint k = 0, AnnParams? ann = null, Query? filter = null)
    {
        var vector = new VectorQuery { Field = field, K = k, Params = ann, Filter = filter };
        vector.Query.AddRange(values);
        _request.Retrievers.Add(new Retriever { Vector = vector });
        return this;
    }

    /// <summary>A sparse vector retriever over term weights.</summary>
    public Search Sparse(string field, IReadOnlyDictionary<string, float> terms, uint k = 0)
    {
        var query = new Struct();
        foreach (var (term, weight) in terms)
        {
            query.Fields[term] = Value.ForNumber(weight);
        }
        _request.Retrievers.Add(new Retriever { Sparse = new SparseQuery { Field = field, Query = query, K = k } });
        return this;
    }

    /// <summary>How several retrievers' results combine.</summary>
    public Search Fuse(Loams.Collection.V1.Fusion fusion)
    {
        _request.Fusion = fusion;
        return this;
    }

    /// <summary>A non-scoring filter every hit must pass.</summary>
    public Search Filter(Query filter)
    {
        _request.Filter = filter;
        return this;
    }

    /// <summary>The sort keys, in order.</summary>
    public Search Sort(params SortKey[] keys)
    {
        _request.Sort.Clear();
        _request.Sort.AddRange(keys);
        return this;
    }

    /// <summary>At most this many hits.</summary>
    public Search Limit(uint limit)
    {
        _request.Limit = limit;
        return this;
    }

    /// <summary>Skips this many hits.</summary>
    public Search Offset(uint offset)
    {
        _request.Offset = offset;
        return this;
    }

    /// <summary>The page after the hit with these sort values (<c>search_after</c>).</summary>
    public Search After(IEnumerable<Value> sortValues)
    {
        _request.SearchAfter.Clear();
        _request.SearchAfter.AddRange(sortValues);
        return this;
    }

    /// <summary>Drops hits scoring below this.</summary>
    public Search MinScore(float score)
    {
        _request.ScoreThreshold = score;
        return this;
    }

    /// <summary>The projection: which source paths, vectors and typed fields each hit carries.</summary>
    public Search Select(IEnumerable<string>? include = null, IEnumerable<string>? exclude = null,
        IEnumerable<string>? vectors = null, IEnumerable<string>? fields = null)
    {
        var select = new Struct();
        if (include is not null || exclude is not null)
        {
            // Only the lists the caller gave, as the builder corpus spells it.
            var source = new Struct();
            if (include is not null)
            {
                source.Fields["include"] = Value.ForList([.. include.Select(Value.ForString)]);
            }
            if (exclude is not null)
            {
                source.Fields["exclude"] = Value.ForList([.. exclude.Select(Value.ForString)]);
            }
            select.Fields["source"] = Value.ForStruct(source);
        }
        if (vectors is not null)
        {
            select.Fields["vectors"] = Value.ForList([.. vectors.Select(Value.ForString)]);
        }
        if (fields is not null)
        {
            select.Fields["fields"] = Value.ForList([.. fields.Select(Value.ForString)]);
        }
        _request.Select = select;
        return this;
    }

    /// <summary>Highlights matches in a field.</summary>
    public Search Highlight(string field, string? pre = null, string? post = null, uint? fragmentSize = null,
        uint? fragments = null)
    {
        _request.Highlight ??= new Highlight();
        var h = new HighlightField { Field = field };
        if (pre is not null)
        {
            h.PreTag = pre;
        }
        if (post is not null)
        {
            h.PostTag = post;
        }
        if (fragmentSize is { } size)
        {
            h.FragmentSize = size;
        }
        if (fragments is { } n)
        {
            h.NumberOfFragments = n;
        }
        _request.Highlight.Fields.Add(h);
        return this;
    }

    /// <summary>Groups hits by a field's value.</summary>
    public Search GroupBy(string field, uint? size = null, uint? limit = null)
    {
        var g = new GroupBy { Field = field };
        if (size is { } s)
        {
            g.GroupSize = s;
        }
        if (limit is { } l)
        {
            g.Limit = l;
        }
        _request.GroupBy = g;
        return this;
    }

    /// <summary>Expands each hit through a graph (<c>expand_spec</c>).</summary>
    public Search Expand(string graph, string fromField, uint hops = 1, ExpandDirection direction = ExpandDirection.Unspecified,
        IEnumerable<string>? edgeTypes = null, uint limitPerSeed = 0, uint limit = 0)
    {
        var spec = new ExpandSpec
        {
            Graph = graph,
            FromField = fromField,
            Hops = hops,
            Direction = direction,
            LimitPerSeed = limitPerSeed,
            Limit = limit,
        };
        if (edgeTypes is not null)
        {
            spec.EdgeTypes.AddRange(edgeTypes);
        }
        _request.ExpandSpec = spec;
        return this;
    }

    /// <summary>The read's consistency.</summary>
    public Search Consistency(Consistency consistency)
    {
        _request.Consistency = consistency;
        return this;
    }

    /// <summary>
    /// The request. Each call returns a fresh copy. A retriever's <c>k</c> is sent only
    /// when the caller set it; left unset, the engine fills in <c>max(10, offset + limit)</c>.
    /// </summary>
    public SearchRequest Build() => _request.Clone();

    public static implicit operator SearchRequest(Search search) => search.Build();
}
