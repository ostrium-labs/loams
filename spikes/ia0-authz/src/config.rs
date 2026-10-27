//! Settings from the environment, with `.env.ia0` (written by `conformance/identity/bootstrap.sh`)
//! as the fallback for anything unset.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};

#[derive(Clone, Debug)]
pub struct Config {
    vars: HashMap<String, String>,
}

impl Config {
    /// Reads `IA0_ENV_FILE` (default `conformance/identity/.env.ia0` from the repository root, or
    /// `.env.ia0` in the working directory), then lets the process environment override it.
    pub fn load() -> Result<Self> {
        let mut vars = HashMap::new();
        let candidates = [
            std::env::var("IA0_ENV_FILE").ok(),
            Some("conformance/identity/.env.ia0".to_owned()),
            Some("../../conformance/identity/.env.ia0".to_owned()),
            Some(".env.ia0".to_owned()),
        ];
        for path in candidates.into_iter().flatten() {
            if Path::new(&path).is_file() {
                let text =
                    std::fs::read_to_string(&path).with_context(|| format!("read {path}"))?;
                for line in text.lines() {
                    let line = line.trim();
                    if line.is_empty() || line.starts_with('#') {
                        continue;
                    }
                    if let Some((k, v)) = line.split_once('=') {
                        vars.insert(k.trim().to_owned(), v.trim().to_owned());
                    }
                }
                vars.insert("IA0_ENV_FILE_USED".to_owned(), path);
                break;
            }
        }
        for (k, v) in std::env::vars() {
            if k.starts_with("IA0_") {
                vars.insert(k, v);
            }
        }
        Ok(Self { vars })
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.vars
            .get(key)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }

    pub fn req(&self, key: &str) -> Result<&str> {
        self.get(key)
            .with_context(|| format!("{key} is not set (run conformance/identity/up.sh)"))
    }

    pub fn or<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        self.get(key).unwrap_or(default)
    }

    /// The directory `.env.ia0` was read from, for resolving relative paths such as the agent key.
    pub fn base_dir(&self) -> &Path {
        self.get("IA0_ENV_FILE_USED")
            .and_then(|p| Path::new(p).parent())
            .unwrap_or_else(|| Path::new("."))
    }

    pub fn path(&self, key: &str) -> Result<std::path::PathBuf> {
        let p = Path::new(self.req(key)?);
        Ok(if p.is_absolute() {
            p.to_owned()
        } else {
            self.base_dir().join(p)
        })
    }
}
