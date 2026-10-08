//! `loams-wal`'s command line.
#![cfg(feature = "server")]
#![allow(clippy::unwrap_used)]

/// The feeder is gone (PG2 Task 31, deleted after Task 32's pageserver end
/// to end test): loams-wal serves the pageserver itself, and no option
/// hands WAL to a stock safekeeper.
#[test]
fn no_feeder_flag_exists() {
    let cmd = loams_safekeeper::cli::command();
    let names: Vec<String> = cmd
        .get_arguments()
        .filter_map(|a| a.get_long().map(str::to_string))
        .collect();
    assert!(
        !names.iter().any(|n| n.contains("feed")),
        "feeder options: {names:?}"
    );
    assert!(
        cmd.clone()
            .try_get_matches_from(["loams-wal", "--feed-safekeeper", "127.0.0.1:5457"])
            .is_err()
    );
    // The broker options that replace it are there.
    assert!(names.iter().any(|n| n == "broker-endpoint"), "{names:?}");
}
