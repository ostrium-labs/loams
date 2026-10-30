//! `loams migrate` (design §54 §6, D927; UP1 Task 7): the one-shot service
//! that runs before the engine. `run` migrates the stack's stores to this
//! binary's release, `plan` says what `run` would do, `status` shows the stack
//! record and the ledger, and `verify` probes every product.
//!
//! Exit codes: 0 done; 1 internal; 2 usage; 65 (`EX_DATAERR`) a migration
//! failed (the ledger says which); 75 (`EX_TEMPFAIL`) another run holds the
//! lease; 78 (`EX_CONFIG`) refused (`upgrade_step_not_adjacent`,
//! `snapshot_required`, `stack_unrecognised`, …).

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use clap::{Args, Subcommand, ValueEnum};
use loams::MetaBackend;
use loams_common::meta::MetaStore;
use loams_meta::{MetaClient, MetaClientConfig, MetaConfig, MetaNode, Router, SystemClock};
use loams_migrate::migrations::MIGRATIONS;
use loams_migrate::verify::{Probe, verify_stores};
use loams_migrate::{Cx, Ledger, RunError, RunOptions, Runner, UpgradePath};
use loams_store::Store;
use serde_json::json;

/// `loams migrate <command>`.
#[derive(Debug, Subcommand)]
// Parsed once per process: the size of a variant does not matter.
#[allow(clippy::large_enum_variant)]
pub(crate) enum MigrateCommand {
    /// Migrate the stack's stores to this release (or `--target-release`).
    Run {
        #[command(flatten)]
        stores: StoreArgs,
        /// The release to migrate to [default: this binary's].
        #[arg(long)]
        target_release: Option<String>,
        /// Discard a failed migration's partial writes, then exit.
        #[arg(long, value_name = "ID")]
        abandon: Option<String>,
        /// Run a point-of-no-return migration without a recorded snapshot.
        #[arg(long)]
        no_snapshot_i_accept_data_loss: bool,
        #[arg(long, value_enum, default_value_t = Output::Text)]
        output: Output,
    },
    /// What `run` would do, read-only.
    Plan {
        #[command(flatten)]
        stores: StoreArgs,
        #[arg(long)]
        target_release: Option<String>,
        #[arg(long, value_enum, default_value_t = Output::Json)]
        output: Output,
    },
    /// The stack record and the ledger, read-only.
    Status {
        #[command(flatten)]
        stores: StoreArgs,
        #[arg(long, value_enum, default_value_t = Output::Json)]
        output: Output,
    },
    /// Snapshot the Loams-owned stores (writers stopped) into `<to>/<id>/`,
    /// and record it as a GC root.
    Snapshot {
        #[command(flatten)]
        stores: StoreArgs,
        /// The snapshots directory (mode 0700).
        #[arg(long)]
        to: PathBuf,
        /// agentd's data directory, when the stack runs it.
        #[arg(long)]
        agentd_dir: Option<PathBuf>,
        #[arg(long)]
        target_release: Option<String>,
        /// The snapshot's id (a ULID), when a helper (BR) wrote into
        /// `<to>/<id>/tikv` first.
        #[arg(long)]
        id: Option<String>,
        /// How TiKV was backed up into `<to>/<id>/tikv` (by the CLI's
        /// helper): `br` or `cold`.
        #[arg(long, requires = "tikv_version")]
        tikv: Option<String>,
        /// The TiKV version the backup was taken at (`v8.5.8`).
        #[arg(long)]
        tikv_version: Option<String>,
        /// How long the drain may take, in seconds.
        #[arg(long, default_value_t = 300)]
        drain_timeout_secs: u64,
        #[arg(long, value_enum, default_value_t = Output::Json)]
        output: Output,
    },
    /// Restore the Loams-owned stores from a snapshot directory (the stack
    /// stopped). Refuses an incomplete or corrupt snapshot.
    Restore {
        #[command(flatten)]
        stores: StoreArgs,
        /// The snapshot's directory (`<snapshots>/<id>`).
        #[arg(long)]
        from: PathBuf,
        #[arg(long)]
        agentd_dir: Option<PathBuf>,
        /// The version of the fresh TiKV cluster the CLI's helper restored
        /// the snapshot's BR backup (or cold copy) into (`v8.5.8`).
        #[arg(long)]
        tikv_version: Option<String>,
        #[arg(long, value_enum, default_value_t = Output::Json)]
        output: Output,
    },
    /// List or prune snapshots.
    Snapshots {
        #[command(subcommand)]
        command: SnapshotsCommand,
    },
    /// The read probes of every product (and the engine's, with --engine).
    Verify {
        #[command(flatten)]
        stores: StoreArgs,
        /// The engine's base URL, for `/ready` and `GetInstance`.
        #[arg(long)]
        engine: Option<String>,
        #[arg(long)]
        target_release: Option<String>,
        #[arg(long, value_enum, default_value_t = Output::Json)]
        output: Output,
    },
}

/// `loams migrate snapshots …`.
#[derive(Debug, Subcommand)]
// Parsed once per process: the size of a variant does not matter.
#[allow(clippy::large_enum_variant)]
pub(crate) enum SnapshotsCommand {
    /// Every snapshot under the directory: ids, sizes and digests only.
    List {
        #[arg(long)]
        dir: PathBuf,
    },
    /// Remove a snapshot: its GC root, then its files.
    Prune {
        #[command(flatten)]
        stores: StoreArgs,
        #[arg(long)]
        dir: PathBuf,
        #[arg(long)]
        id: String,
    },
}

/// The stores of one stack (the same flags as the engine's).
#[derive(Debug, Args)]
pub(crate) struct StoreArgs {
    /// The metastore: tikv://<pd>[,<pd>]/<keyspace> [default: the embedded
    /// store in --data-dir].
    #[arg(long, value_parser = MetaBackend::parse)]
    meta: Option<MetaBackend>,
    /// The bucket [default: file://<data-dir>/bucket].
    #[arg(long)]
    bucket: Option<String>,
    /// The engine's data directory (the embedded metastore, Durable's SQLite).
    #[arg(long)]
    pub(crate) data_dir: PathBuf,
    /// Live's store, `tikv://<pd>[,<pd>]/<keyspace>` (Live migrations only).
    #[arg(long)]
    live_store: Option<String>,
    /// The House data directory.
    #[arg(long)]
    house_dir: Option<PathBuf>,
    /// `pg-control`'s data directory.
    #[arg(long)]
    pg_control_dir: Option<PathBuf>,
    /// The release the stack says it runs (its LOAMS_VERSION): adopting a
    /// preview.2 stack needs it.
    #[arg(long)]
    declared_release: Option<String>,
    /// Loams Postgres's pageserver management API (`http://pageserver:9898`):
    /// a snapshot drains the safekeeper and branches every timeline;
    /// `0001-safekeeper-volume` drains before the safekeeper is recreated.
    #[arg(long, requires = "safekeeper")]
    pageserver: Option<String>,
    /// Loams Postgres's safekeeper HTTP API (`http://safekeeper1:7676`).
    #[arg(long, requires = "pageserver")]
    safekeeper: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum Output {
    Text,
    Json,
}

/// The release this binary migrates to by default.
fn this_release() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// A metastore handle and what keeps it alive.
pub(crate) struct OpenStores {
    pub(crate) cx: Cx,
    node: Option<MetaNode>,
}

impl OpenStores {
    pub(crate) async fn close(self) {
        if let Some(node) = self.node
            && let Err(err) = node.shutdown().await
        {
            tracing::warn!(%err, "stopping the embedded metastore");
        }
    }
}

fn bucket_url(args: &StoreArgs) -> Result<String, String> {
    if let Some(bucket) = &args.bucket {
        return Ok(bucket.clone());
    }
    let dir = args.data_dir.join("bucket");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let dir = dir
        .canonicalize()
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    url::Url::from_directory_path(&dir)
        .map(String::from)
        .map_err(|()| format!("{} is not an absolute path", dir.display()))
}

pub(crate) async fn open(args: &StoreArgs) -> Result<OpenStores, String> {
    let store = Store::from_url(&bucket_url(args)?, Vec::<(String, String)>::new())
        .map_err(|e| format!("the bucket: {e}"))?;
    let (meta, node): (Arc<dyn MetaStore>, Option<MetaNode>) =
        match args.meta.clone().unwrap_or_default() {
            MetaBackend::Raft => {
                let node = MetaNode::start(
                    MetaConfig::new(1, args.data_dir.join("meta"), store.clone()),
                    &Router::new(),
                )
                .await
                .map_err(|e| format!("the embedded metastore: {e}"))?;
                node.initialize([1])
                    .await
                    .map_err(|e| format!("the embedded metastore: {e}"))?;
                node.wait_for_leader(std::time::Duration::from_secs(30))
                    .await
                    .map_err(|e| format!("the embedded metastore: {e}"))?;
                let client = MetaClient::new(
                    node.clone(),
                    Vec::new(),
                    Arc::new(SystemClock),
                    MetaClientConfig::default(),
                );
                (Arc::new(client), Some(node))
            }
            #[cfg(feature = "tikv")]
            MetaBackend::Tikv(config) => (
                Arc::new(
                    loams_meta_tikv::TikvMeta::open(config)
                        .await
                        .map_err(|e| format!("the TiKV metastore: {e}"))?,
                ),
                None,
            ),
        };
    let mut cx = Cx::new(meta);
    cx.bucket = Some(store);
    cx.data_dir = Some(args.data_dir.clone());
    cx.house_dir = args.house_dir.clone();
    cx.pg_control_dir = args.pg_control_dir.clone();
    cx.declared_release = args.declared_release.clone();
    cx.postgres = args.pageserver.clone().zip(args.safekeeper.clone());
    cx.live = open_live(args.live_store.as_deref()).await?;
    Ok(OpenStores { cx, node })
}

#[cfg(feature = "live-tikv")]
async fn open_live(url: Option<&str>) -> Result<Option<loams_kv::Store>, String> {
    let Some(url) = url else {
        return Ok(None);
    };
    let rest = url
        .strip_prefix("tikv://")
        .ok_or_else(|| format!("--live-store {url:?}: expected tikv://<pd>[,<pd>]/<keyspace>"))?;
    let (hosts, keyspace) = rest
        .split_once('/')
        .ok_or_else(|| format!("--live-store {url:?}: the keyspace is missing"))?;
    let pd = hosts.split(',').map(str::to_string).collect();
    let config = loams_kv::StoreConfig::Tikv(loams_tikv::TikvConfig::new(pd, keyspace));
    loams_kv::Store::open(config)
        .await
        .map(Some)
        .map_err(|e| format!("Live's store: {e}"))
}

#[cfg(not(feature = "live-tikv"))]
async fn open_live(url: Option<&str>) -> Result<Option<loams_kv::Store>, String> {
    match url {
        None => Ok(None),
        Some(url) => Err(format!(
            "--live-store {url:?}: this loams was built without the live-tikv feature"
        )),
    }
}

/// The lease holder of this process.
fn holder() -> String {
    let host = std::env::var("HOSTNAME").unwrap_or_else(|_| "migrate".to_string());
    format!(
        "migrate-{host}-{}-{}",
        std::process::id(),
        ulid::Ulid::generate()
    )
}

/// The exit code of a run's error.
fn exit_of(err: &RunError) -> ExitCode {
    match err {
        RunError::Refused { .. } => ExitCode::from(78),
        RunError::Failed { .. } => ExitCode::from(65),
        RunError::Ledger(loams_migrate::ledger::Error::LeaseHeld { .. }) => ExitCode::from(75),
        RunError::Crashed(_) | RunError::Ledger(_) => ExitCode::FAILURE,
    }
}

fn print(output: Output, value: &serde_json::Value, text: impl FnOnce() -> String) {
    match output {
        Output::Json => println!("{value}"),
        Output::Text => println!("{}", text()),
    }
}

/// Runs Resonate's own migrations on the Durable SQLite store, if there is
/// one (the `durable-resonate` step of §54 §6.1): serving never migrates it.
#[cfg(feature = "durable")]
async fn durable_resonate(data_dir: &Path) -> Result<bool, String> {
    let path = data_dir.join("durable").join("default.db");
    if !path.exists() {
        return Ok(false);
    }
    loams_durable::DurableServer::migrate(loams_durable::DurableStore::Sqlite { path })
        .await
        .map(|()| true)
        .map_err(|e| format!("the durable store's migrations: {e}"))
}

#[cfg(not(feature = "durable"))]
async fn durable_resonate(_data_dir: &Path) -> Result<bool, String> {
    Ok(false)
}

pub(crate) async fn run(command: MigrateCommand) -> ExitCode {
    let started = Instant::now();
    let stores = match &command {
        MigrateCommand::Snapshots {
            command: SnapshotsCommand::List { dir },
        } => return list_snapshots(dir),
        MigrateCommand::Run { stores, .. }
        | MigrateCommand::Plan { stores, .. }
        | MigrateCommand::Status { stores, .. }
        | MigrateCommand::Snapshot { stores, .. }
        | MigrateCommand::Restore { stores, .. }
        | MigrateCommand::Snapshots {
            command: SnapshotsCommand::Prune { stores, .. },
        }
        | MigrateCommand::Verify { stores, .. } => stores,
    };
    let raft_dir = embedded_meta_dir(stores);
    // The embedded metastore's directory is copied, or put back, while no
    // node has it open.
    let mut staged_raft = None;
    match &command {
        MigrateCommand::Snapshot { to, .. } => {
            if let Some(meta_dir) = raft_dir.as_ref().filter(|d| d.exists()) {
                match loams_migrate::snapshot::stage_raft(meta_dir, to) {
                    Ok(staged) => staged_raft = Some(staged),
                    Err(err) => {
                        eprintln!("loams migrate: {err}");
                        return ExitCode::FAILURE;
                    }
                }
            }
        }
        MigrateCommand::Restore { from, .. } => {
            if let Some(meta_dir) = &raft_dir
                && let Err(err) = loams_migrate::snapshot::restore_raft(from, meta_dir)
            {
                eprintln!("loams migrate: {err}");
                return snapshot_exit(&err);
            }
        }
        _ => {}
    }
    let open = match open(stores).await {
        Ok(open) => open,
        Err(err) => {
            eprintln!("loams migrate: {err}");
            return ExitCode::FAILURE;
        }
    };
    let code = execute(&command, &open.cx, started, staged_raft.as_deref()).await;
    open.close().await;
    if let Some(staged) = staged_raft {
        let _ = std::fs::remove_dir_all(staged);
    }
    code
}

/// `<data-dir>/meta` when the metastore is the embedded one.
fn embedded_meta_dir(stores: &StoreArgs) -> Option<PathBuf> {
    match stores.meta.clone().unwrap_or_default() {
        MetaBackend::Raft => Some(stores.data_dir.join("meta")),
        #[cfg(feature = "tikv")]
        MetaBackend::Tikv(_) => None,
    }
}

fn snapshot_exit(err: &loams_migrate::snapshot::SnapshotError) -> ExitCode {
    use loams_migrate::snapshot::SnapshotError;
    match err {
        SnapshotError::Incomplete(_) | SnapshotError::Corrupt(_) => ExitCode::from(78),
        SnapshotError::NotFound(_) => ExitCode::from(66),
        SnapshotError::Io(_) => ExitCode::FAILURE,
    }
}

fn list_snapshots(dir: &Path) -> ExitCode {
    match loams_migrate::snapshot::list(dir) {
        Ok(listed) => {
            println!("{}", serde_json::to_value(&listed).unwrap_or_default());
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("loams migrate: {err}");
            snapshot_exit(&err)
        }
    }
}

async fn execute(
    command: &MigrateCommand,
    cx: &Cx,
    started: Instant,
    staged_raft: Option<&Path>,
) -> ExitCode {
    let path = UpgradePath::compiled();
    let runner = Runner::new(MIGRATIONS, path);
    match command {
        MigrateCommand::Run {
            stores,
            target_release,
            abandon,
            no_snapshot_i_accept_data_loss,
            output,
        } => {
            let target = target_release.as_deref().unwrap_or(this_release());
            let mut cx = cx.clone();
            if let Some(id) = abandon {
                return match runner.abandon(&mut cx, id, &holder()).await {
                    Ok(()) => {
                        print(*output, &json!({"abandoned": id}), || {
                            format!("loams migrate: abandoned {id}")
                        });
                        ExitCode::SUCCESS
                    }
                    Err(err) => {
                        eprintln!("loams migrate: {}: {err}", err.code());
                        exit_of(&err)
                    }
                };
            }
            let mut opts = RunOptions::new(holder());
            opts.no_snapshot_i_accept_data_loss = *no_snapshot_i_accept_data_loss;
            opts.image_digest = std::env::var("LOAMS_IMAGE_DIGEST").ok();
            opts.new_stack = match loams_migrate::startup::has_user_data(&*cx.meta).await {
                Ok(has) => !has,
                Err(err) => {
                    eprintln!("loams migrate: {err}");
                    return ExitCode::FAILURE;
                }
            };
            let durable = match durable_resonate(&stores.data_dir).await {
                Ok(durable) => durable,
                Err(err) => {
                    eprintln!("loams migrate: migration_failed: {err}");
                    return ExitCode::from(65);
                }
            };
            match runner.run(&mut cx, target, &opts).await {
                Ok(report) => {
                    let value = json!({
                        "from": report.from,
                        "to": report.to,
                        "applied": report.applied,
                        "stack_written": report.stack_written,
                        "durable_migrated": durable,
                        "host_changes": cx.host.changes(),
                        "elapsed_ms": started.elapsed().as_millis() as u64,
                    });
                    print(*output, &value, || {
                        if report.applied.is_empty() {
                            format!(
                                "loams migrate: the stack is at {}; nothing to do",
                                report.to
                            )
                        } else {
                            format!(
                                "loams migrate: {} -> {}: applied {}",
                                report.from.as_deref().unwrap_or("(unstamped)"),
                                report.to,
                                report.applied.join(", ")
                            )
                        }
                    });
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    tracing::error!(code = err.code(), %err, "migrate run stopped");
                    eprintln!("loams migrate: {}: {err}", err.code());
                    exit_of(&err)
                }
            }
        }
        MigrateCommand::Plan {
            target_release,
            output,
            ..
        } => {
            let target = target_release.as_deref().unwrap_or(this_release());
            match runner.plan(cx, target).await {
                Ok(plan) => {
                    let value = serde_json::to_value(&plan).unwrap_or_default();
                    print(*output, &value, || {
                        format!(
                            "{} -> {}: {} migrations",
                            plan.from.as_deref().unwrap_or("(unstamped)"),
                            plan.to,
                            plan.steps.len()
                        )
                    });
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    eprintln!("loams migrate: {}: {err}", err.code());
                    exit_of(&err)
                }
            }
        }
        MigrateCommand::Snapshot {
            to,
            agentd_dir,
            target_release,
            id,
            tikv,
            tikv_version,
            drain_timeout_secs,
            stores,
            output,
            ..
        } => {
            use loams_migrate::snapshot::tikv::TikvMethod;
            let tikv = match (tikv.as_deref(), tikv_version) {
                (Some("br"), Some(v)) => Some((TikvMethod::Br, v.clone())),
                (Some("cold"), Some(v)) => Some((TikvMethod::Cold, v.clone())),
                (Some(other), _) => {
                    eprintln!("loams migrate: --tikv {other:?}: expected br or cold");
                    return ExitCode::from(2);
                }
                _ => None,
            };
            let postgres = match (&stores.pageserver, &stores.safekeeper) {
                (Some(pageserver), Some(safekeeper)) => {
                    Some(loams_migrate::snapshot::PostgresOptions {
                        pageserver: pageserver.clone(),
                        safekeeper: safekeeper.clone(),
                        drain_timeout: std::time::Duration::from_secs(*drain_timeout_secs),
                    })
                }
                _ => None,
            };
            let opts = loams_migrate::snapshot::SnapshotOptions {
                id: id.clone(),
                tikv,
                postgres,
                target_release: target_release
                    .clone()
                    .unwrap_or_else(|| this_release().to_string()),
                raft_dir: staged_raft.map(Path::to_path_buf),
                agentd_dir: agentd_dir.clone(),
                ..Default::default()
            };
            match loams_migrate::snapshot::snapshot(cx, to, &opts).await {
                Ok(manifest) => {
                    let value = json!({
                        "id": manifest.id,
                        "dir": to.join(&manifest.id),
                        "created_at_ms": manifest.created_at_ms,
                        "release": manifest.release,
                        "components": manifest.components,
                    });
                    print(*output, &value, || {
                        format!("loams migrate: snapshot {} taken", manifest.id)
                    });
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    eprintln!("loams migrate: {err}");
                    snapshot_exit(&err)
                }
            }
        }
        MigrateCommand::Restore {
            from,
            agentd_dir,
            tikv_version,
            output,
            ..
        } => match loams_migrate::snapshot::restore(
            cx,
            from,
            None,
            agentd_dir.as_deref(),
            tikv_version.as_deref(),
        )
        .await
        {
            Ok(mut report) => {
                if staged_raft.is_none() && embedded_restored(from) {
                    report.restored.insert(0, "raft".to_string());
                }
                let value = serde_json::to_value(&report).unwrap_or_default();
                print(*output, &value, || {
                    format!("loams migrate: restored {}", report.restored.join(", "))
                });
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("loams migrate: {err}");
                snapshot_exit(&err)
            }
        },
        MigrateCommand::Snapshots {
            command: SnapshotsCommand::Prune { dir, id, .. },
        } => match loams_migrate::snapshot::prune(cx, dir, id).await {
            Ok(()) => {
                println!("{}", json!({"pruned": id}));
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("loams migrate: {err}");
                snapshot_exit(&err)
            }
        },
        MigrateCommand::Snapshots {
            command: SnapshotsCommand::List { .. },
        } => unreachable!("listed without the stores"),
        MigrateCommand::Status { output, .. } => match status(cx).await {
            Ok(value) => {
                print(*output, &value, || value.to_string());
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("loams migrate: {err}");
                ExitCode::FAILURE
            }
        },
        MigrateCommand::Verify {
            engine,
            target_release,
            output,
            ..
        } => {
            let target = target_release.as_deref().unwrap_or(this_release());
            let mut report = verify_stores(cx, target).await;
            if let Some(engine) = engine {
                report
                    .products
                    .insert("engine".to_string(), engine_probe(engine, target).await);
            }
            let value = serde_json::to_value(&report).unwrap_or_default();
            print(*output, &value, || {
                report
                    .products
                    .iter()
                    .map(|(product, probe)| {
                        format!("{product}: {:?} ({})", probe.state, probe.detail)
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
            if report.passed() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(65)
            }
        }
    }
}

/// Whether snapshot `from` holds the embedded metastore (restored before
/// the node opened).
fn embedded_restored(from: &Path) -> bool {
    loams_migrate::snapshot::SnapshotManifest::read(from)
        .is_ok_and(|m| m.components.iter().any(|c| c.store == "raft"))
}

/// The stack record and the ledger as JSON.
async fn status(cx: &Cx) -> Result<serde_json::Value, String> {
    let Some(ledger) = Ledger::open_existing(cx.meta.clone())
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(json!({"stack": null, "ledger": []}));
    };
    let stack = ledger.stack().await.map_err(|e| e.to_string())?;
    let entries = ledger.entries().await.map_err(|e| e.to_string())?;
    Ok(json!({
        "stack": stack.map(|s| json!({
            "release": s.release,
            "formats": s.formats,
            "image_digest": s.image_digest,
        })),
        "ledger": entries.iter().map(|e| json!({
            "seq": e.seq,
            "id": e.id,
            "from": e.release_from,
            "to": e.release_to,
            "store": e.store,
            "class": e.class,
            "state": e.state,
            "attempts": e.attempts,
            "error": e.error.as_ref().map(|err| json!({"code": err.code, "message": err.message})),
        })).collect::<Vec<_>>(),
    }))
}

/// The engine's `/ready` and `GetInstance.serverVersion`.
async fn engine_probe(base: &str, release: &str) -> Probe {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(client) => client,
        Err(e) => return Probe::failed(e.to_string()),
    };
    let base = base.trim_end_matches('/');
    match client.get(format!("{base}/ready")).send().await {
        Ok(r) if r.status().is_success() => {}
        Ok(r) => return Probe::failed(format!("/ready answered {}", r.status())),
        Err(e) => return Probe::failed(format!("/ready: {e}")),
    }
    let instance = client
        .post(format!(
            "{base}/loams.instance.v1.InstanceService/GetInstance"
        ))
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await;
    match instance {
        Ok(r) if r.status().is_success() => match r.json::<serde_json::Value>().await {
            Ok(body) if body["serverVersion"] == release => {
                Probe::ok(format!("ready, serving {release}"))
            }
            Ok(body) => Probe::failed(format!(
                "GetInstance says {}, not {release}",
                body["serverVersion"]
            )),
            Err(e) => Probe::failed(format!("GetInstance: {e}")),
        },
        Ok(r) => Probe::failed(format!("GetInstance answered {}", r.status())),
        Err(e) => Probe::failed(format!("GetInstance: {e}")),
    }
}
