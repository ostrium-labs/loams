// The filter builder (SDKP §2.4): the same verbs in every SDK, each a pure
// function onto the `loams.collection.v1.Query` oneof.
//
//     var where = Filter.Eq("tags", "policy") & Filter.Range("n", gte: 2);
//
// `&`, `|` and `~` are optional sugar for `And`, `Or` and `Not`.

using Google.Protobuf.WellKnownTypes;
using Loams.Collection.V1;

namespace Loams;

/// <summary>Builds <see cref="Query"/> filters.</summary>
public static class Filter
{
    /// <summary>Every document (<c>match_all</c>).</summary>
    public static Query All() => new() { MatchAll = new Empty() };

    /// <summary>No document (<c>match_none</c>).</summary>
    public static Query None() => new() { MatchNone = new Empty() };

    /// <summary>The field equals the value exactly (<c>term</c>).</summary>
    public static Query Eq(string field, object? value) =>
        new() { Term = new TermQuery { Field = field, Value = Values.Of(value) } };

    /// <summary>The field equals one of the values (<c>terms</c>).</summary>
    public static Query In(string field, params object?[] values) => In(field, (IEnumerable<object?>)values);

    /// <summary>The field equals one of the values (<c>terms</c>).</summary>
    public static Query In(string field, IEnumerable<object?> values)
    {
        var terms = new TermsQuery { Field = field };
        terms.Values.AddRange(values.Select(Values.Of));
        return new Query { Terms = terms };
    }

    /// <summary>The field is in a range; each bound left null is open.</summary>
    public static Query Range(string field, object? gt = null, object? gte = null, object? lt = null, object? lte = null)
    {
        var range = new RangeQuery { Field = field };
        if (gt is not null)
        {
            range.Gt = Values.Of(gt);
        }
        if (gte is not null)
        {
            range.Gte = Values.Of(gte);
        }
        if (lt is not null)
        {
            range.Lt = Values.Of(lt);
        }
        if (lte is not null)
        {
            range.Lte = Values.Of(lte);
        }
        return new Query { Range = range };
    }

    /// <summary>The field has a value.</summary>
    public static Query Exists(string field) => new() { Exists = new FieldQuery { Field = field } };

    /// <summary>The field is null or absent.</summary>
    public static Query IsNull(string field) => new() { IsNull = new FieldQuery { Field = field } };

    /// <summary>The field is an empty list or string.</summary>
    public static Query IsEmpty(string field) => new() { IsEmpty = new FieldQuery { Field = field } };

    /// <summary>The field starts with the prefix.</summary>
    public static Query Prefix(string field, string value) => new() { Prefix = new PrefixQuery { Field = field, Value = value } };

    /// <summary>The field matches a <c>*</c>/<c>?</c> pattern.</summary>
    public static Query Wildcard(string field, string pattern) =>
        new() { Wildcard = new WildcardQuery { Field = field, Pattern = pattern } };

    /// <summary>The field is within <paramref name="edits"/> edits of the value; null means <c>auto</c>.</summary>
    public static Query Fuzzy(string field, string value, uint? edits = null) =>
        new() { Fuzzy = new FuzzyQuery { Field = field, Value = value, Fuzziness = FuzzinessOf(edits) } };

    /// <summary>A full-text match on one field (scored).</summary>
    public static Query Match(string field, string text, bool all = false, string? minimumShouldMatch = null,
        string? analyzer = null)
    {
        var match = new MatchQuery { Field = field, Text = text };
        if (all)
        {
            match.Operator = "and";
        }
        if (minimumShouldMatch is not null)
        {
            match.MinimumShouldMatch = minimumShouldMatch;
        }
        if (analyzer is not null)
        {
            match.Analyzer = analyzer;
        }
        return new Query { Match = match };
    }

    /// <summary>A phrase match on one field.</summary>
    public static Query Phrase(string field, string text, uint slop = 0) =>
        new() { MatchPhrase = new MatchPhraseQuery { Field = field, Text = text, Slop = slop } };

    /// <summary>A full-text match across several fields.</summary>
    public static Query MultiMatch(string text, params string[] fields)
    {
        var multi = new MultiMatchQuery { Text = text };
        multi.Fields.AddRange(fields.Select((f) => new MultiMatchField { Field = f }));
        return new Query { MultiMatch = multi };
    }

    /// <summary>A query-string query (<c>title:refund AND n:[1 TO 3]</c>).</summary>
    public static Query QueryString(string query, params string[] defaultFields)
    {
        var q = new QueryStringQuery { Query = query };
        q.DefaultFields.AddRange(defaultFields);
        return new Query { QueryString = q };
    }

    /// <summary>The documents with these ids.</summary>
    public static Query Ids(params DocumentId[] ids) => Ids((IEnumerable<DocumentId>)ids);

    /// <summary>The documents with these ids.</summary>
    public static Query Ids(IEnumerable<DocumentId> ids)
    {
        var query = new Query();
        query.Ids.AddRange(ids.Select(Values.DocumentIdValue));
        return query;
    }

    /// <summary>All of the queries (<c>bool.must</c>).</summary>
    public static Query And(params Query[] queries)
    {
        var b = new BoolQuery();
        b.Must.AddRange(queries);
        return new Query { Bool = b };
    }

    /// <summary>Any of the queries (<c>bool.should</c>).</summary>
    public static Query Or(params Query[] queries) => Or(null, queries);

    /// <summary>At least <paramref name="minimumShouldMatch"/> of the queries.</summary>
    public static Query Or(string? minimumShouldMatch, params Query[] queries)
    {
        var b = new BoolQuery();
        b.Should.AddRange(queries);
        if (minimumShouldMatch is not null)
        {
            b.MinimumShouldMatch = minimumShouldMatch;
        }
        return new Query { Bool = b };
    }

    /// <summary>None of the queries (<c>bool.must_not</c>).</summary>
    public static Query Not(params Query[] queries)
    {
        var b = new BoolQuery();
        b.MustNot.AddRange(queries);
        return new Query { Bool = b };
    }

    /// <summary>All of the queries, without scoring (<c>bool.filter</c>; the plan's <c>filter</c> verb).</summary>
    public static Query NonScoring(params Query[] queries)
    {
        var b = new BoolQuery();
        b.Filter.AddRange(queries);
        return new Query { Bool = b };
    }

    /// <summary>Multiplies a query's score.</summary>
    public static Query Boost(Query query, float boost) => new() { Boost = new BoostQuery { Query = query, Boost = boost } };

    /// <summary>Gives every match of a query the same score.</summary>
    public static Query ConstantScore(Query query, float score) =>
        new() { ConstantScore = new ConstantScoreQuery { Query = query, Score = score } };

    private static Fuzziness FuzzinessOf(uint? edits) =>
        edits is { } n ? new Fuzziness { Edits = n } : new Fuzziness { Auto = new Empty() };

    // ---- the native IR, for the document RPCs' `google.protobuf.Value filter` ----

    /// <summary>
    /// A filter in the engine's native JSON IR, as <c>ScrollDocuments</c>,
    /// <c>CountDocuments</c>, <c>DeleteByFilter</c> and <c>PatchByFilter</c> take it
    /// in their <c>filter</c> field today. The same mapping
    /// <c>crates/loams/src/api/connect_query_filters.rs</c> applies to a search
    /// filter. Retired when those requests gain a typed <c>where</c> (SDKP P0a).
    /// </summary>
    public static Value ToIr(Query query)
    {
        ArgumentNullException.ThrowIfNull(query);
        if (query.QueryCase == Query.QueryOneofCase.None)
        {
            if (query.Ids.Count == 0)
            {
                throw new ArgumentException("a filter names one query", nameof(query));
            }
            return Obj(("ids", List(query.Ids.Select(IdIr))));
        }
        if (query.Ids.Count > 0)
        {
            throw new ArgumentException("a filter names one query; ids and another arm are two", nameof(query));
        }
        return query.QueryCase switch
        {
            Query.QueryOneofCase.MatchAll => Value.ForString("match_all"),
            Query.QueryOneofCase.MatchNone => Value.ForString("match_none"),
            Query.QueryOneofCase.Match => Arm("match", MatchIr(query.Match)),
            Query.QueryOneofCase.MatchPhrase => Arm("match_phrase", Obj(("field", Value.ForString(query.MatchPhrase.Field)),
                ("text", Value.ForString(query.MatchPhrase.Text)),
                ("slop", query.MatchPhrase.Slop == 0 ? null : Value.ForNumber(query.MatchPhrase.Slop)))),
            Query.QueryOneofCase.MultiMatch => Arm("multi_match", Obj(
                ("fields", query.MultiMatch.Fields.Count == 0 ? null : List(query.MultiMatch.Fields.Select((f) =>
                    List([Value.ForString(f.Field), Value.ForNumber(f.Boost == 0 ? 1 : f.Boost)])))),
                ("text", Value.ForString(query.MultiMatch.Text)),
                ("kind", Value.ForString(KindIr(query.MultiMatch.Kind))),
                ("operator", Value.ForString(query.MultiMatch.Operator == BoolOperator.And ? "and" : "or")),
                ("tie_breaker", query.MultiMatch.HasTieBreaker ? Value.ForNumber(query.MultiMatch.TieBreaker) : null))),
            Query.QueryOneofCase.Term => Arm("term", Obj(("field", Value.ForString(query.Term.Field)),
                ("value", query.Term.Value ?? Value.ForNull()))),
            Query.QueryOneofCase.Terms => Arm("terms", Obj(("field", Value.ForString(query.Terms.Field)),
                ("values", List(query.Terms.Values)))),
            Query.QueryOneofCase.Range => Arm("range", Obj(("field", Value.ForString(query.Range.Field)),
                ("gt", query.Range.Gt), ("gte", query.Range.Gte), ("lt", query.Range.Lt), ("lte", query.Range.Lte))),
            Query.QueryOneofCase.Exists => Arm("exists", Obj(("field", Value.ForString(query.Exists.Field)))),
            Query.QueryOneofCase.IsNull => Arm("is_null", Obj(("field", Value.ForString(query.IsNull.Field)))),
            Query.QueryOneofCase.IsEmpty => Arm("is_empty", Obj(("field", Value.ForString(query.IsEmpty.Field)))),
            Query.QueryOneofCase.Prefix => Arm("prefix", Obj(("field", Value.ForString(query.Prefix.Field)),
                ("value", Value.ForString(query.Prefix.Value)))),
            Query.QueryOneofCase.Wildcard => Arm("wildcard", Obj(("field", Value.ForString(query.Wildcard.Field)),
                ("pattern", Value.ForString(query.Wildcard.Pattern)))),
            Query.QueryOneofCase.Fuzzy => Arm("fuzzy", Obj(("field", Value.ForString(query.Fuzzy.Field)),
                ("value", Value.ForString(query.Fuzzy.Value)), ("fuzziness", FuzzinessIr(query.Fuzzy.Fuzziness)))),
            Query.QueryOneofCase.QueryString => Arm("query_string", Obj(("query", Value.ForString(query.QueryString.Query)),
                ("default_fields", query.QueryString.DefaultFields.Count == 0 ? null
                    : List(query.QueryString.DefaultFields.Select(Value.ForString))),
                ("default_operator", Value.ForString(query.QueryString.DefaultOperator == BoolOperator.And ? "and" : "or")))),
            Query.QueryOneofCase.Bool => Arm("bool", Obj(
                ("must", Many(query.Bool.Must)), ("should", Many(query.Bool.Should)),
                ("must_not", Many(query.Bool.MustNot)), ("filter", Many(query.Bool.Filter)),
                ("minimum_should_match", query.Bool.HasMinimumShouldMatch ? Value.ForString(query.Bool.MinimumShouldMatch) : null))),
            Query.QueryOneofCase.Boost => Arm("boost", Obj(("query", ToIr(query.Boost.Query)),
                ("boost", Value.ForNumber(query.Boost.Boost)))),
            Query.QueryOneofCase.ConstantScore => Arm("constant_score", Obj(("query", ToIr(query.ConstantScore.Query)),
                ("score", Value.ForNumber(query.ConstantScore.Score)))),
            _ => throw new ArgumentException($"no native IR for {query.QueryCase}; use a search filter", nameof(query)),
        };
    }

    private static Value MatchIr(MatchQuery m) => Obj(("field", Value.ForString(m.Field)), ("text", Value.ForString(m.Text)),
        ("operator", m.Operator.Length == 0 ? null : Value.ForString(m.Operator)),
        ("minimum_should_match", m.HasMinimumShouldMatch ? Value.ForString(m.MinimumShouldMatch) : null),
        ("fuzziness", m.Fuzziness is null ? null : FuzzinessIr(m.Fuzziness)),
        ("analyzer", m.Analyzer.Length == 0 ? null : Value.ForString(m.Analyzer)));

    private static Value FuzzinessIr(Fuzziness? f) => f?.FuzzinessCase == Fuzziness.FuzzinessOneofCase.Edits
        ? Value.ForNumber(f.Edits)
        : Value.ForString("auto");

    private static string KindIr(MultiMatchKind kind) => kind switch
    {
        MultiMatchKind.MostFields => "most_fields",
        MultiMatchKind.CrossFields => "cross_fields",
        MultiMatchKind.Phrase => "phrase",
        MultiMatchKind.PhrasePrefix => "phrase_prefix",
        _ => "best_fields",
    };

    // A `DocumentId`-shaped Value as the IR's bare key.
    private static Value IdIr(Value id)
    {
        if (id.KindCase == Value.KindOneofCase.StructValue && id.StructValue.Fields.TryGetValue("uint", out var n))
        {
            // Exact as a JSON number up to 2^53; past that the IR keeps the string.
            return n.KindCase == Value.KindOneofCase.StringValue
                && ulong.TryParse(n.StringValue, System.Globalization.NumberStyles.None,
                    System.Globalization.CultureInfo.InvariantCulture, out var u)
                && u <= (1UL << 53)
                ? Value.ForNumber(u)
                : n;
        }
        if (id.KindCase == Value.KindOneofCase.StructValue && id.StructValue.Fields.TryGetValue("string", out var s))
        {
            return s;
        }
        return id;
    }

    private static Value? Many(IEnumerable<Query> queries)
    {
        var list = queries.Select(ToIr).ToList();
        return list.Count == 0 ? null : List(list);
    }

    private static Value Arm(string name, Value body) => Obj((name, body));

    private static Value List(IEnumerable<Value> values)
    {
        var list = new ListValue();
        list.Values.AddRange(values);
        return new Value { ListValue = list };
    }

    private static Value Obj(params (string Key, Value? Value)[] entries)
    {
        var s = new Struct();
        foreach (var (key, value) in entries)
        {
            if (value is not null)
            {
                s.Fields[key] = value;
            }
        }
        return Value.ForStruct(s);
    }
}
