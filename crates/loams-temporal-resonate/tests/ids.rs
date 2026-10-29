//! The promise-ID grammar (§74 §5.4, D1503, as amended by X1.R13).
#![allow(clippy::unwrap_used)]

use loams_temporal_resonate::ids::{
    IdError, MAX_PROMISE_ID_LEN, PromiseId, TAG_KIND, TAG_NS, TAG_RUN, TAG_WF, TimeoutKind,
    WorkflowRef, namespace,
};
use proptest::prelude::*;

const NS: &str = "01JC0FFEE0000000000000000.4f1c9a52-6d1e-4c7b-9a0e-2b3c4d5e6f70";
const RUN: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";

fn arb_id() -> impl Strategy<Value = PromiseId> {
    let s = || any::<String>();
    let kind = prop::sample::select(TimeoutKind::ALL.to_vec());
    prop_oneof![
        (s(), s(), s(), any::<i64>()).prop_map(|(ns, wf, run, e)| PromiseId::Wft {
            ns,
            wf: wf.into(),
            run,
            scheduled_event_id: e
        }),
        (s(), s(), s(), any::<i64>(), any::<i32>()).prop_map(|(ns, wf, run, e, a)| {
            PromiseId::Act {
                ns,
                wf: wf.into(),
                run,
                scheduled_event_id: e,
                attempt: a,
            }
        }),
        (s(), s(), s(), any::<i64>()).prop_map(|(ns, wf, run, e)| PromiseId::Tmr {
            ns,
            wf: wf.into(),
            run,
            started_event_id: e
        }),
        (s(), s(), s(), any::<i64>(), kind, any::<i32>()).prop_map(
            |(ns, wf, run, e, kind, attempt)| PromiseId::To {
                ns,
                wf: wf.into(),
                run,
                event_id: e,
                kind,
                attempt
            }
        ),
        (s(), s(), any::<i64>()).prop_map(|(ns, schedule_id, t)| PromiseId::Start {
            ns,
            schedule_id,
            nominal_time_unix_ms: t
        }),
        (s(), s(), s(), any::<i64>()).prop_map(|(ns, schedule_id, generation, t)| {
            PromiseId::Sch {
                ns,
                schedule_id,
                generation,
                wake_unix_ms: t,
            }
        }),
        // Separators everywhere, near and past the limit.
        ("[a-z.:%/]{0,20}", "[a-z.:%/]{200,400}", "[a-z.:]{0,10}").prop_map(|(ns, wf, run)| {
            PromiseId::Wft {
                ns,
                wf: wf.into(),
                run,
                scheduled_event_id: 3,
            }
        }),
        // Workflow IDs that look like a hash.
        ("h[0-9a-f]{32}").prop_map(|wf| PromiseId::Tmr {
            ns: NS.into(),
            wf: wf.into(),
            run: RUN.into(),
            started_event_id: 9
        }),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2048))]

    #[test]
    fn ids_roundtrip(id in arb_id()) {
        let (s, tags) = id.encode();
        prop_assert!(s.is_ascii());
        let back = PromiseId::parse(&s).unwrap();
        match tags.get(TAG_WF) {
            None => prop_assert_eq!(&back, &id),
            Some(raw) => {
                prop_assert_eq!(id.wf().and_then(WorkflowRef::as_id), Some(raw.as_str()));
                prop_assert_eq!(back.wf(), Some(&WorkflowRef::Hash(WorkflowRef::hash_of(raw))));
                prop_assert!(back.wf().unwrap().matches(raw));
            }
        }
        // Canonical both ways: the parsed ID encodes to the same string.
        prop_assert_eq!(back.encode().0, s.clone());
        prop_assert_eq!(tags.get(TAG_NS), Some(&id.ns().to_string()));
        prop_assert_eq!(tags.get(TAG_KIND).map(String::as_str), Some(id.kind()));
        prop_assert_eq!(tags.get(TAG_RUN).map(String::as_str), id.run());
        prop_assert_eq!(back.origin(), id.origin());
    }

    #[test]
    fn ids_have_exactly_one_colon(id in arb_id()) {
        // The Postgres store refuses more than one `:`; the text before it is
        // the run, Resonate's origin. `start` IDs are origins of their own.
        let (s, _) = id.encode();
        let colons = s.matches(':').count();
        match id {
            PromiseId::Start { .. } => prop_assert_eq!(colons, 0, "{}", s),
            _ => {
                prop_assert_eq!(colons, 1, "{}", s);
                let (origin, lineage) = s.split_once(':').unwrap();
                prop_assert_eq!(origin.to_string(), id.origin());
                let prefix = format!("{}.", id.kind());
                prop_assert!(lineage.starts_with(&prefix), "{} does not start with {}", lineage, prefix);
            }
        }
    }

    #[test]
    fn ids_parse_never_panics(s in ".*") {
        let _ = PromiseId::parse(&s);
    }
}

#[test]
fn ids_one_origin_per_run() {
    let base = |run: &str| PromiseId::Wft {
        ns: NS.into(),
        wf: "order-42".into(),
        run: run.into(),
        scheduled_event_id: 2,
    };
    let to = |attempt| PromiseId::To {
        ns: NS.into(),
        wf: "order-42".into(),
        run: RUN.into(),
        event_id: 5,
        kind: TimeoutKind::StartToClose,
        attempt,
    };
    let act = PromiseId::Act {
        ns: NS.into(),
        wf: "order-42".into(),
        run: RUN.into(),
        scheduled_event_id: 5,
        attempt: 1,
    };
    assert_eq!(base(RUN).origin(), act.origin());
    assert_eq!(act.origin(), to(1).origin());
    assert_ne!(base(RUN).origin(), base("other-run").origin());
    // A timeout belongs to one attempt.
    assert_ne!(to(1).encode().0, to(2).encode().0);
}

#[test]
fn ids_hash_fallback_past_limit() {
    let id = |wf: String| PromiseId::Wft {
        ns: NS.into(),
        wf: wf.into(),
        run: RUN.into(),
        scheduled_event_id: 7,
    };
    // The longest workflow ID that still fits stays raw; one byte more hashes.
    let overhead = id(String::new()).encode().0.len();
    let fits = "w".repeat(MAX_PROMISE_ID_LEN - overhead);
    let (s, tags) = id(fits.clone()).encode();
    assert_eq!(s.len(), MAX_PROMISE_ID_LEN);
    assert!(s.contains(&fits));
    assert!(!tags.contains_key(TAG_WF));

    let past = format!("{fits}w");
    let (s, tags) = id(past.clone()).encode();
    assert!(s.len() <= MAX_PROMISE_ID_LEN);
    let hash = hex::encode(WorkflowRef::hash_of(&past));
    assert_eq!(s, format!("{NS}.h{hash}.{RUN}:wft.7"));
    assert_eq!(tags.get(TAG_WF), Some(&past));
    assert_eq!(
        PromiseId::parse(&s).unwrap().wf(),
        Some(&WorkflowRef::Hash(WorkflowRef::hash_of(&past)))
    );

    // Escapes count: 100 '.' are 300 encoded bytes, so they hash too.
    let dots = ".".repeat(100);
    let (s, tags) = id(dots.clone()).encode();
    assert!(s.len() <= MAX_PROMISE_ID_LEN);
    assert_eq!(tags.get(TAG_WF), Some(&dots));

    // An ID still too long once hashed (a huge namespace) is refused.
    let huge = PromiseId::Start {
        ns: "n".repeat(300),
        schedule_id: "s".into(),
        nominal_time_unix_ms: 0,
    };
    assert!(matches!(
        huge.encode_checked(),
        Err(IdError::TooLong { .. })
    ));
    assert!(id(past).encode_checked().is_ok());
}

#[test]
fn ids_reject_raw_colon() {
    // A raw ':' anywhere past the first is refused.
    assert!(matches!(
        PromiseId::parse(&format!("{NS}.a.{RUN}:wft.1:2")),
        Err(IdError::Colons(_))
    ));
    // Encoding never lets a separator through.
    let id = PromiseId::Tmr {
        ns: NS.into(),
        wf: "a:b.c%d".into(),
        run: RUN.into(),
        started_event_id: 4,
    };
    let (s, _) = id.encode();
    assert_eq!(s, format!("{NS}.a%3Ab%2Ec%25d.{RUN}:tmr.4"));
    // Only the canonical form parses: lower-case escapes, escaped unreserved
    // characters, stray '%', raw '.' in a segment, non-canonical numbers and
    // unknown kinds are refused.
    for bad in [
        format!("{NS}.a%3ab.{RUN}:tmr.4"),
        format!("{NS}.%61.{RUN}:tmr.4"),
        format!("{NS}.a%.{RUN}:tmr.4"),
        format!("{NS}.a b.{RUN}:tmr.4"),
        format!("{NS}.a.{RUN}:tmr.04"),
        format!("{NS}.a.{RUN}:tmr.+4"),
        format!("{NS}.a.{RUN}:tmr.-0"),
        format!("{NS}.a.{RUN}:tmr.4.5"),
        format!("{NS}.a.{RUN}:act.4"),
        format!("{NS}.a.{RUN}:to.4.S2S.1"),
        format!("{NS}.a.{RUN}:to.4.s2c"),
        format!("{NS}.a.{RUN}:nope.4"),
        format!("{RUN}:wft.1"),
        "a.b".to_string(),
        "a.b.x".to_string(),
    ] {
        assert!(PromiseId::parse(&bad).is_err(), "{bad} parsed");
    }
}

#[test]
fn ids_vectors() {
    let ns = namespace(
        "01JC0FFEE0000000000000000",
        "4f1c9a52-6d1e-4c7b-9a0e-2b3c4d5e6f70",
    );
    assert_eq!(ns, NS);
    let vectors: Vec<(PromiseId, &str)> = vec![
        (
            PromiseId::Wft {
                ns: NS.into(),
                wf: "order-42".into(),
                run: RUN.into(),
                scheduled_event_id: 2,
            },
            "01JC0FFEE0000000000000000.4f1c9a52-6d1e-4c7b-9a0e-2b3c4d5e6f70.order-42.0f8fad5b-d9cb-469f-a165-70867728950e:wft.2",
        ),
        (
            PromiseId::Act {
                ns: NS.into(),
                wf: "payments:42".into(),
                run: RUN.into(),
                scheduled_event_id: 5,
                attempt: 3,
            },
            "01JC0FFEE0000000000000000.4f1c9a52-6d1e-4c7b-9a0e-2b3c4d5e6f70.payments%3A42.0f8fad5b-d9cb-469f-a165-70867728950e:act.5.3",
        ),
        (
            PromiseId::Tmr {
                ns: NS.into(),
                wf: "café 100%".into(),
                run: RUN.into(),
                started_event_id: 11,
            },
            "01JC0FFEE0000000000000000.4f1c9a52-6d1e-4c7b-9a0e-2b3c4d5e6f70.caf%C3%A9%20100%25.0f8fad5b-d9cb-469f-a165-70867728950e:tmr.11",
        ),
        (
            PromiseId::To {
                ns: NS.into(),
                wf: "w.v1".into(),
                run: RUN.into(),
                event_id: 5,
                kind: TimeoutKind::ScheduleToClose,
                attempt: 2,
            },
            "01JC0FFEE0000000000000000.4f1c9a52-6d1e-4c7b-9a0e-2b3c4d5e6f70.w%2Ev1.0f8fad5b-d9cb-469f-a165-70867728950e:to.5.sc2c.2",
        ),
        (
            PromiseId::Start {
                ns: NS.into(),
                schedule_id: "nightly/report".into(),
                nominal_time_unix_ms: 1_792_800_000_000,
            },
            "01JC0FFEE0000000000000000.4f1c9a52-6d1e-4c7b-9a0e-2b3c4d5e6f70.nightly%2Freport.1792800000000",
        ),
        (
            PromiseId::Sch {
                ns: NS.into(),
                schedule_id: "nightly/report".into(),
                generation: "7c9e6679742540de944be07fc1f90ae7".into(),
                wake_unix_ms: 1_792_800_000_000,
            },
            "01JC0FFEE0000000000000000.4f1c9a52-6d1e-4c7b-9a0e-2b3c4d5e6f70.nightly%2Freport.7c9e6679742540de944be07fc1f90ae7:sch.1792800000000",
        ),
    ];
    for (id, want) in vectors {
        assert_eq!(id.encode().0, want);
        assert_eq!(PromiseId::parse(want).unwrap(), id);
    }
    for (k, s) in TimeoutKind::ALL
        .iter()
        .zip(["s2s", "s2c", "sc2c", "hb", "wft"])
    {
        assert_eq!(k.as_str(), s);
    }

    // The hash: the first 16 bytes of SHA-256, lower-case hex.
    assert_eq!(
        hex::encode(WorkflowRef::hash_of("payments:42")),
        "0803ac83ded529b06147de9b3bc39643"
    );
    let long = format!("order/{}", "x".repeat(240));
    let (s, tags) = PromiseId::Wft {
        ns: NS.into(),
        wf: long.clone().into(),
        run: RUN.into(),
        scheduled_event_id: 2,
    }
    .encode();
    assert_eq!(
        s,
        "01JC0FFEE0000000000000000.4f1c9a52-6d1e-4c7b-9a0e-2b3c4d5e6f70.h3042d570f5d8ff3d52258591b87bd790.0f8fad5b-d9cb-469f-a165-70867728950e:wft.2"
    );
    assert_eq!(tags.get(TAG_WF), Some(&long));
    assert_eq!(tags.get(TAG_KIND).map(String::as_str), Some("wft"));
    assert_eq!(tags.get(TAG_RUN).map(String::as_str), Some(RUN));
}
