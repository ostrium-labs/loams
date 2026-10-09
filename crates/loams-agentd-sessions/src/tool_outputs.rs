//! Full tool outputs, kept on this device (plan DD1 rulings T0-16, T2-4, T2-13).
//!
//! The doc carries only a tool part's one-line summary. The full output text,
//! the full diff (JSON) and a finished subagent's frozen transcript are written
//! here instead of the remote sidecar the fork used, keyed by the same
//! doc-resident refs: `{chatId}/{partId}` and `{chatId}/{partId}.diff`.
//! Layout: `<root>/<chatId>/<percent-encoded partId>[.diff]`.
//!
//! The store is bounded ([`ToolOutputLimits`]): each file holds at most
//! `file_cap` bytes of its output (cut at a UTF-8 boundary and flagged
//! truncated), the whole profile at most `budget` bytes (oldest files are
//! evicted first), and an archived chat's outputs are deleted once they are
//! older than `archived_retention`. Each file starts with one header byte,
//! [`HEADER_COMPLETE`] or [`HEADER_TRUNCATED`]; offsets in [`ToolOutputChunk`]
//! count body bytes after it. Reads are paged: one read returns at most
//! [`TOOL_OUTPUT_MAX_FRAME`] bytes.

use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use loams_agentd_doc::SidecarPayload;
use serde::Serialize;

use crate::EngineError;

/// Default per-file cap: 4 MiB of output.
pub const TOOL_OUTPUT_FILE_CAP: u64 = 4 * 1024 * 1024;
/// Default total budget per profile: 1 GiB.
pub const TOOL_OUTPUT_BUDGET: u64 = 1024 * 1024 * 1024;
/// Default age after which an archived chat's outputs are deleted: 30 days.
pub const TOOL_OUTPUT_ARCHIVED_RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// The most body bytes one read returns.
pub const TOOL_OUTPUT_MAX_FRAME: u64 = 256 * 1024;

/// First byte of a file whose output is stored whole.
const HEADER_COMPLETE: u8 = b'c';
/// First byte of a file whose output was cut at the per-file cap.
const HEADER_TRUNCATED: u8 = b't';

/// Bounds of the tool-output store. `EngineConfig::tool_outputs` sets them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolOutputLimits {
    /// Bytes of one output (or diff) kept; the rest is cut and flagged.
    pub file_cap: u64,
    /// Bytes the whole store may hold before the oldest files are evicted.
    pub budget: u64,
    /// Age after which an archived chat's outputs are deleted.
    pub archived_retention: Duration,
}

impl Default for ToolOutputLimits {
    fn default() -> Self {
        Self {
            file_cap: TOOL_OUTPUT_FILE_CAP,
            budget: TOOL_OUTPUT_BUDGET,
            archived_retention: TOOL_OUTPUT_ARCHIVED_RETENTION,
        }
    }
}

/// One page of a stored output (`FetchToolBlob`'s reply).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolOutputChunk {
    /// The page's text. A page never splits a UTF-8 character.
    pub text: String,
    /// Body offset of the page.
    pub offset: u64,
    /// Body bytes in the page; the next page starts at `offset + len`.
    pub len: u64,
    /// Body bytes stored.
    pub total: u64,
    /// The output was cut at the per-file cap when it was written.
    pub truncated: bool,
    /// This page reaches the end of the stored body.
    pub eof: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct ToolOutputs {
    root: PathBuf,
    /// Serializes writes, eviction and sweeps, so the byte count stays exact.
    state: Arc<Mutex<State>>,
}

#[derive(Debug)]
struct State {
    limits: ToolOutputLimits,
    /// Bytes on disk; `None` until the first write scans the store.
    used: Option<u64>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl ToolOutputs {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            root,
            state: Arc::new(Mutex::new(State {
                limits: ToolOutputLimits::default(),
                used: None,
            })),
        }
    }

    pub(crate) fn set_limits(&self, limits: ToolOutputLimits) {
        lock(&self.state).limits = limits;
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
        let diff = payload
            .diff
            .as_ref()
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|e| EngineError::Other(format!("diff encode: {e}")))?;
        let mut state = lock(&self.state);
        let mut used = match state.used {
            Some(used) => used,
            None => self.files().iter().map(|f| f.len).sum(),
        };
        let cap = state.limits.file_cap;
        let mut result = Ok(());
        if let Some(output) = &payload.output {
            result = put(&path, output.as_bytes(), cap, &mut used);
        }
        if let (Ok(()), Some(json)) = (&result, &diff) {
            result = put(&path.with_file_name(diff_name(&path)), json, cap, &mut used);
        }
        // A failed write leaves the count unknown: rescan on the next one.
        state.used = result.is_ok().then_some(used);
        if used > state.limits.budget {
            self.evict(&mut state);
        }
        result
    }

    /// Evict the oldest files until the store is at 90% of its budget, so
    /// the next few writes do not each pay for a scan.
    fn evict(&self, state: &mut State) {
        let mut files = self.files();
        files.sort_by_key(|f| f.modified);
        let mut used: u64 = files.iter().map(|f| f.len).sum();
        let target = state.limits.budget / 10 * 9;
        let mut evicted = 0usize;
        for file in &files {
            if used <= target {
                break;
            }
            if std::fs::remove_file(&file.path).is_ok() {
                used = used.saturating_sub(file.len);
                evicted += 1;
            }
        }
        self.remove_empty_chat_dirs();
        tracing::info!(
            evicted,
            used,
            budget = state.limits.budget,
            "tool outputs over budget: oldest evicted"
        );
        state.used = Some(used);
    }

    /// Delete archived chats' outputs older than the retention.
    pub(crate) fn sweep_archived(&self, archived_chat_ids: &[String], now: SystemTime) {
        let mut state = lock(&self.state);
        let Some(cutoff) = now.checked_sub(state.limits.archived_retention) else {
            return;
        };
        let mut removed = 0u64;
        for chat_id in archived_chat_ids {
            if !valid_chat_id(chat_id) {
                continue;
            }
            for file in files_in(&self.root.join(chat_id)) {
                if file.modified < cutoff && std::fs::remove_file(&file.path).is_ok() {
                    removed += file.len;
                }
            }
        }
        if removed > 0 {
            self.remove_empty_chat_dirs();
            state.used = state.used.map(|used| used.saturating_sub(removed));
        }
    }

    /// Read one page of a stored output by its ref (`{chatId}/{partId}` or
    /// `….diff`): `len` body bytes (at most [`TOOL_OUTPUT_MAX_FRAME`], the
    /// default) from `offset`, widened or narrowed to a character boundary.
    pub(crate) fn read(
        &self,
        blob_ref: &str,
        offset: u64,
        len: Option<u64>,
    ) -> Result<ToolOutputChunk, EngineError> {
        let path = match blob_ref.strip_suffix(".diff") {
            Some(base) => {
                let path = self.path(base)?;
                path.with_file_name(diff_name(&path))
            }
            None => self.path(blob_ref)?,
        };
        let mut file = std::fs::File::open(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                EngineError::Other(format!("tool output not found: {blob_ref}"))
            }
            _ => EngineError::Io(e),
        })?;
        let size = file.metadata()?.len();
        let mut header = [0u8; 1];
        file.read_exact(&mut header)?;
        let total = size.saturating_sub(1);
        let offset = offset.min(total);
        let want = len
            .unwrap_or(TOOL_OUTPUT_MAX_FRAME)
            .clamp(1, TOOL_OUTPUT_MAX_FRAME)
            .min(total - offset);
        // Up to 3 more bytes finish a character the page would split.
        let mut buf = vec![0u8; usize::try_from((want + 3).min(total - offset)).unwrap_or(0)];
        file.seek(SeekFrom::Start(1 + offset))?;
        file.read_exact(&mut buf)?;
        let cut = page_cut(&buf, usize::try_from(want).unwrap_or(0));
        buf.truncate(cut);
        let text = String::from_utf8(buf)
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
        let len = cut as u64;
        Ok(ToolOutputChunk {
            text,
            offset,
            len,
            total,
            truncated: header[0] == HEADER_TRUNCATED,
            eof: offset + len >= total,
        })
    }

    /// Delete a chat's outputs with the chat.
    pub(crate) fn purge_chat(&self, chat_id: &str) {
        if !valid_chat_id(chat_id) {
            return;
        }
        let mut state = lock(&self.state);
        let dir = self.root.join(chat_id);
        if let Err(error) = std::fs::remove_dir_all(&dir)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(chat = %chat_id, %error, "tool outputs not deleted");
        }
        state.used = None;
    }

    /// Every stored file (two levels: chat directories, then files).
    fn files(&self) -> Vec<StoredFile> {
        let Ok(chats) = std::fs::read_dir(&self.root) else {
            return Vec::new();
        };
        chats
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
            .flat_map(|entry| files_in(&entry.path()))
            .collect()
    }

    fn remove_empty_chat_dirs(&self) {
        if let Ok(chats) = std::fs::read_dir(&self.root) {
            for entry in chats.flatten() {
                // Fails, as intended, on a directory that still has files.
                let _ = std::fs::remove_dir(entry.path());
            }
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

#[derive(Debug)]
struct StoredFile {
    path: PathBuf,
    len: u64,
    modified: SystemTime,
}

fn files_in(dir: &Path) -> Vec<StoredFile> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let meta = entry.metadata().ok().filter(|m| m.is_file())?;
            Some(StoredFile {
                path: entry.path(),
                len: meta.len(),
                modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            })
        })
        .collect()
}

/// Write `bytes`, cut to `cap` at a character boundary, behind its header,
/// and keep `used` exact.
fn put(path: &Path, bytes: &[u8], cap: u64, used: &mut u64) -> Result<(), EngineError> {
    let (body, truncated) = cap_utf8(bytes, cap);
    let mut file = Vec::with_capacity(body.len() + 1);
    file.push(if truncated {
        HEADER_TRUNCATED
    } else {
        HEADER_COMPLETE
    });
    file.extend_from_slice(body);
    let old = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    write_atomic(path, &file)?;
    *used = used.saturating_sub(old) + file.len() as u64;
    Ok(())
}

fn is_continuation(byte: u8) -> bool {
    byte & 0xC0 == 0x80
}

/// `bytes` cut to at most `cap`, never inside a UTF-8 character.
fn cap_utf8(bytes: &[u8], cap: u64) -> (&[u8], bool) {
    let cap = usize::try_from(cap).unwrap_or(usize::MAX);
    if bytes.len() <= cap {
        return (bytes, false);
    }
    let mut cut = cap;
    while cut > 0 && is_continuation(bytes[cut]) {
        cut -= 1;
    }
    (&bytes[..cut], true)
}

/// Where a page of `want` bytes ends in `buf` (which may hold up to 3 more):
/// back to the last character boundary, or, when the page holds less than
/// one character, forward past it.
fn page_cut(buf: &[u8], want: usize) -> usize {
    if want >= buf.len() {
        return buf.len();
    }
    let mut cut = want;
    while cut > 0 && is_continuation(buf[cut]) {
        cut -= 1;
    }
    if cut > 0 {
        return cut;
    }
    cut = want.max(1);
    while cut < buf.len() && is_continuation(buf[cut]) {
        cut += 1;
    }
    cut
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

    fn output(part_id: &str, text: &str) -> SidecarPayload {
        SidecarPayload {
            part_id: part_id.into(),
            output: Some(text.into()),
            diff: None,
        }
    }

    fn store(root: &Path, limits: ToolOutputLimits) -> ToolOutputs {
        let outputs = ToolOutputs::new(root.to_path_buf());
        outputs.set_limits(limits);
        outputs
    }

    fn whole(outputs: &ToolOutputs, blob_ref: &str) -> String {
        outputs.read(blob_ref, 0, None).expect(blob_ref).text
    }

    fn age(outputs: &ToolOutputs, blob_ref: &str, by: Duration) {
        let path = outputs.path(blob_ref).expect("path");
        let file = std::fs::File::options()
            .write(true)
            .open(&path)
            .expect("open");
        file.set_modified(SystemTime::now() - by).expect("mtime");
    }

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
        let chunk = outputs.read("chat-1/p1", 0, None).expect("output");
        assert_eq!(
            chunk,
            ToolOutputChunk {
                text: "the whole output".into(),
                offset: 0,
                len: 16,
                total: 16,
                truncated: false,
                eof: true,
            }
        );
        let stored: loams_agentd_proto::ToolDiff =
            serde_json::from_str(&whole(&outputs, "chat-1/p1.diff")).expect("json");
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
                outputs.read(blob_ref, 0, None).is_err(),
                "{blob_ref} must be refused"
            );
        }
    }

    #[test]
    fn an_output_over_the_file_cap_is_cut_at_a_character_and_flagged() {
        let dir = tempfile::tempdir().expect("tempdir");
        let limits = ToolOutputLimits {
            file_cap: 5,
            ..ToolOutputLimits::default()
        };
        let outputs = store(dir.path(), limits);
        outputs.write("chat-1", &output("long", "0123456789"));
        // "abcd" + "é" (2 bytes) is 6 bytes: the cut falls inside "é".
        outputs.write("chat-1", &output("wide", "abcdé!"));
        outputs.write("chat-1", &output("short", "01234"));
        let long = outputs.read("chat-1/long", 0, None).expect("long");
        assert_eq!((long.text.as_str(), long.total), ("01234", 5));
        assert!(long.truncated);
        let wide = outputs.read("chat-1/wide", 0, None).expect("wide");
        assert_eq!(wide.text, "abcd");
        assert!(wide.truncated);
        let short = outputs.read("chat-1/short", 0, None).expect("short");
        assert_eq!(short.text, "01234");
        assert!(!short.truncated);
    }

    #[test]
    fn reads_are_paged_by_offset_and_len() {
        let dir = tempfile::tempdir().expect("tempdir");
        let outputs = ToolOutputs::new(dir.path().to_path_buf());
        outputs.write("chat-1", &output("p", "0123456789"));
        let first = outputs.read("chat-1/p", 0, Some(4)).expect("first");
        assert_eq!(
            (first.text.as_str(), first.len, first.eof),
            ("0123", 4, false)
        );
        let rest = outputs.read("chat-1/p", 4, Some(100)).expect("rest");
        assert_eq!(
            (rest.text.as_str(), rest.offset, rest.eof),
            ("456789", 4, true)
        );
        let past = outputs.read("chat-1/p", 50, None).expect("past");
        assert_eq!((past.text.as_str(), past.offset, past.eof), ("", 10, true));

        // A page never splits a character: "aé" is a, C3, A9.
        outputs.write("chat-1", &output("u", "aéz"));
        let a = outputs.read("chat-1/u", 0, Some(2)).expect("a");
        assert_eq!((a.text.as_str(), a.len), ("a", 1));
        let e = outputs.read("chat-1/u", 1, Some(1)).expect("é");
        assert_eq!((e.text.as_str(), e.len), ("é", 2));

        // One page is at most the max frame.
        let big = "x".repeat(usize::try_from(TOOL_OUTPUT_MAX_FRAME).unwrap_or(0) * 2 + 1);
        outputs.write("chat-1", &output("big", &big));
        let page = outputs.read("chat-1/big", 0, Some(u64::MAX)).expect("big");
        assert_eq!(page.len, TOOL_OUTPUT_MAX_FRAME);
        assert_eq!(page.total, TOOL_OUTPUT_MAX_FRAME * 2 + 1);
        assert!(!page.eof);
    }

    #[test]
    fn the_budget_evicts_the_oldest_outputs_first() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Each file is 1 header byte + 10 body bytes; three do not fit in 30.
        let limits = ToolOutputLimits {
            budget: 30,
            ..ToolOutputLimits::default()
        };
        let outputs = store(dir.path(), limits);
        outputs.write("chat-a", &output("p1", "0123456789"));
        age(&outputs, "chat-a/p1", Duration::from_secs(3 * 3600));
        outputs.write("chat-b", &output("p2", "0123456789"));
        age(&outputs, "chat-b/p2", Duration::from_secs(2 * 3600));
        outputs.write("chat-a", &output("p3", "0123456789"));
        assert!(
            outputs.read("chat-a/p1", 0, None).is_err(),
            "oldest evicted"
        );
        assert_eq!(whole(&outputs, "chat-b/p2"), "0123456789");
        assert_eq!(whole(&outputs, "chat-a/p3"), "0123456789");
    }

    #[test]
    fn archived_chats_lose_outputs_past_the_retention() {
        let dir = tempfile::tempdir().expect("tempdir");
        let outputs = ToolOutputs::new(dir.path().to_path_buf());
        let month = Duration::from_secs(31 * 24 * 3600);
        outputs.write("archived", &output("old", "o"));
        outputs.write("archived", &output("new", "n"));
        outputs.write("live", &output("old", "o"));
        age(&outputs, "archived/old", month);
        age(&outputs, "live/old", month);
        outputs.sweep_archived(&["archived".to_string()], SystemTime::now());
        assert!(outputs.read("archived/old", 0, None).is_err());
        assert_eq!(whole(&outputs, "archived/new"), "n");
        assert_eq!(
            whole(&outputs, "live/old"),
            "o",
            "a live chat keeps its outputs"
        );
    }
}
