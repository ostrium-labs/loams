//! `loams-house-mine`: see the library's documentation.

use std::process::ExitCode;

use clap::Parser as _;
use loams_house_mine::cli::Cli;

fn main() -> ExitCode {
    match Cli::parse().command.run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("loams-house-mine: {err:#}");
            ExitCode::FAILURE
        }
    }
}
