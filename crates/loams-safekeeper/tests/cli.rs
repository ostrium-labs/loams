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

/// Bad broker options stop `loams-wal` at startup, with the reason, before
/// it listens (PG2 Task 32 review, I4).
#[test]
fn bad_broker_options_are_refused_at_startup() {
    // Port 0: if the checks let a case through it would listen; none may.
    const LOOPBACK: [&str; 4] = ["--listen-pg", "127.0.0.1:0", "--listen-http", "127.0.0.1:0"];
    let run = |args: &[&str]| {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_loams-wal"))
            .args(args)
            .output()
            .unwrap();
        assert!(!out.status.success(), "{args:?} was accepted");
        String::from_utf8_lossy(&out.stderr).into_owned()
    };
    fn with<'a>(extra: &[&'a str]) -> Vec<&'a str> {
        LOOPBACK
            .iter()
            .copied()
            .chain(extra.iter().copied())
            .collect()
    }
    assert!(run(&with(&["--broker-endpoint", "https://b:50051"])).contains("Task 38"));
    assert!(run(&with(&["--broker-endpoint", "b:50051"])).contains("no scheme"));
    assert!(
        run(&with(&[
            "--broker-endpoint",
            "http://b:50051",
            "--advertise-pg",
            "wal"
        ]))
        .contains("host:port")
    );
    let e = run(&[
        "--listen-pg",
        "0.0.0.0:0",
        "--listen-http",
        "0.0.0.0:0",
        "--auth-token",
        "t",
        "--trusted-network",
        "--broker-endpoint",
        "http://b:50051",
        "--advertise-pg",
        "0.0.0.0:5454",
        "--advertise-http",
        "10.0.0.5:7676",
    ]);
    assert!(e.contains("--advertise-pg") && e.contains("reach"), "{e}");
}
