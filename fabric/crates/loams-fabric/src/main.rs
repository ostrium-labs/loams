//! `loams-fabric`: see the library's documentation.

use std::io::Write as _;
use std::process::ExitCode;

use clap::Parser as _;
use loams_fabric::cli::{Cli, HouseArgs, Role};
use loams_fabric::house;
use loams_house::{HouseError, HouseFile, SandboxMode, WorkersMode};

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.role {
        Role::House(args) => match run_house(&args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("loams-fabric house: {}", err.message());
                ExitCode::FAILURE
            }
        },
    }
}

fn run_house(args: &HouseArgs) -> Result<(), HouseError> {
    let file = match &args.config {
        Some(path) => HouseFile::load(path)?,
        None => HouseFile::default(),
    };
    let settings = house::resolve(args, file)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| {
            HouseError::from(loams_house::ChError::network_error(format!(
                "the runtime: {err}"
            )))
        })?;
    runtime.block_on(async move {
        let notes = house::not_served_yet(&settings);
        let key = settings.local_key.clone();
        let workers = settings.workers;
        // In-process workers have no sandbox, whatever --sandbox says.
        let mode = match workers {
            WorkersMode::Inproc => SandboxMode::None,
            WorkersMode::Process => settings.sandbox,
        };
        let running = house::start(settings).await?;
        for note in notes {
            eprintln!("loams-fabric house: {note}");
        }
        if let Some(path) = key {
            eprintln!(
                "loams-fabric house: single-node user `{}`; its password is in {}",
                house::LOCAL_USER,
                path.display()
            );
        }
        // The one line on stdout, which supervisors (the desktop, tests) read for
        // the address when the port was 0.
        println!(
            "loams-fabric house: ClickHouse HTTP on http://{} (workers={}, sandbox={})",
            running.local_addr(),
            workers.as_str(),
            mode
        );
        let _ = std::io::stdout().flush();
        wait_for_stop().await;
        running.shutdown();
        Ok(())
    })
}

/// SIGINT or SIGTERM.
async fn wait_for_stop() {
    use tokio::signal::unix::{SignalKind, signal};
    let (Ok(mut term), Ok(mut int)) = (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
    ) else {
        // No signal handling: run until killed.
        std::future::pending::<()>().await;
        return;
    };
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
}
