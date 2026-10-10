//! LV1 plan Task 4: argument validators and visibility. A function's `args`
//! validator is built with `loams:server`'s `v`, listed in its
//! `FunctionMeta`, and checked before the handler runs; internal functions
//! say so. The vocabulary round trip is a proptest: a validator written in
//! JavaScript arrives as the same `Validator`, values generated from it
//! pass `check`, and a mutated value fails at the mutated path.

mod common;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};

use common::*;
use loams_live::testing::TestStore;
use loams_live::validate::{self, FieldViolation, PathElem, Validator};
use loams_live::{DocId, LiveError, LiveValue, TableId, live_test};
use loams_live_js::{Bundle, JsConfig, Visibility};
use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};

const COUNTER: &str = r#"
import { query, mutation, internalQuery, internalMutation, v } from "loams:server";

export const counter = {
  bump: mutation({
    args: { n: v.int64(), tag: v.optional(v.string()) },
    handler: async (ctx, { n }) => {
      await ctx.db.insert("bumps", { n });
      return n;
    },
  }),
  follow: mutation({
    args: v.object({ user: v.id("users") }),
    handler: async (ctx, { user }) => {
      await ctx.db.insert("bumps", { user });
      return null;
    },
  }),
  user: mutation(async (ctx) => await ctx.db.insert("users", { name: "ann" })),
  count: query(async (ctx) => (await ctx.db.query("bumps").collect()).length),
  peek: query({
    args: { id: v.id("users") },
    handler: async (ctx, { id }) => await ctx.db.get(id),
  }),
  reset: internalMutation({
    args: { all: v.boolean() },
    handler: async () => null,
  }),
  audit: internalQuery(async () => null),
};
"#;

fn field_path(names: &[&str]) -> Vec<PathElem> {
    names
        .iter()
        .map(|n| PathElem::Field((*n).to_string()))
        .collect()
}

async fn count(r: &loams_live::Runner, bundle: &Bundle) -> LiveValue {
    query(r, &function(bundle, "counter:count"), unit())
        .await
        .expect("count runs")
        .result
}

/// The arguments are refused with `INVALID_ARGUMENT`, naming `path`.
fn assert_refused(result: Result<impl std::fmt::Debug, LiveError>, path: &str) {
    match result {
        Err(LiveError::InvalidArgument(m)) => assert!(m.contains(path), "{path}: {m}"),
        other => panic!("{path}: expected INVALID_ARGUMENT, got {other:?}"),
    }
}

async fn args_validator_rejects_before_handler_runs(store: TestStore) {
    let r = runner(&store).await;
    let bundle = load(COUNTER).await;
    let bump = function(&bundle, "counter:bump");
    assert_eq!(
        mutate(&r, &bump, obj(&[("n", LiveValue::I64(1))]))
            .await
            .expect("valid arguments")
            .result,
        LiveValue::I64(1)
    );
    assert_eq!(count(&r, &bundle).await, LiveValue::F64(1.0));

    for (args, path) in [
        (obj(&[("n", s("one"))]), "$.n"),
        (obj(&[("n", LiveValue::F64(1.0))]), "$.n"),
        (obj(&[]), "$.n"),
        (
            obj(&[("n", LiveValue::I64(1)), ("extra", s("x"))]),
            "$.extra",
        ),
        (
            obj(&[("n", LiveValue::I64(1)), ("tag", LiveValue::Null)]),
            "$.tag",
        ),
        (LiveValue::Null, "$"),
    ] {
        assert_refused(mutate(&r, &bump, args.clone()).await, path);
    }
    // The handler never ran: the side counter is unchanged.
    assert_eq!(count(&r, &bundle).await, LiveValue::F64(1.0));
    mutate(
        &r,
        &bump,
        obj(&[("n", LiveValue::I64(2)), ("tag", s("ok"))]),
    )
    .await
    .expect("an optional field may be present");
    assert_eq!(count(&r, &bundle).await, LiveValue::F64(2.0));

    // v.id(table): a well-formed id of that table only.
    let user = mutate(&r, &function(&bundle, "counter:user"), unit())
        .await
        .expect("a user")
        .result;
    let follow = function(&bundle, "counter:follow");
    mutate(&r, &follow, obj(&[("user", user.clone())]))
        .await
        .expect("an id of users");
    let bump_id = DocId::random(TableId(1)).expect("an id").to_string();
    for bad in [s(&bump_id), s("not-an-id"), LiveValue::I64(7)] {
        assert_refused(mutate(&r, &follow, obj(&[("user", bad)])).await, "$.user");
    }
    assert_eq!(count(&r, &bundle).await, LiveValue::F64(3.0));

    // Queries are checked too, before their handler reads anything.
    let peek = function(&bundle, "counter:peek");
    let q = query(&r, &peek, obj(&[("id", user)])).await.expect("peek");
    assert_eq!(field(&q.result, "name"), s("ann"));
    assert_refused(query(&r, &peek, obj(&[("id", s("nope"))])).await, "$.id");
}
live_test!(args_validator_rejects_before_handler_runs);

#[tokio::test]
async fn internal_function_metadata_is_internal() {
    let bundle = load(COUNTER).await;
    let metas: BTreeMap<String, (Visibility, Option<Validator>)> = bundle
        .functions()
        .into_iter()
        .map(|m| (m.path, (m.visibility, m.args)))
        .collect();
    assert_eq!(
        metas["counter:reset"],
        (
            Visibility::Internal,
            Some(Validator::Object(BTreeMap::from([(
                "all".to_string(),
                Validator::Boolean
            )])))
        )
    );
    assert_eq!(metas["counter:audit"], (Visibility::Internal, None));
    assert_eq!(metas["counter:count"], (Visibility::Public, None));
    assert_eq!(
        metas["counter:bump"],
        (
            Visibility::Public,
            Some(Validator::Object(BTreeMap::from([
                ("n".to_string(), Validator::Int64),
                (
                    "tag".to_string(),
                    Validator::Optional(Box::new(Validator::String))
                ),
            ])))
        )
    );
    for (path, visibility) in [
        ("counter:reset", Visibility::Internal),
        ("counter:audit", Visibility::Internal),
        ("counter:bump", Visibility::Public),
        ("counter:peek", Visibility::Public),
    ] {
        assert_eq!(function(&bundle, path).visibility(), visibility, "{path}");
    }
    // A built-in function is public by default (LV1 row T0-13).
    let get = loams_live::system::lookup(loams_live::system::GET).expect("get");
    assert_eq!(get.visibility(), Visibility::Public);
}

#[tokio::test]
async fn validator_definitions_are_checked_at_load() {
    for (body, message) in [
        (
            "query({ args: { x: 5 }, handler: () => null })",
            "validator",
        ),
        ("query({ args: v.string(), handler: () => null })", "object"),
        (
            "query({ args: { x: v.array(5) }, handler: () => null })",
            "v.array",
        ),
        (
            "query({ args: { x: v.array(v.optional(v.string())) }, handler: () => null })",
            "v.optional",
        ),
        (
            "query({ args: { x: v.union() }, handler: () => null })",
            "v.union",
        ),
        (
            "query({ args: { x: v.literal({}) }, handler: () => null })",
            "v.literal",
        ),
        (
            "query({ args: { x: v.literal(NaN) }, handler: () => null })",
            "v.literal",
        ),
        (
            "query({ args: { x: v.literal(2n ** 64n) }, handler: () => null })",
            "v.literal",
        ),
        (
            "query({ args: { x: v.id(5) }, handler: () => null })",
            "v.id",
        ),
        (
            "query({ args: { x: v.object(null) }, handler: () => null })",
            "v.object",
        ),
    ] {
        let source = format!(
            "import {{ query, v }} from \"loams:server\";\nexport const m = {{ f: {body} }};\n"
        );
        match try_load(&source, JsConfig::default()).await {
            Err(LiveError::InvalidArgument(m)) => assert!(m.contains(message), "{body}: {m}"),
            other => panic!("{body}: expected a load error, got {other:?}"),
        }
    }
}

// ---- the vocabulary round trip ----

fn scalar() -> impl Strategy<Value = LiveValue> {
    prop_oneof![
        Just(LiveValue::Null),
        any::<bool>().prop_map(LiveValue::Bool),
        any::<i64>().prop_map(LiveValue::I64),
        (-1.0e6..1.0e6f64).prop_map(LiveValue::F64),
        "[a-z é]{0,5}".prop_map(LiveValue::Str),
    ]
}

fn field_name() -> impl Strategy<Value = String> {
    prop::sample::select(vec!["a", "b", "c", "x1", "_id", "a b", "__proto__", "é"])
        .prop_map(str::to_string)
}

fn validator() -> impl Strategy<Value = Validator> {
    let leaf = prop_oneof![
        Just(Validator::Null),
        Just(Validator::Int64),
        Just(Validator::Float64),
        Just(Validator::Boolean),
        Just(Validator::String),
        Just(Validator::Bytes),
        Just(Validator::Any),
        scalar().prop_map(Validator::Literal),
        prop::sample::select(vec!["users", "posts"]).prop_map(|t| Validator::Id(t.to_string())),
    ];
    leaf.prop_recursive(3, 24, 4, |inner| {
        prop_oneof![
            inner.clone().prop_map(|v| Validator::Array(Box::new(v))),
            prop::collection::btree_map(field_name(), (inner.clone(), any::<bool>()), 0..4)
                .prop_map(|fields| {
                    Validator::Object(
                        fields
                            .into_iter()
                            .map(|(k, (v, optional))| {
                                (
                                    k,
                                    if optional {
                                        Validator::Optional(Box::new(v))
                                    } else {
                                        v
                                    },
                                )
                            })
                            .collect(),
                    )
                }),
            prop::collection::vec(inner, 1..4).prop_map(Validator::Union),
        ]
    })
}

/// A value `validator` accepts (ids syntactically: `check` resolves no
/// table).
fn value_for(validator: &Validator) -> BoxedStrategy<LiveValue> {
    match validator {
        Validator::Null => Just(LiveValue::Null).boxed(),
        Validator::Int64 => any::<i64>().prop_map(LiveValue::I64).boxed(),
        Validator::Float64 => (-1.0e9..1.0e9f64).prop_map(LiveValue::F64).boxed(),
        Validator::Boolean => any::<bool>().prop_map(LiveValue::Bool).boxed(),
        Validator::String => ".{0,6}".prop_map(LiveValue::Str).boxed(),
        Validator::Bytes => prop::collection::vec(any::<u8>(), 0..6)
            .prop_map(LiveValue::Bytes)
            .boxed(),
        Validator::Any => scalar().boxed(),
        Validator::Literal(v) => Just(v.clone()).boxed(),
        Validator::Id(_) => (1..1000u32, any::<[u8; 16]>())
            .prop_map(|(t, bytes)| {
                LiveValue::Str(
                    DocId {
                        table: TableId(t),
                        bytes,
                    }
                    .to_string(),
                )
            })
            .boxed(),
        Validator::Array(el) => prop::collection::vec(value_for(el), 0..3)
            .prop_map(LiveValue::Array)
            .boxed(),
        Validator::Object(fields) => {
            let parts: Vec<BoxedStrategy<Option<(String, LiveValue)>>> = fields
                .iter()
                .map(|(name, v)| {
                    let name = name.clone();
                    match v {
                        Validator::Optional(inner) => prop::option::of(value_for(inner))
                            .prop_map(move |v| v.map(|v| (name.clone(), v)))
                            .boxed(),
                        v => value_for(v)
                            .prop_map(move |v| Some((name.clone(), v)))
                            .boxed(),
                    }
                })
                .collect();
            parts
                .prop_map(|parts| LiveValue::Object(parts.into_iter().flatten().collect()))
                .boxed()
        }
        Validator::Union(members) => {
            prop::strategy::Union::new(members.iter().map(value_for).collect::<Vec<_>>()).boxed()
        }
        Validator::Optional(inner) => value_for(inner),
    }
}

/// A value `validator` refuses, if there is an obvious one.
fn bad_value(validator: &Validator) -> Option<LiveValue> {
    Some(match validator {
        Validator::Null => LiveValue::Bool(true),
        Validator::Int64 => LiveValue::F64(1.0),
        Validator::Float64 => LiveValue::I64(1),
        Validator::Boolean => LiveValue::Null,
        Validator::String | Validator::Array(_) => LiveValue::I64(0),
        Validator::Bytes => LiveValue::Str("bytes".into()),
        Validator::Object(_) => LiveValue::Array(Vec::new()),
        Validator::Literal(LiveValue::Str(s)) => LiveValue::Str(format!("{s}!")),
        Validator::Literal(_) => LiveValue::Str("literal".into()),
        Validator::Id(_) => LiveValue::Str("not-an-id".into()),
        Validator::Any => return None,
        Validator::Optional(inner) => return bad_value(inner),
        Validator::Union(members) => {
            return [
                LiveValue::Null,
                LiveValue::Bool(false),
                LiveValue::I64(0),
                LiveValue::F64(0.5),
                LiveValue::Str("zzz".into()),
                LiveValue::Bytes(vec![1]),
                LiveValue::Array(Vec::new()),
                LiveValue::Object(BTreeMap::new()),
            ]
            .into_iter()
            .find(|c| members.iter().all(|m| validate::check(m, c).is_err()));
        }
    })
}

/// One change that breaks a valid value at exactly one path.
#[derive(Debug, Clone)]
enum Mutation {
    Replace(Vec<PathElem>, LiveValue),
    AddField(Vec<PathElem>, String),
    RemoveField(Vec<PathElem>, String),
}

impl Mutation {
    /// The path the violation must name.
    fn expected(&self) -> Vec<PathElem> {
        match self {
            Mutation::Replace(path, _) => path.clone(),
            Mutation::AddField(path, name) | Mutation::RemoveField(path, name) => {
                let mut p = path.clone();
                p.push(PathElem::Field(name.clone()));
                p
            }
        }
    }

    fn apply(&self, value: &mut LiveValue) {
        let (path, last) = match self {
            Mutation::Replace(path, bad) => {
                *at(value, path) = bad.clone();
                return;
            }
            Mutation::AddField(path, name) | Mutation::RemoveField(path, name) => (path, name),
        };
        let LiveValue::Object(fields) = at(value, path) else {
            panic!("{path:?} is an object");
        };
        match self {
            Mutation::AddField(..) => {
                fields.insert(last.clone(), LiveValue::Null);
            }
            _ => {
                fields.remove(last);
            }
        }
    }
}

fn at<'v>(value: &'v mut LiveValue, path: &[PathElem]) -> &'v mut LiveValue {
    path.iter().fold(value, |v, elem| match (v, elem) {
        (LiveValue::Array(items), PathElem::Index(i)) => &mut items[*i],
        (LiveValue::Object(fields), PathElem::Field(f)) => fields.get_mut(f).expect("a field"),
        (v, elem) => panic!("{elem:?} of {v:?}"),
    })
}

/// Every mutation of `value` (valid for `validator`) that one violation at
/// a known path reports. Nothing inside a union: a broken branch value is
/// reported at the union.
fn mutations(validator: &Validator, value: &LiveValue, path: &[PathElem], out: &mut Vec<Mutation>) {
    if let Some(bad) = bad_value(validator) {
        out.push(Mutation::Replace(path.to_vec(), bad));
    }
    match (validator, value) {
        (Validator::Array(el), LiveValue::Array(items)) => {
            for (i, item) in items.iter().enumerate() {
                let mut p = path.to_vec();
                p.push(PathElem::Index(i));
                mutations(el, item, &p, out);
            }
        }
        (Validator::Object(fields), LiveValue::Object(present)) => {
            out.push(Mutation::AddField(path.to_vec(), "zz_unknown".into()));
            for (name, v) in fields {
                let (inner, optional) = match v {
                    Validator::Optional(inner) => (&**inner, true),
                    v => (v, false),
                };
                if let Some(item) = present.get(name) {
                    let mut p = path.to_vec();
                    p.push(PathElem::Field(name.clone()));
                    mutations(inner, item, &p, out);
                    if !optional {
                        out.push(Mutation::RemoveField(path.to_vec(), name.clone()));
                    }
                }
            }
        }
        _ => {}
    }
}

/// `validator` as `loams:server`'s `v` builds it.
fn js(validator: &Validator) -> String {
    match validator {
        Validator::Null => "v.null()".into(),
        Validator::Int64 => "v.int64()".into(),
        Validator::Float64 => "v.float64()".into(),
        Validator::Boolean => "v.boolean()".into(),
        Validator::String => "v.string()".into(),
        Validator::Bytes => "v.bytes()".into(),
        Validator::Any => "v.any()".into(),
        Validator::Literal(v) => format!("v.literal({})", js_scalar(v)),
        Validator::Id(table) => format!("v.id({})", js_string(table)),
        Validator::Array(el) => format!("v.array({})", js(el)),
        Validator::Optional(inner) => format!("v.optional({})", js(inner)),
        Validator::Union(members) => format!(
            "v.union({})",
            members.iter().map(js).collect::<Vec<_>>().join(", ")
        ),
        Validator::Object(fields) => format!("v.object({})", js_fields(fields)),
    }
}

fn js_fields(fields: &BTreeMap<String, Validator>) -> String {
    let fields: Vec<String> = fields
        .iter()
        .map(|(k, v)| format!("[{}]: {}", js_string(k), js(v)))
        .collect();
    format!("{{ {} }}", fields.join(", "))
}

fn js_scalar(v: &LiveValue) -> String {
    match v {
        LiveValue::Null => "null".into(),
        LiveValue::Bool(b) => b.to_string(),
        LiveValue::I64(i) => format!("{i}n"),
        LiveValue::F64(f) => format!("{f}"),
        LiveValue::Str(s) => js_string(s),
        other => panic!("not a literal: {other:?}"),
    }
}

fn js_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == ' ' || c == '_' {
            out.push(c);
        } else {
            out.push_str(&format!("\\u{{{:x}}}", u32::from(c)));
        }
    }
    out.push('"');
    out
}

#[test]
fn validator_vocabulary_roundtrip() {
    let rt = tokio::runtime::Runtime::new().expect("a runtime");
    let config = JsConfig {
        contexts: 1,
        ..JsConfig::default()
    };
    let strategy = validator().prop_flat_map(|v| {
        let value = value_for(&v);
        (Just(v), value, any::<prop::sample::Index>())
    });
    const CASES: u32 = 96;
    let mut runner = TestRunner::new(Config {
        cases: CASES,
        ..Config::default()
    });
    let mutated = AtomicU32::new(0);
    runner
        .run(&strategy, |(v, value, pick)| {
            // JavaScript to Rust: `args: { x: <v> }` arrives as the same
            // validator.
            let source = format!(
                "import {{ query, v }} from \"loams:server\";\n\
                 export const m = {{ f: query({{ args: {{ x: {} }}, handler: () => null }}) }};\n",
                js(&v)
            );
            let bundle = rt
                .block_on(try_load(&source, config.clone()))
                .map_err(|e| TestCaseError::fail(format!("{source}: {e}")))?;
            let args = bundle.functions().remove(0).args;
            let want = Validator::Object(BTreeMap::from([("x".to_string(), v.clone())]));
            prop_assert_eq!(args, Some(want), "{}", source);

            // Values generated from the validator pass it.
            if let Err(violations) = validate::check(&v, &value) {
                return Err(TestCaseError::fail(format!(
                    "{value:?} fails {v:?}: {}",
                    validate::describe(&violations)
                )));
            }
            // A mutated value fails with the mutated path.
            let mut all = Vec::new();
            mutations(&v, &value, &[], &mut all);
            if all.is_empty() {
                return Ok(());
            }
            mutated.fetch_add(1, Ordering::Relaxed);
            let mutation = pick.get(&all).clone();
            let mut broken = value.clone();
            mutation.apply(&mut broken);
            match validate::check(&v, &broken) {
                Ok(()) => Err(TestCaseError::fail(format!(
                    "{mutation:?} of {value:?} still passes {v:?}"
                ))),
                Err(violations) => {
                    let paths: Vec<&Vec<PathElem>> = violations
                        .iter()
                        .map(|f: &FieldViolation| &f.path)
                        .collect();
                    let expected = mutation.expected();
                    prop_assert_eq!(
                        paths,
                        vec![&expected],
                        "{:?} of {:?} for {:?}",
                        mutation,
                        value,
                        v
                    );
                    Ok(())
                }
            }
        })
        .expect("the vocabulary round-trips");
    // Not vacuous: most cases broke their value somewhere.
    let mutated = mutated.into_inner();
    assert!(
        mutated >= CASES / 2,
        "only {mutated} of {CASES} cases mutated a value"
    );
}

#[test]
fn violation_paths_render_like_javascript() {
    let path = vec![
        PathElem::Field("a".into()),
        PathElem::Index(3),
        PathElem::Field("b c".into()),
    ];
    let violation = FieldViolation {
        path,
        message: "expected int64, got string".into(),
    };
    assert_eq!(
        violation.to_string(),
        "$.a[3][\"b c\"]: expected int64, got string"
    );
    assert_eq!(validate::render_path(&[]), "$");
    assert_eq!(validate::render_path(&field_path(&["x"])), "$.x");
}

/// LV1 plan Task 5, `functions_suite_runs_isolated`: every test above again,
/// with its bundles in isolated worker processes (Linux only, where
/// `isolated` exists). The cases are
/// `functions_suite_runs_isolated::<test>[::<backend>]`.
#[cfg(target_os = "linux")]
mod functions_suite_runs_isolated {
    use super::*;

    live_test!(args_validator_rejects_before_handler_runs, |store| {
        crate::common::isolated(super::args_validator_rejects_before_handler_runs(store))
    });

    #[test]
    fn internal_function_metadata_is_internal() {
        crate::common::isolated_sync(super::internal_function_metadata_is_internal);
    }

    #[test]
    fn validator_definitions_are_checked_at_load() {
        crate::common::isolated_sync(super::validator_definitions_are_checked_at_load);
    }

    #[test]
    fn validator_vocabulary_roundtrip() {
        crate::common::isolated_sync(super::validator_vocabulary_roundtrip);
    }

    #[test]
    fn violation_paths_render_like_javascript() {
        crate::common::isolated_sync(super::violation_paths_render_like_javascript);
    }
}
