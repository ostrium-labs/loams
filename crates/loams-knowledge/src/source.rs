//! Where a knowledge set is read from: a repository at one commit.

use std::collections::BTreeMap;
use std::path::PathBuf;

use async_trait::async_trait;
use bytes::Bytes;

use crate::error::KnowledgeError;

/// One tracked path at a commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackedFile {
    /// The path, `/`-separated, relative to the repository's root.
    pub path: String,
    /// Its size in bytes (0 for a submodule).
    pub size: u64,
    /// A regular file: not a symlink and not a submodule.
    pub regular: bool,
}

/// A repository's tracked files at a commit.
#[async_trait]
pub trait KnowledgeSource: Send + Sync {
    /// Every tracked path at `commit`.
    async fn tracked_files(&self, commit: &str) -> Result<Vec<TrackedFile>, KnowledgeError>;
    /// The bytes of `path` at `commit`; `None` if it is not tracked there.
    async fn read(&self, commit: &str, path: &str) -> Result<Option<Bytes>, KnowledgeError>;
}

/// A local git repository, read with the `git` CLI. Only committed
/// content is read: the working tree and index are never looked at.
#[derive(Clone, Debug)]
pub struct GitCheckout {
    dir: PathBuf,
}

impl GitCheckout {
    /// The repository at `dir` (a work tree or a bare repository).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    async fn git(&self, args: &[&str]) -> Result<std::process::Output, KnowledgeError> {
        tokio::process::Command::new("git")
            .arg("-C")
            .arg(&self.dir)
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
            .await
            .map_err(|e| KnowledgeError::Source(format!("running git: {e}")))
    }
}

#[async_trait]
impl KnowledgeSource for GitCheckout {
    async fn tracked_files(&self, commit: &str) -> Result<Vec<TrackedFile>, KnowledgeError> {
        let out = self
            .git(&["ls-tree", "-r", "-z", "--long", "--full-tree", commit])
            .await?;
        if !out.status.success() {
            return Err(KnowledgeError::Source(format!(
                "git ls-tree {commit}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        let mut files = Vec::new();
        for rec in out.stdout.split(|b| *b == 0).filter(|r| !r.is_empty()) {
            // `<mode> <type> <object> <size>\t<path>`
            let rec = std::str::from_utf8(rec)
                .map_err(|_| KnowledgeError::Source("a path is not UTF-8".into()))?;
            let (meta, path) = rec
                .split_once('\t')
                .ok_or_else(|| KnowledgeError::Source(format!("git ls-tree record {rec:?}")))?;
            let mut fields = meta.split_whitespace();
            let mode = fields.next().unwrap_or_default();
            let kind = fields.next().unwrap_or_default();
            let size = fields.nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            files.push(TrackedFile {
                path: path.to_string(),
                size,
                regular: kind == "blob" && matches!(mode, "100644" | "100755"),
            });
        }
        Ok(files)
    }

    async fn read(&self, commit: &str, path: &str) -> Result<Option<Bytes>, KnowledgeError> {
        let spec = format!("{commit}:{path}");
        let out = self.git(&["cat-file", "blob", &spec]).await?;
        if out.status.success() {
            Ok(Some(Bytes::from(out.stdout)))
        } else {
            Ok(None)
        }
    }
}

/// Files already in memory, the same at every commit: for tests and for
/// callers that hold the tree themselves.
#[derive(Clone, Debug, Default)]
pub struct MemorySource {
    files: BTreeMap<String, (Bytes, bool)>,
}

impl MemorySource {
    /// An empty source.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a regular file.
    pub fn file(mut self, path: &str, data: impl Into<Bytes>) -> Self {
        self.files.insert(path.to_string(), (data.into(), true));
        self
    }

    /// Adds a symlink (not regular; its target as the content).
    pub fn symlink(mut self, path: &str, target: &str) -> Self {
        self.files
            .insert(path.to_string(), (Bytes::from(target.to_string()), false));
        self
    }
}

#[async_trait]
impl KnowledgeSource for MemorySource {
    async fn tracked_files(&self, _commit: &str) -> Result<Vec<TrackedFile>, KnowledgeError> {
        Ok(self
            .files
            .iter()
            .map(|(path, (data, regular))| TrackedFile {
                path: path.clone(),
                size: data.len() as u64,
                regular: *regular,
            })
            .collect())
    }

    async fn read(&self, _commit: &str, path: &str) -> Result<Option<Bytes>, KnowledgeError> {
        Ok(self.files.get(path).map(|(d, _)| d.clone()))
    }
}
