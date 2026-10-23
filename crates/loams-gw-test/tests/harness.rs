//! The harness skeleton (KV1 plan KV0 Task 8).

use loams_gw_test::{Backend, Report, run_everywhere, runners, scenarios};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_scenario_same_report_on_every_backend() {
    let scenario = scenarios::find("cas-and-scan").expect("the scenario");
    let comparison = run_everywhere(scenario.as_ref()).await;
    let ran: Vec<Backend> = comparison.reports.iter().map(|(b, _)| *b).collect();
    assert!(ran.contains(&Backend::Embedded) && ran.contains(&Backend::Memory));
    assert!(comparison.agree(), "{}", comparison.describe());
    let report = &comparison.reports[0].1;
    assert_eq!(report.facts()["0. create a"], "Ok(true)");
    assert_eq!(report.facts()["1. create a again"], "Ok(false)");
    assert_eq!(report.facts()["3. stale swap a"], "Ok(false)");
    assert!(report.facts()["scan forward"].contains("[97]"), "{report}");
}

#[test]
fn reports_diff_by_fact() {
    let mut a = Report::new();
    a.fact("x", 1).fact("y", 2);
    let mut b = Report::new();
    b.fact("x", 1).fact("z", 3);
    assert_eq!(
        a.diff(&b),
        vec![("y", Some("2"), None), ("z", None, Some("3"))]
    );
    assert!(a.diff(&a).is_empty());
}

#[test]
fn every_runner_is_a_documented_placeholder() {
    for r in runners::all() {
        assert!(!r.about.is_empty());
        assert!(
            matches!(r.status, runners::Status::Planned { .. }),
            "{}",
            r.name
        );
    }
}
