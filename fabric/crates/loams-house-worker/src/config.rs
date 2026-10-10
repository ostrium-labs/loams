//! The worker's arguments and the chDB configuration it writes for itself.
//!
//! A worker is started by the front with an **empty environment** and these
//! arguments only; neither carries a credential (§49 §13.1, HS1 Task 2's
//! `worker_env_has_no_secret`). Everything it needs to know about a tenant arrives
//! later in `Bind`, and that carries no credential either.
//!
//! At boot it writes two files into its private temporary directory:
//!
//! * `config.xml`, the engine's `--config-file`: the users file, `user_files_path`,
//!   the House's `display_name`, a `user_scripts_path` that cannot exist (HS1
//!   Task 5) and the three in-memory metadata caches sized from the memory limit
//!   (HS1 R1.4).
//! * `users.xml`, which **redefines `default`** with explicit grants instead of
//!   chDB's all-powerful one (HS1 R1.8: chDB cannot `REVOKE`, but it can be told
//!   what to grant at connect). No `FILE`, `URL`, `REMOTE`, `WRITE ON S3`, `SYSTEM`,
//!   `CREATE TABLE` or `CREATE FUNCTION`; `READ ON S3` only for the worker's
//!   loopback forwarder, when it has one.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use loams_house_ipc::SandboxMode;

/// The default memory limit a worker sizes itself for: §49 §12's 4 GiB per query
/// plus the 512 MiB headroom its cgroup gets.
pub const DEFAULT_MEMORY_LIMIT: u64 = 4 * 1024 * 1024 * 1024 + 512 * 1024 * 1024;

/// The grants of HS1 R1.8, on `*.*`.
pub const GRANTS: &str = "GRANT SELECT, SHOW, CREATE TEMPORARY TABLE, CREATE VIEW, DROP VIEW, \
                          CREATE DATABASE, DROP DATABASE, INSERT ON *.*";

pub use crate::settings::{
    MAX_EXECUTION_TIME_S, MAX_QUERY_MEMORY, MIN_EXECUTION_TIME_S, PINNED_OFF, PINNED_PATHS,
    PINNED_SWITCHES, PINNED_VALUES, max_threads_cap,
};

/// `user_scripts_path`: a directory that cannot exist, so `executable()` and
/// executable UDFs find no script even with L1 bypassed (HS1 Task 5). It is
/// under a file, outside the worker's writable directory (fix round 1): nothing,
/// the worker included, can create it.
pub const NO_USER_SCRIPTS: &str = "/dev/null/loams-no-user-scripts";

/// The worker's command line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerArgs {
    /// The worker's private temporary directory: chDB's `--path`, its config
    /// files, `user_files_path` and the process's working directory all live in
    /// it, and (from HS1 Task 6) Landlock lets it write nowhere else.
    pub tmp_dir: PathBuf,
    /// An opaque id the front chose, for logs and `X-Loams-Worker`.
    pub worker_id: String,
    /// The memory the worker is sized for, in bytes.
    pub memory_limit: u64,
    /// The loopback endpoint the worker reads the bucket through, which its
    /// `READ ON S3` grant is scoped to (HS1 Tasks 6 and 9). None: no S3 at all.
    pub s3_endpoint: Option<String>,
    /// How the worker seals itself (`--sandbox`, HS1 Task 6). `netns` when the
    /// flag is absent: a worker is sealed unless the front says otherwise.
    pub sandbox: SandboxMode,
    /// `netns` with a forwarder: the loopback port it listens on, and the
    /// forwarder's socket is fd 4 (`--forwarder-port`).
    pub forwarder_port: Option<u16>,
    /// The cgroup v2 directory the front made for this worker (`--cgroup`).
    pub cgroup: Option<PathBuf>,
}

impl WorkerArgs {
    /// Parses `--tmp-dir <p> --worker-id <id> [--memory-limit <bytes>]
    /// [--s3-endpoint <url>] [--sandbox netns|pods|none] [--forwarder-port <p>]
    /// [--cgroup <dir>]`.
    pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Self, String> {
        let mut tmp_dir = None;
        let mut worker_id = None;
        let mut memory_limit = DEFAULT_MEMORY_LIMIT;
        let mut s3_endpoint = None;
        let mut sandbox = SandboxMode::Netns;
        let mut forwarder_port = None;
        let mut cgroup = None;
        let mut args = args.into_iter();
        while let Some(flag) = args.next() {
            let mut value = || args.next().ok_or_else(|| format!("{flag} needs a value"));
            match flag.as_str() {
                "--tmp-dir" => tmp_dir = Some(PathBuf::from(value()?)),
                "--worker-id" => worker_id = Some(value()?),
                "--memory-limit" => {
                    let raw = value()?;
                    memory_limit = raw
                        .parse()
                        .map_err(|_| format!("--memory-limit {raw:?} is not a byte count"))?;
                }
                "--s3-endpoint" => s3_endpoint = Some(value()?),
                "--sandbox" => sandbox = value()?.parse()?,
                "--forwarder-port" => {
                    let raw = value()?;
                    forwarder_port = Some(
                        raw.parse()
                            .map_err(|_| format!("--forwarder-port {raw:?} is not a port"))?,
                    );
                }
                "--cgroup" => cgroup = Some(PathBuf::from(value()?)),
                other => return Err(format!("unknown argument {other:?}")),
            }
        }
        if forwarder_port.is_some() && sandbox != SandboxMode::Netns {
            return Err("--forwarder-port is only for --sandbox netns".to_string());
        }
        Ok(Self {
            tmp_dir: tmp_dir.ok_or("--tmp-dir is required")?,
            worker_id: worker_id.ok_or("--worker-id is required")?,
            memory_limit,
            s3_endpoint,
            sandbox,
            forwarder_port,
            cgroup,
        })
    }

    /// The arguments that [`WorkerArgs::parse`] reads back as `self`.
    pub fn to_args(&self) -> Vec<String> {
        let mut out = vec![
            "--tmp-dir".to_string(),
            self.tmp_dir.display().to_string(),
            "--worker-id".to_string(),
            self.worker_id.clone(),
            "--memory-limit".to_string(),
            self.memory_limit.to_string(),
        ];
        if let Some(endpoint) = &self.s3_endpoint {
            out.push("--s3-endpoint".to_string());
            out.push(endpoint.clone());
        }
        out.push("--sandbox".to_string());
        out.push(self.sandbox.as_str().to_string());
        if let Some(port) = self.forwarder_port {
            out.push("--forwarder-port".to_string());
            out.push(port.to_string());
        }
        if let Some(cgroup) = &self.cgroup {
            out.push("--cgroup".to_string());
            out.push(cgroup.display().to_string());
        }
        out
    }

    /// The TCP port of `--s3-endpoint` (`http://host:port/…`), which a `pods`
    /// worker may connect to and nothing else.
    pub fn s3_endpoint_port(&self) -> Option<u16> {
        let endpoint = self.s3_endpoint.as_deref()?;
        let rest = endpoint
            .split_once("://")
            .map_or(endpoint, |(_, rest)| rest);
        let authority = rest.split('/').next()?;
        let (_, port) = authority.rsplit_once(':')?;
        port.parse().ok()
    }

    /// chDB's `--path`.
    pub fn data_dir(&self) -> PathBuf {
        self.tmp_dir.join("data")
    }

    /// `user_files_path`, and the process's working directory (HS1 R1.10).
    pub fn files_dir(&self) -> PathBuf {
        self.tmp_dir.join("files")
    }

    /// The `--config-file`.
    pub fn config_file(&self) -> PathBuf {
        self.tmp_dir.join("config.xml")
    }

    /// The users file `config.xml` names.
    pub fn users_file(&self) -> PathBuf {
        self.tmp_dir.join("users.xml")
    }
}

/// The three per-process metadata caches of HS1 R1.4, together an eighth of the
/// worker's memory: 20 % Iceberg metadata, 60 % Parquet footers, 20 % query
/// conditions (the defaults' own proportions are 128 : 512 : 100 MiB).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheSizes {
    /// `iceberg_metadata_files_cache_size`.
    pub iceberg_metadata: u64,
    /// `parquet_metadata_cache_size`.
    pub parquet_metadata: u64,
    /// `query_condition_cache_size`.
    pub query_condition: u64,
}

impl CacheSizes {
    /// The sizes for a worker with `memory_limit` bytes.
    pub fn for_memory(memory_limit: u64) -> Self {
        let total = memory_limit / 8;
        Self {
            iceberg_metadata: total / 5,
            parquet_metadata: total / 5 * 3,
            query_condition: total / 5,
        }
    }
}

/// `config.xml`.
pub fn config_xml(args: &WorkerArgs) -> String {
    let caches = CacheSizes::for_memory(args.memory_limit);
    format!(
        "<clickhouse>\n  \
           <users_config>{users}</users_config>\n  \
           <user_files_path>{files}/</user_files_path>\n  \
           <display_name>{display}</display_name>\n  \
           <user_scripts_path>{scripts}/</user_scripts_path>\n  \
           <iceberg_metadata_files_cache_size>{iceberg}</iceberg_metadata_files_cache_size>\n  \
           <parquet_metadata_cache_size>{parquet}</parquet_metadata_cache_size>\n  \
           <query_condition_cache_size>{condition}</query_condition_cache_size>\n\
         </clickhouse>\n",
        users = xml_text(&args.users_file().display().to_string()),
        files = xml_text(&args.files_dir().display().to_string()),
        display = xml_text(crate::settings::DISPLAY_NAME),
        scripts = xml_text(NO_USER_SCRIPTS),
        iceberg = caches.iceberg_metadata,
        parquet = caches.parquet_metadata,
        condition = caches.query_condition,
    )
}

/// `users.xml`: `default` redefined with HS1 R1.8's grants and the `worker`
/// profile. A profile named `default` must exist too, or connect fails with `180`.
pub fn users_xml(args: &WorkerArgs) -> String {
    let mut grants = format!("        <query>{}</query>\n", xml_text(GRANTS));
    if let Some(endpoint) = &args.s3_endpoint {
        let _ = writeln!(
            grants,
            "        <query>GRANT READ ON S3({})</query>",
            xml_text(&sql_string(&s3_grant_regex(endpoint)))
        );
    }
    let mut pinned = String::new();
    let mut constraints = String::new();
    for name in PINNED_OFF {
        let _ = writeln!(pinned, "      <{name}>0</{name}>");
        let _ = writeln!(constraints, "        <{name}><readonly/></{name}>");
    }
    for (name, value) in PINNED_VALUES {
        let _ = writeln!(pinned, "      <{name}>{value}</{name}>");
        let _ = writeln!(constraints, "        <{name}><readonly/></{name}>");
    }
    for name in PINNED_PATHS.iter().chain(PINNED_SWITCHES) {
        let _ = writeln!(constraints, "        <{name}><readonly/></{name}>");
    }
    // The caps of FL2 Ruling 10, enforced by the engine after its own parsing and
    // conversion, wherever a setting comes from (URL, `SET`, `SETTINGS`), fix
    // round 1, I3. The profile starts at the caps, so `0` is a change and checked.
    let _ = writeln!(
        pinned,
        "      <max_memory_usage>{MAX_QUERY_MEMORY}</max_memory_usage>"
    );
    let _ = writeln!(
        pinned,
        "      <max_execution_time>{MAX_EXECUTION_TIME_S}</max_execution_time>"
    );
    let _ = writeln!(
        constraints,
        "        <max_memory_usage><min>1</min><max>{MAX_QUERY_MEMORY}</max></max_memory_usage>"
    );
    let _ = writeln!(
        constraints,
        "        <max_execution_time><min>{MIN_EXECUTION_TIME_S}</min><max>{MAX_EXECUTION_TIME_S}</max></max_execution_time>"
    );
    let _ = writeln!(
        constraints,
        "        <max_threads><max>{}</max></max_threads>",
        max_threads_cap()
    );
    format!(
        "<clickhouse>\n  \
           <users>\n    \
             <default>\n      \
               <password></password>\n      \
               <networks><ip>127.0.0.1</ip></networks>\n      \
               <profile>worker</profile>\n      \
               <quota>default</quota>\n      \
               <grants>\n{grants}      </grants>\n    \
             </default>\n  \
           </users>\n  \
           <profiles>\n    \
             <default/>\n    \
             <worker>\n{pinned}      <constraints>\n{constraints}      </constraints>\n    \
             </worker>\n  \
           </profiles>\n  \
           <quotas><default/></quotas>\n\
         </clickhouse>\n"
    )
}

/// The `READ ON S3` regex for a loopback endpoint: the endpoint, escaped, then
/// anything. `house-cache` scopes keys to the namespace's prefix (HS1 Task 9).
pub fn s3_grant_regex(endpoint: &str) -> String {
    let mut out = String::with_capacity(endpoint.len() + 4);
    for ch in endpoint.trim_end_matches('/').chars() {
        if ".+*?()[]{}|^$\\".contains(ch) {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push_str("/.*");
    out
}

/// A ClickHouse string literal: quoted, with `\\` and `'` escaped. The regex's own
/// backslashes must survive the literal (`\\.` in the SQL is `\.` in the regex),
/// and a quote in an endpoint must not end it (HS1 Task 2 review M4).
pub fn sql_string(text: &str) -> String {
    format!("'{}'", text.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// Escapes text for an XML element body.
fn xml_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Writes `config.xml` and `users.xml` and creates the directories.
pub fn write_files(args: &WorkerArgs) -> std::io::Result<()> {
    for dir in [&args.tmp_dir, &args.data_dir(), &args.files_dir()] {
        std::fs::create_dir_all(dir)?;
    }
    write(&args.config_file(), &config_xml(args))?;
    write(&args.users_file(), &users_xml(args))
}

fn write(path: &Path, text: &str) -> std::io::Result<()> {
    std::fs::write(path, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> WorkerArgs {
        WorkerArgs {
            tmp_dir: PathBuf::from("/w/7"),
            worker_id: "w7".to_string(),
            memory_limit: 8 * 1024 * 1024 * 1024,
            s3_endpoint: Some("http://127.0.0.1:41887/".to_string()),
            sandbox: SandboxMode::Netns,
            forwarder_port: Some(9180),
            cgroup: Some(PathBuf::from("/sys/fs/cgroup/house/w7")),
        }
    }

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_string()).collect()
    }

    #[test]
    fn args_roundtrip() {
        let netns = args();
        assert_eq!(WorkerArgs::parse(netns.to_args()), Ok(netns));
        assert!(WorkerArgs::parse(vec!["--worker-id".to_string(), "x".to_string()]).is_err());
        assert!(WorkerArgs::parse(vec!["--secret".to_string(), "x".to_string()]).is_err());
        let pods = WorkerArgs {
            sandbox: SandboxMode::Pods,
            forwarder_port: None,
            cgroup: None,
            ..args()
        };
        assert_eq!(WorkerArgs::parse(pods.to_args()), Ok(pods));
    }

    #[test]
    fn a_worker_is_sealed_unless_told_otherwise() {
        let bare =
            WorkerArgs::parse(strings(&["--tmp-dir", "/w", "--worker-id", "w"])).expect("parses");
        assert_eq!(bare.sandbox, SandboxMode::Netns);
        assert!(
            WorkerArgs::parse(strings(&[
                "--tmp-dir",
                "/w",
                "--worker-id",
                "w",
                "--sandbox",
                "off"
            ]))
            .is_err()
        );
        assert!(
            WorkerArgs::parse(strings(&[
                "--tmp-dir",
                "/w",
                "--worker-id",
                "w",
                "--sandbox",
                "none",
                "--forwarder-port",
                "9180"
            ]))
            .is_err(),
            "a forwarder needs the netns"
        );
    }

    #[test]
    fn s3_endpoint_port() {
        assert_eq!(args().s3_endpoint_port(), Some(41887));
        let none = WorkerArgs {
            s3_endpoint: Some("http://house-front".to_string()),
            ..args()
        };
        assert_eq!(none.s3_endpoint_port(), None);
        let host = WorkerArgs {
            s3_endpoint: Some("http://10.0.0.5:9180/bucket".to_string()),
            ..args()
        };
        assert_eq!(host.s3_endpoint_port(), Some(9180));
    }

    #[test]
    fn sql_string_escapes_backslashes_and_quotes() {
        assert_eq!(sql_string(r"a\.b"), r"'a\\.b'");
        assert_eq!(sql_string("it's"), r"'it\'s'");
        let users = users_xml(&WorkerArgs {
            s3_endpoint: Some("http://127.0.0.1:1/x'); GRANT ALL ON *.* TO default; --".into()),
            ..args()
        });
        let line = users
            .lines()
            .find(|l| l.contains("ON S3("))
            .expect("the S3 grant");
        let inner = line
            .trim()
            .strip_prefix("<query>GRANT READ ON S3('")
            .and_then(|l| l.strip_suffix("')</query>"))
            .expect("one literal");
        let bytes = inner.as_bytes();
        for (at, byte) in bytes.iter().enumerate() {
            if *byte == b'\'' {
                let backslashes = bytes[..at]
                    .iter()
                    .rev()
                    .take_while(|b| **b == b'\\')
                    .count();
                assert!(
                    backslashes % 2 == 1,
                    "an unescaped quote ends the literal early: {line}"
                );
            }
        }
    }

    #[test]
    fn config_names_the_house_and_no_scripts() {
        let config = config_xml(&args());
        assert!(config.contains("<display_name>loams-house</display_name>"));
        // Outside the worker's writable directory, under a file: nothing, not even
        // the worker itself, can create it (fix round 1).
        assert!(config.contains(&format!(
            "<user_scripts_path>{NO_USER_SCRIPTS}/</user_scripts_path>"
        )));
        assert!(
            NO_USER_SCRIPTS.starts_with("/dev/null/"),
            "{NO_USER_SCRIPTS}"
        );
        assert!(!Path::new(NO_USER_SCRIPTS).starts_with(&args().tmp_dir));
    }

    #[test]
    fn caches_are_an_eighth_of_memory() {
        let sizes = CacheSizes::for_memory(8 * 1024 * 1024 * 1024);
        assert_eq!(
            sizes.iceberg_metadata + sizes.parquet_metadata + sizes.query_condition,
            1024 * 1024 * 1024 / 5 * 5
        );
        assert!(sizes.parquet_metadata > sizes.iceberg_metadata);
    }

    #[test]
    fn users_file_grants_only_r1_8() {
        let users = users_xml(&args());
        assert!(users.contains(GRANTS));
        assert!(
            users.contains(r"GRANT READ ON S3('http://127\\.0\\.0\\.1:41887/.*')"),
            "the regex's backslashes are doubled inside the SQL literal:\n{users}"
        );
        assert!(
            users.contains("<networks><ip>127.0.0.1</ip></networks>"),
            "R1.11"
        );
        for absent in [
            "FILE",
            "URL",
            "REMOTE",
            "WRITE",
            "SYSTEM",
            "CREATE TABLE",
            "FUNCTION",
        ] {
            assert!(
                !users.contains(absent),
                "{absent} must not be granted:\n{users}"
            );
        }
        for name in crate::settings::pinned() {
            assert!(users.contains(&format!("<{name}><readonly/></{name}>")));
        }
        assert!(
            users.contains(
                "<default_temporary_table_engine>Memory</default_temporary_table_engine>"
            )
        );
        assert!(
            users
                .contains("<max_memory_usage><min>1</min><max>4294967296</max></max_memory_usage>")
        );
        assert!(users.contains("<max_memory_usage>4294967296</max_memory_usage>"));
        assert!(users.contains(
            "<max_execution_time><min>0.000001</min><max>300</max></max_execution_time>"
        ));
        assert!(users.contains("<max_execution_time>300</max_execution_time>"));
        assert!(users.contains("<max_threads><max>"));
        let none = users_xml(&WorkerArgs {
            s3_endpoint: None,
            ..args()
        });
        assert!(!none.contains("ON S3"), "no endpoint, no S3 grant");
    }
}
