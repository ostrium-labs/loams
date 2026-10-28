//! Aggregate names (plan HE1 Task 28): combinator compositions read as
//! ClickHouse reads them, parameters carried in the planned name, and the
//! outermost `-If` planned as `FILTER`.

// `unwrap_used` is a warning and CI runs `-D warnings`; tests unwrap freely.
#![allow(clippy::unwrap_used)]

use loams_house_sql::aggregates::{Combinator, Param, parse, split};
use loams_house_sql::guard;

#[test]
fn combinator_names_parse_as_clickhouse() {
    use Combinator::*;
    // Outermost first, as `AggregateFunctionFactory::getImpl` peels them.
    let cases: &[(&str, &str, &[Combinator])] = &[
        ("sum", "sum", &[]),
        ("sumIf", "sum", &[If]),
        ("countIf", "count", &[If]),
        ("sumArrayIf", "sum", &[If, Array]),
        ("sumIfArray", "sum", &[Array, If]),
        ("groupUniqArrayArrayIf", "groupUniqArray", &[If, Array]),
        ("groupArray", "groupArray", &[]),
        ("groupArrayArray", "groupArray", &[Array]),
        ("groupArrayDistinct", "groupArray", &[Distinct]),
        ("sumMap", "sum", &[Map]),
        ("maxMap", "max", &[Map]),
        ("sumMapIf", "sum", &[If, Map]),
        ("sumMappedArrays", "sumMappedArrays", &[]),
        ("countDistinct", "count", &[Distinct]),
        ("countDistinctIf", "count", &[If, Distinct]),
        ("sumForEach", "sum", &[ForEach]),
        ("sumForEachIf", "sum", &[If, ForEach]),
        ("sumOrNull", "sum", &[OrNull]),
        ("sumOrDefault", "sum", &[OrDefault]),
        ("maxSimpleState", "max", &[SimpleState]),
        ("uniqMergeState", "uniq", &[State, Merge]),
        ("argMax", "argMax", &[]),
        ("argMaxIf", "argMax", &[If]),
        ("topKWeighted", "topKWeighted", &[]),
        // The analyzer reads `…IfOrNull` as `…OrNullIf` (26.9 measured).
        ("sumIfOrNull", "sum", &[If, OrNull]),
        ("countIfOrNull", "count", &[If, OrNull]),
        ("avgIfOrDefault", "avg", &[OrDefault, If]),
        // Aliases resolve to their targets.
        ("array_agg", "groupArray", &[]),
        ("last_value", "anyLast", &[]),
        ("any_value", "any", &[]),
    ];
    for (name, base, combinators) in cases {
        let (b, c) = split(name).unwrap_or_else(|| panic!("{name} does not split"));
        assert_eq!((b.as_str(), c.as_slice()), (*base, *combinators), "{name}");
    }
    for not_aggregate in ["plus", "toStartOfDay", "plusIf", "noSuchFunctionIf"] {
        assert!(split(not_aggregate).is_none(), "{not_aggregate}");
    }
}

#[test]
fn parameters_are_literals_in_the_planned_name() {
    let g = guard("SELECT topK(10)(x), windowFunnel(3600, 'strict_order')(t, a, b), quantile(0.5)(x), groupArray(-1)(x) FROM t", &[]).unwrap();
    let text = g.statement.to_string();
    assert!(text.contains("\"topK(10)\"(x)"), "{text}");
    assert!(
        text.contains("\"windowFunnel(3600, 'strict_order')\"(t, a, b)"),
        "{text}"
    );
    assert!(text.contains("\"quantile(0.5)\"(x)"), "{text}");
    let n = parse("windowFunnel(3600, 'strict_order')").unwrap();
    assert_eq!(
        n.params,
        vec![Param::UInt(3600), Param::String("strict_order".into())]
    );
    assert_eq!(
        parse("groupArray(-1)").unwrap().params,
        vec![Param::Int(-1)]
    );
    // A parameter that is not a literal is ClickHouse's 36; a scalar
    // function's parameters are 309.
    assert_eq!(guard("SELECT topK(x)(y) FROM t", &[]).unwrap_err().code, 36);
    assert_eq!(
        guard("SELECT plus(1)(x, y) FROM t", &[]).unwrap_err().code,
        309
    );
}

#[test]
fn the_outermost_if_is_a_filter() {
    let g = guard("SELECT sumIf(x, x > 1), countIf(y), sumArrayIf(a, c), sumIfArray(a, b), countIfOrNull(c) FROM t", &[]).unwrap();
    let text = g.statement.to_string();
    assert!(text.contains("\"sum\"(x) FILTER (WHERE x > 1)"), "{text}");
    assert!(text.contains("\"count\"(1) FILTER (WHERE y)"), "{text}");
    assert!(text.contains("\"sumArray\"(a) FILTER (WHERE c)"), "{text}");
    assert!(text.contains("\"sumIfArray\"(a, b)"), "{text}");
    assert!(
        text.contains("\"countOrNull\"(1) FILTER (WHERE c)"),
        "{text}"
    );
    // Column names keep the call as written.
    assert!(text.contains("AS \"sumIf(x, greater(x, 1))\""), "{text}");
}
