package dev.loams;

import com.google.protobuf.Empty;
import com.google.protobuf.ListValue;
import com.google.protobuf.Struct;
import com.google.protobuf.Value;
import dev.loams.gen.loams.collection.v1.BoolOperator;
import dev.loams.gen.loams.collection.v1.BoolQuery;
import dev.loams.gen.loams.collection.v1.BoostQuery;
import dev.loams.gen.loams.collection.v1.ConstantScoreQuery;
import dev.loams.gen.loams.collection.v1.DocumentId;
import dev.loams.gen.loams.collection.v1.FieldQuery;
import dev.loams.gen.loams.collection.v1.Fuzziness;
import dev.loams.gen.loams.collection.v1.FuzzyQuery;
import dev.loams.gen.loams.collection.v1.MatchPhraseQuery;
import dev.loams.gen.loams.collection.v1.MatchQuery;
import dev.loams.gen.loams.collection.v1.MultiMatchField;
import dev.loams.gen.loams.collection.v1.MultiMatchKind;
import dev.loams.gen.loams.collection.v1.MultiMatchQuery;
import dev.loams.gen.loams.collection.v1.PrefixQuery;
import dev.loams.gen.loams.collection.v1.Query;
import dev.loams.gen.loams.collection.v1.QueryStringQuery;
import dev.loams.gen.loams.collection.v1.RangeQuery;
import dev.loams.gen.loams.collection.v1.TermQuery;
import dev.loams.gen.loams.collection.v1.TermsQuery;
import dev.loams.gen.loams.collection.v1.WildcardQuery;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

/**
 * The filter builder (SDKP §2.4): the same verbs in every SDK, each a pure function onto the {@code
 * loams.collection.v1.Query} oneof.
 *
 * <pre>{@code
 * Query where = Filter.and(Filter.eq("tags", "policy"), Filter.range("n", null, 2, null, null));
 * }</pre>
 */
public final class Filter {

    private Filter() {}

    /** Every document ({@code match_all}). */
    public static Query all() {
        return Query.newBuilder().setMatchAll(Empty.getDefaultInstance()).build();
    }

    /** No document ({@code match_none}). */
    public static Query none() {
        return Query.newBuilder().setMatchNone(Empty.getDefaultInstance()).build();
    }

    /** The field equals the value exactly ({@code term}). */
    public static Query eq(String field, Object value) {
        return Query.newBuilder().setTerm(TermQuery.newBuilder().setField(field).setValue(Values.of(value))).build();
    }

    /** The field equals one of the values ({@code terms}). */
    public static Query in(String field, Object... values) {
        return in(field, Arrays.asList(values));
    }

    /** The field equals one of the values ({@code terms}). */
    public static Query in(String field, List<?> values) {
        TermsQuery.Builder terms = TermsQuery.newBuilder().setField(field);
        for (Object value : values) {
            terms.addValues(Values.of(value));
        }
        return Query.newBuilder().setTerms(terms).build();
    }

    /** The field is in a range; each bound left null is open. */
    public static Query range(String field, Object gt, Object gte, Object lt, Object lte) {
        RangeQuery.Builder range = RangeQuery.newBuilder().setField(field);
        if (gt != null) {
            range.setGt(Values.of(gt));
        }
        if (gte != null) {
            range.setGte(Values.of(gte));
        }
        if (lt != null) {
            range.setLt(Values.of(lt));
        }
        if (lte != null) {
            range.setLte(Values.of(lte));
        }
        return Query.newBuilder().setRange(range).build();
    }

    /** {@code field > value}. */
    public static Query gt(String field, Object value) {
        return range(field, value, null, null, null);
    }

    /** {@code field >= value}. */
    public static Query gte(String field, Object value) {
        return range(field, null, value, null, null);
    }

    /** {@code field < value}. */
    public static Query lt(String field, Object value) {
        return range(field, null, null, value, null);
    }

    /** {@code field <= value}. */
    public static Query lte(String field, Object value) {
        return range(field, null, null, null, value);
    }

    /** The field has a value. */
    public static Query exists(String field) {
        return Query.newBuilder().setExists(FieldQuery.newBuilder().setField(field)).build();
    }

    /** The field is null or absent. */
    public static Query isNull(String field) {
        return Query.newBuilder().setIsNull(FieldQuery.newBuilder().setField(field)).build();
    }

    /** The field is an empty list or string. */
    public static Query isEmpty(String field) {
        return Query.newBuilder().setIsEmpty(FieldQuery.newBuilder().setField(field)).build();
    }

    /** The field starts with the prefix. */
    public static Query prefix(String field, String value) {
        return Query.newBuilder().setPrefix(PrefixQuery.newBuilder().setField(field).setValue(value)).build();
    }

    /** The field matches a {@code *}/{@code ?} pattern. */
    public static Query wildcard(String field, String pattern) {
        return Query.newBuilder().setWildcard(WildcardQuery.newBuilder().setField(field).setPattern(pattern)).build();
    }

    /** The field is within {@code edits} edits of the value; null means {@code auto}. */
    public static Query fuzzy(String field, String value, Integer edits) {
        return Query.newBuilder()
                .setFuzzy(FuzzyQuery.newBuilder().setField(field).setValue(value).setFuzziness(fuzziness(edits)))
                .build();
    }

    /** A full-text match on one field (scored). */
    public static Query match(String field, String text) {
        return Query.newBuilder().setMatch(MatchQuery.newBuilder().setField(field).setText(text)).build();
    }

    /** A full-text match on one field that needs every term. */
    public static Query matchAll(String field, String text) {
        return Query.newBuilder()
                .setMatch(MatchQuery.newBuilder().setField(field).setText(text).setOperator("and"))
                .build();
    }

    /** A phrase match on one field. */
    public static Query phrase(String field, String text, int slop) {
        return Query.newBuilder()
                .setMatchPhrase(MatchPhraseQuery.newBuilder().setField(field).setText(text).setSlop(slop))
                .build();
    }

    /** A full-text match across several fields. */
    public static Query multiMatch(String text, String... fields) {
        MultiMatchQuery.Builder multi = MultiMatchQuery.newBuilder().setText(text);
        for (String field : fields) {
            multi.addFields(MultiMatchField.newBuilder().setField(field));
        }
        return Query.newBuilder().setMultiMatch(multi).build();
    }

    /** A query-string query. */
    public static Query queryString(String query, String... defaultFields) {
        return Query.newBuilder()
                .setQueryString(QueryStringQuery.newBuilder().setQuery(query).addAllDefaultFields(List.of(defaultFields)))
                .build();
    }

    /** The documents with these ids (longs, strings, UUIDs or DocumentIds). */
    public static Query ids(Object... ids) {
        Query.Builder query = Query.newBuilder();
        for (DocumentId id : Values.ids(ids)) {
            query.addIds(Values.documentIdValue(id));
        }
        return query.build();
    }

    /** All of the queries ({@code bool.must}). */
    public static Query and(Query... queries) {
        return Query.newBuilder().setBool(BoolQuery.newBuilder().addAllMust(List.of(queries))).build();
    }

    /** Any of the queries ({@code bool.should}). */
    public static Query or(Query... queries) {
        return Query.newBuilder().setBool(BoolQuery.newBuilder().addAllShould(List.of(queries))).build();
    }

    /** At least {@code minimumShouldMatch} of the queries. */
    public static Query orAtLeast(String minimumShouldMatch, Query... queries) {
        return Query.newBuilder()
                .setBool(BoolQuery.newBuilder().addAllShould(List.of(queries)).setMinimumShouldMatch(minimumShouldMatch))
                .build();
    }

    /** None of the queries ({@code bool.must_not}). */
    public static Query not(Query... queries) {
        return Query.newBuilder().setBool(BoolQuery.newBuilder().addAllMustNot(List.of(queries))).build();
    }

    /** All of the queries, without scoring ({@code bool.filter}; the plan's {@code filter} verb). */
    public static Query nonScoring(Query... queries) {
        return Query.newBuilder().setBool(BoolQuery.newBuilder().addAllFilter(List.of(queries))).build();
    }

    /** Multiplies a query's score. */
    public static Query boost(Query query, float boost) {
        return Query.newBuilder().setBoost(BoostQuery.newBuilder().setQuery(query).setBoost(boost)).build();
    }

    /** Gives every match of a query the same score. */
    public static Query constantScore(Query query, float score) {
        return Query.newBuilder()
                .setConstantScore(ConstantScoreQuery.newBuilder().setQuery(query).setScore(score))
                .build();
    }

    private static Fuzziness fuzziness(Integer edits) {
        return edits == null
                ? Fuzziness.newBuilder().setAuto(Empty.getDefaultInstance()).build()
                : Fuzziness.newBuilder().setEdits(edits).build();
    }

    // ---- the native IR, for the document RPCs' `google.protobuf.Value filter` ----

    /**
     * A filter in the engine's native JSON IR, as {@code ScrollDocuments}, {@code CountDocuments},
     * {@code DeleteByFilter} and {@code PatchByFilter} take it in their {@code filter} field today:
     * the mapping {@code crates/loams/src/api/connect_query_filters.rs} applies to a search filter.
     * Retired when those requests gain a typed {@code where} (SDKP P0a).
     */
    public static Value toIr(Query query) {
        if (query.getQueryCase() == Query.QueryCase.QUERY_NOT_SET) {
            if (query.getIdsCount() == 0) {
                throw new IllegalArgumentException("loams: a filter names one query");
            }
            List<Value> ids = new ArrayList<>();
            for (Value id : query.getIdsList()) {
                ids.add(idIr(id));
            }
            return obj("ids", list(ids));
        }
        if (query.getIdsCount() > 0) {
            throw new IllegalArgumentException("loams: a filter names one query; ids and another arm are two");
        }
        return switch (query.getQueryCase()) {
            case MATCH_ALL -> str("match_all");
            case MATCH_NONE -> str("match_none");
            case MATCH -> arm("match", matchIr(query.getMatch()));
            case MATCH_PHRASE -> arm("match_phrase", obj(
                    "field", str(query.getMatchPhrase().getField()),
                    "text", str(query.getMatchPhrase().getText()),
                    "slop", query.getMatchPhrase().getSlop() == 0 ? null : num(query.getMatchPhrase().getSlop())));
            case MULTI_MATCH -> {
                MultiMatchQuery m = query.getMultiMatch();
                List<Value> fields = new ArrayList<>();
                for (MultiMatchField f : m.getFieldsList()) {
                    fields.add(list(List.of(str(f.getField()), num(f.getBoost() == 0 ? 1 : f.getBoost()))));
                }
                yield arm("multi_match", obj(
                        "fields", fields.isEmpty() ? null : list(fields),
                        "text", str(m.getText()),
                        "kind", str(kindIr(m.getKind())),
                        "operator", str(m.getOperator() == BoolOperator.BOOL_OPERATOR_AND ? "and" : "or"),
                        "tie_breaker", m.hasTieBreaker() ? num(m.getTieBreaker()) : null));
            }
            case TERM -> arm("term", obj("field", str(query.getTerm().getField()),
                    "value", query.getTerm().hasValue() ? query.getTerm().getValue() : Values.of(null)));
            case TERMS -> arm("terms", obj("field", str(query.getTerms().getField()),
                    "values", list(query.getTerms().getValuesList())));
            case RANGE -> {
                RangeQuery r = query.getRange();
                yield arm("range", obj("field", str(r.getField()),
                        "gt", r.hasGt() ? r.getGt() : null, "gte", r.hasGte() ? r.getGte() : null,
                        "lt", r.hasLt() ? r.getLt() : null, "lte", r.hasLte() ? r.getLte() : null));
            }
            case EXISTS -> arm("exists", obj("field", str(query.getExists().getField())));
            case IS_NULL -> arm("is_null", obj("field", str(query.getIsNull().getField())));
            case IS_EMPTY -> arm("is_empty", obj("field", str(query.getIsEmpty().getField())));
            case PREFIX -> arm("prefix", obj("field", str(query.getPrefix().getField()),
                    "value", str(query.getPrefix().getValue())));
            case WILDCARD -> arm("wildcard", obj("field", str(query.getWildcard().getField()),
                    "pattern", str(query.getWildcard().getPattern())));
            case FUZZY -> arm("fuzzy", obj("field", str(query.getFuzzy().getField()),
                    "value", str(query.getFuzzy().getValue()), "fuzziness", fuzzinessIr(query.getFuzzy().getFuzziness())));
            case QUERY_STRING -> {
                QueryStringQuery q = query.getQueryString();
                List<Value> fields = new ArrayList<>();
                for (String f : q.getDefaultFieldsList()) {
                    fields.add(str(f));
                }
                yield arm("query_string", obj("query", str(q.getQuery()),
                        "default_fields", fields.isEmpty() ? null : list(fields),
                        "default_operator", str(q.getDefaultOperator() == BoolOperator.BOOL_OPERATOR_AND ? "and" : "or")));
            }
            case BOOL -> {
                BoolQuery b = query.getBool();
                yield arm("bool", obj(
                        "must", many(b.getMustList()), "should", many(b.getShouldList()),
                        "must_not", many(b.getMustNotList()), "filter", many(b.getFilterList()),
                        "minimum_should_match", b.hasMinimumShouldMatch() ? str(b.getMinimumShouldMatch()) : null));
            }
            case BOOST -> arm("boost", obj("query", toIr(query.getBoost().getQuery()),
                    "boost", num(query.getBoost().getBoost())));
            case CONSTANT_SCORE -> arm("constant_score", obj("query", toIr(query.getConstantScore().getQuery()),
                    "score", num(query.getConstantScore().getScore())));
            default -> throw new IllegalArgumentException("loams: no native IR for " + query.getQueryCase());
        };
    }

    private static Value matchIr(MatchQuery m) {
        return obj("field", str(m.getField()), "text", str(m.getText()),
                "operator", m.getOperator().isEmpty() ? null : str(m.getOperator()),
                "minimum_should_match", m.hasMinimumShouldMatch() ? str(m.getMinimumShouldMatch()) : null,
                "fuzziness", m.hasFuzziness() ? fuzzinessIr(m.getFuzziness()) : null,
                "analyzer", m.getAnalyzer().isEmpty() ? null : str(m.getAnalyzer()));
    }

    private static Value fuzzinessIr(Fuzziness f) {
        return f.getFuzzinessCase() == Fuzziness.FuzzinessCase.EDITS ? num(f.getEdits()) : str("auto");
    }

    private static String kindIr(MultiMatchKind kind) {
        return switch (kind) {
            case MULTI_MATCH_KIND_MOST_FIELDS -> "most_fields";
            case MULTI_MATCH_KIND_CROSS_FIELDS -> "cross_fields";
            case MULTI_MATCH_KIND_PHRASE -> "phrase";
            case MULTI_MATCH_KIND_PHRASE_PREFIX -> "phrase_prefix";
            default -> "best_fields";
        };
    }

    private static Value idIr(Value id) {
        if (id.getKindCase() == Value.KindCase.STRUCT_VALUE) {
            Struct s = id.getStructValue();
            if (s.containsFields("uint")) {
                // Exact as a JSON number up to 2^53; past that the IR keeps the string.
                Value n = s.getFieldsOrThrow("uint");
                if (n.getKindCase() == Value.KindCase.STRING_VALUE) {
                    try {
                        long u = Long.parseUnsignedLong(n.getStringValue());
                        if (Long.compareUnsigned(u, 1L << 53) <= 0) {
                            return Value.newBuilder().setNumberValue(u).build();
                        }
                    } catch (NumberFormatException e) {
                        return n;
                    }
                }
                return n;
            }
            if (s.containsFields("string")) {
                return s.getFieldsOrThrow("string");
            }
        }
        return id;
    }

    private static Value many(List<Query> queries) {
        if (queries.isEmpty()) {
            return null;
        }
        List<Value> out = new ArrayList<>();
        for (Query q : queries) {
            out.add(toIr(q));
        }
        return list(out);
    }

    private static Value str(String s) {
        return Value.newBuilder().setStringValue(s).build();
    }

    private static Value num(double d) {
        return Value.newBuilder().setNumberValue(d).build();
    }

    private static Value list(List<Value> values) {
        return Value.newBuilder().setListValue(ListValue.newBuilder().addAllValues(values)).build();
    }

    private static Value arm(String name, Value body) {
        return obj(name, body);
    }

    private static Value obj(Object... pairs) {
        Struct.Builder s = Struct.newBuilder();
        for (int i = 0; i < pairs.length; i += 2) {
            if (pairs[i + 1] != null) {
                s.putFields((String) pairs[i], (Value) pairs[i + 1]);
            }
        }
        return Value.newBuilder().setStructValue(s).build();
    }
}
