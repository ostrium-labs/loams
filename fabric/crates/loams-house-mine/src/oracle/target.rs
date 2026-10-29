//! The oracle's targets (`targets.toml`) and the HTTP client that runs a
//! statement on one of them.
//!
//! ```toml
//! [[target]]
//! name = "clickhouse-server@26.9"
//! url = "http://127.0.0.1:18123"
//! image = "docker.io/clickhouse/clickhouse-server:26.9.2.8@sha256:…"
//! baseline = true          # exactly one target is the oracle
//! user = "oracle"
//! password_env = "LOAMS_ORACLE_PASSWORD"
//! ```
//!
//! Every statement goes over the ClickHouse HTTP interface as a `POST` body;
//! a case's query gets `FORMAT JSONCompactEachRowWithNamesAndTypes` appended.

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Context as _, bail};
use serde::Deserialize;
use serde_json::Value;

use super::normalise::{Outcome, ResultSet};

/// The format every case's result is read in.
pub const FORMAT: &str = "JSONCompactEachRowWithNamesAndTypes";

/// `targets.toml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Targets {
    #[serde(rename = "target", default)]
    pub targets: Vec<Target>,
}

/// One `[[target]]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub name: String,
    pub url: String,
    /// The pinned image, for the record (the runner does not start it).
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub baseline: bool,
    #[serde(default)]
    pub user: Option<String>,
    /// The environment variable holding the password (never the password).
    #[serde(default)]
    pub password_env: Option<String>,
    #[serde(default)]
    pub database: Option<String>,
    /// Settings sent with every statement, as URL parameters.
    #[serde(default)]
    pub settings: BTreeMap<String, String>,
    /// Seeded from the baseline instead of running setup statements: after a
    /// corpus's setup, the baseline's tables are copied to it (`seed.rs`).
    #[serde(default)]
    pub seed: bool,
}

impl Targets {
    /// Reads `targets.toml` and checks it names one baseline and no name twice.
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let targets: Self =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        targets.validate()?;
        Ok(targets)
    }

    fn validate(&self) -> anyhow::Result<()> {
        let baselines = self.targets.iter().filter(|t| t.baseline).count();
        if baselines != 1 {
            bail!("targets.toml names {baselines} baselines; exactly one target is the oracle");
        }
        let mut names: Vec<&str> = self.targets.iter().map(|t| t.name.as_str()).collect();
        names.sort_unstable();
        if let Some(w) = names.windows(2).find(|w| w[0] == w[1]) {
            bail!("target {} is named twice", w[0]);
        }
        Ok(())
    }
}

/// A target could not be asked at all (refused connection, timeout): the run
/// stops, because no comparison is possible.
#[derive(Debug)]
pub struct Unreachable(pub String);

impl std::fmt::Display for Unreachable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Unreachable {}

/// An HTTP client for the targets.
#[derive(Debug)]
pub struct Client {
    agent: ureq::Agent,
}

impl Default for Client {
    fn default() -> Self {
        Self::new(Duration::from_secs(120))
    }
}

impl Client {
    pub fn new(timeout: Duration) -> Self {
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(timeout))
            .build()
            .into();
        Self { agent }
    }

    /// Runs a case's query and reads its result.
    pub fn query(&self, target: &Target, sql: &str) -> Result<Outcome, Unreachable> {
        let body = format!("{}\nFORMAT {FORMAT}", sql.trim_end().trim_end_matches(';'));
        let (status, code, text) = self.post(target, &body)?;
        Ok(parse_response(status, code.as_deref(), &text))
    }

    /// Runs a case's query in `format` and returns the answer's bytes, or the
    /// error outcome.
    pub fn query_bytes(
        &self,
        target: &Target,
        sql: &str,
        format: &str,
    ) -> Result<Result<Vec<u8>, Outcome>, Unreachable> {
        let body = format!("{}\nFORMAT {format}", sql.trim_end().trim_end_matches(';'));
        Ok(match self.fetch(target, &body)? {
            Ok(bytes) => Ok(bytes),
            Err(text) => Err(error_outcome(None, &text)),
        })
    }

    /// Runs a setup statement; `Ok(None)` when it succeeded, else the error.
    pub fn execute(&self, target: &Target, sql: &str) -> Result<Option<Outcome>, Unreachable> {
        let (status, code, text) = self.post(target, sql)?;
        if status == 200 && code.is_none() {
            Ok(None)
        } else {
            Ok(Some(error_outcome(code.as_deref(), &text)))
        }
    }

    /// Runs `sql` and returns the raw answer bytes (`FORMAT Parquet`), or the
    /// error text.
    pub fn fetch(
        &self,
        target: &Target,
        sql: &str,
    ) -> Result<Result<Vec<u8>, String>, Unreachable> {
        let mut request = self.request(target, &[]);
        request = request.header("Content-Type", "text/plain");
        let mut response = request
            .send(sql)
            .map_err(|e| Unreachable(format!("{}: {e}", target.name)))?;
        let ok = response.status().as_u16() == 200
            && response
                .headers()
                .get("X-ClickHouse-Exception-Code")
                .is_none();
        let bytes = response
            .body_mut()
            .with_config()
            .limit(1024 * 1024 * 1024)
            .read_to_vec()
            .map_err(|e| Unreachable(format!("{}: reading the answer: {e}", target.name)))?;
        Ok(if ok {
            Ok(bytes)
        } else {
            Err(String::from_utf8_lossy(&bytes).into_owned())
        })
    }

    /// Posts `body` to `path` on the target with extra query parameters and
    /// headers; the error text when the target refused it.
    pub fn post_to(
        &self,
        target: &Target,
        path: &str,
        params: &[(String, String)],
        headers: &[(&str, String)],
        body: &[u8],
    ) -> Result<Result<(), String>, Unreachable> {
        let url = format!(
            "{}/{}",
            target.url.trim_end_matches('/'),
            path.trim_start_matches('/')
        );
        let mut request = self
            .agent
            .post(&url)
            .query_pairs(params.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        for (k, v) in headers {
            request = request.header(*k, v);
        }
        let mut response = request
            .send(body)
            .map_err(|e| Unreachable(format!("{}: {e}", target.name)))?;
        if response.status().as_u16() == 200 {
            Ok(Ok(()))
        } else {
            Ok(Err(response
                .body_mut()
                .read_to_string()
                .unwrap_or_default()))
        }
    }

    fn request(
        &self,
        target: &Target,
        extra: &[(String, String)],
    ) -> ureq::RequestBuilder<ureq::typestate::WithBody> {
        let mut params: Vec<(String, String)> = Vec::new();
        if let Some(db) = &target.database {
            params.push(("database".into(), db.clone()));
        }
        params.extend(target.settings.iter().map(|(k, v)| (k.clone(), v.clone())));
        params.extend(extra.iter().cloned());
        let mut request = self.agent.post(&target.url).query_pairs(params);
        if let Some(user) = &target.user {
            request = request.header("X-ClickHouse-User", user);
        }
        if let Some(var) = &target.password_env {
            let password = std::env::var(var).unwrap_or_default();
            request = request.header("X-ClickHouse-Key", password);
        }
        request
    }

    fn post(
        &self,
        target: &Target,
        body: &str,
    ) -> Result<(u16, Option<String>, String), Unreachable> {
        let mut params: Vec<(String, String)> = Vec::new();
        if let Some(db) = &target.database {
            params.push(("database".into(), db.clone()));
        }
        params.extend(target.settings.iter().map(|(k, v)| (k.clone(), v.clone())));
        let mut request = self.agent.post(&target.url).query_pairs(params);
        if let Some(user) = &target.user {
            request = request.header("X-ClickHouse-User", user);
        }
        if let Some(var) = &target.password_env {
            let password = std::env::var(var).unwrap_or_default();
            request = request.header("X-ClickHouse-Key", password);
        }
        let mut response = request
            .send(body)
            .map_err(|e| Unreachable(format!("{}: {e}", target.name)))?;
        let status = response.status().as_u16();
        let code = response
            .headers()
            .get("X-ClickHouse-Exception-Code")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let text = response
            .body_mut()
            .with_config()
            .limit(256 * 1024 * 1024)
            .read_to_string()
            .map_err(|e| Unreachable(format!("{}: reading the answer: {e}", target.name)))?;
        Ok((status, code, text))
    }
}

/// Reads an answer in [`FORMAT`]. A non-200 status, an exception code header,
/// or a line that is not a JSON array (an exception written after the rows
/// began) is an error outcome.
pub fn parse_response(status: u16, code: Option<&str>, text: &str) -> Outcome {
    if status != 200 || code.is_some() {
        return error_outcome(code, text);
    }
    let mut parsed = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        match serde_json::from_str::<Value>(line) {
            Ok(Value::Array(items)) => parsed.push(items),
            _ => return error_outcome(None, text),
        }
    }
    let mut parsed = parsed.into_iter();
    let strings = |row: Option<Vec<Value>>| -> Vec<String> {
        row.unwrap_or_default()
            .into_iter()
            .map(|v| match v {
                Value::String(s) => s,
                other => other.to_string(),
            })
            .collect()
    };
    let names = strings(parsed.next());
    let types = strings(parsed.next());
    Outcome::Rows(ResultSet {
        names,
        types,
        rows: parsed.collect(),
    })
}

/// The exception in an answer: the code from the header, else from the
/// `Code: N.` in the text.
fn error_outcome(code: Option<&str>, text: &str) -> Outcome {
    let from_text = text.find("Code: ").and_then(|at| {
        let digits: String = text[at + 6..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        digits.parse().ok()
    });
    let code = code
        .and_then(|c| c.trim().parse().ok())
        .or(from_text)
        .unwrap_or(0);
    let message = text
        .lines()
        .rev()
        .find(|l| l.contains("Code: "))
        .or_else(|| text.lines().next())
        .unwrap_or("")
        .trim();
    // The server's version at the end changes with every pin, not the error.
    let message = match message.rfind(" (version ") {
        Some(at) => &message[..at],
        None => message,
    };
    Outcome::Error {
        code,
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn answers_parse_into_names_types_and_rows() {
        let text = "[\"k\",\"s\"]\n[\"UInt64\",\"String\"]\n[\"1\",\"a\"]\n[\"2\",\"b\"]\n";
        let Outcome::Rows(rows) = parse_response(200, None, text) else {
            panic!("rows");
        };
        assert_eq!(rows.names, ["k", "s"]);
        assert_eq!(rows.types, ["UInt64", "String"]);
        assert_eq!(
            rows.rows,
            vec![vec![json!("1"), json!("a")], vec![json!("2"), json!("b")]]
        );
    }

    #[test]
    fn exceptions_are_errors_with_their_code() {
        let text =
            "Code: 47. DB::Exception: Unknown expression identifier `x`. (UNKNOWN_IDENTIFIER)";
        assert!(matches!(
            parse_response(404, Some("47"), text),
            Outcome::Error { code: 47, .. }
        ));
        assert!(matches!(
            parse_response(500, None, text),
            Outcome::Error { code: 47, .. }
        ));
        let mid_stream = "[\"k\"]\n[\"UInt8\"]\n[1]\nCode: 395. DB::Exception: thrown. (FUNCTION_THROW_IF_VALUE_IS_NON_ZERO)\n";
        assert!(matches!(
            parse_response(200, None, mid_stream),
            Outcome::Error { code: 395, .. }
        ));
    }
}
