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
//!   what to grant at connect), and a settings profile that pins every setting a
//!   request may not change (HS1 R-ACL.1).
//!
//! # The engine's access control (HS1 R-ACL.1)
//!
//! chDB authenticates **every** connection as `default`: `--user` is ignored and
//! `EXECUTE AS` is disabled, because its local server never reads
//! `access_control_improvements` (measured on 26.9.2.1). So there is one login
//! identity, and the two connections differ by their settings: the user
//! connection's `readonly = 2` and `allow_ddl = 0` take DDL, `INSERT` into tables,
//! `SYSTEM`, `KILL`, access management and `ALLOW SQL SECURITY NONE` away from it,
//! and neither can be set back (measured). What `default` holds:
//!
//! * no source at all — no `READ` or `WRITE` on `FILE`, `URL`, `S3`, `REMOTE`,
//!   `MYSQL`, `POSTGRES`, `ODBC`, `JDBC`, `HDFS`, `HIVE`, `MONGO`, `REDIS`,
//!   `SQLITE`, `AZURE`, `KAFKA`, `NATS`, `RABBITMQ`, `BIGQUERY`, `DISK` — so every
//!   table function, engine and dictionary that reaches outside is `497`, on the
//!   user connection and the control connection alike;
//! * `SELECT`, `SHOW` and `CREATE TEMPORARY TABLE` (temporary tables and the
//!   generator table functions);
//! * for the control connection's SQL functions (HE1 R14.11): `CREATE`/`DROP
//!   FUNCTION`, which the user connection's `readonly` takes away from it;
//! * for the control connection's dictionaries (HE1 R14.12): `CREATE`/`DROP
//!   DICTIONARY` (outside `system` and `information_schema`), and `dictGet` for
//!   both. A dictionary the House makes reads a read view of the statement
//!   (`SOURCE(CLICKHOUSE(DB … TABLE …))`, the local server), never a source;
//! * for the control connection's read views (§49 §7): `CREATE`/`DROP VIEW`,
//!   `CREATE`/`DROP DATABASE` (outside `system` and `information_schema`) and
//!   `ALLOW SQL SECURITY NONE`. A read view names its columns and is `SQL SECURITY
//!   NONE`, so it reads the bucket through the forwarder with no grant, and is
//!   created without one (a view that names its columns is not analysed at
//!   creation, measured). The user connection can create neither such a view nor
//!   any other: `readonly` strips `ALLOW SQL SECURITY NONE` (`164`), and
//!   `default` holds no `SET DEFINER`.
//!
//! No `SYSTEM`, `INTROSPECTION`, `CREATE TABLE`, `INSERT`, `KILL QUERY` or
//! access management. `system` tables stay readable whatever the grants say (chDB never sets
//! `select_from_system_db_requires_grant`, measured), so which `system` tables a
//! statement reads is the deny list's (R5.2) and the front's (R11.6).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use loams_house_ipc::SandboxMode;

/// The default memory limit a worker sizes itself for: §49 §12's 4 GiB per query
/// plus the 512 MiB headroom its cgroup gets.
pub const DEFAULT_MEMORY_LIMIT: u64 = 4 * 1024 * 1024 * 1024 + 512 * 1024 * 1024;

/// The grants of `default`, the worker's only login identity (HS1 R1.8,
/// R-ACL.1): nothing on any source.
pub const GRANTS: &[&str] = &[
    "GRANT SELECT, SHOW, CREATE TEMPORARY TABLE, CREATE VIEW, DROP VIEW, CREATE DATABASE, \
     DROP DATABASE, ALLOW SQL SECURITY NONE, CREATE FUNCTION, DROP FUNCTION, \
     CREATE DICTIONARY, DROP DICTIONARY, dictGet ON *.*",
    // Without these the control connection could make views in `system` and drop
    // `information_schema` (measured).
    "REVOKE CREATE VIEW, DROP VIEW, CREATE DATABASE, DROP DATABASE, CREATE DICTIONARY, \
     DROP DICTIONARY ON system.*",
    "REVOKE CREATE VIEW, DROP VIEW, CREATE DATABASE, DROP DATABASE, CREATE DICTIONARY, \
     DROP DICTIONARY ON information_schema.*",
    "REVOKE CREATE VIEW, DROP VIEW, CREATE DATABASE, DROP DATABASE, CREATE DICTIONARY, \
     DROP DICTIONARY ON INFORMATION_SCHEMA.*",
    "REVOKE CREATE VIEW, DROP VIEW, CREATE DATABASE, DROP DATABASE, CREATE DICTIONARY, \
     DROP DICTIONARY ON _temporary_and_external_tables.*",
];

/// What a worker started with `--test-open-acl` grants instead: chDB's own
/// all-powerful `default`, with no settings profile. Test builds only
/// (`loams_house_ipc::TEST_HOOKS`): it shows that a refusal came from the
/// engine's access control and not from another layer (R-ACL.1).
pub const OPEN_GRANTS: &[&str] = &["GRANT ALL ON *.*"];

pub use crate::settings::{
    MAX_BYTES_TO_READ, MAX_EXECUTION_TIME_S, MAX_QUERY_MEMORY, MAX_RESULT_BYTES,
    MIN_EXECUTION_TIME_S, PINNED_OFF, PINNED_PATHS, PINNED_SWITCHES, PINNED_VALUES,
    max_threads_cap, pinned_by_default,
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
    /// The loopback endpoint the worker reads the bucket through (HS1 Tasks 6 and
    /// 9), which the read views name (`Ready::s3_endpoint`). It is no grant: only
    /// the views reach it (R-ACL.1).
    pub s3_endpoint: Option<String>,
    /// How the worker seals itself (`--sandbox`, HS1 Task 6). `netns` when the
    /// flag is absent: a worker is sealed unless the front says otherwise.
    pub sandbox: SandboxMode,
    /// `netns` with a forwarder: the loopback port it listens on, and the
    /// forwarder's socket is fd 4 (`--forwarder-port`).
    pub forwarder_port: Option<u16>,
    /// The cgroup v2 directory the front made for this worker (`--cgroup`).
    pub cgroup: Option<PathBuf>,
    /// `--test-open-acl`: [`OPEN_GRANTS`] and no settings profile. Accepted only
    /// by a test build (`loams_house_ipc::TEST_HOOKS`, R-ACL.1).
    pub open_acl: bool,
    /// The largest `max_execution_time` a statement may set, in whole seconds
    /// (`--max-execution-time`, D1571): the front's cap, held by the profile.
    pub max_execution_time_s: u64,
}

impl WorkerArgs {
    /// Parses `--tmp-dir <p> --worker-id <id> [--memory-limit <bytes>]
    /// [--s3-endpoint <url>] [--sandbox netns|pods|none] [--forwarder-port <p>]
    /// [--cgroup <dir>] [--test-open-acl]`.
    pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Self, String> {
        let mut tmp_dir = None;
        let mut worker_id = None;
        let mut memory_limit = DEFAULT_MEMORY_LIMIT;
        let mut s3_endpoint = None;
        let mut sandbox = SandboxMode::Netns;
        let mut forwarder_port = None;
        let mut cgroup = None;
        let mut open_acl = false;
        let mut max_execution_time_s = MAX_EXECUTION_TIME_S;
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
                "--max-execution-time" => {
                    let raw = value()?;
                    max_execution_time_s =
                        raw.parse().ok().filter(|s| *s > 0).ok_or_else(|| {
                            format!("--max-execution-time {raw:?} is not a number of seconds")
                        })?;
                }
                "--test-open-acl" if loams_house_ipc::TEST_HOOKS => open_acl = true,
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
            open_acl,
            max_execution_time_s,
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
        if self.open_acl {
            out.push("--test-open-acl".to_string());
        }
        out.push("--max-execution-time".to_string());
        out.push(self.max_execution_time_s.to_string());
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

/// `users.xml`: `default` redefined with R-ACL.1's grants and the `worker`
/// profile. A profile named `default` must exist too, or connect fails with `180`.
/// Private: [`write_files`] is the way in, and it refuses `open_acl` outside a
/// test build.
fn users_xml(args: &WorkerArgs) -> String {
    let grants: String = if args.open_acl { OPEN_GRANTS } else { GRANTS }
        .iter()
        .map(|query| format!("        <query>{}</query>\n", xml_text(query)))
        .collect();
    let profile = if args.open_acl {
        String::new()
    } else {
        worker_profile(args)
    };
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
             <worker>\n{profile}    </worker>\n  \
           </profiles>\n  \
           <quotas><default/></quotas>\n\
         </clickhouse>\n"
    )
}

/// The `worker` profile's body: the pinned values and every constraint.
fn worker_profile(args: &WorkerArgs) -> String {
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
    // R-ACL.1: every other setting chDB has that no list leaves changeable.
    for name in pinned_by_default() {
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
        "      <max_execution_time>{}</max_execution_time>",
        args.max_execution_time_s
    );
    let _ = writeln!(
        constraints,
        "        <max_memory_usage><min>1</min><max>{MAX_QUERY_MEMORY}</max></max_memory_usage>"
    );
    let _ = writeln!(
        constraints,
        "        <max_execution_time><min>{MIN_EXECUTION_TIME_S}</min><max>{}</max></max_execution_time>",
        args.max_execution_time_s
    );
    let _ = writeln!(
        constraints,
        "        <max_threads><max>{}</max></max_threads>",
        max_threads_cap()
    );
    // HE1 R9.11: settings a client may send but only switched off (a cached
    // result could outlive the snapshot its read view pinned).
    for name in loams_house_ipc::policy::OFF_ONLY_SETTINGS {
        let _ = writeln!(constraints, "        <{name}><max>0</max></{name}>");
    }
    // HS1 Task 21: §49 §12's read and result caps, held the same way.
    for (name, cap) in [
        ("max_bytes_to_read", MAX_BYTES_TO_READ),
        ("max_result_bytes", MAX_RESULT_BYTES),
    ] {
        let _ = writeln!(pinned, "      <{name}>{cap}</{name}>");
        let _ = writeln!(
            constraints,
            "        <{name}><min>1</min><max>{cap}</max></{name}>"
        );
    }
    format!("{pinned}      <constraints>\n{constraints}      </constraints>\n")
}

/// Escapes text for an XML element body.
fn xml_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Writes `config.xml` and `users.xml` and creates the directories.
///
/// `open_acl` is refused unless this is a test build (`TEST_HOOKS`): `WorkerArgs`
/// is a public struct, so a caller could set it without [`WorkerArgs::parse`]
/// (PR #480 review).
pub fn write_files(args: &WorkerArgs) -> std::io::Result<()> {
    open_acl_allowed(args, loams_house_ipc::TEST_HOOKS)?;
    for dir in [&args.tmp_dir, &args.data_dir(), &args.files_dir()] {
        std::fs::create_dir_all(dir)?;
    }
    write(&args.config_file(), &config_xml(args))?;
    write(&args.users_file(), &users_xml(args))
}

/// `InvalidInput` for `open_acl` in a build without the test hooks.
fn open_acl_allowed(args: &WorkerArgs, test_hooks: bool) -> std::io::Result<()> {
    if args.open_acl && !test_hooks {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "open_acl is for test builds only (HS1 R-ACL.1)",
        ));
    }
    Ok(())
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
            open_acl: false,
            max_execution_time_s: MAX_EXECUTION_TIME_S,
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
    fn the_endpoint_is_no_grant() {
        // R-ACL.1: the endpoint reaches the views' text, never `users.xml`, so a
        // hostile one cannot end a grant early (the HS1 Task 2 review M4 case).
        let users = users_xml(&WorkerArgs {
            s3_endpoint: Some("http://127.0.0.1:1/x'); GRANT ALL ON *.* TO default; --".into()),
            ..args()
        });
        assert!(!users.contains("127.0.0.1:1/"), "{users}");
        assert!(!users.contains("GRANT ALL"), "{users}");
    }

    #[test]
    fn open_acl_is_for_test_builds_only() {
        let open = WorkerArgs {
            open_acl: true,
            ..args()
        };
        let parsed = WorkerArgs::parse(open.to_args());
        // `write_files` refuses it in a shipped build, whatever built the args.
        assert_eq!(
            open_acl_allowed(&open, false).expect_err("refused").kind(),
            std::io::ErrorKind::InvalidInput
        );
        assert!(open_acl_allowed(&open, true).is_ok());
        assert!(open_acl_allowed(&args(), false).is_ok());
        if loams_house_ipc::TEST_HOOKS {
            assert_eq!(parsed, Ok(open.clone()));
            let users = users_xml(&open);
            assert!(users.contains("<query>GRANT ALL ON *.*</query>"), "{users}");
            assert!(!users.contains("<constraints>"), "{users}");
        } else {
            assert!(parsed.is_err(), "a shipped worker refuses --test-open-acl");
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
    fn users_file_grants_no_source() {
        let users = users_xml(&args());
        for query in GRANTS {
            assert!(
                users.contains(&format!("<query>{query}</query>")),
                "{query}"
            );
        }
        assert!(
            users.contains("<networks><ip>127.0.0.1</ip></networks>"),
            "R1.11"
        );
        // R-ACL.1: no source, no host access, no DDL beyond the read views.
        let grants: String = users
            .lines()
            .filter(|l| l.contains("<query>GRANT"))
            .collect();
        for absent in [
            " ON S3",
            " ON URL",
            " ON FILE",
            " ON REMOTE",
            "READ",
            "WRITE",
            "SOURCES",
            "SYSTEM",
            "INTROSPECTION",
            "CREATE TABLE",
            "FUNCTION",
            "DICTIONARY",
            "dictGet",
            "INSERT",
            "DEFINER",
            "ALL ",
            "KILL",
            "IMPERSONATE",
            "ACCESS",
        ] {
            assert!(
                !grants.contains(absent),
                "{absent} must not be granted:\n{grants}"
            );
        }
        assert!(users.contains("<max_rows_to_group_by><readonly/></max_rows_to_group_by>"));
        assert!(
            !users.contains("<max_block_size><readonly/>"),
            "an allowed setting"
        );
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
            "<max_execution_time><min>0.000001</min><max>900</max></max_execution_time>"
        ));
        assert!(users.contains("<max_execution_time>900</max_execution_time>"));
        // D1571: the front's cap, passed as --max-execution-time.
        let tighter = users_xml(&WorkerArgs {
            max_execution_time_s: 120,
            ..args()
        });
        assert!(tighter.contains("<max>120</max></max_execution_time>"));
        let parsed = WorkerArgs::parse(
            WorkerArgs {
                max_execution_time_s: 1200,
                ..args()
            }
            .to_args(),
        )
        .expect("parses");
        assert_eq!(parsed.max_execution_time_s, 1200);
        assert!(
            WorkerArgs::parse(
                [
                    "--tmp-dir",
                    "/w",
                    "--worker-id",
                    "w",
                    "--max-execution-time",
                    "0"
                ]
                .map(String::from)
            )
            .is_err()
        );
        assert!(users.contains("<max_threads><max>"));
        assert!(users.contains("<use_query_cache><max>0</max></use_query_cache>"));
        assert!(users.contains("<max_bytes_to_read>1099511627776</max_bytes_to_read>"));
        assert!(users.contains(
            "<max_bytes_to_read><min>1</min><max>1099511627776</max></max_bytes_to_read>"
        ));
        assert!(users.contains("<max_result_bytes>10737418240</max_result_bytes>"));
        assert!(
            users.contains(
                "<max_result_bytes><min>1</min><max>10737418240</max></max_result_bytes>"
            )
        );
        let none = users_xml(&WorkerArgs {
            s3_endpoint: None,
            ..args()
        });
        assert!(!none.contains("ON S3"), "no endpoint, no S3 grant");
    }
}
