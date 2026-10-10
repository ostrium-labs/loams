//! LV1 plan Task 3 (R1 Task 13 items 1–3): bundles load, their query and
//! mutation exports run through the `Runner` with read sets, a rerun on
//! conflict is invisible, and unknown paths are not found. On the embedded
//! store, and on TiKV with `LOAMS_TEST_PD`.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use common::*;
use futures::future::BoxFuture;
use loams_live::testing::TestStore;
use loams_live::{DocId, FnKind, Function, LiveError, LiveTxn, LiveValue, live_test, pb, system};
use loams_live_js::{Bundle, JsConfig, Visibility};
use tokio::sync::Notify;

const MESSAGES: &str = r#"
import { query, mutation, internalQuery } from "loams:server";

export const messages = {
  send: mutation({
    handler: async (ctx, args) => {
      return await ctx.db.insert("messages", { channel: args.channel, body: args.body });
    },
  }),
  list: query(async (ctx, args) => {
    return await ctx.db.query("messages").order("desc").take(10);
  }),
  one: query({
    handler: async (ctx, { id }) => await ctx.db.get(id),
  }),
  first: query(async (ctx) => await ctx.db.query("messages").first()),
  sneakyInsert: query(async (ctx) => {
    // A query's ctx.db has no writes; the host refuses them too.
    return typeof ctx.db.insert;
  }),
  count: internalQuery(async (ctx) => (await ctx.db.query("messages").collect()).length),
};
"#;

fn id_of(v: &LiveValue) -> DocId {
    match v {
        LiveValue::Str(id) => id.parse().expect("a document id"),
        other => panic!("an id, not {other:?}"),
    }
}

#[tokio::test]
async fn bundle_lists_its_functions() {
    let bundle = load(MESSAGES).await;
    let mut metas = bundle.functions();
    metas.sort_by(|a, b| a.path.cmp(&b.path));
    let listed: Vec<(&str, FnKind, Visibility)> = metas
        .iter()
        .map(|m| (m.path.as_str(), m.kind, m.visibility))
        .collect();
    assert_eq!(
        listed,
        vec![
            ("messages:count", FnKind::Query, Visibility::Internal),
            ("messages:first", FnKind::Query, Visibility::Public),
            ("messages:list", FnKind::Query, Visibility::Public),
            ("messages:one", FnKind::Query, Visibility::Public),
            ("messages:send", FnKind::Mutation, Visibility::Public),
            ("messages:sneakyInsert", FnKind::Query, Visibility::Public),
        ]
    );
    assert!(metas.iter().all(|m| m.args.is_none()));
    let f = function(&bundle, "messages:send");
    assert_eq!(f.name(), "messages:send");
    assert_eq!(f.kind(), FnKind::Mutation);
}

async fn query_and_mutation_run_and_record_read_sets(store: TestStore) {
    let r = runner(&store).await;
    let bundle = load(MESSAGES).await;
    let send = function(&bundle, "messages:send");
    let m = mutate(
        &r,
        &send,
        obj(&[("channel", s("general")), ("body", s("hi"))]),
    )
    .await
    .expect("send commits");
    let id = id_of(&m.result);
    assert!(m.journal.is_some(), "the insert wrote a journal entry");

    let one = query(
        &r,
        &function(&bundle, "messages:one"),
        obj(&[("id", s(&id.to_string()))]),
    )
    .await
    .expect("one runs");
    assert_eq!(field(&one.result, "body"), s("hi"));
    assert_eq!(field(&one.result, "_id"), s(&id.to_string()));
    assert!(matches!(
        field(&one.result, "_creationTime"),
        LiveValue::I64(_)
    ));
    let key = r.app().document(&id);
    assert!(one.read_set.points.contains(&key), "a get reads its key");
    assert!(one.read_set.ranges.is_empty());

    let list = query(&r, &function(&bundle, "messages:list"), unit())
        .await
        .expect("list runs");
    assert_eq!(items(&list.result).len(), 1);
    assert_eq!(
        list.read_set.ranges.len(),
        1,
        "a query reads one index range"
    );

    let first = query(&r, &function(&bundle, "messages:first"), unit())
        .await
        .expect("first runs");
    assert_eq!(field(&first.result, "body"), s("hi"));

    // A second insert lands in the list's range, not in the get's.
    let m2 = mutate(&r, &send, obj(&[("channel", s("x")), ("body", s("again"))]))
        .await
        .expect("send commits");
    let key2 = r.app().document(&id_of(&m2.result));
    assert!(!one.read_set.covers(&key2));

    let count = query(&r, &function(&bundle, "messages:count"), unit())
        .await
        .expect("count runs");
    assert_eq!(count.result, LiveValue::F64(2.0));

    let sneaky = query(&r, &function(&bundle, "messages:sneakyInsert"), unit())
        .await
        .expect("runs");
    assert_eq!(sneaky.result, s("undefined"));

    // The runner refuses a mutation through Query and a query through Mutate.
    assert!(matches!(
        query(&r, &send, unit()).await,
        Err(LiveError::InvalidArgument(_))
    ));
}
live_test!(query_and_mutation_run_and_record_read_sets);

const COUNTERS: &str = r#"
import { mutation } from "loams:server";

export const counters = {
  bump: mutation(async (ctx, { id }) => {
    const doc = await ctx.db.get(id);
    console.log("bump from", doc.n);
    await ctx.db.patch(id, { n: doc.n + 1n });
    await ctx.db.insert("bumps", { from: doc.n });
    return { n: doc.n + 1n, random: Math.random() };
  }),
};
"#;

/// Runs the JavaScript mutation, then on its first attempt lets the test
/// write the counter before the attempt commits.
struct Interfere {
    inner: Arc<dyn Function>,
    ran: Notify,
    go: Notify,
    first: AtomicBool,
}

impl Function for Interfere {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn kind(&self) -> FnKind {
        self.inner.kind()
    }

    fn call<'a>(
        &'a self,
        txn: &'a mut LiveTxn<'_>,
        args: LiveValue,
    ) -> BoxFuture<'a, Result<LiveValue, LiveError>> {
        Box::pin(async move {
            let result = self.inner.call(txn, args).await?;
            if !self.first.swap(true, Ordering::SeqCst) {
                self.ran.notify_one();
                self.go.notified().await;
            }
            Ok(result)
        })
    }
}

async fn mutation_rerun_on_conflict_is_invisible_to_the_caller(store: TestStore) {
    let r = runner(&store).await;
    let insert = system::lookup(system::INSERT).expect("insert");
    let created = mutate(
        &r,
        &insert,
        obj(&[
            ("table", s("counters")),
            ("fields", obj(&[("n", LiveValue::I64(0))])),
        ]),
    )
    .await
    .expect("the counter is created");
    let id = id_of(&created.result);

    let bundle = load(COUNTERS).await;
    let f = Arc::new(Interfere {
        inner: function(&bundle, "counters:bump"),
        ran: Notify::new(),
        go: Notify::new(),
        first: AtomicBool::new(false),
    });
    let task = {
        let (r, f, id) = (r.clone(), f.clone(), id.to_string());
        tokio::spawn(async move { r.mutate(f, obj(&[("id", s(&id))]), None).await })
    };
    f.ran.notified().await;
    let patch = system::lookup(system::PATCH).expect("patch");
    mutate(
        &r,
        &patch,
        obj(&[
            ("id", s(&id.to_string())),
            ("fields", obj(&[("n", LiveValue::I64(10))])),
        ]),
    )
    .await
    .expect("the interfering patch commits");
    f.go.notify_one();
    let m = task.await.expect("the task").expect("bump commits");

    assert_eq!(m.attempts, 2, "one rerun");
    assert_eq!(field(&m.result, "n"), LiveValue::I64(11));
    // Only the committing attempt's console output reaches the caller.
    assert_eq!(m.output.logs.len(), 1);
    assert_eq!(m.output.logs[0].line, "bump from 10n");
    assert_eq!(m.output.dropped, 0);
    // The first attempt's insert was rolled back.
    let all = system::lookup(system::QUERY).expect("query");
    let bumps = query(&r, &all, obj(&[("table", s("bumps"))]))
        .await
        .expect("bumps");
    let bumps = items(&bumps.result);
    assert_eq!(bumps.len(), 1);
    assert_eq!(field(&bumps[0], "from"), LiveValue::I64(10));
}
live_test!(mutation_rerun_on_conflict_is_invisible_to_the_caller);

#[tokio::test]
async fn unknown_function_is_not_found() {
    let bundle = load(MESSAGES).await;
    for path in [
        "messages:nope",
        "messages",
        "nope:send",
        "send",
        "",
        "_system:get",
        "messages:send:x",
        "messages:__proto__",
        "messages:constructor",
    ] {
        assert!(bundle.function(path).is_none(), "{path:?} is not found");
    }
}

#[tokio::test]
async fn bundle_load_refuses_bad_bundles() {
    let bad = async |source: &str| match Bundle::load(source, JsConfig::default()).await {
        Ok(_) => panic!("the bundle loads: {source}"),
        Err(e) => e,
    };
    // Syntax error, a throw at the top level, an import other than
    // loams:server, a function exported outside a module object, a
    // reserved module name, and argument validators before Task 4.
    for (source, needle) in [
        ("export const m = {", "SyntaxError"),
        ("throw new Error('boom')", "boom"),
        ("import x from 'fs'; export const m = {};", "fs"),
        (
            "import { query } from 'loams:server'; export const list = query(async () => 1);",
            "module object",
        ),
        (
            "import { query } from 'loams:server'; export const _sys = { q: query(async () => 1) };",
            "_sys",
        ),
        (
            "import { query, v } from 'loams:server'; export const m = { q: query({ args: { a: 1 }, handler: async () => 1 }) };",
            "validator",
        ),
        (
            "import { query } from 'loams:server'; query(42);",
            "handler",
        ),
    ] {
        let e = bad(source).await;
        assert_eq!(
            e.code(),
            pb::ErrorCode::ERROR_CODE_INVALID_ARGUMENT,
            "{source}: {e}"
        );
        assert!(e.to_string().contains(needle), "{source}: {e}");
    }
    let huge = format!("// {}", "x".repeat(loams_live_js::MAX_BUNDLE_BYTES));
    assert!(matches!(
        bad(&huge).await,
        LiveError::LimitExceeded {
            limit: "max_bundle_bytes",
            ..
        }
    ));
    let many: String = (0..=loams_live_js::MAX_EXPORTS)
        .map(|i| format!("f{i}: query(async () => {i}),"))
        .collect();
    let many = format!("import {{ query }} from 'loams:server'; export const m = {{ {many} }};");
    assert!(matches!(
        bad(&many).await,
        LiveError::LimitExceeded {
            limit: "max_exports",
            ..
        }
    ));
}

const ERRORS: &str = r#"
import { query } from "loams:server";

class AppError extends Error {
  constructor(message) {
    super(message);
    this.name = "AppError";
  }
}

export const errors = {
  thrown: query(async () => { throw new AppError("nope"); }),
  thrownValue: query(async () => { throw 42; }),
  badIndex: query(async (ctx) => await ctx.db.query("t").withIndex("by_nothing").collect()),
  caughtHostError: query(async (ctx) => {
    try {
      await ctx.db.get("not-an-id");
      return "no error";
    } catch (e) {
      return e.message.length > 0;
    }
  }),
  badResult: query(async () => () => 1),
  never: query(() => new Promise(() => {})),
};
"#;

async fn function_errors_map_to_live_errors(store: TestStore) {
    let r = runner(&store).await;
    let bundle = load(ERRORS).await;
    let run = |path: &'static str| {
        let (r, f) = (r.clone(), function(&bundle, path));
        async move { query(&r, &f, unit()).await }
    };
    match run("errors:thrown").await {
        Err(LiveError::FunctionError(m)) => assert!(m.contains("AppError: nope"), "{m}"),
        other => panic!("a function error, not {other:?}"),
    }
    match run("errors:thrownValue").await {
        Err(LiveError::FunctionError(m)) => assert!(m.contains("42"), "{m}"),
        other => panic!("a function error, not {other:?}"),
    }
    // A host error the handler does not catch keeps its own code.
    let mut t = system::lookup(system::INSERT).expect("insert");
    mutate(&r, &t, obj(&[("table", s("t")), ("fields", obj(&[]))]))
        .await
        .expect("t exists");
    t = function(&bundle, "errors:badIndex");
    match query(&r, &t, unit()).await {
        Err(LiveError::NotFound(m)) => assert!(m.contains("by_nothing"), "{m}"),
        other => panic!("not found, not {other:?}"),
    }
    assert_eq!(
        run("errors:caughtHostError").await.expect("runs").result,
        LiveValue::Bool(true)
    );
    match run("errors:badResult").await {
        Err(LiveError::FunctionError(m)) => assert!(m.contains("function"), "{m}"),
        other => panic!("a function error, not {other:?}"),
    }
    match run("errors:never").await {
        Err(LiveError::FunctionError(m)) => assert!(m.contains("never settled"), "{m}"),
        other => panic!("a function error, not {other:?}"),
    }
}
live_test!(function_errors_map_to_live_errors);

const VALUES: &str = r#"
import { query } from "loams:server";

export const values = {
  echo: query(async (ctx, args) => args),
  make: query(async () => ({
    i: 9007199254740993n,
    f: 1.5,
    whole: 2,
    s: "é",
    b: true,
    n: null,
    u: undefined,
    bytes: new Uint8Array([0, 255, 7]).buffer,
    arr: [1n, undefined, "x"],
  })),
  bigTooBig: query(async () => 2n ** 64n),
  typed: query(async () => new Uint8Array([1])),
  proxy: query(async () => new Proxy({}, { get() { return 1; } })),
  date: query(async () => new Date(0)),
  cycle: query(async () => { const a = {}; a.a = a; return a; }),
  types: query(async (ctx, args) => Object.fromEntries(
    Object.entries(args).map(([k, v]) => [k, v instanceof ArrayBuffer ? "ArrayBuffer" : typeof v]))),
};
"#;

async fn values_round_trip_between_rust_and_javascript(store: TestStore) {
    let r = runner(&store).await;
    let bundle = load(VALUES).await;
    let args = obj(&[
        ("i", LiveValue::I64(i64::MIN)),
        ("f", LiveValue::F64(-0.25)),
        ("s", s("ü")),
        ("b", LiveValue::Bool(false)),
        ("n", LiveValue::Null),
        ("bytes", LiveValue::Bytes(vec![1, 2, 255])),
        (
            "arr",
            LiveValue::Array(vec![LiveValue::I64(1), obj(&[("x", LiveValue::F64(2.0))])]),
        ),
    ]);
    let echo = query(&r, &function(&bundle, "values:echo"), args.clone())
        .await
        .expect("echo");
    assert_eq!(echo.result, args);
    let types = query(&r, &function(&bundle, "values:types"), args)
        .await
        .expect("types");
    assert_eq!(field(&types.result, "i"), s("bigint"));
    assert_eq!(field(&types.result, "f"), s("number"));
    assert_eq!(field(&types.result, "bytes"), s("ArrayBuffer"));
    assert_eq!(field(&types.result, "arr"), s("object"));

    let made = query(&r, &function(&bundle, "values:make"), unit())
        .await
        .expect("make")
        .result;
    assert_eq!(field(&made, "i"), LiveValue::I64(9_007_199_254_740_993));
    assert_eq!(field(&made, "f"), LiveValue::F64(1.5));
    assert_eq!(field(&made, "whole"), LiveValue::F64(2.0));
    assert_eq!(field(&made, "s"), s("é"));
    assert_eq!(field(&made, "bytes"), LiveValue::Bytes(vec![0, 255, 7]));
    let LiveValue::Object(fields) = &made else {
        panic!("an object")
    };
    assert!(!fields.contains_key("u"), "an undefined field is left out");
    assert_eq!(
        field(&made, "arr"),
        LiveValue::Array(vec![LiveValue::I64(1), LiveValue::Null, s("x")])
    );
    for path in [
        "values:bigTooBig",
        "values:typed",
        "values:proxy",
        "values:date",
        "values:cycle",
    ] {
        match query(&r, &function(&bundle, path), unit()).await {
            Err(LiveError::FunctionError(_)) => {}
            other => panic!("{path}: a function error, not {other:?}"),
        }
    }
}
live_test!(values_round_trip_between_rust_and_javascript);

const FAULTS: &str = r#"
import { query, mutation } from "loams:server";

export const faults = {
  swallow: mutation(async (ctx, { id }) => {
    try {
      await ctx.db.get(id);
    } catch (e) {
      console.log("caught");
      await ctx.db.insert("swallowed", { message: String(e) });
      return "swallowed";
    }
    await ctx.db.insert("kept", { x: 1n });
    return "ok";
  }),
  swallowQuery: query(async (ctx) => {
    try {
      return await ctx.db.query("kept").collect();
    } catch (e) {
      console.log("caught");
      return "swallowed";
    }
  }),
};
"#;

/// Injects a storage error into the first store call of the first attempt
/// it runs, and keeps what that attempt returned.
struct FaultOnce {
    inner: Arc<dyn Function>,
    injected: AtomicBool,
    first: std::sync::Mutex<Option<Result<LiveValue, LiveError>>>,
    /// The console lines of the attempt with the fault.
    lines: std::sync::Mutex<Vec<String>>,
}

impl Function for FaultOnce {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn kind(&self) -> FnKind {
        self.inner.kind()
    }

    fn call<'a>(
        &'a self,
        txn: &'a mut LiveTxn<'_>,
        args: LiveValue,
    ) -> BoxFuture<'a, Result<LiveValue, LiveError>> {
        Box::pin(async move {
            let first = !self.injected.swap(true, Ordering::SeqCst);
            if first {
                txn.fail_next_store_call(loams_kv::TxnError::Conflict);
            }
            let result = self.inner.call(txn, args).await;
            if first {
                *self.first.lock().expect("lock") = Some(result.clone());
                *self.lines.lock().expect("lock") =
                    txn.output().logs.iter().map(|l| l.line.clone()).collect();
            }
            result
        })
    }
}

/// I3: a storage error inside a host call (here an injected conflict) is
/// never swallowed by the handler's `try`: the call fails with the storage
/// error, and the runner reruns the mutation from scratch.
async fn storage_errors_in_host_calls_are_never_swallowed(store: TestStore) {
    let r = runner(&store).await;
    let bundle = load(FAULTS).await;
    let insert = system::lookup(system::INSERT).expect("insert");
    let id = mutate(
        &r,
        &insert,
        obj(&[
            ("table", s("kept")),
            ("fields", obj(&[("x", LiveValue::I64(0))])),
        ]),
    )
    .await
    .expect("insert")
    .result;
    let f = Arc::new(FaultOnce {
        inner: function(&bundle, "faults:swallow"),
        injected: AtomicBool::new(false),
        first: std::sync::Mutex::new(None),
        lines: std::sync::Mutex::new(Vec::new()),
    });
    let m = r
        .mutate(f.clone(), obj(&[("id", id)]), None)
        .await
        .expect("the rerun commits");
    assert_eq!(m.result, s("ok"));
    assert_eq!(m.attempts, 2, "the conflict reran the mutation");
    match f.first.lock().expect("lock").take() {
        Some(Err(LiveError::Txn(loams_kv::TxnError::Conflict))) => {}
        other => panic!("the first attempt failed with the conflict, not {other:?}"),
    }
    assert!(
        f.lines.lock().expect("lock").is_empty(),
        "the handler's catch never ran"
    );
    let all = system::lookup(system::QUERY).expect("query");
    let swallowed = query(&r, &all, obj(&[("table", s("swallowed"))]))
        .await
        .expect("swallowed");
    assert!(items(&swallowed.result).is_empty(), "the catch never ran");
    let kept = query(&r, &all, obj(&[("table", s("kept"))]))
        .await
        .expect("kept");
    assert_eq!(items(&kept.result).len(), 2);

    // A query: the storage error is the call's.
    let q = FaultOnce {
        inner: function(&bundle, "faults:swallowQuery"),
        injected: AtomicBool::new(false),
        first: std::sync::Mutex::new(None),
        lines: std::sync::Mutex::new(Vec::new()),
    };
    let at = r.store().now().await.expect("now");
    match r.query(&q, unit(), at).await {
        Err(LiveError::Txn(loams_kv::TxnError::Conflict)) => {}
        other => panic!("the conflict, not {other:?}"),
    }
    assert!(
        q.lines.lock().expect("lock").is_empty(),
        "the handler's catch never ran"
    );
    // The slot serves the next call.
    let again = r.query(&q, unit(), at).await.expect("no fault this time");
    assert_eq!(items(&again.result).len(), 2);
}
live_test!(storage_errors_in_host_calls_are_never_swallowed);

const PATCHES: &str = r#"
import { mutation } from "loams:server";

export const patches = {
  unset: mutation(async (ctx) => {
    const id = await ctx.db.insert("notes", { title: "a", body: "b" });
    let refused;
    try {
      await ctx.db.patch(id, { body: undefined });
    } catch (e) {
      refused = String(e);
    }
    await ctx.db.patch(id, { title: "c" });
    const doc = await ctx.db.get(id);
    return { refused, title: doc.title, body: doc.body };
  }),
};
"#;

/// Fix round 1 (minor): R1 cannot remove a field by patching (row T8-9),
/// while Convex's `patch` removes a field set to `undefined`. Rather than
/// silently keep the field, `ctx.db.patch` refuses an `undefined` field
/// and names `replace`.
async fn patch_with_an_undefined_field_is_refused(store: TestStore) {
    let r = runner(&store).await;
    let bundle = load(PATCHES).await;
    let m = mutate(&r, &function(&bundle, "patches:unset"), unit())
        .await
        .expect("unset");
    let LiveValue::Str(refused) = field(&m.result, "refused") else {
        panic!("the patch is refused: {:?}", m.result)
    };
    assert!(
        refused.starts_with("TypeError") && refused.contains("body") && refused.contains("replace"),
        "{refused}"
    );
    assert_eq!(field(&m.result, "title"), s("c"));
    assert_eq!(field(&m.result, "body"), s("b"));
}
live_test!(patch_with_an_undefined_field_is_refused);

const PAGES: &str = r#"
import { query, mutation } from "loams:server";

export const pages = {
  fill: mutation(async (ctx, { n }) => {
    for (let i = 0n; i < n; i++) {
      await ctx.db.insert("items", { i });
    }
  }),
  list: query(async (ctx, { cursor, numItems, order }) =>
    await ctx.db
      .query("items")
      .withIndex("by_creation_time")
      .order(order ?? "asc")
      .paginate({ cursor, numItems })),
  caught: query(async (ctx, { cursor }) => {
    try {
      await ctx.db.query("items").paginate({ cursor, numItems: 2 });
      return "accepted";
    } catch (e) {
      return String(e);
    }
  }),
  badOptions: query(async (ctx, { options }) => {
    try {
      await ctx.db.query("items").paginate(options);
      return "accepted";
    } catch (e) {
      return String(e);
    }
  }),
};
"#;

/// LV1 plan Task 4: `paginate({ cursor, numItems })` from JavaScript returns
/// `{ page, continueCursor, isDone }` and its read set ends at the page's
/// last key; a forged cursor is `INVALID_ARGUMENT` (`live_bad_cursor`) when
/// it escapes the handler, and a catchable error inside it.
async fn paginate_in_javascript(store: TestStore) {
    let r = runner(&store).await;
    let bundle = load(PAGES).await;
    mutate(
        &r,
        &function(&bundle, "pages:fill"),
        obj(&[("n", LiveValue::I64(5))]),
    )
    .await
    .expect("fill");
    let list = function(&bundle, "pages:list");
    let page = |cursor: LiveValue| {
        let (r, list) = (&r, &list);
        async move {
            query(
                r,
                list,
                obj(&[("cursor", cursor), ("numItems", LiveValue::F64(2.0))]),
            )
            .await
        }
    };
    // The documents share one creation time (one mutation inserted them),
    // so their order is the ids'; the pages hold each document once.
    let ids = |q: &loams_live::Queried| -> Vec<LiveValue> {
        items(&field(&q.result, "page"))
            .iter()
            .map(|d| field(d, "i"))
            .collect()
    };
    let first = page(LiveValue::Null).await.expect("page 1");
    assert_eq!(ids(&first).len(), 2);
    assert_eq!(field(&first.result, "isDone"), LiveValue::Bool(false));
    assert_eq!(first.read_set.ranges.len(), 1, "a page reads one range");
    let second = page(field(&first.result, "continueCursor"))
        .await
        .expect("page 2");
    assert_eq!(ids(&second).len(), 2);
    let third = page(field(&second.result, "continueCursor"))
        .await
        .expect("page 3");
    assert_eq!(ids(&third).len(), 1);
    assert_eq!(field(&third.result, "isDone"), LiveValue::Bool(true));
    let mut seen: Vec<i64> = [&first, &second, &third]
        .iter()
        .flat_map(|q| ids(q))
        .map(|v| match v {
            LiveValue::I64(i) => i,
            other => panic!("i is an int64, not {other:?}"),
        })
        .collect();
    seen.sort_unstable();
    assert_eq!(seen, vec![0, 1, 2, 3, 4]);
    // Page 1's range ends at its last key: page 2's index entries are
    // outside it.
    let doc = &items(&field(&second.result, "page"))[0];
    let (LiveValue::Str(id), LiveValue::I64(ms)) = (field(doc, "_id"), field(doc, "_creationTime"))
    else {
        panic!("an id and a creation time")
    };
    let id: DocId = id.parse().expect("an id");
    let entry = r.app().index_entry(
        loams_live::IndexId::BY_CREATION_TIME,
        &[],
        u64::try_from(ms).expect("a creation time"),
        &id,
    );
    assert!(second.read_set.covers(&entry));
    assert!(!first.read_set.covers(&entry));
    assert!(
        !first.read_set.ranges[0].contains(&second.read_set.ranges[0].lo),
        "page 2 starts after page 1's range"
    );

    let LiveValue::Str(good) = field(&first.result, "continueCursor") else {
        panic!("a cursor string")
    };
    let mut forged = good.into_bytes();
    let last = forged.len() - 1;
    forged[last] = if forged[last] == b'A' { b'B' } else { b'A' };
    let forged = String::from_utf8(forged).expect("ascii");
    match page(s(&forged)).await {
        Err(e) => {
            assert_eq!(e.code(), pb::ErrorCode::ERROR_CODE_INVALID_ARGUMENT, "{e}");
            assert_eq!(e.reason(), Some("live_bad_cursor"), "{e}");
        }
        Ok(q) => panic!("the forged cursor was accepted: {:?}", q.result),
    }
    let caught = query(
        &r,
        &function(&bundle, "pages:caught"),
        obj(&[("cursor", s(&forged))]),
    )
    .await
    .expect("the handler catches it");
    let LiveValue::Str(caught) = caught.result else {
        panic!("a message")
    };
    assert!(caught.contains("cursor"), "{caught}");

    for options in [
        LiveValue::Null,
        obj(&[("cursor", LiveValue::Null)]),
        obj(&[
            ("cursor", LiveValue::Null),
            ("numItems", LiveValue::F64(0.0)),
        ]),
        obj(&[
            ("cursor", LiveValue::I64(5)),
            ("numItems", LiveValue::F64(1.0)),
        ]),
    ] {
        let q = query(
            &r,
            &function(&bundle, "pages:badOptions"),
            obj(&[("options", options.clone())]),
        )
        .await
        .expect("the handler catches it");
        let LiveValue::Str(m) = q.result else {
            panic!("a message")
        };
        assert!(
            m.starts_with("TypeError") && m.contains("paginate"),
            "{options:?}: {m}"
        );
    }
}
live_test!(paginate_in_javascript);
