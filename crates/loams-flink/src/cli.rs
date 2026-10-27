//! `loams flink`: the client for managed Flink SQL (D1989). Preview.
//!
//! ```text
//! loams flink -n <ns> -e <instance> sql                 # interactive REPL
//! loams flink -n <ns> -e <instance> sql -x 'SHOW CATALOGS'
//! loams flink -n <ns> -e <instance> sql -f pipeline.sql
//! loams flink -n <ns> -e <instance> jobs list|get|create|start|stop|resume|delete
//! loams flink -n <ns> -e <instance> bootstrap          # the session setup, redacted
//! loams flink bootstrap --config session.toml          # render a spec locally
//! loams flink jars                                      # the connector-jar bundle
//! ```
//!
//! It talks to the Loams API (`--url`, `LOAMS_URL`) with a bearer token
//! (`LOAMS_TOKEN`), never to Flink directly.

use std::io::{IsTerminal as _, Write as _};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Subcommand};
use serde_json::{Value, json};
use tokio::io::AsyncBufReadExt as _;

use crate::api::split::split_statements;
use crate::catalog::{RenderCtx, SessionSpec};
use crate::connectors::jars;

/// `loams flink …`.
#[derive(Debug, Args)]
pub struct FlinkArgs {
    /// The Loams API.
    #[arg(
        long,
        env = "LOAMS_URL",
        default_value = "http://127.0.0.1:8080",
        global = true
    )]
    pub url: String,
    /// A Loams API key or token.
    #[arg(long, env = "LOAMS_TOKEN", hide_env_values = true, global = true)]
    pub token: Option<String>,
    /// The Loams namespace.
    #[arg(long, short = 'n', env = "LOAMS_NAMESPACE", global = true)]
    pub namespace: Option<String>,
    /// The Flink engine instance (`eng_…`).
    #[arg(long, short = 'e', env = "LOAMS_FLINK_ENGINE", global = true)]
    pub engine: Option<String>,
    #[command(subcommand)]
    pub command: FlinkCommand,
}

#[derive(Debug, Subcommand)]
pub enum FlinkCommand {
    /// Run Flink SQL: a REPL on a terminal, else the statements of `-x`,
    /// `-f` or stdin.
    Sql {
        /// Statements to run (`;`-separated), then exit.
        #[arg(long, short = 'x')]
        execute: Option<String>,
        /// A file of statements to run, then exit.
        #[arg(long, short = 'f')]
        file: Option<PathBuf>,
        /// Rows to show per statement before cutting a result off.
        #[arg(long, default_value_t = 1000)]
        max_rows: usize,
    },
    /// Saved SQL jobs (INSERT INTO pipelines).
    Jobs {
        #[command(subcommand)]
        command: JobsCommand,
    },
    /// Print the session bootstrap script (secrets redacted): the
    /// instance's from the API, or a local session spec's (`--config`).
    Bootstrap {
        /// A session spec (TOML), rendered locally.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Print the connector-jar bundle the Loams Flink image carries.
    Jars {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum JobsCommand {
    List,
    Get {
        id: String,
    },
    /// Create a job from a SQL file: its last statement is the job (one
    /// INSERT or EXECUTE STATEMENT SET), the ones before it its setup.
    Create {
        #[arg(long)]
        name: String,
        #[arg(long)]
        file: PathBuf,
        /// Extra session tables and catalogs (TOML session spec; secret
        /// references only).
        #[arg(long)]
        session: Option<PathBuf>,
        #[arg(long)]
        description: Option<String>,
    },
    Start {
        id: String,
    },
    /// Stop with a savepoint (resume continues from it).
    Stop {
        id: String,
        /// Stop without a savepoint: the job cannot be resumed.
        #[arg(long)]
        no_savepoint: bool,
        /// Flush event-time windows first (for a final stop).
        #[arg(long)]
        drain: bool,
    },
    Resume {
        id: String,
    },
    Delete {
        id: String,
    },
}

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error("{0}")]
    Usage(String),
    #[error("{code}: {message}")]
    Api { code: String, message: String },
    #[error("cannot reach the Loams API: {0}")]
    Http(String),
    #[error("{0}")]
    Io(String),
}

/// Run `loams flink …`.
pub fn run(args: &FlinkArgs) -> ExitCode {
    let _ =
        rustls::crypto::CryptoProvider::install_default(rustls::crypto::ring::default_provider());
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("loams flink: {e}");
            return ExitCode::FAILURE;
        }
    };
    match rt.block_on(main(args)) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("loams flink: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn main(args: &FlinkArgs) -> Result<ExitCode, CliError> {
    match &args.command {
        FlinkCommand::Jars { json } => {
            print_jars(*json);
            Ok(ExitCode::SUCCESS)
        }
        FlinkCommand::Bootstrap { config: Some(path) } => {
            let text = std::fs::read_to_string(path)
                .map_err(|e| CliError::Io(format!("{}: {e}", path.display())))?;
            let spec: SessionSpec = toml::from_str(&text)
                .map_err(|e| CliError::Usage(format!("{}: {e}", path.display())))?;
            let script = spec
                .script()
                .and_then(|s| s.render_script(&RenderCtx::redacted()))
                .map_err(|e| CliError::Usage(e.to_string()))?;
            print!("{script}");
            Ok(ExitCode::SUCCESS)
        }
        FlinkCommand::Bootstrap { config: None } => {
            let c = Client::new(args)?;
            let v = c.call(reqwest::Method::GET, "/bootstrap", None).await?;
            print!("{}", v["script"].as_str().unwrap_or_default());
            Ok(ExitCode::SUCCESS)
        }
        FlinkCommand::Jobs { command } => jobs(&Client::new(args)?, command).await,
        FlinkCommand::Sql {
            execute,
            file,
            max_rows,
        } => {
            let c = Client::new(args)?;
            let text = match (execute, file) {
                (Some(_), Some(_)) => {
                    return Err(CliError::Usage("give -x or -f, not both".into()));
                }
                (Some(sql), None) => Some(sql.clone()),
                (None, Some(path)) => Some(
                    std::fs::read_to_string(path)
                        .map_err(|e| CliError::Io(format!("{}: {e}", path.display())))?,
                ),
                (None, None) if !std::io::stdin().is_terminal() => {
                    let mut s = String::new();
                    std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)
                        .map_err(|e| CliError::Io(e.to_string()))?;
                    Some(s)
                }
                (None, None) => None,
            };
            match text {
                Some(t) => batch(&c, &t, *max_rows).await,
                None => repl(&c, *max_rows).await,
            }
        }
    }
}

fn print_jars(as_json: bool) {
    if as_json {
        let v = json!({
            "flink_version": jars::FLINK_VERSION,
            "base_image": jars::FLINK_IMAGE,
            "image": jars::image_ref(),
            "jars": jars::BUNDLE,
        });
        println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
        return;
    }
    println!(
        "Flink {} ({}), bundle {} -> {}",
        jars::FLINK_VERSION,
        jars::FLINK_IMAGE,
        jars::BUNDLE_REVISION,
        jars::image_ref()
    );
    for j in jars::BUNDLE {
        println!(
            "  {:<36} {:<12} {:<13} sha256:{}",
            j.artifact, j.version, j.license, j.sha256
        );
    }
}

/// The API client, scoped to one namespace and instance.
struct Client {
    http: reqwest::Client,
    base: String,
    token: Option<String>,
}

impl Client {
    fn new(args: &FlinkArgs) -> Result<Self, CliError> {
        let ns = args.namespace.as_deref().ok_or_else(|| {
            CliError::Usage("set the namespace: --namespace/-n or LOAMS_NAMESPACE".into())
        })?;
        let engine = args.engine.as_deref().ok_or_else(|| {
            CliError::Usage("set the Flink instance: --engine/-e or LOAMS_FLINK_ENGINE".into())
        })?;
        for (what, v) in [("namespace", ns), ("engine", engine)] {
            if v.is_empty()
                || !v
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            {
                return Err(CliError::Usage(format!(
                    "{what} {v:?} is not [A-Za-z0-9_-]+"
                )));
            }
        }
        let http = reqwest::Client::builder()
            .build()
            .map_err(|e| CliError::Http(e.to_string()))?;
        Ok(Self {
            http,
            base: format!(
                "{}/v1/namespaces/{ns}/engines/{engine}/flink",
                args.url.trim_end_matches('/')
            ),
            token: args.token.clone(),
        })
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let req = self.http.request(method, format!("{}{path}", self.base));
        match &self.token {
            Some(t) => req.bearer_auth(t),
            None => req,
        }
    }

    async fn error_of(resp: reqwest::Response) -> CliError {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        match serde_json::from_str::<Value>(&text) {
            Ok(v) if v.get("error").is_some() => CliError::Api {
                code: v["error"].as_str().unwrap_or("error").to_string(),
                message: v["message"].as_str().unwrap_or_default().to_string(),
            },
            _ => CliError::Api {
                code: status.as_u16().to_string(),
                message: text,
            },
        }
    }

    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, CliError> {
        let mut req = self.request(method, path);
        if let Some(b) = body {
            req = req
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(b.to_string());
        }
        let resp = req
            .send()
            .await
            .map_err(|e| CliError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(Self::error_of(resp).await);
        }
        let text = resp
            .text()
            .await
            .map_err(|e| CliError::Http(e.to_string()))?;
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).map_err(|e| CliError::Http(format!("bad JSON: {e}")))
    }
}

// --- jobs -------------------------------------------------------------------

fn print_job(j: &Value) {
    let s = |k: &str| j[k].as_str().unwrap_or("-").to_string();
    println!(
        "{}  {}  {}  flink:{}  savepoint:{}",
        s("id"),
        s("state"),
        s("name"),
        s("flink_job_id"),
        s("savepoint")
    );
    if let Some(m) = j["message"].as_str() {
        println!("  {m}");
    }
}

async fn jobs(c: &Client, cmd: &JobsCommand) -> Result<ExitCode, CliError> {
    use reqwest::Method;
    let job = match cmd {
        JobsCommand::List => {
            let v = c.call(Method::GET, "/jobs", None).await?;
            for j in v["jobs"].as_array().into_iter().flatten() {
                print_job(j);
            }
            return Ok(ExitCode::SUCCESS);
        }
        JobsCommand::Get { id } => c.call(Method::GET, &format!("/jobs/{id}"), None).await?,
        JobsCommand::Create {
            name,
            file,
            session,
            description,
        } => {
            let text = std::fs::read_to_string(file)
                .map_err(|e| CliError::Io(format!("{}: {e}", file.display())))?;
            let (mut stmts, rest) = split_statements(&text);
            if !rest.trim().is_empty() {
                stmts.push(rest.trim().to_string());
            }
            let Some(statement) = stmts.pop() else {
                return Err(CliError::Usage(format!(
                    "{} has no statement",
                    file.display()
                )));
            };
            let session = match session {
                Some(p) => {
                    let t = std::fs::read_to_string(p)
                        .map_err(|e| CliError::Io(format!("{}: {e}", p.display())))?;
                    let spec: SessionSpec = toml::from_str(&t)
                        .map_err(|e| CliError::Usage(format!("{}: {e}", p.display())))?;
                    serde_json::to_value(spec).map_err(|e| CliError::Usage(e.to_string()))?
                }
                None => json!({}),
            };
            c.call(
                Method::POST,
                "/jobs",
                Some(json!({
                    "name": name,
                    "description": description,
                    "statement": statement,
                    "setup": stmts,
                    "session": session,
                })),
            )
            .await?
        }
        JobsCommand::Start { id } => {
            c.call(Method::POST, &format!("/jobs/{id}/start"), None)
                .await?
        }
        JobsCommand::Stop {
            id,
            no_savepoint,
            drain,
        } => {
            c.call(
                Method::POST,
                &format!("/jobs/{id}/stop"),
                Some(json!({ "savepoint": !no_savepoint, "drain": drain })),
            )
            .await?
        }
        JobsCommand::Resume { id } => {
            c.call(Method::POST, &format!("/jobs/{id}/resume"), None)
                .await?
        }
        JobsCommand::Delete { id } => {
            c.call(Method::DELETE, &format!("/jobs/{id}"), None).await?;
            println!("deleted {id}");
            return Ok(ExitCode::SUCCESS);
        }
    };
    print_job(&job);
    Ok(if job["state"] == "failed" {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

// --- SQL --------------------------------------------------------------------

async fn open(c: &Client) -> Result<String, CliError> {
    let v = c.call(reqwest::Method::POST, "/sessions", None).await?;
    v["session"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| CliError::Http("no session id in the answer".into()))
}

async fn close(c: &Client, session: &str) {
    let _ = c
        .call(
            reqwest::Method::DELETE,
            &format!("/sessions/{session}"),
            None,
        )
        .await;
}

/// Format a field the way Flink's tableau output does.
pub fn show_field(v: &Value) -> String {
    match v {
        Value::Null => "<NULL>".to_string(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// One NDJSON line of a result stream, as text for a terminal. `None` for
/// lines that print nothing.
pub fn render_line(v: &Value) -> Option<String> {
    match v["type"].as_str()? {
        "columns" => {
            let names: Vec<String> = v["columns"]
                .as_array()?
                .iter()
                .map(|c| c["name"].as_str().unwrap_or("?").to_string())
                .collect();
            Some(format!("op | {}", names.join(" | ")))
        }
        "rows" => {
            let lines: Vec<String> = v["rows"]
                .as_array()?
                .iter()
                .map(|r| {
                    let fields: Vec<String> = r["fields"]
                        .as_array()
                        .map(|f| f.iter().map(show_field).collect())
                        .unwrap_or_default();
                    format!(
                        "{} | {}",
                        r["kind"].as_str().unwrap_or("+I"),
                        fields.join(" | ")
                    )
                })
                .collect();
            Some(lines.join("\n"))
        }
        "job" => Some(format!(
            "Job submitted: {}",
            v["job_id"].as_str().unwrap_or("?")
        )),
        "end" => {
            let n = v["rows"].as_u64().unwrap_or(0);
            let cut = if v["truncated"].as_bool() == Some(true) {
                " (cut off; raise --max-rows)"
            } else {
                ""
            };
            if n == 0 {
                Some(format!("OK{cut}"))
            } else {
                Some(format!("{n} row(s){cut}"))
            }
        }
        "error" => Some(format!(
            "ERROR {}: {}",
            v["error"].as_str().unwrap_or("error"),
            v["message"].as_str().unwrap_or_default()
        )),
        _ => None,
    }
}

/// Run one statement and print its streamed results; Ctrl-C cancels it.
/// Returns whether it succeeded.
async fn statement(
    c: &Client,
    session: &str,
    sql: &str,
    max_rows: usize,
) -> Result<bool, CliError> {
    use reqwest::Method;
    let v = c
        .call(
            Method::POST,
            &format!("/sessions/{session}/statements"),
            Some(json!({ "statement": sql })),
        )
        .await?;
    let id = v["statement"]
        .as_str()
        .ok_or_else(|| CliError::Http("no statement id in the answer".into()))?
        .to_string();
    let resp = c
        .request(
            Method::GET,
            &format!(
                "/sessions/{session}/statements/{id}/results?max_rows={max_rows}&wait_ms=3600000"
            ),
        )
        .send()
        .await
        .map_err(|e| CliError::Http(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(Client::error_of(resp).await);
    }
    let read = async {
        let mut resp = resp;
        let mut buf: Vec<u8> = Vec::new();
        let mut ok = true;
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| CliError::Http(e.to_string()))?
        {
            buf.extend_from_slice(&chunk);
            while let Some(pos) = buf.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = buf.drain(..=pos).collect();
                let Ok(v) = serde_json::from_slice::<Value>(&line) else {
                    continue;
                };
                if v["type"] == "error" {
                    ok = false;
                }
                if let Some(text) = render_line(&v) {
                    if v["type"] == "error" {
                        eprintln!("{text}");
                    } else {
                        println!("{text}");
                    }
                }
            }
        }
        Ok::<bool, CliError>(ok)
    };
    tokio::select! {
        r = read => r,
        _ = tokio::signal::ctrl_c() => {
            let _ = c
                .call(Method::DELETE, &format!("/sessions/{session}/statements/{id}"), None)
                .await;
            eprintln!("Cancelled.");
            Ok(false)
        }
    }
}

async fn batch(c: &Client, text: &str, max_rows: usize) -> Result<ExitCode, CliError> {
    let (mut stmts, rest) = split_statements(text);
    if !rest.trim().is_empty() {
        stmts.push(rest.trim().to_string());
    }
    let session = open(c).await?;
    let mut code = ExitCode::SUCCESS;
    for s in &stmts {
        match statement(c, &session, s, max_rows).await {
            Ok(true) => {}
            Ok(false) => {
                code = ExitCode::FAILURE;
                break;
            }
            Err(e) => {
                eprintln!("{e}");
                code = ExitCode::FAILURE;
                break;
            }
        }
    }
    close(c, &session).await;
    Ok(code)
}

const HELP: &str = "End each statement with ';'. Commands: \\q quit, \\jobs list saved jobs, \\bootstrap show the session setup, \\h this help.
Lake tables are loams.<database>.<table> (Iceberg on Lakekeeper); Postgres tables are loams_pg.<database>.<table>.";

fn prompt(continuation: bool) {
    print!("{}", if continuation { "     -> " } else { "flink> " });
    let _ = std::io::stdout().flush();
}

async fn repl(c: &Client, max_rows: usize) -> Result<ExitCode, CliError> {
    let session = open(c).await?;
    println!("Loams Flink SQL (preview). Session {session}.\n{HELP}");
    let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    let mut buffer = String::new();
    prompt(false);
    loop {
        let line = match lines.next_line().await {
            Ok(Some(l)) => l,
            Ok(None) => break,
            Err(e) => {
                eprintln!("{e}");
                break;
            }
        };
        if buffer.trim().is_empty() {
            match line.trim() {
                "\\q" | "quit" | "exit" => break,
                "\\h" | "help" => {
                    println!("{HELP}");
                    prompt(false);
                    continue;
                }
                "\\jobs" => {
                    if let Err(e) = jobs(c, &JobsCommand::List).await {
                        eprintln!("{e}");
                    }
                    prompt(false);
                    continue;
                }
                "\\bootstrap" => {
                    match c.call(reqwest::Method::GET, "/bootstrap", None).await {
                        Ok(v) => print!("{}", v["script"].as_str().unwrap_or_default()),
                        Err(e) => eprintln!("{e}"),
                    }
                    prompt(false);
                    continue;
                }
                _ => {}
            }
        }
        buffer.push_str(&line);
        buffer.push('\n');
        let (stmts, rest) = split_statements(&buffer);
        buffer = rest;
        for s in stmts {
            match statement(c, &session, &s, max_rows).await {
                Ok(_) => {}
                // The session expired (idle): open a new one and say so.
                Err(CliError::Api { code, .. }) if code == "not_found" => {
                    close(c, &session).await;
                    eprintln!("The session expired; reconnect with `loams flink sql`.");
                    return Ok(ExitCode::FAILURE);
                }
                Err(e) => eprintln!("{e}"),
            }
        }
        prompt(!buffer.trim().is_empty());
    }
    close(c, &session).await;
    println!();
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Debug, Parser)]
    struct Top {
        #[command(subcommand)]
        cmd: Cmd,
    }

    #[derive(Debug, clap::Subcommand)]
    enum Cmd {
        Flink(FlinkArgs),
    }

    #[test]
    fn parses_like_the_loams_binary() {
        let top = Top::try_parse_from([
            "loams", "flink", "-n", "ns1", "-e", "eng_x", "jobs", "stop", "fj_1", "--drain",
        ])
        .unwrap_or_else(|e| panic!("{e}"));
        let Cmd::Flink(a) = top.cmd;
        assert_eq!(a.namespace.as_deref(), Some("ns1"));
        assert!(matches!(
            a.command,
            FlinkCommand::Jobs {
                command: JobsCommand::Stop {
                    drain: true,
                    no_savepoint: false,
                    ..
                }
            }
        ));
        let top = Top::try_parse_from(["loams", "flink", "sql", "-x", "SELECT 1", "-n", "ns1"])
            .unwrap_or_else(|e| panic!("{e}"));
        let Cmd::Flink(a) = top.cmd;
        assert!(matches!(
            a.command,
            FlinkCommand::Sql {
                execute: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn renders_stream_lines() {
        assert_eq!(
            render_line(&json!({"type":"columns","columns":[{"name":"a"},{"name":"b"}]}))
                .as_deref(),
            Some("op | a | b")
        );
        assert_eq!(
            render_line(&json!({"type":"rows","rows":[{"kind":"-U","fields":["x", null, 3]}]}))
                .as_deref(),
            Some("-U | x | <NULL> | 3")
        );
        assert_eq!(
            render_line(&json!({"type":"end","rows":0,"truncated":false})).as_deref(),
            Some("OK")
        );
        assert_eq!(
            render_line(&json!({"type":"end","rows":5,"truncated":true})).as_deref(),
            Some("5 row(s) (cut off; raise --max-rows)")
        );
        assert_eq!(render_line(&json!({"type":"other"})), None);
    }
}
