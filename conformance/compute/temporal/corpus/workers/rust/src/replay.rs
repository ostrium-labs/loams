//! `replay`: every history in a directory replays against the corpus's Rust
//! workflows with the SDK's `WorkflowReplayer`, which runs Temporal's
//! sdk-core replay worker (`temporalio_sdk_core::init_replay_worker`): the
//! same Core state machines the Python, .NET, Ruby and TypeScript SDKs
//! embed. A history passes when Core reports no nondeterminism, no invalid
//! history and no workflow-task failure.
//!
//! It reads the binary `History` messages (`<scenario>[.run<k>].binpb`, the
//! exporter's GetWorkflowExecutionHistory bytes), not the protojson: the
//! SDK's `WorkflowHistory::from_json` reads its own serde format, not the
//! `temporal workflow show -o json` one.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use prost::Message;
use temporalio_client::WorkflowHistory;
use temporalio_common::protos::temporal::api::history::v1::{History, history_event::Attributes};
use temporalio_sdk::workflow_replayer::{WorkflowReplayer, WorkflowReplayerOptions};

use crate::{
    register_corpus_workflows,
    scenarios::{scenario_of_file, workflow_id},
};

/// The `.binpb` histories of `dir`, sorted.
pub fn history_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("read {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "binpb"))
        .collect();
    files.sort();
    Ok(files)
}

/// Gives the run the workflow ID its generator used when the started event
/// does not carry one: child workflow IDs derive from it, and the replayer
/// otherwise uses a placeholder ID.
pub fn set_workflow_id(history: &mut History, id: &str) {
    if let Some(Attributes::WorkflowExecutionStartedEventAttributes(a)) = history
        .events
        .first_mut()
        .and_then(|e| e.attributes.as_mut())
        && a.workflow_id.is_empty()
    {
        a.workflow_id = id.to_owned();
    }
}

pub fn load(path: &Path) -> Result<History> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let mut history =
        History::decode(bytes.as_slice()).with_context(|| format!("decode {}", path.display()))?;
    if history.events.is_empty() {
        bail!("{}: no events", path.display());
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let scenario =
        scenario_of_file(name).with_context(|| format!("{name}: not a corpus history"))?;
    set_workflow_id(&mut history, &workflow_id(scenario));
    Ok(history)
}

pub async fn replay(dir: &Path) -> Result<()> {
    let files = history_files(dir)?;
    if files.is_empty() {
        bail!("no .binpb histories in {}", dir.display());
    }
    let mut opts = WorkflowReplayerOptions::new().build();
    register_corpus_workflows!(opts);
    let replayer = WorkflowReplayer::new(opts)?;
    let mut histories = Vec::with_capacity(files.len());
    for f in &files {
        histories.push(WorkflowHistory::from(load(f)?));
    }
    let results = replayer.replay_workflows(histories).await?;
    if results.len() != files.len() {
        bail!(
            "{} histories, {} replay results",
            files.len(),
            results.len()
        );
    }
    let mut failures = Vec::new();
    for (f, r) in files.iter().zip(results) {
        let name = f.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        match r.replay_failure {
            None => println!("replay: ok   {name}"),
            Some(failure) => {
                println!("replay: FAIL {name}: {failure}");
                failures.push(format!("{name}: {failure}"));
            }
        }
    }
    if !failures.is_empty() {
        bail!(
            "{} of {} histories did not replay:\n{}",
            failures.len(),
            files.len(),
            failures.join("\n")
        );
    }
    println!(
        "replay: {} histories replayed with sdk-core's replayer",
        files.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use temporalio_common::protos::temporal::api::history::v1::{
        HistoryEvent, WorkflowExecutionStartedEventAttributes,
    };

    fn started(id: &str) -> History {
        let attrs = WorkflowExecutionStartedEventAttributes {
            workflow_id: id.to_owned(),
            ..Default::default()
        };
        History {
            events: vec![HistoryEvent {
                event_id: 1,
                attributes: Some(Attributes::WorkflowExecutionStartedEventAttributes(attrs)),
                ..Default::default()
            }],
        }
    }

    fn id_of(h: &History) -> String {
        match h.events[0].attributes.as_ref() {
            Some(Attributes::WorkflowExecutionStartedEventAttributes(a)) => a.workflow_id.clone(),
            _ => String::new(),
        }
    }

    #[test]
    fn sets_a_missing_workflow_id_only() {
        let mut h = started("");
        set_workflow_id(&mut h, "corpus-rust-hello");
        assert_eq!(id_of(&h), "corpus-rust-hello");
        let mut h = started("server-set");
        set_workflow_id(&mut h, "corpus-rust-hello");
        assert_eq!(id_of(&h), "server-set");
    }

    #[test]
    fn a_binpb_round_trip_keeps_the_events() {
        let dir = std::env::temp_dir().join(format!("corpus-rust-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("hello.binpb");
        std::fs::write(&path, started("").encode_to_vec()).unwrap();
        std::fs::write(dir.join("hello.json"), b"{}").unwrap();
        assert_eq!(history_files(&dir).unwrap(), vec![path.clone()]);
        let h = load(&path).unwrap();
        assert_eq!(h.events.len(), 1);
        assert_eq!(id_of(&h), "corpus-rust-hello");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
