//! `tidb.toml` and the bootstrap SQL of one branch's `tidb-server` pool
//! (§47 §5.1; plan SQ1 Task 2).
//!
//! Keys checked against `pkg/config/config.go` at v8.5.8. Two settings are
//! global system variables in v8.5.8, not config items, so they are rendered
//! as SQL ([`tidb_init_sql`]), which TiDB runs once, at the keyspace's first
//! bootstrap (`initialize-sql-file`):
//! - `tidb_server_memory_limit = '80%'` of the container or pod limit, and
//!   `tidb_mem_quota_query` (40 % of the class memory, in bytes);
//! - `tidb_redact_log = OFF` (R2.8): TiDB redacts error messages when they
//!   are created, so ON or MARKER would send clients `Duplicate entry '?'`
//!   and break MySQL compatibility (D735). The log pipeline protects the
//!   slow and general logs instead: they are not shipped off the pod by
//!   default.
//!
//! The control plane re-applies [`tidb_globals`] with `SET GLOBAL`
//! unconditionally after every bootstrap, copy and class change; the
//! statements are idempotent. That also covers TiDB treating a failed
//! `initialize-sql-file` statement as a warning only (R2.2).
//!
//! Nothing rendered here is a secret: TLS material is referenced by path.

use std::fmt::Write as _;

use crate::model::{BranchId, Class, Endpoints};

/// The version string TiDB advertises (Q658).
pub const SERVER_VERSION: &str = "8.0.11-TiDB-v8.5.8-Loams";
/// The directory a runtime mounts the rendered files into.
pub const CONFIG_DIR: &str = "/etc/tidb";
/// The rendered `tidb.toml` inside the container.
pub const CONFIG_PATH: &str = "/etc/tidb/tidb.toml";
/// The rendered bootstrap SQL inside the container.
pub const INIT_SQL_PATH: &str = "/etc/tidb/init.sql";
/// Gate → TiDB TLS: `ca.crt`, `tls.crt`, `tls.key` (a Kubernetes TLS Secret).
pub const TLS_DIR: &str = "/etc/tidb/tls";
/// TiDB → PD/TiKV TLS, same file names, when [`Endpoints::cluster_tls`].
pub const CLUSTER_TLS_DIR: &str = "/etc/tidb/cluster-tls";

/// Renders `tidb.toml` for one member of `branch`'s pool. Per-member values
/// (listen host and ports) are command-line flags, not config.
pub fn tidb(branch: &BranchId, class: Class, endpoints: &Endpoints) -> String {
    let mut s = String::with_capacity(1536);
    // `write!` to a String cannot fail.
    let _ = write!(
        s,
        "\
# Rendered by loams-sqldb (render::tidb). Do not edit.
# Branch {branch}, class {class}: {vcpu} vCPU, {mem} MiB.

store = \"tikv\"
path = \"{path}\"
# Every Loams TiDB serves exactly one keyspace (D260).
keyspace-name = \"{branch}\"
# One region per database, not one per system table (R1.4).
split-table = false
server-version = \"{SERVER_VERSION}\"
enable-global-kill = true
# Runs once, at the keyspace's first bootstrap: memory limits and log
# redaction (global variables in v8.5.8, not config items).
initialize-sql-file = \"{INIT_SQL_PATH}\"

[instance]
tidb_enable_ddl = true

[performance]
# The v8.5.8 default (true) holds the port until statistics load (R1.3).
force-init-stats = false
lite-init-stats = true

[proxy-protocol]
# loams-sqlgate's networks; every connection from them carries a PROXY header.
networks = \"{networks}\"
header-timeout = 5
fallbackable = false

[security]
ssl-ca = \"{TLS_DIR}/ca.crt\"
ssl-cert = \"{TLS_DIR}/tls.crt\"
ssl-key = \"{TLS_DIR}/tls.key\"
tls-version = \"TLSv1.2\"
",
        vcpu = Vcpu(class.vcpu_millis()),
        mem = class.memory_mib(),
        path = endpoints.pd().join(","),
        networks = endpoints
            .gate_networks()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(","),
    );
    if endpoints.cluster_tls() {
        let _ = write!(
            s,
            "\
cluster-ssl-ca = \"{CLUSTER_TLS_DIR}/ca.crt\"
cluster-ssl-cert = \"{CLUSTER_TLS_DIR}/tls.crt\"
cluster-ssl-key = \"{CLUSTER_TLS_DIR}/tls.key\"
"
        );
    }
    s.push_str(
        "
[log]
level = \"info\"
# tidb_redact_log stays OFF (errors must keep MySQL's text). The slow and
# general logs carry literals and are not shipped off the pod by default.
",
    );
    s
}

/// The global variables a pool of `class` needs, as `(name, SQL literal)`.
/// Applied by [`tidb_init_sql`] at bootstrap, and re-applied with `SET
/// GLOBAL` (idempotently) after every bootstrap, copy and class change.
pub fn tidb_globals(class: Class) -> Vec<(&'static str, String)> {
    vec![
        // A percentage of the memory TiDB sees (its cgroup limit), so it
        // follows the class of whichever pod reads it (R2.2).
        ("tidb_server_memory_limit", "'80%'".to_owned()),
        (
            "tidb_mem_quota_query",
            class.mem_quota_query_bytes().to_string(),
        ),
        ("tidb_redact_log", "'OFF'".to_owned()),
    ]
}

/// The bootstrap SQL (`initialize-sql-file`) for a pool of `class`.
pub fn tidb_init_sql(class: Class) -> String {
    let mut s = format!(
        "-- Rendered by loams-sqldb (render::tidb_init_sql). Do not edit.\n\
         -- Class {class}: {} MiB; runs once, at the keyspace's first bootstrap.\n",
        class.memory_mib()
    );
    for (name, value) in tidb_globals(class) {
        let _ = writeln!(s, "SET GLOBAL {name} = {value};");
    }
    s
}

/// Thousandths of a vCPU, printed as a decimal (`0.25`, `4`).
struct Vcpu(u32);

impl std::fmt::Display for Vcpu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (whole, frac) = (self.0 / 1000, self.0 % 1000);
        if frac == 0 {
            write!(f, "{whole}")
        } else {
            let frac = format!("{frac:03}");
            write!(f, "{whole}.{}", frac.trim_end_matches('0'))
        }
    }
}
