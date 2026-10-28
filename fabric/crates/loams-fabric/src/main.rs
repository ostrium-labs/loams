//! `loams-fabric`: see the library's documentation.

use std::io::Write as _;
use std::process::ExitCode;

use clap::Parser as _;
use loams_fabric::cli::{Cli, ConnectArgs, HouseArgs, Role};
use loams_fabric::{connect, house};
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
        Role::Connect(args) => run_connect(&args),
    }
}

/// `connect`: exit 0 on SIGTERM, 75 when fenced by a newer process, 78 on a
/// bad instance file or flag, 69 when the bucket cannot be reached.
fn run_connect(args: &ConnectArgs) -> ExitCode {
    let scrubber = loams_connector_sdk::Scrubber::new();
    let subscriber = connect::log::ScrubbingSubscriber::stderr(scrubber.clone());
    if tracing::subscriber::set_global_default(subscriber).is_err() {
        eprintln!("loams-fabric connect: could not install the log subscriber");
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(err) => {
            eprintln!("loams-fabric connect: the runtime: {err}");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(async move {
        let running = match connect::Connect::start(args, scrubber.clone()).await {
            Ok(r) => r,
            Err(err) => {
                let message = scrubber.scrub(&err.message);
                eprintln!("loams-fabric connect: refusing to start:\n{message}");
                return ExitCode::from(err.code as u8);
            }
        };
        println!(
            "loams-fabric connect: epoch {}, admin API on http://{}",
            running.epoch(),
            running.admin.local_addr()
        );
        let _ = std::io::stdout().flush();
        match running.run(wait_for_stop()).await {
            connect::Exit::Stopped => ExitCode::SUCCESS,
            connect::Exit::Fenced(why) => {
                eprintln!("loams-fabric connect: fenced: {why}");
                ExitCode::from(75)
            }
        }
    })
}

fn run_house(args: &HouseArgs) -> Result<(), HouseError> {
    let file = match &args.config {
        Some(path) => HouseFile::load(path)?,
        None => HouseFile::default(),
    };
    let settings = house::resolve(args, file)?;
    // The startup gate (design §54 §4.3, UP1 Task 4): refuse a data
    // directory this release does not read, before anything listens.
    if let Err(err) =
        loams_common::formats::check_dir_stamp(&settings.data_dir, loams_common::formats::HOUSE_DIR)
    {
        eprintln!("loams-fabric house: refusing to start: format_unreadable: {err}");
        std::process::exit(78);
    }
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
        match running.native_addrs() {
            (None, None) => {}
            (plain, tls) => eprintln!(
                "loams-fabric house: ClickHouse native protocol on {}",
                plain
                    .map(|a| format!("tcp://{a}"))
                    .into_iter()
                    .chain(tls.map(|a| format!("tls://{a}")))
                    .collect::<Vec<_>>()
                    .join(" and ")
            ),
        }
        if let Some(admin) = running.admin_addr() {
            eprintln!("loams-fabric house: metrics on http://{admin}/metrics");
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
        let tls = running
            .tls_addr()
            .map(|addr| format!(" and https://{addr}"))
            .unwrap_or_default();
        println!(
            "loams-fabric house: ClickHouse HTTP on http://{}{tls} (workers={}, sandbox={})",
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
