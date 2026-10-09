//! The headless binary's command line (plan DD1 Task 1).

use std::process::Command;

fn agentd() -> Command {
    Command::new(env!("CARGO_BIN_EXE_loams-agentd"))
}

#[test]
fn loams_agentd_version_runs() {
    let output = agentd().arg("version").output().expect("run loams-agentd");
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).expect("utf-8");
    assert_eq!(
        stdout.trim(),
        format!("loams-agentd {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn removed_subcommands_are_rejected() {
    // Headed mode, appshot, sync, WorkOS login and logout, the daemon
    // manager and self-update are gone (D781, D782).
    for removed in [
        "headless", "login", "logout", "sync", "appshot", "daemon", "update",
    ] {
        let output = agentd().arg(removed).output().expect("run loams-agentd");
        assert!(
            !output.status.success(),
            "`{removed}` must not exist: {output:?}"
        );
    }
    // The hidden `loams` entry point exists only for the Loams Bot harness
    // (T1-2): every other link subcommand is a usage error. `mock` and `bot`
    // would bind a port or reach the instance; `login`, `logout` and
    // `status` are the WorkOS-era CLI.
    for removed in ["login", "logout", "bot", "status", "mock", "bot-acpx"] {
        let output = agentd()
            .args(["loams", removed])
            .output()
            .expect("run loams-agentd");
        assert_eq!(
            output.status.code(),
            Some(2),
            "`loams {removed}` must be rejected as a usage error: {output:?}"
        );
    }
    let output = agentd().arg("loams").output().expect("run loams-agentd");
    assert_eq!(output.status.code(), Some(2), "bare `loams`: {output:?}");
}

#[test]
fn no_subcommand_does_not_start_anything() {
    // There is no headed default: a bare invocation prints usage and fails.
    let output = agentd().output().expect("run loams-agentd");
    assert!(!output.status.success(), "{output:?}");
}
