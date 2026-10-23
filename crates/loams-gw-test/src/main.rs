//! `loams-gw-test`: lists and runs the KV gateways' harness scenarios.
//!
//! ```text
//! loams-gw-test list
//! loams-gw-test run <scenario>
//! ```
//!
//! `run` runs the scenario on every available backend (TiKV when
//! `LOAMS_TEST_PD` is set) and exits non-zero when their reports differ.

use std::process::ExitCode;

use loams_gw_test::{run_everywhere, runners, scenarios};

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["list"] => {
            println!("scenarios:");
            for s in scenarios::all() {
                println!("  {:<16} {}", s.name(), s.about());
            }
            println!("runners:");
            for r in runners::all() {
                let runners::Status::Planned { milestone } = r.status;
                println!("  {:<16} {} (planned, {milestone})", r.name, r.about);
            }
            ExitCode::SUCCESS
        }
        ["run", name] => {
            let Some(scenario) = scenarios::find(name) else {
                eprintln!("no scenario named {name}; see `loams-gw-test list`");
                return ExitCode::from(2);
            };
            let comparison = run_everywhere(scenario.as_ref()).await;
            for (backend, report) in &comparison.reports {
                println!("[{backend}]\n{report}");
            }
            for backend in &comparison.skipped {
                println!("[{backend}] skipped: not available");
            }
            print!("{}", comparison.describe());
            if comparison.agree() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        _ => {
            eprintln!("usage: loams-gw-test list | loams-gw-test run <scenario>");
            ExitCode::from(2)
        }
    }
}
