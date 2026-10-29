//! Outcomes, and the JSON-lines report CI turns into the parity matrix.

use std::io::Write as _;
use std::time::Instant;

use serde_json::json;

use crate::provider::{self, Kind};

/// The report file.
pub const REPORT_ENV: &str = "LOAMS_BROWSER_PARITY_REPORT";

/// Why a scenario did not pass.
#[derive(Debug)]
pub enum Failure {
    /// The scenario ran and an assertion failed.
    Fail(String),
    /// The provider is selected but not configured (no binary, no URL, no
    /// encoder). A skip, or a failure under `LOAMS_BROWSER_PARITY_REQUIRE`.
    Unconfigured(String),
    /// The scenario does not apply to this provider by design. Recorded as
    /// `n/a` in the matrix and never counted as a pass.
    NotApplicable(String),
    /// The engine does not implement the capability at all. Recorded as
    /// `unsupported`, never as a pass, and does not fail the run: the gap is
    /// documented (`docs/browser-provider-parity.md`) rather than faked. If
    /// the engine starts implementing it, the scenario passes on its own.
    Unsupported(String),
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Failure::Fail(message)
    }
}

impl From<&str> for Failure {
    fn from(message: &str) -> Self {
        Failure::Fail(message.to_string())
    }
}

/// A scenario's result: a pass carries a note for the report.
pub type Outcome = Result<String, Failure>;

/// Fail with a message unless `condition` holds.
pub fn ensure(condition: bool, message: impl FnOnce() -> String) -> Result<(), Failure> {
    if condition {
        Ok(())
    } else {
        Err(Failure::Fail(message()))
    }
}

/// Run `scenario` for every selected provider, record each result, and panic
/// if any failed.
pub async fn run<F, Fut>(name: &'static str, scenario: F)
where
    F: Fn(Kind) -> Fut,
    Fut: Future<Output = Outcome>,
{
    let providers = provider::selected();
    if providers.is_empty() {
        eprintln!(
            "{name}: skipped, no provider selected ({} is unset)",
            provider::PROVIDERS_ENV
        );
        return;
    }
    let mut failures = Vec::new();
    for kind in providers {
        let started = Instant::now();
        let outcome = scenario(kind).await;
        let elapsed_ms = started.elapsed().as_millis() as u64;
        let (status, detail) = match &outcome {
            Ok(note) => ("pass", note.clone()),
            Err(Failure::Fail(message)) => ("fail", message.clone()),
            Err(Failure::NotApplicable(reason)) => ("n/a", reason.clone()),
            Err(Failure::Unsupported(reason)) => ("unsupported", reason.clone()),
            Err(Failure::Unconfigured(reason)) if provider::required() => {
                ("fail", format!("not configured: {reason}"))
            }
            Err(Failure::Unconfigured(reason)) => ("skip", reason.clone()),
        };
        eprintln!(
            "[{}] {name}: {status} ({elapsed_ms} ms) {}",
            kind.as_str(),
            redact(&detail)
        );
        record(kind, name, status, &detail, elapsed_ms);
        if status == "fail" {
            failures.push(format!("[{}] {}", kind.as_str(), redact(&detail)));
        }
    }
    assert!(
        failures.is_empty(),
        "{name} failed: {}",
        failures.join("; ")
    );
}

fn record(kind: Kind, scenario: &str, status: &str, detail: &str, elapsed_ms: u64) {
    let Ok(path) = std::env::var(REPORT_ENV) else {
        return;
    };
    let line = json!({
        "provider": kind.as_str(),
        "scenario": scenario,
        "status": status,
        "detail": redact(detail),
        "elapsed_ms": elapsed_ms,
        "engine": loams_sidebar_browser::PINNED_ENGINE_VERSION,
    });
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path);
    match file {
        Ok(mut file) => {
            let _ = writeln!(file, "{line}");
        }
        Err(error) => eprintln!("cannot append to {path}: {error}"),
    }
}

/// Keep the canary and the service token out of the report, whatever a
/// failure message happened to quote.
fn redact(detail: &str) -> String {
    let mut out = detail.to_string();
    let secrets = [
        provider::canary(),
        std::env::var(provider::SERVICE_TOKEN_ENV).ok(),
    ];
    for secret in secrets.into_iter().flatten() {
        if secret.len() >= 8 {
            out = out.replace(&secret, "<redacted>");
        }
    }
    out
}
