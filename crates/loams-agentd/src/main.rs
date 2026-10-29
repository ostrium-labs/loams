//! loams-agentd: Loams Desktop's per-user agent daemon (design §50).
//!
//! Headless only (D782): `run` serves the sessions engine and its harnesses on
//! a loopback RPC port, `mcp` is the stdio MCP shim injected into harness
//! runs, `status` reports a running daemon, `service` manages the per-user
//! service and linger, `install-runtime` installs the versioned binaries, and
//! `version` prints the build.
//! Two hidden entry points serve harnesses: `loams bot-acp` (Loams Bot over
//! ACP) and `--noop-browser` (browser suppression for Antigravity sign-in).

#![cfg_attr(windows, windows_subsystem = "windows")]

mod cli;
mod status;

use std::process::ExitCode;

use clap::Parser;
use cli::{AgentCommand, Cli, Command, LoamsCommand, RunArgs, ServeArgs, ServiceCommand};
use loams_agentd::config::Config;
use loams_agentd::paths::Paths;
use loams_agentd::server::{ServeOptions, serve};
use loams_agentd::{
    EXIT_ALREADY_RUNNING, EXIT_INTERNAL, EXIT_INVALID_CONFIG, discovery, hello, lock, token,
};

/// mimalloc, macOS only: libmalloc never returns the streaming churn's
/// high-water pages, so transient allocation became permanent RSS
/// (docs/memory-plan.md §1). Pinned to mimalloc v2 in the workspace manifest —
/// the crate's default v3 has the same pathology (churn retained as permanent
/// RSS, ~6x glibc's growth on identical workloads, no idle recovery). Linux
/// measured flat on glibc, so it keeps the system allocator.
#[cfg(target_os = "macos")]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// glibc gives threads their own arenas (up to 8 × cores) and on free returns
/// only the top of each heap, so churn freed mid-heap stays resident: a
/// headless engine held 6.6GB in 129 arenas after a day. `malloc_trim` also
/// releases the free pages inside every arena, so long-running modes call it
/// once a minute. Capping arenas instead (`M_ARENA_MAX=2`) reclaimed less and
/// cost ~6x the engine's CPU in arena-lock contention while chats streamed
/// (docs/memory-plan.md).
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn spawn_malloc_trimmer() {
    const PERIOD: std::time::Duration = std::time::Duration::from_secs(60);
    let spawned = std::thread::Builder::new()
        .name("malloc-trim".into())
        .spawn(|| {
            loop {
                std::thread::sleep(PERIOD);
                let started = std::time::Instant::now();
                // SAFETY: malloc_trim takes no pointers and locks each arena
                // itself, so it is safe to call from any thread at any time.
                #[allow(unsafe_code)]
                let released = unsafe { libc::malloc_trim(0) } != 0;
                tracing::debug!(released, elapsed = ?started.elapsed(), "malloc_trim");
            }
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "malloc trimmer not started");
    }
}

fn main() -> ExitCode {
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--noop-browser")) {
        // A harness's `BROWSER` points here so a sign-in page never opens
        // (the Antigravity driver, `loams-agentd-harness` `noop_browser`).
        return ExitCode::SUCCESS;
    }
    #[cfg(windows)]
    attach_parent_console();
    let cli = Cli::parse();
    match cli.command {
        Command::Run(args) => run(&args, None),
        Command::Serve(args) => serve_remote(&args),
        Command::Mcp(config) => {
            // `mcp` owns stdout for the protocol: a single log line on it
            // would corrupt the JSON-RPC stream, so diagnostics go to stderr.
            init_stderr_tracing("warn");
            let mut mcp = loams_agentd_mcp::McpConfig::from_env();
            if mcp.endpoint.port.is_none() {
                // Outside a harness run: find the daemon through discovery.
                mcp.endpoint.port = config
                    .resolve()
                    .and_then(|path| Paths::of_config(&path).ok())
                    .and_then(|paths| discovery::read(&paths))
                    .map(|found| found.port);
            }
            report(
                tokio::runtime::Runtime::new()
                    .map_err(anyhow::Error::from)
                    .and_then(|runtime| runtime.block_on(loams_agentd_mcp::run(mcp))),
            )
        }
        Command::Status(config) => {
            init_stderr_tracing("warn");
            let Some(config) = config.resolve() else {
                eprintln!(
                    "loams-agentd: status needs --config <path> (or {})",
                    cli::CONFIG_ENV
                );
                return ExitCode::from(2);
            };
            report(status::status(&config))
        }
        Command::Service { command } => {
            init_stderr_tracing("warn");
            service(command)
        }
        Command::InstallRuntime(args) => {
            init_stderr_tracing("warn");
            match loams_agentd::runtime_install::install(&args.from, &args.to) {
                Ok(installed) => {
                    println!(
                        "Installed {} in {}.",
                        installed.version,
                        installed.dir.display()
                    );
                    for version in installed.kept_in_use {
                        eprintln!("loams-agentd: {version} is in use and was kept");
                    }
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("loams-agentd: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Command::Agent {
            command: AgentCommand::Conformance(args),
        } => {
            init_stderr_tracing("warn");
            conformance(args)
        }
        Command::Version => {
            println!("loams-agentd {}", hello::VERSION);
            ExitCode::SUCCESS
        }
        Command::Loams {
            command: LoamsCommand::BotAcp,
        } => {
            // `loams bot-acp` speaks ACP on stdout, like `mcp`.
            init_stderr_tracing("warn");
            let code = tokio::runtime::Runtime::new()
                .map_err(anyhow::Error::from)
                .and_then(|runtime| {
                    runtime.block_on(loams_agentd_link::cli::run(vec!["bot-acp".to_owned()]))
                });
            match code {
                Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
                Err(error) => report(Err(error)),
            }
        }
    }
}

/// `agent conformance`: 0 when the package reaches LAPC Core, 1 when it
/// does not, 2 when the run could not happen.
fn conformance(args: cli::ConformanceArgs) -> ExitCode {
    use loams_agentd_marketplace::conformance::{self, CheckStatus, Options, Services, Target};
    let failed = |error: &dyn std::fmt::Display| {
        eprintln!("loams-agentd: {error}");
        ExitCode::from(2)
    };
    let target = match Target::resolve(&args.target, args.package.as_deref()) {
        Ok(target) => target,
        Err(error) => return failed(&error),
    };
    let options = Options {
        node: args.node,
        npm: args.npm,
        work_dir: args.work_dir,
        services: Services {
            pg_url: args.pg_url,
            durable_url: args.durable_url,
            ..Services::default()
        },
        ..Options::default()
    };
    let report = match tokio::runtime::Runtime::new()
        .map_err(|e| e.to_string())
        .and_then(|rt| {
            rt.block_on(conformance::run(&target, &options))
                .map_err(|e| e.to_string())
        }) {
        Ok(report) => report,
        Err(error) => return failed(&error),
    };
    for check in &report.checks {
        let status = match check.status {
            CheckStatus::Pass => "pass",
            CheckStatus::Fail => "FAIL",
            CheckStatus::Skip => "skip",
        };
        eprintln!("{status:4}  {}  {}", check.id, check.title);
        if check.status != CheckStatus::Pass {
            for detail in &check.details {
                eprintln!("        {detail}");
            }
        }
    }
    eprintln!(
        "{} {}: LAPC level {}",
        report.subject.id, report.subject.version, report.level
    );
    let json = report.to_json();
    match &args.out {
        Some(path) => {
            if let Err(error) = std::fs::write(path, &json) {
                return failed(&format!("{}: {error}", path.display()));
            }
            if args.sign {
                match conformance::sign_report(path, None) {
                    Ok(bundle) => eprintln!("signed: {}", bundle.display()),
                    Err(error) => return failed(&error),
                }
            }
        }
        None => print!("{json}"),
    }
    if report.passed() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// A one-shot command's result as an exit code.
fn report(result: anyhow::Result<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("loams-agentd: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// `service …`: what it did on stdout; on failure the error on stderr and
/// exit 1, or 4 for an invalid config.
fn service(command: ServiceCommand) -> ExitCode {
    use loams_agentd::service::{self, Action, ServiceError, linger};
    let (action, config) = match command {
        ServiceCommand::Install(config) => (Action::Install, config),
        ServiceCommand::Uninstall(config) => (Action::Uninstall, config),
        ServiceCommand::Start(config) => (Action::Start, config),
        ServiceCommand::Stop(config) => (Action::Stop, config),
        ServiceCommand::Status(config) => (Action::Status, config),
        ServiceCommand::EnableLinger => return service_result(linger::set(true)),
        ServiceCommand::DisableLinger => return service_result(linger::set(false)),
    };
    let Some(config) = config.resolve() else {
        eprintln!(
            "loams-agentd: service needs --config <path> (or {})",
            cli::CONFIG_ENV
        );
        return ExitCode::from(EXIT_INVALID_CONFIG);
    };
    let result = service::run(action, &config);
    if let Err(ServiceError::Config(error)) = &result {
        eprintln!("loams-agentd: {error}");
        return ExitCode::from(EXIT_INVALID_CONFIG);
    }
    service_result(result)
}

fn service_result(result: Result<String, loams_agentd::service::ServiceError>) -> ExitCode {
    match result {
        Ok(text) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("loams-agentd: {error}");
            ExitCode::FAILURE
        }
    }
}

/// `serve --remote …`: every flag is checked first (a missing one exits 4
/// and names it), then it runs as `run` with the remote listener on
/// (plan SD1 Task 7).
fn serve_remote(args: &ServeArgs) -> ExitCode {
    let flags = args.flags();
    if let Err(error) = flags.settings() {
        eprintln!("loams-agentd: {error}");
        return ExitCode::from(EXIT_INVALID_CONFIG);
    }
    if args.run.config.resolve().is_none() {
        eprintln!(
            "loams-agentd: serve --remote needs --config (or {})",
            cli::CONFIG_ENV
        );
        return ExitCode::from(EXIT_INVALID_CONFIG);
    }
    run(&args.run, Some(&flags))
}

/// `run`: config (exit 4), directory, lock (exit 3), then the server; any
/// other failure exits 70 (design §50 §5.1). On Windows `run --service` is
/// the parent that runs `run --worker` and restarts it. `remote` holds
/// `serve --remote`'s flags; without them `[remote]` decides, and it is off
/// by default.
fn run(args: &RunArgs, remote: Option<&loams_agentd::remote::RemoteFlags>) -> ExitCode {
    let Some(config_path) = args.config.resolve() else {
        eprintln!(
            "loams-agentd: run needs --config <path> (or {})",
            cli::CONFIG_ENV
        );
        return ExitCode::from(EXIT_INVALID_CONFIG);
    };
    #[cfg(windows)]
    if args.service {
        return ExitCode::from(loams_agentd::service::windows::supervise(&config_path));
    }
    let config = match Config::load(&config_path) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("loams-agentd: {error}");
            return ExitCode::from(EXIT_INVALID_CONFIG);
        }
    };
    let mode = args.mode().unwrap_or(config.daemon.mode);
    // The remote listener's TLS files and issuer are checked before the
    // lock: a bad one is a config error (exit 4), like a bad config.toml.
    let remote = match loams_agentd::remote::resolve(remote, &config.remote).and_then(|settings| {
        settings
            .map(|settings| {
                loams_agentd::remote::options(
                    &settings,
                    loams_agentd_rpc::Limits::default().auth_failure_delay,
                )
            })
            .transpose()
    }) {
        Ok(remote) => remote,
        Err(error) => {
            eprintln!("loams-agentd: {error}");
            return ExitCode::from(EXIT_INVALID_CONFIG);
        }
    };
    let paths = match Paths::for_config(&config_path) {
        Ok(paths) => paths,
        Err(error) => {
            eprintln!(
                "loams-agentd: cannot create the daemon directory beside {}: {error}",
                config_path.display()
            );
            return ExitCode::from(EXIT_INTERNAL);
        }
    };
    // The lock comes before the token, the listener, the log file and any
    // store: a second instance changes nothing.
    let lock = match lock::acquire(&paths) {
        Ok(lock) => lock,
        Err(lock::LockError::Held { holder }) => return already_running(&paths, &holder),
        Err(lock::LockError::Io(error)) => {
            eprintln!(
                "loams-agentd: cannot take {}: {error}",
                paths.lock().display()
            );
            return ExitCode::from(EXIT_INTERNAL);
        }
    };

    init_run_tracing(&paths);
    if std::env::var_os(cli::CONFIG_ENV).is_some_and(|value| !value.is_empty()) {
        tracing::info!(config = %config_path.display(), "config from {}", cli::CONFIG_ENV);
    }
    // Service and child launches have no visible stderr. Mirror the panic
    // location and backtrace into the daemon log.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!(panic = %info,
            backtrace = %std::backtrace::Backtrace::force_capture(),
            "application panic");
        default_hook(info);
    }));
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    spawn_malloc_trimmer();

    let result = tokio::runtime::Runtime::new()
        .map_err(anyhow::Error::from)
        .and_then(|runtime| {
            runtime.block_on(serve(ServeOptions {
                paths,
                config,
                mode,
                default_harness: harness_from_env(),
                remote,
            }))
        });
    drop(lock);
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(error = format!("{error:#}"), "loams-agentd failed");
            eprintln!("loams-agentd: {error:#}");
            ExitCode::from(EXIT_INTERNAL)
        }
    }
}

/// Another daemon holds the lock: confirm it with `Hello` (it may still be
/// starting, so discovery is retried briefly), report it and exit 3.
fn already_running(paths: &Paths, holder: &str) -> ExitCode {
    const WAIT: std::time::Duration = std::time::Duration::from_secs(5);
    let confirmed = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()
        .and_then(|runtime| {
            runtime.block_on(async {
                let deadline = tokio::time::Instant::now() + WAIT;
                loop {
                    if let Some(found) = discovery::read(paths)
                        && let Ok(token) = token::read(paths)
                        && let Ok(reply) = hello::probe(found.port, token.expose()).await
                        && reply.pid == found.pid
                    {
                        return Some((reply, found.port));
                    }
                    if tokio::time::Instant::now() > deadline {
                        return None;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            })
        });
    match confirmed {
        Some((reply, port)) => eprintln!(
            "loams-agentd: already running (pid {}); version {}, port {port}",
            reply.pid, reply.version
        ),
        None => {
            eprintln!("loams-agentd: already running (pid {holder}); it has not answered Hello yet")
        }
    }
    ExitCode::from(EXIT_ALREADY_RUNNING)
}

/// `filter` with the directives that keep tokens out of the log, which no
/// `RUST_LOG` can turn back on (`loams_agentd_rpc::SECRET_SAFE_LOG_DIRECTIVES`).
fn secret_safe(mut filter: tracing_subscriber::EnvFilter) -> tracing_subscriber::EnvFilter {
    for directive in loams_agentd_rpc::SECRET_SAFE_LOG_DIRECTIVES {
        match directive.parse() {
            Ok(directive) => filter = filter.add_directive(directive),
            Err(error) => eprintln!("loams-agentd: bad log directive {directive}: {error}"),
        }
    }
    filter
}

/// One-shot and stdio commands: stderr only, `RUST_LOG` overrides `default`.
fn init_stderr_tracing(default: &str) {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    let filter = secret_safe(
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| default.into()),
    );
    tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(std::io::stderr),
        )
        .init();
}

/// `run` logs at info to stdout and mirrors it to `<dir>/logs/agentd.log`: a
/// daemon started by a service manager or by Electron has no visible stdout.
/// (What a supervisor captures from stdout and stderr goes to its own
/// `agentd.stdout.log`, ruling T6-8, so the two never write one file.)
/// loro's internal block-encode diagnostics log at info on every snapshot
/// export, enough to fill a disk; they are quieted (RUST_LOG overrides).
fn init_run_tracing(paths: &Paths) {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    let filter = secret_safe(
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| "info,loro_internal=warn,loro=warn".into()),
    );
    let registry = tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer());
    match open_log_file_in(&paths.logs(), "agentd") {
        Some(file) => registry
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(std::sync::Arc::new(file)),
            )
            .init(),
        None => registry.init(),
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn attach_parent_console() {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE,
        STD_OUTPUT_HANDLE, SetStdHandle,
    };

    // The GUI subsystem prevents Explorer from creating a console at startup.
    // Reuse an existing parent's console for CLI output and cargo run, without
    // allocating one. Attach before Clap so help and argument errors work too.
    // Preserve redirected pipes/files: attaching may replace standard handles.
    // SAFETY: GetStdHandle, AttachConsole and SetStdHandle take no pointers; only handles this
    // process already holds are restored.
    unsafe {
        let saved = [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE]
            .map(|id| (id, GetStdHandle(id)));
        if AttachConsole(ATTACH_PARENT_PROCESS) != 0 {
            for (id, handle) in saved {
                if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
                    SetStdHandle(id, handle);
                }
            }
        }
    }
}

/// `LOAMS_DESKTOP_HARNESS` (kebab-case id) picks the default harness for chats without a
/// config row — `mock` powers the e2e smoke; default `claude-code`.
fn harness_from_env() -> loams_agentd_sessions::HarnessId {
    match std::env::var("LOAMS_DESKTOP_HARNESS")
        .as_deref()
        .map(str::trim)
    {
        Ok("mock") => loams_agentd_sessions::HarnessId::Mock,
        Ok("codex") => loams_agentd_sessions::HarnessId::Codex,
        Ok("devin") => loams_agentd_sessions::HarnessId::Devin,
        Ok("grok") => loams_agentd_sessions::HarnessId::Grok,
        Ok("hermes") => loams_agentd_sessions::HarnessId::Hermes,
        Ok("pi") => loams_agentd_sessions::HarnessId::Pi,
        Ok("antigravity") => loams_agentd_sessions::HarnessId::Antigravity,
        Ok("goose") => loams_agentd_sessions::HarnessId::Goose,
        _ => loams_agentd_sessions::HarnessId::ClaudeCode,
    }
}

/// `{dir}/{name}.log`, the previous launch preserved as `.old`.
///
/// The returned file holds an exclusive `flock` for the process lifetime:
/// rotate-on-launch is only safe when nothing is still WRITING the current
/// file. On 2026-08-04 a dev build launched twice next to the running
/// installed app — the first rename put the daemon's live log at `.old`, the
/// second unlinked it entirely, and the daemon spent the rest of the incident
/// logging to an orphaned inode. A launch that finds the canonical file
/// locked logs to `{name}.{pid}.log` instead; the next lock-holding
/// launch sweeps pid-suffixed files older than a week.
fn open_log_file_in(dir: &std::path::Path, name: &str) -> Option<std::fs::File> {
    std::fs::create_dir_all(dir).ok()?;
    let path = dir.join(format!("{name}.log"));
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        // Probe the CURRENT inode for a live writer before touching it.
        let preexisting = path.exists();
        let existing = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .ok()?;
        // SAFETY: flock on a descriptor this function owns for the call; it touches no memory.
        #[allow(unsafe_code)]
        let rc = unsafe { libc::flock(existing.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 {
            // A live process owns the canonical log — leave it alone.
            return std::fs::File::create(dir.join(format!("{name}.{}.log", std::process::id())))
                .ok();
        }
        // No live writer: rotate, create fresh, and lock it as ours. (The
        // probe's flock dies with `existing`; a first-ever launch has nothing
        // to rotate — the probe itself created the empty file.)
        drop(existing);
        if preexisting {
            let _ = std::fs::rename(&path, dir.join(format!("{name}.log.old")));
        }
        let file = std::fs::File::create(&path).ok()?;
        // SAFETY: flock on a descriptor this function owns for the call; it touches no memory.
        #[allow(unsafe_code)]
        unsafe {
            libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB)
        };
        sweep_stale_pid_logs(dir, name);
        Some(file)
    }
    #[cfg(not(unix))]
    {
        let _ = std::fs::rename(&path, dir.join(format!("{name}.log.old")));
        std::fs::File::create(&path).ok()
    }
}

#[cfg(all(test, unix))]
mod log_file_tests {
    use super::open_log_file_in;

    #[test]
    fn second_launch_never_rotates_a_live_processes_log() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        // First launch owns the canonical file and keeps writing.
        let first = open_log_file_in(dir, "agentd").expect("first log");
        assert!(dir.join("agentd.log").is_file());
        // Second launch while the first is alive: canonical file untouched,
        // pid-suffixed overflow file instead (the 2026-08-04 clobber).
        let second = open_log_file_in(dir, "agentd").expect("second log");
        let pid_path = dir.join(format!("agentd.{}.log", std::process::id()));
        assert!(pid_path.is_file(), "expected pid-suffixed overflow log");
        assert!(
            !dir.join("agentd.log.old").exists(),
            "live canonical log must not be rotated away"
        );
        drop(second);
        // After the owner exits, a fresh launch rotates normally.
        drop(first);
        let third = open_log_file_in(dir, "agentd").expect("third log");
        assert!(dir.join("agentd.log.old").is_file(), "rotation resumes");
        drop(third);
    }
}

/// Delete `{name}.{pid}.log` overflow files older than a week — they only
/// exist when a second instance raced a live one for the canonical log.
#[cfg(unix)]
fn sweep_stale_pid_logs(dir: &std::path::Path, name: &str) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let prefix = format!("{name}.");
    let week = std::time::Duration::from_secs(7 * 24 * 60 * 60);
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(middle) = name
            .strip_prefix(&prefix)
            .and_then(|rest| rest.strip_suffix(".log"))
        else {
            continue;
        };
        if !middle.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > week);
        if stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}
