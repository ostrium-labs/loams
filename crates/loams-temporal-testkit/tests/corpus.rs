//! DX1 Task 2's tests: the corpus loads and its hashes match; the Rust
//! normaliser masks exactly what `normalize.py` masks and agrees with it on
//! the corpus; the differ reports paths.

use std::path::Path;
use std::process::Command;

use loams_temporal_testkit::{Corpus, Options, compare, diff, normalize, normalized_history};
use serde_json::{Value, json};

#[test]
fn corpus_loads_and_hashes_match() {
    let Some(corpus) = Corpus::for_tests() else {
        return;
    };
    assert!(
        corpus.histories.len() >= 60,
        "{} histories",
        corpus.histories.len()
    );
    for lang in ["go", "ts"] {
        for scenario in [
            "hello",
            "two_activities_sequential",
            "activity_parallel_fanout_10",
            "signal_then_complete",
            "query_while_running",
            "cancel_requested",
            "terminated",
            "workflow_failed_application_error",
            "activity_heartbeat_details",
        ] {
            let runs = corpus.runs(lang, scenario);
            assert_eq!(runs.len(), 1, "{lang}/{scenario}");
            assert_eq!(runs[0].phase, "DX1");
        }
    }
    assert_eq!(corpus.runs("go", "continue_as_new_3").len(), 4);
}

fn event(id: u32, time: &str, run: &str, identity: &str) -> Value {
    json!({
        "eventId": id.to_string(),
        "eventTime": time,
        "eventType": "EVENT_TYPE_WORKFLOW_EXECUTION_STARTED",
        "taskId": (1000 + id).to_string(),
        "workflowExecutionStartedEventAttributes": {
            "workflowType": {"name": "hello"},
            "originalExecutionRunId": run,
            "identity": identity,
            "header": {},
        }
    })
}

#[test]
fn normalize_masks_only_masked_fields() {
    let o = Options::default();
    let a = normalize(
        json!({"events": [event(1, "2026-10-24T10:00:00.123Z", "0b6f3c8a-1111-4a2b-9c3d-123456789abc", "1@a")]}),
        &o,
    );
    let b = normalize(
        json!({"events": [event(1, "2026-10-25T11:30:00Z", "9e8d7c6b-2222-4f5e-8d7c-cba987654321", "2@b")]}),
        &o,
    );
    assert!(compare(&a, &b).is_empty(), "{:?}", compare(&a, &b));
    let c = normalize(
        json!({"events": [event(2, "2026-10-24T10:00:00Z", "0b6f3c8a-1111-4a2b-9c3d-123456789abc", "1@a")]}),
        &o,
    );
    assert!(!compare(&a, &c).is_empty(), "a changed event ID differs");
    let s = normalize(
        json!({"events": [{"eventId": "1", "result": "wf-2026-10-24T10:00:00Z"}]}),
        &o,
    );
    assert_eq!(s["events"][0]["result"], "wf-<time>");
    let f = normalize(
        json!({"events": [{"sdkMetadata": {"langUsedFlags": [7, 3]}}]}),
        &o,
    );
    assert_eq!(
        f["events"][0]["sdkMetadata"]["langUsedFlags"],
        json!([3, 7])
    );
}

#[test]
fn sticky_reads_as_normal_for_the_gateway() {
    let sticky = json!({"events": [{"taskQueue": {"name": "host:abc", "kind": "TASK_QUEUE_KIND_STICKY", "normalName": "q"}}]});
    let normal =
        json!({"events": [{"taskQueue": {"name": "q", "kind": "TASK_QUEUE_KIND_NORMAL"}}]});
    let g = Options::gateway("hello");
    assert!(
        compare(
            &normalize(sticky.clone(), &g),
            &normalize(normal.clone(), &g)
        )
        .is_empty()
    );
    let o = Options::default();
    assert!(!compare(&normalize(sticky, &o), &normalize(normal, &o)).is_empty());
}

#[test]
fn diff_reports_path() {
    let d = diff(
        &json!({"events": [{"a": {"b": 1}}]}),
        &json!({"events": [{"a": {"b": 2}}]}),
    );
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].path, "events[0].a.b");
    assert_eq!(d[0].to_string(), "events[0].a.b: expected 1 got 2");
}

/// The Rust normaliser and `normalize.py` give the same verdict for every
/// pair of same-scenario histories across the two languages' runs and for
/// each history against itself.
#[test]
fn normalize_agrees_with_python() {
    let Some(corpus) = Corpus::for_tests() else {
        return;
    };
    let script = corpus.root.join("normalize.py");
    if Command::new("python3").arg("--version").output().is_err() || !script.is_file() {
        eprintln!("skipped: no python3 or normalize.py");
        return;
    }
    let python_equal = |a: &Path, b: &Path| {
        Command::new("python3")
            .arg("-I")
            .arg(&script)
            .arg("--compare")
            .arg(a)
            .arg(b)
            .output()
            .expect("python3 runs")
            .status
            .success()
    };
    let o = Options::default();
    let mut checked = 0;
    for h in corpus.phase("DX1").filter(|h| h.lang == "go") {
        let json_path = h.path.with_extension("json");
        let mine = normalized_history(&h.history, &o);
        // Same history: equal in both.
        assert!(compare(&mine, &mine).is_empty());
        assert!(python_equal(&json_path, &json_path));
        // Against every other DX1 history: Python's verdict is "different",
        // and so is ours.
        for other in corpus
            .phase("DX1")
            .filter(|x| x.lang == "go" && x.scenario != h.scenario)
        {
            let theirs = normalized_history(&other.history, &o);
            let py = python_equal(&json_path, &other.path.with_extension("json"));
            assert_eq!(
                compare(&mine, &theirs).is_empty(),
                py,
                "{} vs {}",
                h.scenario,
                other.scenario
            );
            checked += 1;
        }
    }
    eprintln!("{checked} pairs agree");
}
