//! The House's resource limits in one place (HS1 Task 21, design §49 §12, D773).
//!
//! # Per query
//!
//! | Limit | Default | Enforced by | Past it |
//! |---|---|---|---|
//! | `max_memory_usage` | 4 GiB ([`MAX_MEMORY_USAGE`]) | chDB setting (the worker profile starts at the cap and constrains it); the worker's cgroup `memory.max` = the cap + 512 MiB | `241` (chDB's, or the front's when the cgroup's OOM killer ends the worker: [`crate::watchdog`] reads `memory.events`) |
//! | `max_execution_time` | 900 s ([`MAX_EXECUTION_TIME_S`], D1571; `[house.limits] max_execution_time_s`) | chDB setting; the lease's watchdog kills the worker at the statement's limit + [`WATCHDOG_GRACE`] | `159` |
//! | `max_threads` | the node's cores | chDB setting; cgroup `cpu.max` | — |
//! | `max_bytes_to_read` | 1 TiB ([`MAX_BYTES_TO_READ`]) | chDB setting | chDB's `307` |
//! | `max_result_bytes` | 10 GiB ([`MAX_RESULT_BYTES`]) | chDB setting | chDB's `396` |
//!
//! A setting a request, a session or a `SETTINGS` clause sends above a cap is
//! refused, never clamped: `164` from the front ([`crate::settings::check`]), or
//! `452` from the worker profile's constraints for what the front cannot see.
//!
//! # Per namespace and per node ([`crate::admission::Admission`])
//!
//! | Limit | Default | Past it |
//! |---|---|---|
//! | Running statements per namespace | 16 ([`MAX_RUNNING_PER_NAMESPACE`]) | queued |
//! | Queued statements per namespace | 64 ([`MAX_QUEUED_PER_NAMESPACE`]), each for at most 30 s ([`QUEUE_TIMEOUT`]) | `202` |
//! | Node memory: the sum of admitted statements' `max_memory_usage` | 85 % of the node's memory ([`NODE_MEMORY_PERCENT`]) | queued, then `202` |
//! | Workers per node and per namespace, pinned workers per namespace | [`crate::admission::PoolConfig`] | waits, then `202` |
//! | Live sessions | 1 024 per front, 64 per user ([`MAX_SESSIONS_PER_USER`]) | `202` |
//! | Pinned workers per user | 2 ([`MAX_PINNED_WORKERS_PER_USER`]) | `202` |
//! | Pipes, materialized views per namespace | 32, 64 ([`MAX_PIPES_PER_NAMESPACE`], [`MAX_VIEWS_PER_NAMESPACE`]; [`crate::catalog::ObjectCaps`]) | `36` |
//!
//! # Per request (the HTTP front; HS1 R3.13)
//!
//! | Limit | Default | Where | Past it |
//! |---|---|---|---|
//! | Request body, decompressed | 16 GiB, at most 100 bytes per compressed byte | [`crate::compress::BodyLimits`] | `36` |
//! | Statement text | 256 KiB | `max_query_size` | `62` |
//! | `buffer_size` | 1 B – 16 MiB | [`crate::config::BUFFER_SIZE_RANGE`] | clamped |
//! | `wait_end_of_query` spool | 1 GiB per response, 8 GiB per front | `wait_end_of_query_max_bytes`, `spool_budget_bytes` | `36`, `202` |
//! | An `INSERT` into a lake table | 1 GiB of decoded rows | `lake_insert_max_bytes` | `36` |
//! | Connections, head, timeouts | 1 024; 128 KiB; `receive_timeout`, `send_timeout`, `keep_alive` | [`crate::config::HouseConfig`] | `431`/closed |
//!
//! `[house.limits]` in the front's TOML sets any of them ([`crate::config::LimitsSection`]).
//! The per-namespace quotas of §49 §15 (rates, daily volumes, storage) are
//! [`crate::quota`]'s, from `[house.quotas]`.

use std::time::Duration;

use loams_house_ipc::Execute;

use crate::settings::SessionLimits;

/// §49 §12: `max_memory_usage` per query, 4 GiB.
pub const MAX_MEMORY_USAGE: u64 = 4 * 1024 * 1024 * 1024;
/// §49 §12 as D1571 amends it (owner, 2026-10-22): `max_execution_time` per
/// query, 900 s by default, the House's `[house.limits] max_execution_time_s`.
pub const MAX_EXECUTION_TIME_S: f64 = 900.0;
/// §49 §12: `max_bytes_to_read` (1 TiB) and `max_result_bytes` (10 GiB) per
/// query, one number for the front and the worker profile.
pub use loams_house_ipc::policy::{MAX_BYTES_TO_READ, MAX_RESULT_BYTES};
/// §49 §12: the watchdog kills a worker this long after its statement's
/// `max_execution_time`.
pub const WATCHDOG_GRACE: Duration = Duration::from_secs(2);
/// §49 §12: statements running at once per namespace.
pub const MAX_RUNNING_PER_NAMESPACE: usize = 16;
/// §49 §12: statements waiting per namespace.
pub const MAX_QUEUED_PER_NAMESPACE: usize = 64;
/// §49 §12: how long a statement waits for admission before `202`.
pub const QUEUE_TIMEOUT: Duration = Duration::from_secs(30);
/// §49 §12: the share of the node's memory admitted statements' caps may sum to.
pub const NODE_MEMORY_PERCENT: u64 = 85;
/// §49 §12: pipes per namespace (`36` past it).
pub const MAX_PIPES_PER_NAMESPACE: usize = 32;
/// §49 §12: materialized views per namespace (`36` past it).
pub const MAX_VIEWS_PER_NAMESPACE: usize = 64;
/// HS1 Task 4 fix round 1, I5: live sessions per user.
pub const MAX_SESSIONS_PER_USER: usize = 64;
/// HS1 Task 4 fix round 1, I5: workers one user's sessions may pin (the
/// namespace's own cap, `max_pinned_workers_per_namespace`, is also 2).
pub const MAX_PINNED_WORKERS_PER_USER: usize = 2;

/// What the front's admission holds statements to (§49 §12).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdmissionLimits {
    /// Statements running at once per namespace.
    pub max_running_per_namespace: usize,
    /// Statements waiting per namespace; one more is `202` at once.
    pub max_queued_per_namespace: usize,
    /// How long a statement waits before `202`.
    pub queue_timeout: Duration,
    /// What admitted statements' `max_memory_usage` may sum to on this node.
    pub node_memory_budget: u64,
}

impl Default for AdmissionLimits {
    fn default() -> Self {
        Self {
            max_running_per_namespace: MAX_RUNNING_PER_NAMESPACE,
            max_queued_per_namespace: MAX_QUEUED_PER_NAMESPACE,
            queue_timeout: QUEUE_TIMEOUT,
            node_memory_budget: node_memory_budget(node_memory_bytes()),
        }
    }
}

/// [`NODE_MEMORY_PERCENT`] of `node`.
pub fn node_memory_budget(node: u64) -> u64 {
    u64::try_from(u128::from(node) * u128::from(NODE_MEMORY_PERCENT) / 100).unwrap_or(u64::MAX)
}

/// The memory this front's node (its pod) has: the tightest `memory.max` of its
/// own cgroup and their ancestors, else the machine's `MemTotal`, else 16 GiB.
pub fn node_memory_bytes() -> u64 {
    let total = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|text| meminfo_total(&text));
    let cgroup = std::fs::read_to_string("/proc/self/cgroup")
        .ok()
        .and_then(|text| own_cgroup_limit(&text, std::path::Path::new("/sys/fs/cgroup")));
    match (cgroup, total) {
        (Some(limit), Some(total)) => limit.min(total),
        (Some(limit), None) => limit,
        (None, Some(total)) => total,
        (None, None) => 16 * 1024 * 1024 * 1024,
    }
}

/// `MemTotal` of `/proc/meminfo`, in bytes.
fn meminfo_total(text: &str) -> Option<u64> {
    let line = text.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    kib.checked_mul(1024)
}

/// The tightest numeric `memory.max` from the cgroup v2 path in `proc_cgroup`
/// (`/proc/self/cgroup`) up to `root`.
fn own_cgroup_limit(proc_cgroup: &str, root: &std::path::Path) -> Option<u64> {
    let path = proc_cgroup
        .lines()
        .find_map(|line| line.strip_prefix("0::"))?
        .trim()
        .trim_start_matches('/');
    let mut dir = root.join(path);
    let mut tightest: Option<u64> = None;
    loop {
        if let Ok(text) = std::fs::read_to_string(dir.join("memory.max"))
            && let Ok(limit) = text.trim().parse::<u64>()
        {
            tightest = Some(tightest.map_or(limit, |t| t.min(limit)));
        }
        if dir == root || !dir.pop() || !dir.starts_with(root) {
            return tightest;
        }
    }
}

/// The last value a statement's settings give `name` (later settings replace
/// earlier ones, as the worker applies them).
fn last<'a>(settings: &'a [(String, String)], name: &str) -> Option<&'a str> {
    settings
        .iter()
        .rev()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

/// The `max_memory_usage` a statement runs with: its last setting, held to the
/// cap (an unreadable one is the cap; the front refused it already).
pub fn statement_memory(settings: &[(String, String)], caps: &SessionLimits) -> u64 {
    last(settings, "max_memory_usage")
        .and_then(crate::settings::parse_size)
        .filter(|n| *n > 0)
        .map_or(caps.max_memory_usage, |n| n.min(caps.max_memory_usage))
}

/// The `max_execution_time` an `Execute` runs with: its `Limits`, else its last
/// setting, else `cap`; never above `cap` (the worker profile's own maximum). A
/// `SETTINGS` clause in the text may lower it further, which chDB enforces itself.
pub fn statement_execution_time(execute: &Execute, cap: Duration) -> Duration {
    let from_settings = || {
        last(&execute.settings, "max_execution_time")
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|s| s.is_finite() && *s > 0.0)
            .map(Duration::from_secs_f64)
    };
    execute
        .limits
        .max_execution_time_ms
        .filter(|ms| *ms > 0)
        .map(Duration::from_millis)
        .or_else(from_settings)
        .map_or(cap, |t| t.min(cap))
}

/// When the watchdog kills a statement that started at `started`: its
/// [`statement_execution_time`] plus [`WATCHDOG_GRACE`].
pub fn watchdog_deadline(
    started: tokio::time::Instant,
    execute: &Execute,
    cap: Duration,
) -> tokio::time::Instant {
    started + statement_execution_time(execute, cap) + WATCHDOG_GRACE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn execute(settings: &[(&str, &str)], limit_ms: Option<u64>) -> Execute {
        Execute {
            query_id: "q".to_string(),
            session: None,
            settings: settings
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            views: Vec::new(),
            sql: "SELECT 1".to_string(),
            format: "TSV".to_string(),
            params: Vec::new(),
            limits: loams_house_ipc::Limits {
                max_execution_time_ms: limit_ms,
                ..Default::default()
            },
            input: None,
            output: loams_house_ipc::Output::Blocks,
            database: None,
        }
    }

    #[test]
    fn the_execution_time_is_the_statements_and_held_to_the_cap() {
        let cap = Duration::from_secs(300);
        assert_eq!(statement_execution_time(&execute(&[], None), cap), cap);
        assert_eq!(
            statement_execution_time(&execute(&[("max_execution_time", "0.5")], None), cap),
            Duration::from_millis(500)
        );
        // The last setting wins; `Limits` win over settings.
        assert_eq!(
            statement_execution_time(
                &execute(
                    &[("max_execution_time", "9"), ("max_execution_time", "2")],
                    None
                ),
                cap
            ),
            Duration::from_secs(2)
        );
        assert_eq!(
            statement_execution_time(&execute(&[("max_execution_time", "9")], Some(1500)), cap),
            Duration::from_millis(1500)
        );
        // Never past the cap, and nothing unreadable or unlimited counts.
        for odd in ["900", "0", "nan", "x"] {
            assert_eq!(
                statement_execution_time(&execute(&[("max_execution_time", odd)], None), cap),
                cap,
                "{odd}"
            );
        }
    }

    #[test]
    fn the_memory_is_the_statements_and_held_to_the_cap() {
        let caps = SessionLimits::default();
        let pairs = |v: &str| vec![("max_memory_usage".to_string(), v.to_string())];
        assert_eq!(statement_memory(&[], &caps), MAX_MEMORY_USAGE);
        assert_eq!(statement_memory(&pairs("1Gi"), &caps), 1 << 30);
        assert_eq!(statement_memory(&pairs("64Gi"), &caps), MAX_MEMORY_USAGE);
        assert_eq!(statement_memory(&pairs("0"), &caps), MAX_MEMORY_USAGE);
    }

    #[test]
    fn node_memory_reads_meminfo_and_the_tightest_cgroup() {
        assert_eq!(
            meminfo_total("MemTotal:       16000000 kB\nMemFree: 1 kB\n"),
            Some(16_000_000 * 1024)
        );
        let root = std::env::temp_dir().join(format!("loams-limits-{}", std::process::id()));
        let pod = root.join("kubepods/pod1");
        let container = pod.join("c1");
        std::fs::create_dir_all(&container).expect("dirs");
        std::fs::write(container.join("memory.max"), "max\n").expect("container");
        std::fs::write(pod.join("memory.max"), "8589934592\n").expect("pod");
        std::fs::write(root.join("kubepods/memory.max"), "17179869184\n").expect("parent");
        assert_eq!(
            own_cgroup_limit("0::/kubepods/pod1/c1\n", &root),
            Some(8 * 1024 * 1024 * 1024)
        );
        assert_eq!(own_cgroup_limit("1:name=x:/\n", &root), None);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(node_memory_budget(100), 85);
        assert!(node_memory_bytes() > 0);
    }

    /// The front's defaults are §49 §12's table (HS1 R3.13, M15).
    #[test]
    fn the_defaults_follow_the_table() {
        let config = crate::config::HouseConfig::default();
        let caps = config.session_limits;
        assert_eq!(caps.max_memory_usage, 4 * 1024 * 1024 * 1024);
        assert!((caps.max_execution_time_s - 900.0).abs() < f64::EPSILON);
        assert_eq!(caps.max_bytes_to_read, 1 << 40);
        assert_eq!(caps.max_result_bytes, 10 * (1 << 30));
        assert_eq!(config.body_limits.max_bytes, 16 * (1 << 30));
        assert_eq!(config.body_limits.max_ratio, 100);
        let admission = &config.admission;
        assert_eq!(admission.max_running_per_namespace, 16);
        assert_eq!(admission.max_queued_per_namespace, 64);
        assert_eq!(admission.queue_timeout, Duration::from_secs(30));
        assert_eq!(config.max_sessions_per_user, 64);
        assert_eq!(config.max_pinned_workers_per_user, 2);
    }
}
