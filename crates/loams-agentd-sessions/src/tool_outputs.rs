//! Full tool outputs, kept on this device (plan DD1 ruling T0-16).
//!
//! The doc carries only a tool part's one-line summary. The full output text,
//! the full diff (JSON) and a finished subagent's frozen transcript are written
//! here instead of the remote sidecar the fork used, keyed by the same
//! doc-resident refs: `{chatId}/{partId}` and `{chatId}/{partId}.diff`.
//! Layout: `<root>/<chatId>/<percent-encoded partId>[.diff]`.

use std::path::{Path, PathBuf};

use loams_agentd_doc::SidecarPayload;

use crate::EngineError;

#[derive(Debug, Clone)]
pub(crate) struct ToolOutputs {
    root: PathBuf,
}

impl ToolOutputs {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Best-effort: a failed write leaves the doc's summary as the record.
    pub(crate) fn write(&self, chat_id: &str, payload: &SidecarPayload) {
        if let Err(error) = self.try_write(chat_id, payload) {
            tracing::warn!(chat = %chat_id, part = %payload.part_id, %error,
                "tool output not kept");
        }
    }

    fn try_write(&self, chat_id: &str, payload: &SidecarPayload) -> Result<(), EngineError> {
        let path = self.path(&format!("{chat_id}/{}", payload.part_id))?;
        if let Some(output) = &payload.output {
            write_atomic(&path, output.as_bytes())?;
        }
        if let Some(diff) = &payload.diff {
            let json = serde_json::to_vec(diff)
                .map_err(|e| EngineError::Other(format!("diff encode: {e}")))?;
            write_atomic(&path.with_file_name(diff_name(&path)), &json)?;
        }
        Ok(())
    }

    /// Read a stored output by its ref (`{chatId}/{partId}` or `….diff`).
    pub(crate) fn read(&self, blob_ref: &str) -> Result<String, EngineError> {
        let path = match blob_ref.strip_suffix(".diff") {
            Some(base) => {
                let path = self.path(base)?;
                path.with_file_name(diff_name(&path))
            }
            None => self.path(blob_ref)?,
        };
        std::fs::read_to_string(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                EngineError::Other(format!("tool output not found: {blob_ref}"))
            }
            _ => EngineError::Io(e),
        })
    }

    /// Delete a chat's outputs with the chat.
    pub(crate) fn purge_chat(&self, chat_id: &str) {
        if !valid_chat_id(chat_id) {
            return;
        }
        let dir = self.root.join(chat_id);
        if let Err(error) = std::fs::remove_dir_all(&dir)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(chat = %chat_id, %error, "tool outputs not deleted");
        }
    }

    /// The file for `{chatId}/{partId}`. The ref shape is the one
    /// `apply_sidecar_refs` writes; anything else is a forged ref, and no
    /// part may name `.` or `..`.
    fn path(&self, blob_ref: &str) -> Result<PathBuf, EngineError> {
        let valid = blob_ref.split_once('/').filter(|(chat, part)| {
            valid_chat_id(chat)
                && !part.is_empty()
                && part.len() <= 200
                && *part != "."
                && *part != ".."
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._:#~-".contains(&b))
        });
        let Some((chat, part)) = valid else {
            return Err(EngineError::Other(format!("bad blob ref: {blob_ref}")));
        };
        Ok(self.root.join(chat).join(encode_part_segment(part)))
    }
}

fn valid_chat_id(chat: &str) -> bool {
    !chat.is_empty()
        && chat.len() <= 128
        && chat
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn diff_name(path: &Path) -> String {
    format!(
        "{}.diff",
        path.file_name().unwrap_or_default().to_string_lossy()
    )
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), EngineError> {
    let dir = path
        .parent()
        .ok_or_else(|| EngineError::Other("tool output path has no parent".into()))?;
    std::fs::create_dir_all(dir)?;
    let tmp = path.with_file_name(format!(
        ".{}.tmp-{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        uuid::Uuid::new_v4()
    ));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(())
}

/// Percent-encode a part id for use as a file name. The part alphabet
/// includes `#` and `:`, which are not portable in file names (`:` is
/// reserved on Windows).
fn encode_part_segment(part_id: &str) -> String {
    let mut out = String::with_capacity(part_id.len());
    for byte in part_id.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_colon_are_escaped_unreserved_pass_through() {
        assert_eq!(encode_part_segment("m1#c1"), "m1%23c1");
        assert_eq!(encode_part_segment("tool:call_9"), "tool%3Acall_9");
        assert_eq!(encode_part_segment("plain-id_0.diff~"), "plain-id_0.diff~");
    }

    #[test]
    fn output_and_diff_are_kept_under_their_refs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let outputs = ToolOutputs::new(dir.path().to_path_buf());
        let diff: loams_agentd_proto::ToolDiff = serde_json::from_value(serde_json::json!({
            "path": "a.txt", "oldText": "a", "newText": "b"
        }))
        .expect("tool diff");
        outputs.write(
            "chat-1",
            &SidecarPayload {
                part_id: "p1".into(),
                output: Some("the whole output".into()),
                diff: Some(diff.clone()),
            },
        );
        assert_eq!(
            outputs.read("chat-1/p1").expect("output"),
            "the whole output"
        );
        let stored: loams_agentd_proto::ToolDiff =
            serde_json::from_str(&outputs.read("chat-1/p1.diff").expect("diff")).expect("json");
        assert_eq!(stored, diff);
    }

    #[test]
    fn forged_refs_are_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let outputs = ToolOutputs::new(dir.path().join("outputs"));
        std::fs::write(dir.path().join("secret"), "x").expect("write");
        for blob_ref in [
            "../secret",
            "chat/..",
            "chat/.",
            "chat/a/b",
            "/etc/passwd",
            "chat",
            "ch at/p",
        ] {
            assert!(
                outputs.read(blob_ref).is_err(),
                "{blob_ref} must be refused"
            );
        }
    }
}
