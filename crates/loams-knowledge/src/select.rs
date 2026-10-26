//! The knowledge set (Q1331; ruling 3 of MS1 Task 7): which tracked files
//! at a commit are published.

use bytes::Bytes;
use serde::Deserialize;

use crate::error::KnowledgeError;
use crate::manifest::check_path;
use crate::source::KnowledgeSource;

/// The repository's knowledge settings, read at the published commit.
pub const MARKER_PATH: &str = ".loams/knowledge.toml";

/// The synthetic entry listing every tracked path, one per line.
pub const FILES_ENTRY: &str = ".loams/knowledge/files.txt";

/// The largest file in the set.
pub const MAX_ENTRY_BYTES: u64 = 1024 * 1024;

/// `.loams/knowledge.toml`. It can mark the repository public and narrow
/// the knowledge set; it can never widen it.
///
/// ```toml
/// public = true
/// include = ["docs/design/", "README.md"]   # optional: only these
/// exclude = ["docs/plans/"]                  # optional
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct KnowledgeToml {
    /// The repository's own statement that its knowledge set is public.
    pub public: bool,
    /// Path prefixes (or exact paths) to keep; empty keeps everything.
    pub include: Vec<String>,
    /// Path prefixes (or exact paths) to drop.
    pub exclude: Vec<String>,
}

impl KnowledgeToml {
    /// Parses the file's text.
    pub fn parse(text: &str) -> Result<Self, KnowledgeError> {
        toml::from_str(text).map_err(|e| KnowledgeError::Invalid(format!("{MARKER_PATH}: {e}")))
    }

    fn keeps(&self, path: &str) -> bool {
        (self.include.is_empty() || self.include.iter().any(|p| under(path, p)))
            && !self.exclude.iter().any(|p| under(path, p))
    }
}

fn under(path: &str, prefix: &str) -> bool {
    let prefix = prefix.trim_matches('/');
    !prefix.is_empty()
        && (path == prefix
            || path
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('/')))
}

/// The default set by path alone: `docs/`, `schemas/`, `proto/`, any
/// `*.proto`, and the root `README*`, `AGENTS.md`, `CLAUDE.md` and
/// `.goosehints`.
fn in_default_set(path: &str) -> bool {
    if !path.contains('/') {
        return path.starts_with("README")
            || matches!(path, "AGENTS.md" | "CLAUDE.md" | ".goosehints");
    }
    under(path, "docs")
        || under(path, "schemas")
        || under(path, "proto")
        || path.ends_with(".proto")
}

fn vendored(path: &str) -> bool {
    path.split('/').any(|seg| {
        matches!(
            seg,
            "vendor" | "third-party" | "third_party" | "node_modules" | "generated"
        )
    })
}

/// Text, not generated: valid UTF-8, no NUL byte, and no `@generated`
/// marker in the first 1 KiB.
fn usable_text(data: &[u8]) -> bool {
    if data.contains(&0) || std::str::from_utf8(data).is_err() {
        return false;
    }
    let head = &data[..data.len().min(1024)];
    !head
        .windows(b"@generated".len())
        .any(|w| w == b"@generated")
}

/// The marker at `commit`, if the repository has one.
pub(crate) async fn marker(
    source: &dyn KnowledgeSource,
    commit: &str,
) -> Result<Option<KnowledgeToml>, KnowledgeError> {
    match source.read(commit, MARKER_PATH).await? {
        Some(data) => {
            let text = std::str::from_utf8(&data)
                .map_err(|_| KnowledgeError::Invalid(format!("{MARKER_PATH} is not UTF-8")))?;
            Ok(Some(KnowledgeToml::parse(text)?))
        }
        None => Ok(None),
    }
}

/// The knowledge set at `commit`, sorted by path, with [`FILES_ENTRY`].
pub(crate) async fn select(
    source: &dyn KnowledgeSource,
    commit: &str,
    settings: &KnowledgeToml,
) -> Result<Vec<(String, Bytes)>, KnowledgeError> {
    let mut tracked = source.tracked_files(commit).await?;
    tracked.sort_by(|a, b| a.path.cmp(&b.path));
    let mut listing = String::new();
    let mut set = Vec::new();
    for f in &tracked {
        listing.push_str(&f.path);
        listing.push('\n');
        let candidate = f.regular
            && f.size <= MAX_ENTRY_BYTES
            && f.path != FILES_ENTRY
            && check_path(&f.path).is_ok()
            && in_default_set(&f.path)
            && !vendored(&f.path)
            && settings.keeps(&f.path);
        if !candidate {
            continue;
        }
        let Some(data) = source.read(commit, &f.path).await? else {
            continue;
        };
        if data.len() as u64 > MAX_ENTRY_BYTES || !usable_text(&data) {
            continue;
        }
        set.push((f.path.clone(), data));
    }
    set.push((FILES_ENTRY.to_string(), Bytes::from(listing)));
    set.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(set)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_set_by_path() {
        for yes in [
            "README.md",
            "README",
            "AGENTS.md",
            "CLAUDE.md",
            ".goosehints",
            "docs/a.md",
            "docs/x/y.md",
            "schemas/a.json",
            "proto/a/b.proto",
            "crates/x/src/a.proto",
        ] {
            assert!(in_default_set(yes), "{yes}");
        }
        for no in [
            "src/main.rs",
            "docsx/a.md",
            "crates/x/README.md",
            "Cargo.toml",
            "LICENSE",
        ] {
            assert!(!in_default_set(no), "{no}");
        }
    }

    #[test]
    fn narrowing_by_prefix() {
        let t = KnowledgeToml {
            public: true,
            include: vec!["docs/design/".into(), "README.md".into()],
            exclude: vec!["docs/design/old".into()],
        };
        assert!(t.keeps("README.md"));
        assert!(t.keeps("docs/design/a.md"));
        assert!(!t.keeps("docs/designs.md"));
        assert!(!t.keeps("docs/design/old/a.md"));
        assert!(!t.keeps("docs/plans/a.md"));
    }

    #[test]
    fn text_only() {
        assert!(usable_text(b"hello"));
        assert!(!usable_text(b"a\0b"));
        assert!(!usable_text(&[0xff, 0xfe]));
        assert!(!usable_text(b"// @generated by x\nfoo"));
    }
}
