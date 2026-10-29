//! The knowledge tools (MS1 Task 7, D1813): `knowledge_get` and
//! `knowledge_search` over the read-only project knowledge cache. Both are
//! `risk: read`; there is no write tool. Every manifest is verified against
//! the repository's pinned Sigstore signer and every entry against the
//! manifest's SHA-256, in `loams-knowledge`.
//!
//! The tools are listed only when `LOAMS_AGENTD_KNOWLEDGE_CONFIG` names a
//! reader config that loads.

use std::path::Path;
use std::sync::Arc;

use loams_knowledge::{KnowledgeReader, ReaderConfig};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::tools::{ToolDef, annotations_for};

/// The variable naming the reader's config file.
pub const ENV_KNOWLEDGE_CONFIG: &str = "LOAMS_AGENTD_KNOWLEDGE_CONFIG";

const GET: &str = "knowledge_get";
const SEARCH: &str = "knowledge_search";

/// Whether `name` is one of the knowledge tools.
pub(crate) fn is_tool(name: &str) -> bool {
    matches!(name, GET | SEARCH)
}

/// The knowledge tools' definitions.
pub(crate) fn defs() -> Vec<ToolDef> {
    let repo_commit = json!({
        "repo": { "type": "string", "description": "Repository name, e.g. ostrium-labs/loams." },
        "commit": { "type": "string", "description": "Full commit id; omit for the newest published commit." }
    });
    let with = |more: Value| {
        let mut props = repo_commit.clone();
        if let (Some(base), Some(more)) = (props.as_object_mut(), more.as_object()) {
            base.extend(more.clone());
        }
        props
    };
    let mut defs = vec![
        ToolDef {
            name: GET,
            description: "Read one file of a public repository's project knowledge cache (docs, READMEs, AGENTS.md, schemas, protos, and .loams/knowledge/files.txt listing every tracked path) at a commit. The file is verified against the commit's signed manifest. Read-only.",
            input_schema: json!({
                "type": "object",
                "properties": with(json!({ "path": { "type": "string", "description": "Path in the repository." } })),
                "required": ["repo", "path"],
            }),
            annotations: Value::Null,
        },
        ToolDef {
            name: SEARCH,
            description: "Search a public repository's project knowledge cache by path and text. Returns the best-matching paths with a snippet; read one with knowledge_get. Read-only.",
            input_schema: json!({
                "type": "object",
                "properties": with(json!({
                    "query": { "type": "string", "description": "Words to look for (case-insensitive)." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 50, "default": 10 }
                })),
                "required": ["repo", "query"],
            }),
            annotations: Value::Null,
        },
    ];
    for d in &mut defs {
        d.annotations = annotations_for(d.name);
    }
    defs
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GetArgs {
    repo: String,
    commit: Option<String>,
    path: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    repo: String,
    commit: Option<String>,
    query: String,
    limit: Option<usize>,
}

/// Runs a knowledge tool.
pub(crate) async fn call(
    reader: &KnowledgeReader,
    name: &str,
    args: Value,
) -> Result<Value, String> {
    let parse_err = |e: serde_json::Error| format!("invalid arguments: {e}");
    match name {
        GET => {
            let a: GetArgs = serde_json::from_value(args).map_err(parse_err)?;
            let f = reader
                .get(&a.repo, a.commit.as_deref(), &a.path)
                .await
                .map_err(|e| e.to_string())?;
            Ok(json!({
                "repo": f.repo,
                "commit": f.commit,
                "path": f.path,
                "sha256": f.sha256.to_hex(),
                "bytes": f.data.len(),
                "text": f.text(),
            }))
        }
        SEARCH => {
            let a: SearchArgs = serde_json::from_value(args).map_err(parse_err)?;
            let r = reader
                .search(
                    &a.repo,
                    a.commit.as_deref(),
                    &a.query,
                    a.limit.unwrap_or(10),
                )
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_value(r).map_err(|e| e.to_string())
        }
        other => Err(format!("unknown tool: {other}")),
    }
}

/// The reader for the config at `path`, or `None` with a warning on
/// stderr: a broken config leaves the tools out, it never stops the shim.
pub async fn load_reader(path: &Path) -> Option<Arc<KnowledgeReader>> {
    let built = match ReaderConfig::load(path).await {
        Ok(cfg) => cfg.reader().await,
        Err(e) => Err(e),
    };
    match built {
        Ok(reader) => Some(Arc::new(reader)),
        Err(e) => {
            tracing::warn!(config = %path.display(), error = %e, "knowledge tools disabled");
            None
        }
    }
}
