//! The corpus's scenarios in plan order (scenarios.json), as the Rust worker
//! runs them: every scenario that is not Go only. manifest.py's `check`
//! reads the lines below with a regex (one `("name", "DXn"),` per line,
//! four spaces in), so keep the shape.

pub const SCENARIOS: &[(&str, &str)] = &[
    ("hello", "DX1"),
    ("two_activities_sequential", "DX1"),
    ("activity_parallel_fanout_10", "DX1"),
    ("signal_then_complete", "DX1"),
    ("query_while_running", "DX1"),
    ("cancel_requested", "DX1"),
    ("terminated", "DX1"),
    ("workflow_failed_application_error", "DX1"),
    ("activity_heartbeat_details", "DX1"),
    ("timer_short", "DX2"),
    ("timer_cancelled", "DX2"),
    ("activity_retry_backoff_3_attempts", "DX2"),
    ("activity_start_to_close_timeout", "DX2"),
    ("activity_schedule_to_start_timeout", "DX2"),
    ("activity_heartbeat_timeout", "DX2"),
    ("signal_with_start", "DX2"),
    ("child_workflow", "DX2"),
    ("child_workflow_cancelled", "DX2"),
    ("continue_as_new_3", "DX2"),
    ("id_reuse_allow_duplicate_failed_only", "DX2"),
    ("id_conflict_fail", "DX2"),
    ("local_activity", "DX2"),
    ("side_effect_and_version_marker", "DX2"),
    ("search_attributes_upsert", "DX3"),
    ("memo", "DX3"),
    ("large_payload_1mb", "DX3"),
    ("update_accepted_completed", "DX4"),
    ("update_rejected", "DX4"),
];

/// The task queue of the Rust worker (the Go one is `corpus-go`).
pub const QUEUE: &str = "corpus-rust";

/// The fixed workflow ID of a scenario's run (the replayers derive child IDs
/// from it, so the generator and the replayer must agree).
pub fn workflow_id(scenario: &str) -> String {
    format!("corpus-rust-{scenario}")
}

/// The scenarios to run: all of them, or the comma-separated subset `only`
/// (in plan order; an unknown name is an error, so a typo never passes as an
/// empty run).
pub fn pick(only: Option<&str>) -> Result<Vec<&'static str>, String> {
    let all: Vec<&'static str> = SCENARIOS.iter().map(|(n, _)| *n).collect();
    let Some(only) = only.filter(|s| !s.is_empty()) else {
        return Ok(all);
    };
    let want: Vec<&str> = only
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if let Some(bad) = want.iter().find(|w| !all.contains(w)) {
        return Err(format!(
            "unknown scenario {bad:?} (the Rust worker runs: {})",
            all.join(",")
        ));
    }
    Ok(all.into_iter().filter(|n| want.contains(n)).collect())
}

/// The scenario a history file belongs to: `<scenario>.json`,
/// `<scenario>.binpb` or `<scenario>.run<k>.<ext>`.
pub fn scenario_of_file(name: &str) -> Option<&str> {
    let stem = name
        .strip_suffix(".binpb")
        .or_else(|| name.strip_suffix(".json"))?;
    let base = match stem.rsplit_once('.') {
        Some((base, run))
            if run.starts_with("run")
                && run[3..].chars().all(|c| c.is_ascii_digit())
                && run.len() > 3 =>
        {
            base
        }
        _ => stem,
    };
    (!base.is_empty()).then_some(base)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// The worker lists every scenario of scenarios.json that is not Go
    /// only, in plan order (what manifest.py check also asserts).
    #[test]
    fn matches_scenarios_json() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios.json");
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let want: Vec<(String, String)> = doc["scenarios"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|s| !s["go_only"].as_bool().unwrap())
            .map(|s| {
                (
                    s["name"].as_str().unwrap().to_owned(),
                    s["phase"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        let have: Vec<(String, String)> = SCENARIOS
            .iter()
            .map(|(n, p)| ((*n).to_owned(), (*p).to_owned()))
            .collect();
        assert_eq!(have, want);
    }

    #[test]
    fn pick_keeps_plan_order_and_refuses_unknown_names() {
        assert_eq!(pick(None).unwrap().len(), SCENARIOS.len());
        assert_eq!(pick(Some("")).unwrap().len(), SCENARIOS.len());
        assert_eq!(pick(Some("memo,hello")).unwrap(), vec!["hello", "memo"]);
        assert!(pick(Some("hello,nope")).is_err());
    }

    #[test]
    fn scenario_of_file_strips_runs_and_extensions() {
        assert_eq!(scenario_of_file("hello.json"), Some("hello"));
        assert_eq!(scenario_of_file("hello.binpb"), Some("hello"));
        assert_eq!(
            scenario_of_file("continue_as_new_3.run4.binpb"),
            Some("continue_as_new_3")
        );
        assert_eq!(
            scenario_of_file("id_reuse_allow_duplicate_failed_only.run1.json"),
            Some("id_reuse_allow_duplicate_failed_only")
        );
        assert_eq!(
            scenario_of_file("generate-rust.json"),
            Some("generate-rust")
        );
        assert_eq!(scenario_of_file("notes.txt"), None);
        assert_eq!(workflow_id("hello"), "corpus-rust-hello");
    }
}
