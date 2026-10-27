//! The ACP adapter on agentd's side (SPIKE): turns the worker's
//! `AcpMessage` frames into the newline-delimited JSON-RPC stream that
//! agentd's `AcpHarness` already speaks, and back. The harness code does
//! not change: in the spike the adapter runs as a process
//! (`loams-agent-worker acp <bundle>`), which the harness spawns like any
//! ACP agent.

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use super::host::AgentWorker;

/// Serves `worker`'s ACP on `input` and `output` until `input` closes or
/// the worker dies; then kills the worker.
pub async fn serve_stdio(
    worker: AgentWorker,
    mut from_agent: mpsc::UnboundedReceiver<String>,
    input: impl AsyncRead + Unpin,
    mut output: impl AsyncWrite + Unpin,
) -> std::io::Result<()> {
    let mut lines = BufReader::new(input).lines();
    let result = loop {
        tokio::select! {
            line = lines.next_line() => match line {
                Ok(Some(line)) if line.trim().is_empty() => {}
                Ok(Some(line)) => worker.acp_send(line),
                Ok(None) => break Ok(()),
                Err(e) => break Err(e),
            },
            message = from_agent.recv() => match message {
                Some(mut json) => {
                    json.push('\n');
                    if let Err(e) = output.write_all(json.as_bytes()).await {
                        break Err(e);
                    }
                    output.flush().await?;
                }
                None => break Ok(()),
            },
            reason = worker.wait() => {
                eprintln!("loams agent-worker: {reason}");
                break Ok(());
            }
        }
    };
    worker.kill();
    result
}
