//! Following a run's log (§85 §11, D2442): bytes are appended to a local
//! mirror as they arrive, and every (re)connection asks for the log from
//! the mirror's length, so a disconnect resumes at the last byte received
//! and the mirror ends byte-identical to the server's log.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use futures::StreamExt;

use super::{TransferError, backoff};
use crate::api::Client;
use crate::run::RunView;

/// Follows run `rid` into the file `log` until the run ends; `on` sees each
/// piece with its offset. Returns the final run.
pub async fn tail(
    client: &Client,
    rid: &str,
    log: &Path,
    mut on: impl FnMut(u64, &[u8]),
) -> Result<RunView, TransferError> {
    let mut file = OpenOptions::new().create(true).append(true).open(log)?;
    let mut attempt = 0u32;
    loop {
        let mut offset = file.metadata()?.len();
        match client.read_log(rid, offset, true).await {
            Ok(mut ls) => {
                attempt = 0;
                while let Some(b) = ls.bytes.next().await {
                    let Ok(b) = b else { break };
                    file.write_all(&b)?;
                    on(offset, &b);
                    offset += b.len() as u64;
                }
                file.flush()?;
                let v = client.run(rid).await?;
                if v.done() && offset >= v.log_bytes {
                    return Ok(v);
                }
            }
            Err(e) if e.is_transient() => {
                attempt += 1;
                client.reset().await;
                tokio::time::sleep(backoff(attempt.min(5))).await;
            }
            Err(e) => return Err(e.into()),
        }
    }
}
