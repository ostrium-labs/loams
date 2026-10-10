//! LV1 plan Task 3 (R1 Task 13 items 2 and 2a; design §20 §6.2): one
//! context per invocation, frozen globals, `Date.now` at the start
//! timestamp, a seeded `Math.random`, `crypto.*` refused, and no timers,
//! `fetch` or `WebAssembly`.

mod common;

use std::collections::BTreeSet;

use common::*;
use loams_live::testing::TestStore;
use loams_live::{LiveError, LiveValue, live_test};

const STATE: &str = r#"
import { query, mutation } from "loams:server";

let counter = 0;
const seen = [];

export const state = {
  bump: query(async () => {
    counter += 1;
    seen.push(counter);
    return [counter, seen.length];
  }),
  bumpMutation: mutation(async () => ++counter),
  pollute: query(async () => {
    globalThis.leaked = "yes";
    Object.defineProperty(globalThis, "alsoLeaked", { value: 1, configurable: true });
    return "polluted";
  }),
  look: query(async () => [typeof globalThis.leaked, typeof globalThis.alsoLeaked]),
};
"#;

async fn module_state_does_not_leak_between_calls(store: TestStore) {
    let r = runner(&store).await;
    let bundle = load(STATE).await;
    let bump = function(&bundle, "state:bump");
    for _ in 0..6 {
        let q = query(&r, &bump, unit()).await.expect("bump");
        assert_eq!(
            q.result,
            LiveValue::Array(vec![LiveValue::F64(1.0), LiveValue::F64(1.0)])
        );
    }
    let bump = function(&bundle, "state:bumpMutation");
    for _ in 0..3 {
        let m = mutate(&r, &bump, unit()).await.expect("bump");
        assert_eq!(m.result, LiveValue::F64(1.0));
    }
    let pollute = function(&bundle, "state:pollute");
    let look = function(&bundle, "state:look");
    for _ in 0..5 {
        query(&r, &pollute, unit()).await.expect("pollute");
    }
    for _ in 0..5 {
        assert_eq!(
            query(&r, &look, unit()).await.expect("look").result,
            LiveValue::Array(vec![s("undefined"), s("undefined")])
        );
    }
}
live_test!(module_state_does_not_leak_between_calls);

const CRYPTO: &str = r#"
import { query, mutation } from "loams:server";

function attempt(f) {
  try {
    f();
    return "allowed";
  } catch (e) {
    return `${e.name}: ${e.message}`;
  }
}

function attempts() {
  return [
    attempt(() => crypto.getRandomValues(new Uint8Array(4))),
    attempt(() => crypto.randomUUID()),
    attempt(() => crypto.subtle),
    attempt(() => globalThis.crypto.anything),
  ];
}

export const crypto_ = {
  inQuery: query(async () => attempts()),
  inMutation: mutation(async () => attempts()),
  uncaught: query(async () => crypto.randomUUID()),
};
"#;

async fn crypto_random_throws_in_queries_and_mutations(store: TestStore) {
    let r = runner(&store).await;
    let bundle = load(CRYPTO).await;
    let expected = "DeterminismError: crypto randomness is not available in queries and mutations; use an action";
    let q = query(&r, &function(&bundle, "crypto_:inQuery"), unit())
        .await
        .expect("query");
    let m = mutate(&r, &function(&bundle, "crypto_:inMutation"), unit())
        .await
        .expect("mutation");
    for result in [q.result, m.result] {
        let got = items(&result);
        assert_eq!(got.len(), 4);
        for g in got {
            assert_eq!(g, s(expected));
        }
    }
    match query(&r, &function(&bundle, "crypto_:uncaught"), unit()).await {
        Err(LiveError::FunctionError(m)) => assert!(m.contains("DeterminismError"), "{m}"),
        other => panic!("a function error, not {other:?}"),
    }
}
live_test!(crypto_random_throws_in_queries_and_mutations);

const CLOCK: &str = r#"
import { query, mutation } from "loams:server";

const atLoad = Date.now();

function clocks() {
  return {
    now: Date.now(),
    newDate: new Date().getTime(),
    dateString: Date() === new Date(Date.now()).toString(),
    explicit: new Date(1000).getTime(),
    parsed: Date.parse("1970-01-01T00:00:01Z"),
    utc: Date.UTC(1970, 0, 1, 0, 0, 2),
    isDate: new Date() instanceof Date,
    ctor: new Date().constructor === Date,
    atLoad,
  };
}

export const clock = {
  read: query(async () => clocks()),
  write: mutation(async (ctx) => {
    const id = await ctx.db.insert("clock", { x: 1n });
    const doc = await ctx.db.get(id);
    return { ...clocks(), created: doc._creationTime };
  }),
  sneaky: query(async () => {
    const OriginalDate = Object.getPrototypeOf(new Date()).constructor;
    return new OriginalDate().getTime();
  }),
};
"#;

async fn date_now_is_start_ts(store: TestStore) {
    let r = runner(&store).await;
    let bundle = load(CLOCK).await;
    let at = r.store().now().await.expect("now");
    let ms = at.physical_ms() as f64;
    let got = r
        .query(&*function(&bundle, "clock:read"), unit(), at)
        .await
        .expect("read")
        .result;
    assert_eq!(field(&got, "now"), LiveValue::F64(ms));
    assert_eq!(field(&got, "newDate"), LiveValue::F64(ms));
    assert_eq!(field(&got, "dateString"), LiveValue::Bool(true));
    assert_eq!(field(&got, "explicit"), LiveValue::F64(1000.0));
    assert_eq!(field(&got, "parsed"), LiveValue::F64(1000.0));
    assert_eq!(field(&got, "utc"), LiveValue::F64(2000.0));
    assert_eq!(field(&got, "isDate"), LiveValue::Bool(true));
    assert_eq!(field(&got, "ctor"), LiveValue::Bool(true));
    // Module evaluation runs before any call: its clock is the epoch.
    assert_eq!(field(&got, "atLoad"), LiveValue::F64(0.0));
    let sneaky = r
        .query(&*function(&bundle, "clock:sneaky"), unit(), at)
        .await
        .expect("sneaky")
        .result;
    assert_eq!(
        sneaky,
        LiveValue::F64(ms),
        "the prototype's constructor is the shim"
    );

    // A mutation's clock is its attempt's start timestamp, which is also
    // the creation time of what it inserts.
    let m = mutate(&r, &function(&bundle, "clock:write"), unit())
        .await
        .expect("write");
    let LiveValue::I64(created) = field(&m.result, "created") else {
        panic!("a creation time")
    };
    assert_eq!(field(&m.result, "now"), LiveValue::F64(created as f64));
    assert!(created as u64 <= m.commit_ts.physical_ms());
}
live_test!(date_now_is_start_ts);

const RANDOM: &str = r#"
import { query, mutation } from "loams:server";

const atLoad = Math.random();

export const random = {
  draw: query(async () => [atLoad, ...Array.from({ length: 8 }, () => Math.random())]),
  drawMutation: mutation(async () => Array.from({ length: 8 }, () => Math.random())),
};
"#;

fn draws(v: &LiveValue) -> Vec<f64> {
    items(v)
        .into_iter()
        .map(|x| match x {
            LiveValue::F64(f) => f,
            other => panic!("a number, not {other:?}"),
        })
        .collect()
}

async fn random_is_repeatable_for_same_ts_and_request(store: TestStore) {
    let r = runner(&store).await;
    let bundle = load(RANDOM).await;
    let draw = function(&bundle, "random:draw");
    let at = r.store().now().await.expect("now");
    let a1 = draws(&query_as(&r, &draw, unit(), at, "req-a").await.expect("a1"));
    let a2 = draws(&query_as(&r, &draw, unit(), at, "req-a").await.expect("a2"));
    let b = draws(&query_as(&r, &draw, unit(), at, "req-b").await.expect("b"));
    assert_eq!(a1, a2, "same timestamp and request: same draws");
    assert_ne!(a1[1..], b[1..], "another request: other draws");
    assert_eq!(
        a1[0], b[0],
        "module evaluation draws the same in every context"
    );
    let later = r.store().now().await.expect("now");
    assert!(later > at);
    let c = draws(
        &query_as(&r, &draw, unit(), later, "req-a")
            .await
            .expect("c"),
    );
    assert_ne!(a1[1..], c[1..], "another timestamp: other draws");
    // Through the runner, a query has no request id: the timestamp alone
    // decides.
    let q1 = draws(&r.query(&*draw, unit(), at).await.expect("q1").result);
    let q2 = draws(&r.query(&*draw, unit(), at).await.expect("q2").result);
    assert_eq!(q1, q2);
    let all: BTreeSet<u64> = a1[1..].iter().chain(&b[1..]).map(|f| f.to_bits()).collect();
    assert_eq!(all.len(), 16, "no repeats within or across the streams");
    for f in a1.iter().chain(&b).chain(&c) {
        assert!((0.0..1.0).contains(f), "{f} is in [0, 1)");
    }
    // A mutation is seeded by its idempotency key and start timestamp.
    let m = function(&bundle, "random:drawMutation");
    let m1 = r
        .mutate(m.clone(), unit(), Some("key-1".into()))
        .await
        .expect("m1");
    let m2 = r
        .mutate(m.clone(), unit(), Some("key-2".into()))
        .await
        .expect("m2");
    assert_ne!(draws(&m1.result), draws(&m2.result));
}
live_test!(random_is_repeatable_for_same_ts_and_request);

const GLOBALS: &str = r#"
import { query } from "loams:server";

function tryIt(f) {
  try {
    f();
    return "ok";
  } catch (e) {
    return e.name;
  }
}

class Problem extends Error {
  constructor(message) {
    super(message);
    this.name = "Problem";
  }
}

export const globals = {
  absent: query(async () => [
    "setTimeout", "setInterval", "setImmediate", "clearTimeout", "clearInterval",
    "fetch", "WebAssembly", "performance", "WeakRef", "FinalizationRegistry",
    "Atomics", "SharedArrayBuffer", "XMLHttpRequest", "WebSocket", "require",
    "process", "Deno", "Bun", "std", "os", "gc", "print", "navigator",
  ].filter((name) => typeof globalThis[name] !== "undefined")),
  names: query(async () => Object.getOwnPropertyNames(globalThis).sort()),
  frozen: query(async () => ({
    mathRandom: tryIt(() => { Math.random = () => 0.5; }),
    dateNow: tryIt(() => { Date.now = () => 0; }),
    arrayProto: tryIt(() => { Array.prototype.extra = 1; }),
    objectProto: tryIt(() => { Object.prototype.polluted = 1; }),
    replaceDate: tryIt(() => { globalThis.Date = null; }),
    deleteMath: tryIt(() => { delete globalThis.Math; }),
    promiseThen: tryIt(() => { Promise.prototype.then = null; }),
    console: tryIt(() => { console.log = null; }),
    asyncProto: tryIt(() => { Object.getPrototypeOf(async function () {}).extra = 1; }),
    iterProto: tryIt(() => { Object.getPrototypeOf(Object.getPrototypeOf([][Symbol.iterator]())).extra = 1; }),
  })),
  overrides: query(async () => {
    const e = new Problem("bad");
    const o = { toString() { return "mine"; } };
    const p = {};
    p.toString = () => "assigned";
    function F() {}
    F.prototype.toString = () => "proto";
    return [e.name, String(e), `${o}`, `${p}`, `${new F()}`, e instanceof Error];
  }),
};
"#;

async fn no_fetch_no_timers(store: TestStore) {
    let r = runner(&store).await;
    let bundle = load(GLOBALS).await;
    let absent = query(&r, &function(&bundle, "globals:absent"), unit())
        .await
        .expect("absent");
    assert_eq!(
        absent.result,
        LiveValue::Array(vec![]),
        "every listed global is absent"
    );
    let names = query(&r, &function(&bundle, "globals:names"), unit())
        .await
        .expect("names");
    let names: BTreeSet<String> = items(&names.result)
        .into_iter()
        .map(|n| match n {
            LiveValue::Str(s) => s,
            other => panic!("a name, not {other:?}"),
        })
        .collect();
    let allowed: BTreeSet<String> = loams_live_js::GLOBALS
        .iter()
        .map(|s| s.to_string())
        .collect();
    let extra: Vec<_> = names.difference(&allowed).collect();
    assert!(extra.is_empty(), "globals outside the allowlist: {extra:?}");
    for needed in [
        "Date",
        "Math",
        "JSON",
        "Promise",
        "console",
        "crypto",
        "ArrayBuffer",
    ] {
        assert!(names.contains(needed), "{needed} is a global");
    }
}
live_test!(no_fetch_no_timers);

async fn globals_are_frozen_but_overridable_by_own_properties(store: TestStore) {
    let r = runner(&store).await;
    let bundle = load(GLOBALS).await;
    let frozen = query(&r, &function(&bundle, "globals:frozen"), unit())
        .await
        .expect("frozen")
        .result;
    let LiveValue::Object(frozen) = frozen else {
        panic!("an object")
    };
    for (what, outcome) in &frozen {
        assert_eq!(outcome, &s("TypeError"), "{what} is refused");
    }
    let overrides = query(&r, &function(&bundle, "globals:overrides"), unit())
        .await
        .expect("overrides")
        .result;
    assert_eq!(
        overrides,
        LiveValue::Array(vec![
            s("Problem"),
            s("Problem: bad"),
            s("mine"),
            s("assigned"),
            s("proto"),
            LiveValue::Bool(true),
        ])
    );
}
live_test!(globals_are_frozen_but_overridable_by_own_properties);

const FLOATING: &str = r#"
import { query, mutation } from "loams:server";

export const floating = {
  // Returns while an async loop it started is still queued: from the
  // eleventh turn on, the loop logs and writes on every turn.
  leave: mutation(async (ctx) => {
    (async () => {
      for (let turn = 0; ; turn++) {
        await null;
        if (turn > 10) {
          console.log("leaked");
          await ctx.db.insert("leaks", { turn: BigInt(turn) });
        }
      }
    })();
    return "left";
  }),
  // The same with no host calls: only CPU, 20 ms a turn.
  leaveSpinning: query(async () => {
    (async () => {
      for (let turn = 0; ; turn++) {
        await null;
        if (turn > 10) {
          for (let i = 0; i < 2e5; i++) {}
        }
      }
    })();
    return "left";
  }),
  ok: query(async () => "fine"),
  okMutation: mutation(async (ctx) => {
    await ctx.db.insert("marks", { x: 1n });
    console.log("mine");
    return "fine";
  }),
};
"#;

/// LV1 Task 3 fix round 1, C1: a call that returns while promise jobs it
/// queued are pending must not leave them for the next call, which would
/// run them with its own host link (its transaction, its console).
async fn leftover_jobs_never_run_in_the_next_call(store: TestStore) {
    let r = runner(&store).await;
    // One slot: every call runs on the same runtime.
    let bundle = load_with(
        FLOATING,
        loams_live_js::JsConfig {
            contexts: 1,
            ..loams_live_js::JsConfig::default()
        },
    )
    .await;
    let leave = function(&bundle, "floating:leave");
    let ok = function(&bundle, "floating:okMutation");
    for _ in 0..3 {
        let m = mutate(&r, &leave, unit()).await.expect("leave");
        assert_eq!(m.result, s("left"));
        let started = std::time::Instant::now();
        let m = mutate(&r, &ok, unit()).await.expect("the next call runs");
        assert_eq!(m.result, s("fine"));
        let lines: Vec<_> = m.output.logs.iter().map(|l| l.line.as_str()).collect();
        assert_eq!(lines, ["mine"], "no line of the earlier call's loop");
        assert!(
            started.elapsed() < std::time::Duration::from_millis(500),
            "the next call is not slowed"
        );
    }
    let all = loams_live::system::lookup(loams_live::system::QUERY).expect("query");
    let leaks = query(&r, &all, obj(&[("table", s("leaks"))]))
        .await
        .expect("leaks");
    assert!(
        items(&leaks.result).is_empty(),
        "the loop never wrote: {:?}",
        leaks.result
    );
    let marks = query(&r, &all, obj(&[("table", s("marks"))]))
        .await
        .expect("marks");
    assert_eq!(items(&marks.result).len(), 3);

    // CPU only: the next call is not slowed by a leftover loop.
    let spin = function(&bundle, "floating:leaveSpinning");
    let ok = function(&bundle, "floating:ok");
    for _ in 0..3 {
        assert_eq!(
            query(&r, &spin, unit()).await.expect("spin").result,
            s("left")
        );
        let started = std::time::Instant::now();
        assert_eq!(query(&r, &ok, unit()).await.expect("ok").result, s("fine"));
        assert!(
            started.elapsed() < std::time::Duration::from_millis(200),
            "the next call is not slowed: {:?}",
            started.elapsed()
        );
    }
}
live_test!(leftover_jobs_never_run_in_the_next_call);

/// C1 at load: the jobs a bundle's top level leaves are run within the
/// load's limits, so none is left for the first call; a top level that
/// never stops queueing jobs fails to load.
#[tokio::test]
async fn bundle_top_level_jobs_run_within_its_limits() {
    let finite = r#"
import { query } from "loams:server";
let settled = 0;
Promise.resolve().then(() => { settled += 1; }).then(() => { settled += 1; });
export const top = { settled: query(async () => settled) };
"#;
    let bundle = load(finite).await;
    let store = TestStore::embedded(option_env!("CARGO_TARGET_TMPDIR")).await;
    let r = runner(&store).await;
    let got = query(&r, &function(&bundle, "top:settled"), unit())
        .await
        .expect("settled");
    assert_eq!(got.result, LiveValue::F64(2.0));

    let forever = r#"
(async () => { for (;;) { await null; } })();
export const top = {};
"#;
    let config = loams_live_js::JsConfig {
        cpu_limit: std::time::Duration::from_millis(200),
        contexts: 1,
        ..loams_live_js::JsConfig::default()
    };
    match try_load(forever, config).await {
        Err(LiveError::FunctionTimeout { function, .. }) => assert_eq!(function, "<bundle>"),
        Err(e) => panic!("a timeout, not {e}"),
        Ok(_) => panic!("a bundle that never stops queueing jobs loads"),
    }
}

const ZONES: &str = r#"
import { query } from "loams:server";

const UTC = Date.UTC;

export const zones = {
  read: query(async () => {
    const d = new Date(UTC(2020, 6, 15, 23, 30, 45, 123));
    const set = new Date(0);
    set.setHours(5);
    set.setMinutes(6);
    set.setFullYear(2001, 1, 3);
    set.setMonth(4);
    set.setDate(9);
    set.setSeconds(7);
    set.setMilliseconds(8);
    const year = new Date(0);
    year.setYear(99);
    return {
      getters: [
        d.getFullYear(), d.getMonth(), d.getDate(), d.getDay(), d.getHours(),
        d.getMinutes(), d.getSeconds(), d.getMilliseconds(), d.getYear(),
      ],
      offset: d.getTimezoneOffset(),
      strings: [
        d.toString(), d.toDateString(), d.toTimeString(), d.toLocaleString(),
        d.toLocaleDateString(), d.toLocaleTimeString(), `${d}`,
        String(new Date(NaN)), String(new Date(NaN).getTimezoneOffset()),
        new Date(UTC(-50, 0, 1)).toString(),
      ],
      called: Date().endsWith("GMT+0000") && Date() === new Date().toString(),
      setters: [set.toISOString(), year.toISOString()],
      utc: [
        new Date(2020, 6, 15, 23, 30).getTime() === UTC(2020, 6, 15, 23, 30),
        new Date(99, 0).getTime() === UTC(1999, 0),
        Date.parse("2020-07-15T23:30:00") === UTC(2020, 6, 15, 23, 30),
        Date.parse("2020-07-15T23:30") === UTC(2020, 6, 15, 23, 30),
        Date.parse("Jul 15 2020 23:30:00") === UTC(2020, 6, 15, 23, 30),
        Date.parse("Jul 15 2020") === UTC(2020, 6, 15),
        Date.parse("7/15/2020, 11:30:00 PM") === UTC(2020, 6, 15, 23, 30),
        Date.parse("2020-07-15") === UTC(2020, 6, 15),
        Date.parse("2020-07-15T23:30:00+02:00") === UTC(2020, 6, 15, 21, 30),
        Date.parse("2020-07-15T23:30:00Z") === UTC(2020, 6, 15, 23, 30),
        Date.parse("Jul 15 2020 23:30:00 EST") === UTC(2020, 6, 16, 4, 30),
        Date.parse("Wed Jul 15 2020 23:30:45 GMT+0100") === UTC(2020, 6, 15, 22, 30, 45),
        Date.parse(d.toString()) === UTC(2020, 6, 15, 23, 30, 45),
        Date.parse(d.toUTCString()) === UTC(2020, 6, 15, 23, 30, 45),
        Date.parse(d.toISOString()) === d.getTime(),
        Date.parse("0050-01-01T00:00:00") === new Date("0050-01-01T00:00:00Z").getTime(),
        new Date("2020-07-15T23:30:00").getTime() === UTC(2020, 6, 15, 23, 30),
        new Date(new Date(2020, 0, 1).toString()).getTime() === UTC(2020, 0, 1),
        new Date({ valueOf: () => "2020-07-15T23:30" }).getTime() === UTC(2020, 6, 15, 23, 30),
        Number.isNaN(Date.parse("not a date")),
        Number.isNaN(new Date(2020, NaN).getTime()),
      ],
    };
  }),
};
"#;

/// I1: the local-time methods of `Date` are UTC whatever the host's time
/// zone, so every node computes the same values. The body runs in child
/// processes with `TZ` set, since the time zone is read once per process.
#[tokio::test]
async fn local_time_methods_are_utc() {
    let test = case_path("local_time_methods_are_utc", None);
    if !is_child(&test) {
        for tz in ["America/New_York", "Asia/Kolkata", "Pacific/Chatham"] {
            run_child(&test, &[("TZ", tz)]);
        }
        return;
    }
    let store = TestStore::embedded(option_env!("CARGO_TARGET_TMPDIR")).await;
    let r = runner(&store).await;
    let bundle = load(ZONES).await;
    let got = query(&r, &function(&bundle, "zones:read"), unit())
        .await
        .expect("read")
        .result;
    let f = |v: f64| LiveValue::F64(v);
    assert_eq!(
        field(&got, "getters"),
        LiveValue::Array(
            [2020.0, 6.0, 15.0, 3.0, 23.0, 30.0, 45.0, 123.0, 120.0]
                .map(f)
                .to_vec()
        )
    );
    assert_eq!(field(&got, "offset"), f(0.0));
    assert_eq!(
        field(&got, "strings"),
        LiveValue::Array(vec![
            s("Wed Jul 15 2020 23:30:45 GMT+0000"),
            s("Wed Jul 15 2020"),
            s("23:30:45 GMT+0000"),
            s("07/15/2020, 11:30:45 PM"),
            s("07/15/2020"),
            s("11:30:45 PM"),
            s("Wed Jul 15 2020 23:30:45 GMT+0000"),
            s("Invalid Date"),
            s("NaN"),
            s("Sun Jan 01 -0050 00:00:00 GMT+0000"),
        ])
    );
    assert_eq!(field(&got, "called"), LiveValue::Bool(true));
    assert_eq!(
        field(&got, "setters"),
        LiveValue::Array(vec![
            s("2001-05-09T05:06:07.008Z"),
            s("1999-01-01T00:00:00.000Z")
        ])
    );
    let utc = items(&field(&got, "utc"));
    for (i, v) in utc.iter().enumerate() {
        assert_eq!(v, &LiveValue::Bool(true), "utc[{i}]");
    }
}

/// LV1 plan Task 5, `functions_suite_runs_isolated`: every test above again,
/// with its bundles in isolated worker processes (Linux only, where
/// `isolated` exists). The cases are
/// `functions_suite_runs_isolated::<test>[::<backend>]`.
#[cfg(target_os = "linux")]
mod functions_suite_runs_isolated {
    use super::*;

    live_test!(module_state_does_not_leak_between_calls, |store| {
        crate::common::isolated(super::module_state_does_not_leak_between_calls(store))
    });
    live_test!(crypto_random_throws_in_queries_and_mutations, |store| {
        crate::common::isolated(super::crypto_random_throws_in_queries_and_mutations(store))
    });
    live_test!(date_now_is_start_ts, |store| crate::common::isolated(
        super::date_now_is_start_ts(store)
    ));
    live_test!(random_is_repeatable_for_same_ts_and_request, |store| {
        crate::common::isolated(super::random_is_repeatable_for_same_ts_and_request(store))
    });
    live_test!(no_fetch_no_timers, |store| crate::common::isolated(
        super::no_fetch_no_timers(store)
    ));
    live_test!(
        globals_are_frozen_but_overridable_by_own_properties,
        |store| crate::common::isolated(
            super::globals_are_frozen_but_overridable_by_own_properties(store)
        )
    );
    live_test!(leftover_jobs_never_run_in_the_next_call, |store| {
        crate::common::isolated(super::leftover_jobs_never_run_in_the_next_call(store))
    });

    #[test]
    fn bundle_top_level_jobs_run_within_its_limits() {
        crate::common::isolated_sync(super::bundle_top_level_jobs_run_within_its_limits);
    }

    #[test]
    fn local_time_methods_are_utc() {
        crate::common::isolated_sync(super::local_time_methods_are_utc);
    }
}
