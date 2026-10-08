//! The worker's arguments and the chDB configuration it writes for itself.
//!
//! A worker is started by the front with an **empty environment** and these
//! arguments only; neither carries a credential (§49 §13.1, HS1 Task 2's
//! `worker_env_has_no_secret`). Everything it needs to know about a tenant arrives
//! later in `Bind`, and that carries no credential either.
//!
//! At boot it writes two files into its private temporary directory:
//!
//! * `config.xml`, the engine's `--config-file`: the users file, `user_files_path`
//!   and the three in-memory metadata caches sized from the memory limit (HS1 R1.4).
//! * `users.xml`, which **redefines `default`** with explicit grants instead of
//!   chDB's all-powerful one (HS1 R1.8: chDB cannot `REVOKE`, but it can be told
//!   what to grant at connect). No `FILE`, `URL`, `REMOTE`, `WRITE ON S3`, `SYSTEM`,
//!   `CREATE TABLE` or `CREATE FUNCTION`; `READ ON S3` only for the worker's
//!   loopback forwarder, when it has one.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// The default memory limit a worker sizes itself for: §49 §12's 4 GiB per query
/// plus the 512 MiB headroom its cgroup gets.
pub const DEFAULT_MEMORY_LIMIT: u64 = 4 * 1024 * 1024 * 1024 + 512 * 1024 * 1024;

/// The grants of HS1 R1.8, on `*.*`.
pub const GRANTS: &str = "GRANT SELECT, SHOW, CREATE TEMPORARY TABLE, CREATE VIEW, DROP VIEW, \
                          CREATE DATABASE, DROP DATABASE, INSERT ON *.*";

/// Settings the worker's profile pins with `<readonly/>` constraints, so a user's
/// `SET` answers `452` (HS1 R1.5, R1.8).
pub const PINNED_OFF: &[&str] = &[
    "allow_introspection_functions",
    "allow_insert_into_iceberg",
    "allow_experimental_iceberg_compaction",
    "allow_iceberg_remove_orphan_files",
];

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
}

impl WorkerArgs {
    /// Parses `--tmp-dir <p> --worker-id <id> [--memory-limit <bytes>]
    /// [--s3-endpoint <url>]`.
    pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Self, String> {
        let mut tmp_dir = None;
        let mut worker_id = None;
        let mut memory_limit = DEFAULT_MEMORY_LIMIT;
        let mut s3_endpoint = None;
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
                other => return Err(format!("unknown argument {other:?}")),
            }
        }
        Ok(Self {
            tmp_dir: tmp_dir.ok_or("--tmp-dir is required")?,
            worker_id: worker_id.ok_or("--worker-id is required")?,
            memory_limit,
            s3_endpoint,
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
        out
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
           <iceberg_metadata_files_cache_size>{iceberg}</iceberg_metadata_files_cache_size>\n  \
           <parquet_metadata_cache_size>{parquet}</parquet_metadata_cache_size>\n  \
           <query_condition_cache_size>{condition}</query_condition_cache_size>\n\
         </clickhouse>\n",
        users = xml_text(&args.users_file().display().to_string()),
        files = xml_text(&args.files_dir().display().to_string()),
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
            "        <query>GRANT READ ON S3('{}')</query>",
            xml_text(&s3_grant_regex(endpoint))
        );
    }
    let mut pinned = String::new();
    let mut constraints = String::new();
    for name in PINNED_OFF {
        let _ = writeln!(pinned, "      <{name}>0</{name}>");
        let _ = writeln!(constraints, "        <{name}><readonly/></{name}>");
    }
    format!(
        "<clickhouse>\n  \
           <users>\n    \
             <default>\n      \
               <password></password>\n      \
               <networks><ip>::/0</ip></networks>\n      \
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
        }
    }

    #[test]
    fn args_roundtrip() {
        let args = args();
        assert_eq!(WorkerArgs::parse(args.to_args()), Ok(args));
        assert!(WorkerArgs::parse(vec!["--worker-id".to_string(), "x".to_string()]).is_err());
        assert!(WorkerArgs::parse(vec!["--secret".to_string(), "x".to_string()]).is_err());
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
        assert!(users.contains("GRANT READ ON S3('http://127\\.0\\.0\\.1:41887/.*')"));
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
        for name in PINNED_OFF {
            assert!(users.contains(&format!("<{name}><readonly/></{name}>")));
        }
        let none = users_xml(&WorkerArgs {
            s3_endpoint: None,
            ..args()
        });
        assert!(!none.contains("ON S3"), "no endpoint, no S3 grant");
    }
}
