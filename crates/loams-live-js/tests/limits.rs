//! LV1 plan Task 3 (R1 Task 13 item 3; design §45 §3.2): the CPU interrupt
//! (`FUNCTION_TIMEOUT`), the memory limit (`FUNCTION_OUT_OF_MEMORY`), a
//! fresh context after either, and `console.*` truncation.

mod common;

use std::time::{Duration, Instant};

use common::*;
use loams_live::testing::TestStore;
use loams_live::{LiveError, LiveValue, LogLevel, live_test, pb};
use loams_live_js::{Bundle, JsConfig};

const CPU: Duration = Duration::from_millis(300);

fn config() -> JsConfig {
    JsConfig {
        cpu_limit: CPU,
        memory_limit: 16 * 1024 * 1024,
        contexts: 1,
        ..JsConfig::default()
    }
}

const HOGS: &str = r#"
import { query, mutation } from "loams:server";

export const hogs = {
  spin: query(async () => { for (;;) {} }),
  spinCaught: query(async () => {
    try { for (;;) {} } catch (e) { return "caught"; }
  }),
  spinMutation: mutation(async (ctx) => {
    await ctx.db.insert("hogs", { at: 1n });
    for (;;) {}
  }),
  spinAfterAwait: query(async (ctx) => {
    await ctx.db.query("hogs").collect();
    for (;;) {}
  }),
  backtrack: query(async () => /^(a+)+$/.test("a".repeat(40) + "b")),
  bomb: query(async () => {
    const keep = [];
    for (let i = 0; ; i++) keep.push("x".repeat(1 << 16) + i);
  }),
  bombCaught: query(async () => {
    const keep = [];
    try {
      for (let i = 0; ; i++) keep.push("x".repeat(1 << 16) + i);
    } catch (e) {
      return String(e);
    }
  }),
  bombCaughtForever: query(async () => {
    const keep = [];
    for (;;) {
      try {
        for (let i = 0; ; i++) keep.push("x".repeat(1 << 16) + i);
      } catch (e) {
        keep.length = 0;
      }
    }
  }),
  bombThenWrite: mutation(async (ctx) => {
    const keep = [];
    try {
      for (let i = 0; ; i++) keep.push("x".repeat(1 << 16) + i);
    } catch (e) {
      keep.length = 0;
      await ctx.db.insert("hogs", { after: 1n });
      return "wrote";
    }
  }),
  spoofOom: query(async () => { throw new InternalError("out of memory"); }),
  throwNull: query(async () => { throw null; }),
  recurse: query(async () => { const f = (n) => f(n + 1) + 1; return f(0); }),
  ok: query(async () => "fine"),
  log: query(async (ctx, { lines }) => {
    console.log("short");
    console.warn("é".repeat(10));
    console.error("an", 1, 2n, { a: [1, "b"] }, null, undefined);
    for (let i = 0; i < lines; i++) console.info("line", i);
    return "logged";
  }),
};
"#;

async fn expect_timeout(r: &loams_live::Runner, bundle: &Bundle, path: &str) {
    let f = function(bundle, path);
    let started = Instant::now();
    let result = if f.kind() == loams_live::FnKind::Query {
        query(r, &f, unit()).await.map(|q| q.result)
    } else {
        mutate(r, &f, unit()).await.map(|m| m.result)
    };
    let took = started.elapsed();
    match result {
        Err(e @ LiveError::FunctionTimeout { .. }) => {
            assert_eq!(e.code(), pb::ErrorCode::ERROR_CODE_FUNCTION_TIMEOUT);
            assert!(e.to_string().contains(path), "{e}");
        }
        other => panic!("{path}: a timeout, not {other:?}"),
    }
    assert!(
        took >= CPU,
        "{path}: stopped after {took:?}, before the limit"
    );
    assert!(took < CPU * 10, "{path}: stopped only after {took:?}");
}

async fn busy_loop_times_out(store: TestStore) {
    let r = runner(&store).await;
    let bundle = load_with(HOGS, config()).await;
    for path in [
        "hogs:spin",
        "hogs:spinCaught",
        "hogs:spinAfterAwait",
        "hogs:backtrack",
        "hogs:spinMutation",
    ] {
        expect_timeout(&r, &bundle, path).await;
    }
    // The timed-out mutation committed nothing.
    let all = loams_live::system::lookup(loams_live::system::QUERY).expect("query");
    let docs = query(&r, &all, obj(&[("table", s("hogs"))]))
        .await
        .expect("hogs");
    assert!(items(&docs.result).is_empty());
}
live_test!(busy_loop_times_out);

async fn allocation_bomb_hits_memory_limit(store: TestStore) {
    let r = runner(&store).await;
    let bundle = load_with(HOGS, config()).await;
    match query(&r, &function(&bundle, "hogs:bomb"), unit()).await {
        Err(e @ LiveError::FunctionOutOfMemory { .. }) => {
            assert_eq!(e.code(), pb::ErrorCode::ERROR_CODE_FUNCTION_OUT_OF_MEMORY);
            assert!(e.to_string().contains("hogs:bomb"), "{e}");
        }
        other => panic!("out of memory, not {other:?}"),
    }
    // Catching QuickJS's out-of-memory error does not help (LV1 row T3-6):
    // the limit stops the call uncatchably, and nothing it does after the
    // limit is kept.
    for path in ["hogs:bombCaught", "hogs:bombCaughtForever"] {
        let started = Instant::now();
        match query(&r, &function(&bundle, path), unit()).await {
            Err(LiveError::FunctionOutOfMemory { function, .. }) => assert_eq!(function, path),
            other => panic!("{path}: out of memory, not {other:?}"),
        }
        assert!(started.elapsed() < CPU, "{path}: stopped at the limit");
    }
    match mutate(&r, &function(&bundle, "hogs:bombThenWrite"), unit()).await {
        Err(LiveError::FunctionOutOfMemory { .. }) => {}
        other => panic!("out of memory, not {other:?}"),
    }
    let all = loams_live::system::lookup(loams_live::system::QUERY).expect("query");
    let hogs = query(&r, &all, obj(&[("table", s("hogs"))]))
        .await
        .expect("hogs");
    assert!(
        items(&hogs.result).is_empty(),
        "nothing written after the limit"
    );
    // The code is the runtime's, never the message's: a thrown error that
    // looks like QuickJS's, or a thrown null, is a function error.
    for path in ["hogs:spoofOom", "hogs:throwNull"] {
        match query(&r, &function(&bundle, path), unit()).await {
            Err(LiveError::FunctionError(_)) => {}
            other => panic!("{path}: a function error, not {other:?}"),
        }
    }
    // Deep recursion is a stack overflow, a plain function error.
    match query(&r, &function(&bundle, "hogs:recurse"), unit()).await {
        Err(LiveError::FunctionError(m)) => assert!(m.contains("stack"), "{m}"),
        other => panic!("a stack overflow, not {other:?}"),
    }
}
live_test!(allocation_bomb_hits_memory_limit);

async fn context_recovers_after_timeout(store: TestStore) {
    let r = runner(&store).await;
    // One context: every call runs on the same runtime.
    let bundle = load_with(HOGS, config()).await;
    let ok = function(&bundle, "hogs:ok");
    for hog in ["hogs:spin", "hogs:bomb", "hogs:recurse", "hogs:spin"] {
        let failed = query(&r, &function(&bundle, hog), unit()).await;
        assert!(failed.is_err(), "{hog} fails");
        let started = Instant::now();
        let fine = query(&r, &ok, unit()).await.expect("the next call runs");
        assert_eq!(fine.result, s("fine"), "after {hog}");
        assert!(
            started.elapsed() < CPU,
            "after {hog}, the next call is not slowed"
        );
    }
    // A dropped caller (here: a timed-out await) also leaves a usable pool.
    let spin = function(&bundle, "hogs:spin");
    let at = r.store().now().await.expect("now");
    let cut = tokio::time::timeout(Duration::from_millis(20), r.query(&*spin, unit(), at)).await;
    assert!(cut.is_err(), "the caller gave up");
    let fine = query(&r, &ok, unit()).await.expect("the next call runs");
    assert_eq!(fine.result, s("fine"));
}
live_test!(context_recovers_after_timeout);

async fn console_output_truncated_at_limits(store: TestStore) {
    let r = runner(&store).await;
    let bundle = load_with(
        HOGS,
        JsConfig {
            console_lines: 5,
            console_line_bytes: 8,
            ..config()
        },
    )
    .await;
    let log = function(&bundle, "hogs:log");
    let q = query(&r, &log, obj(&[("lines", LiveValue::F64(10.0))]))
        .await
        .expect("log runs");
    assert_eq!(q.result, s("logged"));
    let logs = &q.output.logs;
    assert_eq!(logs.len(), 5);
    assert_eq!(q.output.dropped, 8);
    assert_eq!(
        (logs[0].level, logs[0].line.as_str(), logs[0].truncated),
        (LogLevel::Log, "short", false)
    );
    // Ten "é" are 20 bytes: cut to 8 bytes, four characters.
    assert_eq!(
        (logs[1].level, logs[1].line.as_str(), logs[1].truncated),
        (LogLevel::Warn, "éééé", true)
    );
    assert_eq!(logs[2].level, LogLevel::Error);
    assert_eq!(logs[2].line, "an 1 2n ");
    assert_eq!(logs[3].level, LogLevel::Info);
    assert_eq!(logs[3].line, "line 0");

    // The defaults: 64 lines of 4 KiB.
    let defaults = load(HOGS).await;
    let q = query(
        &r,
        &function(&defaults, "hogs:log"),
        obj(&[("lines", LiveValue::F64(100.0))]),
    )
    .await
    .expect("log runs");
    assert_eq!(q.output.logs.len(), 64);
    assert_eq!(q.output.dropped, 103 - 64);
    assert_eq!(
        q.output.logs[2].line,
        r#"an 1 2n { a: [ 1, "b" ] } null undefined"#
    );
}
live_test!(console_output_truncated_at_limits);

#[tokio::test]
async fn bundle_top_level_is_limited_too() {
    let spin = "for (;;) {}";
    match Bundle::load(spin, config()).await {
        Err(LiveError::FunctionTimeout { .. }) => {}
        Err(e) => panic!("a timeout, not {e}"),
        Ok(_) => panic!("a spinning bundle loads"),
    }
    let bomb = "const keep = []; for (let i = 0; ; i++) keep.push('x'.repeat(1 << 16) + i);";
    match Bundle::load(bomb, config()).await {
        Err(LiveError::FunctionOutOfMemory { .. }) => {}
        Err(e) => panic!("out of memory, not {e}"),
        Ok(_) => panic!("a bomb loads"),
    }
}

const SPARSE: &str = r#"
import { query } from "loams:server";

function huge() {
  const a = [];
  a.length = 2 ** 32 - 1;
  return a;
}

function twoFaced() {
  let reads = 0;
  return { get length() { return reads++ === 0 ? 1 : 2 ** 32 - 1; } };
}

export const sparse = {
  join: query(async () => huge().join()),
  toString: query(async () => String(huge())),
  toLocaleString: query(async () => huge().toLocaleString()),
  reverse: query(async () => huge().reverse()),
  slice: query(async () => huge().slice(1)),
  splice: query(async () => huge().splice(1, 1)),
  shift: query(async () => huge().shift()),
  unshift: query(async () => huge().unshift(1)),
  concat: query(async () => [].concat(huge())),
  copyWithin: query(async () => huge().copyWithin(0, 1)),
  sort: query(async () => huge().sort()),
  flat: query(async () => huge().flat()),
  flatNested: query(async () => [[1], huge()].flat()),
  flatMap: query(async () => [1].flatMap(() => huge())),
  generic: query(async () => Array.prototype.join.call({ length: 2 ** 53 - 1 })),
  fill: query(async () => huge().fill(0)),
  with: query(async () => huge().with(0, 1)),
  toReversed: query(async () => huge().toReversed()),
  toSorted: query(async () => huge().toSorted()),
  toSpliced: query(async () => huge().toSpliced(0, 1)),
  // A length that reads small to the guard and huge to the built-in.
  getterReverse: query(async () => Array.prototype.reverse.call(twoFaced())),
  getterJoin: query(async () => Array.prototype.join.call(twoFaced())),
  proxyReverse: query(async () => {
    let reads = 0;
    const p = new Proxy([], {
      get: (t, k) => (k === "length" ? (reads++ === 0 ? 1 : 2 ** 32 - 1) : undefined),
    });
    return Array.prototype.reverse.call(p);
  }),
  valueOfSort: query(async () => {
    let reads = 0;
    return Array.prototype.sort.call({
      length: { valueOf: () => (reads++ === 0 ? 1 : 2 ** 32 - 1) },
    });
  }),
  inheritedGetter: query(async () => {
    let reads = 0;
    const proto = { get length() { return reads++ === 0 ? 1 : 2 ** 32 - 1; } };
    return Array.prototype.slice.call(Object.create(proto));
  }),
  // concat spreads any object with Symbol.isConcatSpreadable.
  spreadableConcat: query(async () =>
    [].concat({ [Symbol.isConcatSpreadable]: true, length: 2 ** 32 - 1 })),
  spreadableReceiver: query(async () =>
    Array.prototype.concat.call({ [Symbol.isConcatSpreadable]: true, length: 2 ** 32 - 1 })),
  spreadableGetter: query(async () => {
    let reads = 0;
    const o = { length: 2 ** 32 - 1 };
    Object.defineProperty(o, Symbol.isConcatSpreadable, { get: () => reads++ > 0 });
    return [].concat(o).length;
  }),
  // An element's getter lengthens a later argument after the guard.
  concatLengthenedLater: query(async () => {
    const later = [1];
    const first = [];
    Object.defineProperty(first, 0, {
      get() { later.length = 2 ** 32 - 1; return 0; },
      enumerable: true,
    });
    return [].concat(first, later).length;
  }),
  flatLengthenedLater: query(async () => {
    const later = [1];
    const first = [];
    Object.defineProperty(first, 0, {
      get() { later.length = 2 ** 32 - 1; return 0; },
      enumerable: true,
    });
    return [first, later].flat().length;
  }),
  dense: query(async () => [
    Array.from({ length: 1000 }, (_, i) => i).join(",").length,
    [3, 1, 2].sort().join(""),
    [[1], [2, [3]]].flat(Infinity).join(""),
    [1, 2].flatMap((x) => [x, x]).join(""),
    [1].concat([2], 3).join(""),
    String([1, [2, 3]]),
    Array.prototype.join.length,
    Array.prototype.flatMap.length,
    String([1, , 3].concat([, 5], 6)),
    [1, , 3].concat([]).hasOwnProperty(1),
    [0, [1, [2, [3]]]].flat(2).length,
    Array.prototype.concat.call("ab", [1]).length,
    [new Uint8Array(3)].flat().length,
  ]),
};
"#;

/// QuickJS runs these Array methods in C without interrupt checks: on a
/// huge sparse array they would run for minutes past the CPU limit. The
/// prelude refuses array-likes longer than any dense array could be.
async fn uninterruptible_array_methods_refuse_huge_arrays(store: TestStore) {
    let r = runner(&store).await;
    for path in [
        "sparse:join",
        "sparse:toString",
        "sparse:toLocaleString",
        "sparse:reverse",
        "sparse:slice",
        "sparse:splice",
        "sparse:shift",
        "sparse:unshift",
        "sparse:concat",
        "sparse:copyWithin",
        "sparse:sort",
        "sparse:flat",
        "sparse:flatNested",
        "sparse:flatMap",
        "sparse:generic",
        "sparse:fill",
        "sparse:with",
        "sparse:toReversed",
        "sparse:toSorted",
        "sparse:toSpliced",
        "sparse:getterReverse",
        "sparse:getterJoin",
        "sparse:proxyReverse",
        "sparse:valueOfSort",
        "sparse:inheritedGetter",
        "sparse:spreadableConcat",
        "sparse:spreadableReceiver",
        "sparse:spreadableGetter",
        "sparse:concatLengthenedLater",
        "sparse:flatLengthenedLater",
    ] {
        // A bundle per case: a regression wedges its slot, not the others.
        let bundle = load_with(SPARSE, config()).await;
        let started = Instant::now();
        let ran = tokio::time::timeout(
            Duration::from_secs(5),
            query(&r, &function(&bundle, path), unit()),
        )
        .await;
        match ran {
            Ok(Err(LiveError::FunctionError(m))) => {
                assert!(
                    (m.starts_with("RangeError") && m.contains("sparse"))
                        || (m.starts_with("TypeError") && m.contains("not supported")),
                    "{path}: {m}"
                );
            }
            Ok(other) => panic!("{path}: a RangeError, not {other:?}"),
            Err(_) => panic!("{path}: still running after 5 s"),
        }
        assert!(started.elapsed() < CPU, "{path}: refused at once");
    }
    let bundle = load_with(SPARSE, config()).await;
    let dense = query(&r, &function(&bundle, "sparse:dense"), unit())
        .await
        .expect("dense arrays work")
        .result;
    assert_eq!(
        dense,
        LiveValue::Array(vec![
            LiveValue::F64(3889.0),
            s("123"),
            s("123"),
            s("1122"),
            s("123"),
            s("1,2,3"),
            LiveValue::F64(1.0),
            LiveValue::F64(1.0),
            s("1,,3,,5,6"),
            LiveValue::Bool(false),
            LiveValue::F64(4.0),
            LiveValue::F64(2.0),
            LiveValue::F64(1.0),
        ])
    );
}
live_test!(uninterruptible_array_methods_refuse_huge_arrays);

const VALUES: &str = r#"
import { query, mutation } from "loams:server";

function sparse(n) { const a = []; a.length = n; return a; }
function shared() {
  const s = "x".repeat(1 << 20);
  return Array.from({ length: 256 }, () => s);
}
function sharedObject() {
  const s = "x".repeat(1 << 20);
  const o = {};
  for (let i = 0; i < 256; i++) o["k" + i] = s;
  return o;
}
function sharedBuffer() {
  const b = new ArrayBuffer(1 << 20);
  return Array.from({ length: 256 }, () => b);
}
function dag(levels) {
  let o = [];
  for (let i = 0; i < levels; i++) o = [o, o];
  return o;
}
const VALUES = {
  sparse30: () => sparse(2 ** 30),
  sparse32: () => sparse(2 ** 32 - 1),
  shared,
  sharedObject,
  sharedBuffer,
  dag: () => dag(40),
};

async function insert(ctx, { value }) {
  try {
    await ctx.db.insert("big", { value: VALUES[value]() });
    return "inserted";
  } catch (e) {
    return String(e);
  }
}

export const values = {
  returned: query(async (ctx, { value }) => VALUES[value]()),
  inserted: mutation(insert),
  insertedByQuery: query(async (ctx, { value }) => {
    try {
      await ctx.db.get(VALUES[value]());
      return "read";
    } catch (e) {
      return String(e);
    }
  }),
  large: query(async () => "x".repeat(4 << 20)),
  ok: query(async () => "fine"),
};
"#;

/// C2: converting a JavaScript value to a Loams value has a budget
/// (`Limits::max_result_bytes` for a result, four times
/// `Limits::max_document_bytes` for a `ctx.db` call's arguments), charged
/// before anything is allocated and for every shared reference, so a huge
/// sparse array, a string shared 256 times or a DAG of shared arrays is a
/// typed error, not an abort or an allocation of gigabytes.
async fn value_conversion_is_budgeted(store: TestStore) {
    let test = format!("value_conversion_is_budgeted::{}", variant(&store));
    if !is_child(&test) {
        run_child(&test, &[]);
        return;
    }
    let r = runner(&store).await;
    // CPU to spare: the budget, not the CPU limit, stops these.
    let bundle = load_with(
        VALUES,
        JsConfig {
            cpu_limit: Duration::from_secs(10),
            ..config()
        },
    )
    .await;
    let returned = function(&bundle, "values:returned");
    let inserted = function(&bundle, "values:inserted");
    let read = function(&bundle, "values:insertedByQuery");
    let ok = function(&bundle, "values:ok");
    for value in [
        "sparse30",
        "sparse32",
        "shared",
        "sharedObject",
        "sharedBuffer",
        "dag",
    ] {
        let args = obj(&[("value", s(value))]);
        let started = Instant::now();
        match query(&r, &returned, args.clone()).await {
            Err(LiveError::LimitExceeded { limit, .. }) => {
                assert_eq!(limit, "max_result_bytes", "{value}");
            }
            other => panic!("{value}: max_result_bytes, not {other:?}"),
        }
        // Bounded by the budget (8 MiB of conversion), not by the value's
        // apparent size (gigabytes, or 2^40 nodes for the DAG).
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{value}: refused within the budget's work: {:?}",
            started.elapsed()
        );
        for (what, f) in [("insert", &inserted), ("get", &read)] {
            let got = if what == "insert" {
                mutate(&r, f, args.clone()).await.map(|m| m.result)
            } else {
                query(&r, f, args.clone()).await.map(|q| q.result)
            };
            match got {
                Ok(LiveValue::Str(m)) => assert!(
                    m.contains("max_document_bytes") && m.contains("ctx.db"),
                    "{what} {value}: {m}"
                ),
                other => panic!("{what} {value}: a catchable limit error, not {other:?}"),
            }
        }
        // The slot is fine afterwards.
        assert_eq!(query(&r, &ok, unit()).await.expect("ok").result, s("fine"));
    }
    let all = loams_live::system::lookup(loams_live::system::QUERY).expect("query");
    let big = query(&r, &all, obj(&[("table", s("big"))]))
        .await
        .expect("big");
    assert!(items(&big.result).is_empty());
    // A large result within the budget converts.
    let large = query(&r, &function(&bundle, "values:large"), unit())
        .await
        .expect("4 MiB is within max_result_bytes");
    assert!(matches!(large.result, LiveValue::Str(ref s) if s.len() == 4 << 20));
}
live_test!(value_conversion_is_budgeted);
