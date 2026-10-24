//! §74 §5.4's promise-ID grammar (D1503).
#![allow(clippy::unwrap_used)]

use loams_temporal_resonate::ids::{
    IdError, MAX_PROMISE_ID_LEN, PromiseId, TAG_KIND, TAG_NS, TAG_RUN, TAG_WF, TimeoutKind,
    WorkflowRef, namespace,
};
use proptest::prelude::*;

const NS: &str = "01JC0FFEE0000000000000000.4f1c9a52-6d1e-4c7b-9a0e-2b3c4d5e6f70";
const RUN: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";

fn segments(id: &PromiseId) -> usize {
    match id {
        PromiseId::Wft { .. } | PromiseId::Tmr { .. } => 5,
        PromiseId::Act { .. } | PromiseId::To { .. } => 6,
        PromiseId::Start { .. } => 4,
    }
}

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
        (s(), s(), s(), any::<i64>(), kind).prop_map(|(ns, wf, run, e, kind)| PromiseId::To {
            ns,
            wf: wf.into(),
            run,
            event_id: e,
            kind
        }),
        (s(), s(), any::<i64>()).prop_map(|(ns, schedule_id, t)| PromiseId::Start {
            ns,
            schedule_id,
            nominal_time_unix_ms: t
        }),
        // Workflow IDs near and past the limit, and ones that look like a hash.
        ("[a-z:%/]{200,400}").prop_map(|wf| PromiseId::Wft {
            ns: NS.into(),
            wf: wf.into(),
            run: RUN.into(),
            scheduled_event_id: 3
        }),
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
        prop_assert_eq!(s.split(':').count(), segments(&id), "a raw ':' leaked: {}", s);
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
        prop_assert_eq!(back.encode().0, s);
        prop_assert_eq!(tags.get(TAG_NS), Some(&id.ns().to_string()));
        prop_assert_eq!(tags.get(TAG_KIND).map(String::as_str), Some(id.kind()));
        prop_assert_eq!(tags.get(TAG_RUN).map(String::as_str), id.run());
    }

    #[test]
    fn ids_parse_never_panics(s in ".*") {
        let _ = PromiseId::parse(&s);
    }
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
    assert_eq!(s, format!("wft:{NS}:h{hash}:{RUN}:7"));
    assert_eq!(tags.get(TAG_WF), Some(&past));
    assert_eq!(
        PromiseId::parse(&s).unwrap().wf(),
        Some(&WorkflowRef::Hash(WorkflowRef::hash_of(&past)))
    );

    // Escapes count: 100 ':' are 300 encoded bytes, so they hash too.
    let colons = ":".repeat(100);
    let (s, tags) = id(colons.clone()).encode();
    assert!(s.len() <= MAX_PROMISE_ID_LEN);
    assert_eq!(tags.get(TAG_WF), Some(&colons));

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
    // A raw ':' inside a segment changes the segment count.
    assert!(matches!(
        PromiseId::parse(&format!("wft:{NS}:a:b:{RUN}:1")),
        Err(IdError::Segments {
            expected: 5,
            found: 6,
            ..
        })
    ));
    // Encoding never lets one through.
    let id = PromiseId::Tmr {
        ns: NS.into(),
        wf: "a:b%c".into(),
        run: RUN.into(),
        started_event_id: 4,
    };
    let (s, _) = id.encode();
    assert_eq!(s, format!("tmr:{NS}:a%3Ab%25c:{RUN}:4"));
    // Only the canonical form parses: lower-case escapes, escaped unreserved
    // characters, stray '%' and non-canonical numbers are refused.
    for bad in [
        format!("tmr:{NS}:a%3ab%25c:{RUN}:4"),
        format!("tmr:{NS}:%61:{RUN}:4"),
        format!("tmr:{NS}:a%:{RUN}:4"),
        format!("tmr:{NS}:a b:{RUN}:4"),
        format!("tmr:{NS}:a:{RUN}:04"),
        format!("tmr:{NS}:a:{RUN}:+4"),
        format!("tmr:{NS}:a:{RUN}:-0"),
        format!("to:{NS}:a:{RUN}:4:S2S"),
        format!("nope:{NS}:a:{RUN}:4"),
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
            "wft:01JC0FFEE0000000000000000.4f1c9a52-6d1e-4c7b-9a0e-2b3c4d5e6f70:order-42:0f8fad5b-d9cb-469f-a165-70867728950e:2",
        ),
        (
            PromiseId::Act {
                ns: NS.into(),
                wf: "payments:42".into(),
                run: RUN.into(),
                scheduled_event_id: 5,
                attempt: 3,
            },
            "act:01JC0FFEE0000000000000000.4f1c9a52-6d1e-4c7b-9a0e-2b3c4d5e6f70:payments%3A42:0f8fad5b-d9cb-469f-a165-70867728950e:5:3",
        ),
        (
            PromiseId::Tmr {
                ns: NS.into(),
                wf: "café 100%".into(),
                run: RUN.into(),
                started_event_id: 11,
            },
            "tmr:01JC0FFEE0000000000000000.4f1c9a52-6d1e-4c7b-9a0e-2b3c4d5e6f70:caf%C3%A9%20100%25:0f8fad5b-d9cb-469f-a165-70867728950e:11",
        ),
        (
            PromiseId::To {
                ns: NS.into(),
                wf: "w".into(),
                run: RUN.into(),
                event_id: 5,
                kind: TimeoutKind::ScheduleToClose,
            },
            "to:01JC0FFEE0000000000000000.4f1c9a52-6d1e-4c7b-9a0e-2b3c4d5e6f70:w:0f8fad5b-d9cb-469f-a165-70867728950e:5:sc2c",
        ),
        (
            PromiseId::Start {
                ns: NS.into(),
                schedule_id: "nightly/report".into(),
                nominal_time_unix_ms: 1_792_800_000_000,
            },
            "start:01JC0FFEE0000000000000000.4f1c9a52-6d1e-4c7b-9a0e-2b3c4d5e6f70:nightly%2Freport:1792800000000",
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
        "wft:01JC0FFEE0000000000000000.4f1c9a52-6d1e-4c7b-9a0e-2b3c4d5e6f70:h3042d570f5d8ff3d52258591b87bd790:0f8fad5b-d9cb-469f-a165-70867728950e:2"
    );
    assert_eq!(tags.get(TAG_WF), Some(&long));
    assert_eq!(tags.get(TAG_KIND).map(String::as_str), Some("wft"));
    assert_eq!(tags.get(TAG_RUN).map(String::as_str), Some(RUN));
}
