//! Writing a secret into the user's cloud secret store (D1827).
//!
//! Through the cloud's own CLI, with the credentials the user already has
//! (`aws`, `gcloud`, `az`): no cloud SDK in the binary and no credential of
//! ours. The value goes to the CLI on stdin (`file:///dev/stdin`,
//! `--data-file=-`, `--file /dev/stdin`), never in argv, so it shows in no
//! process list. Each write adds a version; a secret that does not exist yet
//! is created.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Mutex;

use tokio::io::AsyncWriteExt;

use crate::{Error, Secret};

/// Somewhere a secret can be written by name.
pub trait SecretSink {
    /// Writes `value` as the newest version of secret `name`.
    fn put(&self, name: &str, value: &Secret) -> impl Future<Output = Result<(), Error>> + Send;
}

/// The cloud whose secret store an install uses, with what its CLI needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// AWS Secrets Manager (`aws`); `region` or the CLI's default.
    Aws { region: Option<String> },
    /// Google Cloud Secret Manager (`gcloud`); `project` or the CLI's default.
    Gcp { project: Option<String> },
    /// Azure Key Vault (`az`).
    Azure { vault: String },
}

impl Target {
    /// The full secret name for `<prefix>/confluent-<integration>-<short>`,
    /// with BY1's rule: Google Cloud and Azure names cannot hold `/`, so it
    /// becomes `-` (`loams/prod` -> `loams-prod-confluent-…`).
    #[must_use]
    pub fn secret_name(&self, prefix: &str, integration: &str, short: &str) -> String {
        match self {
            Target::Aws { .. } => format!("{prefix}/confluent-{integration}-{short}"),
            _ => format!(
                "{}-confluent-{integration}-{short}",
                prefix.replace('/', "-")
            ),
        }
    }

    fn default_program(&self) -> &'static str {
        match self {
            Target::Aws { .. } => "aws",
            Target::Gcp { .. } => "gcloud",
            Target::Azure { .. } => "az",
        }
    }
}

/// Writes with the cloud's CLI.
#[derive(Debug, Clone)]
pub struct CliStore {
    target: Target,
    program: PathBuf,
}

impl CliStore {
    /// The CLI from `PATH` (`aws`, `gcloud` or `az`).
    #[must_use]
    pub fn new(target: Target) -> Self {
        let program = PathBuf::from(target.default_program());
        Self { target, program }
    }

    /// Another program in the CLI's place (tests, or a wrapper).
    #[must_use]
    pub fn with_program(mut self, program: impl Into<PathBuf>) -> Self {
        self.program = program.into();
        self
    }

    #[must_use]
    pub fn target(&self) -> &Target {
        &self.target
    }

    /// The write and, when the secret does not exist, the create.
    fn commands(&self, name: &str) -> (Vec<String>, Vec<String>, &'static str) {
        let s = |v: &[&str]| v.iter().map(|x| (*x).to_string()).collect::<Vec<_>>();
        match &self.target {
            Target::Aws { region } => {
                let mut put = s(&[
                    "secretsmanager",
                    "put-secret-value",
                    "--secret-id",
                    name,
                    "--secret-string",
                    "file:///dev/stdin",
                    "--output",
                    "json",
                ]);
                let mut create = s(&[
                    "secretsmanager",
                    "create-secret",
                    "--name",
                    name,
                    "--description",
                    "Loams: a Confluent Cloud key (loams byoc confluent key)",
                    "--secret-string",
                    "file:///dev/stdin",
                    "--output",
                    "json",
                ]);
                if let Some(r) = region {
                    for v in [&mut put, &mut create] {
                        v.extend(["--region".to_string(), r.clone()]);
                    }
                }
                (put, create, "ResourceNotFoundException")
            }
            Target::Gcp { project } => {
                let mut put = s(&[
                    "secrets",
                    "versions",
                    "add",
                    name,
                    "--data-file=-",
                    "--quiet",
                ]);
                let mut create = s(&[
                    "secrets",
                    "create",
                    name,
                    "--replication-policy=automatic",
                    "--data-file=-",
                    "--quiet",
                ]);
                if let Some(p) = project {
                    for v in [&mut put, &mut create] {
                        v.push(format!("--project={p}"));
                    }
                }
                (put, create, "NOT_FOUND")
            }
            Target::Azure { vault } => {
                // `set` creates the secret or adds a version.
                let put = s(&[
                    "keyvault",
                    "secret",
                    "set",
                    "--vault-name",
                    vault,
                    "--name",
                    name,
                    "--file",
                    "/dev/stdin",
                    "--encoding",
                    "utf-8",
                    "--only-show-errors",
                    "--output",
                    "none",
                ]);
                (put, Vec::new(), "")
            }
        }
    }

    /// Runs the CLI with `value` on stdin; returns (success, stderr).
    async fn run(&self, args: &[String], value: &Secret) -> Result<(bool, String), Error> {
        let mut child = tokio::process::Command::new(&self.program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                Error::Store(format!(
                    "could not run {} ({e}); is the cloud's CLI installed and logged in?",
                    self.program.display()
                ))
            })?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(value.expose().as_bytes())
                .await
                .map_err(|e| Error::Store(format!("writing to {}: {e}", self.program.display())))?;
            // Dropping stdin closes it, so the CLI sees the end of the value.
        }
        let out = child
            .wait_with_output()
            .await
            .map_err(|e| Error::Store(format!("{}: {e}", self.program.display())))?;
        let err = String::from_utf8_lossy(&out.stderr);
        Ok((out.status.success(), err.chars().take(400).collect()))
    }
}

impl SecretSink for CliStore {
    async fn put(&self, name: &str, value: &Secret) -> Result<(), Error> {
        let (put, create, missing) = self.commands(name);
        let (ok, err) = self.run(&put, value).await?;
        if ok {
            return Ok(());
        }
        if !missing.is_empty() && err.contains(missing) {
            let (ok, err2) = self.run(&create, value).await?;
            if ok {
                return Ok(());
            }
            return Err(Error::Store(format!("creating {name}: {}", err2.trim())));
        }
        Err(Error::Store(format!("writing {name}: {}", err.trim())))
    }
}

/// An in-memory store, for tests and dry runs.
#[derive(Debug, Default)]
pub struct MemoryStore {
    values: Mutex<BTreeMap<String, Vec<String>>>,
    fail: Mutex<Option<String>>,
}

impl MemoryStore {
    /// Every version written to `name`, oldest first.
    #[must_use]
    pub fn versions(&self, name: &str) -> Vec<String> {
        self.values
            .lock()
            .map(|m| m.get(name).cloned().unwrap_or_default())
            .unwrap_or_default()
    }

    /// The names written so far.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.values
            .lock()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Makes writes to names containing `part` fail.
    pub fn fail_on(&self, part: &str) {
        if let Ok(mut f) = self.fail.lock() {
            *f = Some(part.to_string());
        }
    }
}

impl SecretSink for MemoryStore {
    async fn put(&self, name: &str, value: &Secret) -> Result<(), Error> {
        let fail = self.fail.lock().ok().and_then(|f| f.clone());
        if fail.is_some_and(|p| name.contains(&p)) {
            return Err(Error::Store(format!("writing {name}: refused (test)")));
        }
        if let Ok(mut m) = self.values.lock() {
            m.entry(name.to_string())
                .or_default()
                .push(value.expose().to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_by1() {
        let aws = Target::Aws { region: None };
        let gcp = Target::Gcp { project: None };
        assert_eq!(
            aws.secret_name("loams/prod", "confluent", "api-secret"),
            "loams/prod/confluent-confluent-api-secret"
        );
        assert_eq!(
            gcp.secret_name("loams/prod", "events", "api-key"),
            "loams-prod-confluent-events-api-key"
        );
    }

    #[test]
    fn the_value_is_never_an_argument() {
        for t in [
            Target::Aws {
                region: Some("us-east-1".into()),
            },
            Target::Gcp {
                project: Some("p".into()),
            },
            Target::Azure { vault: "v".into() },
        ] {
            let (put, create, _) = CliStore::new(t).commands("n");
            assert!(
                put.iter()
                    .chain(&create)
                    .any(|a| a.contains("stdin") || a == "--data-file=-")
            );
        }
    }
}
