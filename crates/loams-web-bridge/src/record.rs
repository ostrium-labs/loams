//! Recording: an action log per page, one JSON line per tool call (D2112).
//!
//! Off by default on every provider. When it is on, each page writes
//! `<artifact dir>/<page>.actions.jsonl`: which tool ran, on which (redacted)
//! URL or uid, whether it worked, the scrubbed error if it did not, and how
//! long it took. A value typed into a page is never recorded, not even its
//! length, and every line goes through [`crate::redact::scrub`] so a
//! registered secret cannot reach the file through an error message.
//!
//! Video recording of what the page rendered stays with Desktop's sidebar
//! (`loams_sidebar_browser::VideoRecorder`, D628); this log is what a headless
//! agent run leaves behind for review.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::error::BridgeError;

/// One page's action log.
#[derive(Debug)]
pub struct ActionLog {
    path: PathBuf,
    file: Mutex<std::fs::File>,
}

impl ActionLog {
    /// Open (or append to) the log for `page` under `dir`.
    pub fn open(dir: &Path, page: &str) -> Result<Self, BridgeError> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(format!("{}.actions.jsonl", file_safe(page)));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        Ok(Self {
            path,
            file: Mutex::new(file),
        })
    }

    /// Where the log is.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Record one call. A write failure is logged and otherwise ignored: a
    /// full disk must not fail the agent's browsing.
    pub fn record(&self, tool: &str, target: &str, error: Option<&BridgeError>, took: Duration) {
        let line = serde_json::json!({
            "at_ms": SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
                .unwrap_or(0),
            "tool": tool,
            "target": crate::redact::scrub(target),
            "ok": error.is_none(),
            "error": error.map(|error| crate::redact::scrub(&error.to_string())),
            "took_ms": u64::try_from(took.as_millis()).unwrap_or(u64::MAX),
        });
        let mut file = self
            .file
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Err(error) = writeln!(file, "{line}") {
            tracing::warn!(path = %self.path.display(), %error, "the browser action log could not be written");
        }
    }
}

/// A page id as a file name: letters, digits, `-` and `_` only.
fn file_safe(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "page".to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_json_and_carries_no_registered_secret() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let log =
            ActionLog::open(dir.path(), "obscura/1:2").unwrap_or_else(|error| panic!("{error}"));
        assert!(log.path().ends_with("obscura_1_2.actions.jsonl"));
        crate::redact::register("canary-record-7c1d9e");
        log.record(
            "navigate",
            "example.com/login",
            Some(&BridgeError::cdp("refused canary-record-7c1d9e")),
            Duration::from_millis(12),
        );
        log.record("snapshot", "", None, Duration::from_millis(3));
        let written = std::fs::read_to_string(log.path()).unwrap_or_else(|error| panic!("{error}"));
        assert!(!written.contains("canary-record-7c1d9e"), "{written}");
        let lines: Vec<serde_json::Value> = written
            .lines()
            .map(|line| serde_json::from_str(line).unwrap_or_else(|error| panic!("{error}")))
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["tool"], "navigate");
        assert_eq!(lines[0]["ok"], false);
        assert_eq!(lines[1]["ok"], true);
    }
}
