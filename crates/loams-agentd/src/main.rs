//! loams-agentd: Loams Desktop's per-user agent daemon (design §50).
//!
//! Headless only (D782): `run` serves the sessions engine and its harnesses on
//! the loopback IPC port, `mcp` is the stdio MCP shim injected into harness
//! runs, `status` reports a running daemon, and `version` prints the build.
//! Two hidden entry points serve harnesses: `loams bot-acp` (Loams Bot over
//! ACP) and `--noop-browser` (browser suppression for Antigravity sign-in).

#![cfg_attr(windows, windows_subsystem = "windows")]

mod paths;
mod status;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "loams-agentd",
    about = "Loams Desktop's per-user agent daemon",
    disable_version_flag = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the daemon in the foreground: the sessions engine, its harnesses
    /// and the IPC server (local-only).
    Run,
    /// Serve the MCP (Model Context Protocol) server on stdin/stdout,
    /// proxying to the running daemon's IPC. Harness runs get it injected.
    /// Logs go to stderr; stdout is the protocol.
    Mcp,
    /// Show the data directory and whether a daemon is running.
    Status,
    /// Print the daemon's version.
    Version,
    /// Loams integration used by the Loams Bot harness (`loams bot-acp`).
    #[command(hide = true)]
    Loams {
        #[command(subcommand)]
        command: LoamsCommand,
    },
}

/// The link crate's CLI also has `login`, `logout`, `status`, `bot` and
/// `mock`; the daemon exposes only the entry point the Loams Bot harness
/// launches, so anything else is a usage error (plan DD1 ruling T1-2).
#[derive(Subcommand)]
enum LoamsCommand {
    /// Speak ACP on stdin/stdout as the Loams Bot agent.
    BotAcp,
}

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

fn main() -> anyhow::Result<()> {
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--noop-browser")) {
        // A harness's `BROWSER` points here so a sign-in page never opens
        // (the Antigravity driver, `loams-agentd-harness` `noop_browser`).
        return Ok(());
    }
    #[cfg(windows)]
    attach_parent_console();
    let cli = Cli::parse();
    // Long-running modes log at info, one-shot CLI commands at warn (RUST_LOG
    // overrides either).
    // loro's internal block-encode diagnostics log at info and flood
    // journald on every snapshot export — enough to fill a disk on a
    // long-running headless host. Quiet them by default (RUST_LOG still
    // overrides the whole filter).
    let long_running = matches!(&cli.command, Command::Run);
    // `loams-agentd loams bot-acp` speaks ACP on stdout, like `mcp`.
    let stdout_is_protocol = matches!(&cli.command, Command::Mcp | Command::Loams { .. });
    let default_filter = if long_running {
        "info,loro_internal=warn,loro=warn"
    } else {
        "warn"
    };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| default_filter.into());
    // `run` mirrors stdout logging to {data_dir}/logs: a daemon started by a
    // service manager or by Electron has no visible stdout. One file per
    // launch, the previous launch kept as `.old`.
    let log_file = if long_running {
        open_log_file("run")
    } else {
        None
    };
    {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;
        // `loams-agentd mcp` owns stdout for the protocol: a single log line on it
        // would corrupt the JSON-RPC stream, so its diagnostics go to stderr.
        if stdout_is_protocol {
            tracing_subscriber::registry()
                .with(filter)
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_ansi(false)
                        .with_writer(std::io::stderr),
                )
                .init();
        } else {
            let registry = tracing_subscriber::registry()
                .with(filter)
                .with(tracing_subscriber::fmt::layer());
            match log_file {
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
    }

    if long_running {
        // Finder launches have no visible stderr. Mirror the panic location
        // and backtrace into the same rotating log as engine diagnostics.
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            tracing::error!(panic = %info,
                backtrace = %std::backtrace::Backtrace::force_capture(),
                "application panic");
            default_hook(info);
        }));
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        spawn_malloc_trimmer();
    }

    match cli.command {
        Command::Run => {
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(async {
                let engine = loams_agentd_sessions::Engine::new(engine_config_from_env());
                engine.run().await
            })
        }
        Command::Mcp => {
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(loams_agentd_mcp::run(
                loams_agentd_mcp::McpConfig::from_env(),
            ))
        }
        Command::Status => status::status(&engine_config_from_env()),
        Command::Version => {
            println!("loams-agentd {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Command::Loams {
            command: LoamsCommand::BotAcp,
        } => {
            let runtime = tokio::runtime::Runtime::new()?;
            let code = runtime.block_on(loams_agentd_link::cli::run(vec!["bot-acp".to_owned()]))?;
            std::process::exit(code);
        }
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

/// The env-resolved engine configuration shared by `run` and `status`.
fn engine_config_from_env() -> loams_agentd_sessions::EngineConfig {
    loams_agentd_sessions::EngineConfig {
        data_dir: paths::data_dir(),
        ipc_port: std::env::var("LOAMS_DESKTOP_IPC_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(27654),
        default_harness: harness_from_env(),
        terminal_shell: None,
        tool_outputs: Default::default(),
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
        Ok("cursor") => loams_agentd_sessions::HarnessId::Cursor,
        Ok("devin") => loams_agentd_sessions::HarnessId::Devin,
        Ok("grok") => loams_agentd_sessions::HarnessId::Grok,
        Ok("hermes") => loams_agentd_sessions::HarnessId::Hermes,
        Ok("pi") => loams_agentd_sessions::HarnessId::Pi,
        Ok("antigravity") => loams_agentd_sessions::HarnessId::Antigravity,
        _ => loams_agentd_sessions::HarnessId::ClaudeCode,
    }
}

/// `{data_dir}/logs/loams-agentd-{mode}.log`, previous launch preserved as `.old`.
///
/// The returned file holds an exclusive `flock` for the process lifetime:
/// rotate-on-launch is only safe when nothing is still WRITING the current
/// file. On 2026-08-04 a dev build launched twice next to the running
/// installed app — the first rename put the daemon's live log at `.old`, the
/// second unlinked it entirely, and the daemon spent the rest of the incident
/// logging to an orphaned inode (an entire day of sync diagnostics gone at
/// the exact moment they were needed). A launch that finds the canonical file
/// locked logs to `loams-agentd-{mode}.{pid}.log` instead; the next lock-holding
/// launch sweeps pid-suffixed files older than a week.
fn open_log_file(mode: &str) -> Option<std::fs::File> {
    let dir = paths::data_dir().join("logs");
    open_log_file_in(&dir, mode)
}

/// Dir-parameterized body of [`open_log_file`] (unit-testable without env).
fn open_log_file_in(dir: &std::path::Path, mode: &str) -> Option<std::fs::File> {
    std::fs::create_dir_all(dir).ok()?;
    let path = dir.join(format!("loams-agentd-{mode}.log"));
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
            return std::fs::File::create(
                dir.join(format!("loams-agentd-{mode}.{}.log", std::process::id())),
            )
            .ok();
        }
        // No live writer: rotate, create fresh, and lock it as ours. (The
        // probe's flock dies with `existing`; a first-ever launch has nothing
        // to rotate — the probe itself created the empty file.)
        drop(existing);
        if preexisting {
            let _ = std::fs::rename(&path, dir.join(format!("loams-agentd-{mode}.log.old")));
        }
        let file = std::fs::File::create(&path).ok()?;
        // SAFETY: flock on a descriptor this function owns for the call; it touches no memory.
        #[allow(unsafe_code)]
        unsafe {
            libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB)
        };
        sweep_stale_pid_logs(dir, mode);
        Some(file)
    }
    #[cfg(not(unix))]
    {
        let _ = std::fs::rename(&path, dir.join(format!("loams-agentd-{mode}.log.old")));
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
        let first = open_log_file_in(dir, "run").expect("first log");
        assert!(dir.join("loams-agentd-run.log").is_file());
        // Second launch while the first is alive: canonical file untouched,
        // pid-suffixed overflow file instead (the 2026-08-04 clobber).
        let second = open_log_file_in(dir, "run").expect("second log");
        let pid_path = dir.join(format!("loams-agentd-run.{}.log", std::process::id()));
        assert!(pid_path.is_file(), "expected pid-suffixed overflow log");
        assert!(
            !dir.join("loams-agentd-run.log.old").exists(),
            "live canonical log must not be rotated away"
        );
        drop(second);
        // After the owner exits, a fresh launch rotates normally.
        drop(first);
        let third = open_log_file_in(dir, "run").expect("third log");
        assert!(
            dir.join("loams-agentd-run.log.old").is_file(),
            "rotation resumes"
        );
        drop(third);
    }
}

/// Delete `loams-agentd-{mode}.{pid}.log` overflow files older than a week — they
/// only exist when a second instance raced a live one for the canonical log.
#[cfg(unix)]
fn sweep_stale_pid_logs(dir: &std::path::Path, mode: &str) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let prefix = format!("loams-agentd-{mode}.");
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
