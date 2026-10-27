//! One LAPC Core run, start to finish.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::acp::Agent;
use super::checks::{self, CancelOutcome, Outcome, TermOutcome};
use super::observe::{self, Observer};
use super::report::{self, Limits, PackageInfo, Report, Runner, Subject};
use super::stage::{self, NpmTool};
use super::world::{Phase, Script, World};
use super::{ConformanceError, Options, Target, declared_tools};
use crate::contract::{self, Provisioned, ProxiedMcp};
use crate::{index, listing};

/// The prompts of the scripted turn and of the cancel probe.
const TURN_PROMPT: &str = "LAPC scripted turn: use your tools as the model asks, then answer.";
const CANCEL_PROMPT: &str = "LAPC cancel probe: this turn is cancelled.";
/// How long the cancel probe waits for the agent to call the model.
const CANCEL_WAIT: Duration = Duration::from_secs(2);

fn io(path: &Path) -> impl Fn(std::io::Error) -> ConformanceError + '_ {
    move |e| ConformanceError::Io {
        path: path.display().to_string(),
        error: e.to_string(),
    }
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

async fn node_version(node: &Path) -> Result<String, ConformanceError> {
    let output = tokio::process::Command::new(node)
        .arg("--version")
        .env_clear()
        .output()
        .await
        .map_err(|e| ConformanceError::Unsupported(format!("{}: {e}", node.display())))?;
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim()
        .trim_start_matches('v')
        .to_owned())
}

pub(super) async fn run(target: &Target, options: &Options) -> Result<Report, ConformanceError> {
    let listing = &target.listing;
    if let Err(issues) = listing::validate(listing) {
        let shown: Vec<String> = issues.iter().take(5).map(ToString::to_string).collect();
        return Err(ConformanceError::Input(format!(
            "the listing is not valid against listing schema v1: {}",
            shown.join("; ")
        )));
    }
    let launch_args: Vec<String> = match listing["kind"].as_str() {
        Some("agent") if listing["surfaces"].get("acp").is_some() => Vec::new(),
        Some("agent") => {
            return Err(ConformanceError::Unsupported(
                "the agent serves no ACP; the MK0 runner speaks ACP only (A2A comes in MK1)".into(),
            ));
        }
        Some("harness") if listing["harness"]["launch"]["protocol"] == "acp" => listing["harness"]
            ["launch"]["args"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| a.as_str().map(str::to_owned))
            .collect(),
        _ => {
            return Err(ConformanceError::Unsupported(
                "LAPC runs agents and ACP harnesses; MCP servers and native harnesses have no package to run".into(),
            ));
        }
    };
    if listing["runtime"]["kind"] != "node" {
        return Err(ConformanceError::Unsupported(
            "the MK0 runner runs the node runtime only".into(),
        ));
    }
    let node = options
        .node
        .clone()
        .or_else(|| find_on_path("node"))
        .ok_or_else(|| ConformanceError::Unsupported("no node found (pass --node)".into()))?;
    let node_version = node_version(&node).await?;
    if let Some(range) = listing["runtime"]["version"].as_str()
        && let Ok(req) = semver::VersionReq::parse(range)
        && let Ok(version) = semver::Version::parse(&node_version)
        && !req.matches(&version)
    {
        return Err(ConformanceError::Unsupported(format!(
            "node {node_version} does not satisfy the listing's runtime {range}"
        )));
    }
    let node_dir = node.parent().map(Path::to_owned).unwrap_or_default();
    let npm = options
        .npm
        .clone()
        .or_else(|| find_on_path("npm"))
        .map(|npm| NpmTool {
            npm,
            node_dir: node_dir.clone(),
        });

    let (work, temporary) = match &options.work_dir {
        Some(dir) => (dir.clone(), false),
        None => {
            let nanos = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            (
                std::env::temp_dir().join(format!("loams-lapc-{}-{nanos}", std::process::id())),
                true,
            )
        }
    };
    std::fs::create_dir_all(&work).map_err(io(&work))?;
    let work = work.canonicalize().map_err(io(&work))?;
    let result = run_in(
        target,
        options,
        &work,
        &node,
        &node_version,
        npm.as_ref(),
        launch_args,
    )
    .await;
    if temporary {
        let _ = std::fs::remove_dir_all(&work);
    }
    result
}

/// Every file under `root` but `data/`: path → (size, SHA-256).
fn snapshot(root: &Path) -> BTreeMap<String, (u64, [u8; 32])> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_owned()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if dir == root && entry.file_name() == "data" {
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .display()
                .to_string();
            if kind.is_dir() {
                stack.push(path);
            } else if let Ok(bytes) = std::fs::read(&path) {
                out.insert(
                    relative,
                    (bytes.len() as u64, Sha256::digest(&bytes).into()),
                );
            } else {
                out.insert(relative, (0, [0; 32]));
            }
        }
    }
    out
}

fn diff(
    before: &BTreeMap<String, (u64, [u8; 32])>,
    after: &BTreeMap<String, (u64, [u8; 32])>,
) -> Vec<String> {
    let mut changes = Vec::new();
    for (path, value) in after {
        match before.get(path) {
            None => changes.push(format!("created {path}")),
            Some(old) if old != value => changes.push(format!("changed {path}")),
            Some(_) => {}
        }
    }
    for path in before.keys() {
        if !after.contains_key(path) {
            changes.push(format!("deleted {path}"));
        }
    }
    changes
}

/// A value for a configuration input: its default, else a plain value of
/// its type (secrets get a dummy, never a real one).
fn config_value(input: &Value) -> String {
    if input["secret"].as_bool() == Some(true) {
        return "lapc-dummy-secret".into();
    }
    match &input["default"] {
        Value::String(s) => s.clone(),
        Value::Null => match input["type"].as_str() {
            Some("number" | "integer") => "1".into(),
            Some("boolean") => "true".into(),
            Some("enum") => input["values"][0].as_str().unwrap_or("lapc").into(),
            _ => "lapc".into(),
        },
        other => other.to_string(),
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_in(
    target: &Target,
    options: &Options,
    work: &Path,
    node: &Path,
    node_version: &str,
    npm: Option<&NpmTool>,
    launch_args: Vec<String>,
) -> Result<Report, ConformanceError> {
    let started_unix_ms = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let clock = Instant::now();
    let listing = &target.listing;
    let limits = options.limits;

    let staged = stage::stage(listing, &target.package, work, npm).await?;
    let entrypoint = listing["entrypoint"].as_str().unwrap_or_default();
    let entry = staged.package_dir.join(entrypoint);
    if entrypoint.is_empty() || entrypoint.contains("..") || !entry.is_file() {
        return Err(ConformanceError::Stage(format!(
            "the entrypoint {entrypoint} is not in the package"
        )));
    }

    // The agent's world.
    let declared = declared_tools(listing);
    let strings = |value: &Value| -> Vec<String> {
        value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect()
    };
    let mut script = Script {
        declared: declared.clone(),
        always_ask: strings(&listing["place"]["always_ask"]),
        egress: strings(&listing["place"]["egress"]),
        mcp: BTreeMap::new(),
    };
    for dependency in listing["mcp_servers"].as_array().into_iter().flatten() {
        let Some(id) = dependency["id"].as_str() else {
            continue;
        };
        let name = id.rsplit('/').next().unwrap_or(id).to_owned();
        let tools = listing["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|t| t["mcp_server"] == id)
            .filter_map(|t| t["name"].as_str().map(str::to_owned))
            .collect();
        script.mcp.insert(name, (id.to_owned(), tools));
    }
    let mcp_names: Vec<String> = script.mcp.keys().cloned().collect();
    let seq = Arc::new(AtomicU64::new(0));
    let world = World::start(script.clone(), Arc::clone(&seq))
        .await
        .map_err(io(work))?;
    let base = world.base_url();

    // The environment contract (§73 §3.3).
    let data_dir = work.join("data");
    for sub in ["home", "tmp"] {
        let dir = data_dir.join(sub);
        std::fs::create_dir_all(&dir).map_err(io(&dir))?;
    }
    let token_file = work.join("token");
    contract::write_private(&token_file, b"lapc-core-run").map_err(io(&token_file))?;
    let mcp_config = if mcp_names.is_empty() {
        None
    } else {
        let path = work.join("mcp.json");
        let servers: Vec<ProxiedMcp> = mcp_names
            .iter()
            .map(|name| ProxiedMcp {
                name: name.clone(),
                url: format!("{base}/mcp/{name}"),
            })
            .collect();
        std::fs::write(&path, contract::mcp_config(&servers)).map_err(io(&path))?;
        Some(path)
    };
    let config = listing["config"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|input| Some((input["name"].as_str()?.to_owned(), config_value(input))))
        .collect();
    let provisioned = Provisioned {
        endpoint: format!("{base}/loams"),
        namespace: "lapc".into(),
        data_dir: data_dir.clone(),
        token_file,
        runtime_bin: node.parent().map(Path::to_owned),
        pg_url: options.services.pg_url.clone(),
        qdrant: options.services.qdrant.clone(),
        elasticsearch_url: options.services.elasticsearch_url.clone(),
        durable_url: options.services.durable_url.clone(),
        mcp_config,
        model_proxy: Some(base.clone()),
        egress_proxy: Some(base.clone()),
        config,
        provider_keys: BTreeMap::new(),
    };
    let env = contract::environment(listing, &provisioned);

    let before = snapshot(work);
    let mut args = vec![entry.display().to_string()];
    args.extend(launch_args);
    let mut agent = Agent::spawn(
        node,
        &args,
        &env,
        &data_dir,
        declared.clone(),
        Arc::clone(&seq),
    )
    .map_err(io(node))?;
    let observer = Observer::start(agent.pid);

    let mut outcome = Outcome {
        env_vars: env.len(),
        initialize: Err(String::new()),
        session: None,
        turn: None,
        cancel: None,
        term: None,
        tree_changes: Vec::new(),
        observed: observe::Observed::default(),
        install_code: staged.install_code.clone(),
    };

    // LAPC-C1: ready.
    outcome.initialize = agent
        .request(
            "initialize",
            json!({
                "protocolVersion": 1,
                "clientCapabilities": {
                    "fs": { "readTextFile": false, "writeTextFile": false },
                    "terminal": false,
                },
                "clientInfo": { "name": "loams-lapc", "version": env!("CARGO_PKG_VERSION") },
            }),
            limits.ready,
        )
        .await;

    if outcome.initialize.is_ok() {
        // LAPC-C2: a session.
        let session = agent
            .request(
                "session/new",
                json!({ "cwd": data_dir.display().to_string(), "mcpServers": [] }),
                limits.ready,
            )
            .await
            .and_then(|(value, _)| {
                value["sessionId"]
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| "session/new answered no sessionId".to_owned())
            });
        outcome.session = Some(session.clone());

        if let Ok(session_id) = session {
            // LAPC-C3, C4, C9: the scripted turn.
            world.set_phase(Phase::Turn);
            let turn = agent
                .request(
                    "session/prompt",
                    json!({ "sessionId": session_id, "prompt": [{ "type": "text", "text": TURN_PROMPT }] }),
                    limits.turn,
                )
                .await
                .map(|(value, took)| {
                    (
                        value["stopReason"].as_str().unwrap_or_default().to_owned(),
                        took,
                    )
                });
            outcome.turn = Some(turn);

            // LAPC-C7: cancel.
            world.set_phase(Phase::Cancel);
            world.hang_model();
            outcome.cancel = Some(cancel_probe(&agent, &world, &session_id, limits.stop).await);
            world.release();
        }

        // LAPC-C7: SIGTERM.
        let signalled = Instant::now();
        let already_gone = agent.exited();
        agent.terminate();
        let exited = agent.wait_exit(limits.stop).await;
        outcome.observed = observer.finish();
        let leftovers = |observed: &observe::Observed, root: i32| -> Vec<String> {
            let mut left: Vec<String> = observed
                .processes
                .iter()
                .filter(|(pid, start)| **pid != root && observe::alive(**pid, **start))
                .map(|(pid, _)| format!("process {pid}"))
                .collect();
            if left.is_empty() && agent.group_alive() {
                left.push(format!("a process in group {root}"));
            }
            left
        };
        let mut left = leftovers(&outcome.observed, agent.pid);
        while !left.is_empty() && signalled.elapsed() < limits.stop {
            tokio::time::sleep(Duration::from_millis(50)).await;
            left = leftovers(&outcome.observed, agent.pid);
        }
        outcome.term = Some(TermOutcome {
            exited: exited.map(|d| if already_gone { Duration::ZERO } else { d }),
            already_gone,
            leftovers: left,
        });
    } else {
        outcome.observed = observer.finish();
    }
    agent.kill().await;
    world.release();
    outcome.tree_changes = diff(&before, &snapshot(work));

    let records = world.records();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let node_prefix = node
        .parent()
        .and_then(Path::parent)
        .map(Path::to_owned)
        .unwrap_or_default();
    let log = agent.log();
    let context = checks::Context {
        listing,
        records: &records,
        acp: &log,
        data_dir: &data_dir,
        work,
        home: home.as_deref(),
        node_prefix: &node_prefix,
        limits,
    };
    let checks = checks::evaluate(&context, &outcome);
    let measured = checks::measured(&context, &outcome, agent.stderr_tail());
    let level = report::level_of(&checks).to_owned();
    drop(log);

    let mut egress_recording = vec!["proxy".to_owned()];
    if outcome.observed.available {
        egress_recording.push("procfs".into());
    }
    Ok(Report {
        schema: report::REPORT_SCHEMA_ID.into(),
        suite: "LAPC".into(),
        tested_level: "core".into(),
        level,
        runner: Runner {
            name: "loams-agentd".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
            isolation: "process".into(),
            egress_recording,
            node: node_version.into(),
            limits: Limits {
                ready_ms: limits.ready.as_millis() as u64,
                turn_ms: limits.turn.as_millis() as u64,
                stop_ms: limits.stop.as_millis() as u64,
            },
        },
        subject: Subject {
            id: listing["id"].as_str().unwrap_or_default().into(),
            version: listing["version"].as_str().unwrap_or_default().into(),
            kind: listing["kind"].as_str().unwrap_or_default().into(),
            listing_sha256: hex::encode(Sha256::digest(index::canonical(listing).as_bytes())),
            package: PackageInfo {
                origin: staged.origin.into(),
                name: staged.name,
                version: staged.version,
                integrity_checked: staged.integrity_checked,
            },
        },
        started_unix_ms,
        duration_ms: clock.elapsed().as_millis() as u64,
        checks,
        measured,
    })
}

/// The cancel probe: a turn whose model call is held, cancelled once the
/// agent is waiting on the model (or after two seconds).
async fn cancel_probe(
    agent: &Agent,
    world: &World,
    session_id: &str,
    limit: Duration,
) -> CancelOutcome {
    let mut prompt = agent.rpc.request_now(
        "session/prompt",
        json!({ "sessionId": session_id, "prompt": [{ "type": "text", "text": CANCEL_PROMPT }] }),
    );
    let ended = tokio::select! {
        result = &mut prompt => Some(result),
        () = world.model_held() => None,
        () = tokio::time::sleep(CANCEL_WAIT) => None,
    };
    if let Some(result) = ended {
        return CancelOutcome::EndedFirst(match result {
            Ok(value) => value["stopReason"].as_str().unwrap_or_default().to_owned(),
            Err(error) => error.to_string(),
        });
    }
    agent
        .rpc
        .notify("session/cancel", Some(json!({ "sessionId": session_id })));
    let cancelled = Instant::now();
    match tokio::time::timeout(limit, prompt).await {
        Ok(Ok(value)) if value["stopReason"] == "cancelled" => {
            CancelOutcome::Cancelled(cancelled.elapsed())
        }
        Ok(Ok(value)) => CancelOutcome::Other(format!(
            "the turn ended with stopReason {}",
            value["stopReason"]
        )),
        Ok(Err(error)) => CancelOutcome::Other(format!("the turn failed: {error}")),
        Err(_) => CancelOutcome::NoAnswer,
    }
}
