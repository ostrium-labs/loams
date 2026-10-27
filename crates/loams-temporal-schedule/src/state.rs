//! A schedule's runtime state and the pure steps that move it (D2071):
//! which spec times are due, the catch-up window, the buffer and the overlap
//! policies, limited actions, and what a closed workflow changes.
//!
//! The gateway's driver (in `loams-temporal-frontend`) loads a schedule,
//! runs these steps, performs the [`Plan`] (start, cancel, terminate) and
//! commits the new state by compare-and-set. Nothing here does I/O or reads
//! a clock.

use chrono::{DateTime, Utc};
use loams_temporal_proto::temporal::api::enums::v1::ScheduleOverlapPolicy;
use loams_temporal_proto::temporal::api::schedule::v1::SchedulePolicies;
use serde::{Deserialize, Serialize};

use crate::spec::{CompiledSpec, duration_ms};

/// The most starts a buffer holds; more are dropped (`buffer_dropped`).
pub const MAX_BUFFER: usize = 1000;
/// How many recent actions DescribeSchedule reports.
pub const RECENT_ACTIONS: usize = 10;
/// The catch-up window when the policies leave it unset (one year, as
/// Temporal).
pub const DEFAULT_CATCHUP_MS: i64 = 365 * 24 * 3_600_000;
/// The smallest catch-up window (Temporal's minimum, 10 s).
pub const MIN_CATCHUP_MS: i64 = 10_000;
/// The most spec times one pass looks at; past it the pass asks to run
/// again at once, so a long outage on a per-second spec stays bounded.
pub const MAX_TIMES_PER_PASS: usize = 10_000;

/// `WorkflowExecutionStatus` values the schedule reads.
pub mod status {
    pub const RUNNING: i32 = 1;
    pub const COMPLETED: i32 = 2;
    pub const FAILED: i32 = 3;
    pub const CANCELED: i32 = 4;
    pub const TERMINATED: i32 = 5;
    pub const CONTINUED_AS_NEW: i32 = 6;
    pub const TIMED_OUT: i32 = 7;
}

/// The overlap policy, resolved (never "unspecified").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Overlap {
    Skip,
    BufferOne,
    BufferAll,
    CancelOther,
    TerminateOther,
    AllowAll,
}

impl Overlap {
    /// The proto value, with "unspecified" (and unknown values) as `default`.
    pub fn from_proto(v: i32, default: Overlap) -> Overlap {
        match ScheduleOverlapPolicy::try_from(v) {
            Ok(ScheduleOverlapPolicy::Skip) => Overlap::Skip,
            Ok(ScheduleOverlapPolicy::BufferOne) => Overlap::BufferOne,
            Ok(ScheduleOverlapPolicy::BufferAll) => Overlap::BufferAll,
            Ok(ScheduleOverlapPolicy::CancelOther) => Overlap::CancelOther,
            Ok(ScheduleOverlapPolicy::TerminateOther) => Overlap::TerminateOther,
            Ok(ScheduleOverlapPolicy::AllowAll) => Overlap::AllowAll,
            Ok(ScheduleOverlapPolicy::Unspecified) | Err(_) => default,
        }
    }

    /// The proto value.
    pub fn to_proto(self) -> i32 {
        (match self {
            Overlap::Skip => ScheduleOverlapPolicy::Skip,
            Overlap::BufferOne => ScheduleOverlapPolicy::BufferOne,
            Overlap::BufferAll => ScheduleOverlapPolicy::BufferAll,
            Overlap::CancelOther => ScheduleOverlapPolicy::CancelOther,
            Overlap::TerminateOther => ScheduleOverlapPolicy::TerminateOther,
            Overlap::AllowAll => ScheduleOverlapPolicy::AllowAll,
        }) as i32
    }
}

/// The schedule's policies, resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policies {
    pub overlap: Overlap,
    pub catchup_ms: i64,
    pub pause_on_failure: bool,
    pub keep_original_workflow_id: bool,
}

impl Policies {
    /// From the proto, with Temporal's defaults: skip, a one-year catch-up
    /// window, at least 10 s.
    pub fn from_proto(p: Option<&SchedulePolicies>) -> Policies {
        let Some(p) = p else {
            return Policies {
                overlap: Overlap::Skip,
                catchup_ms: DEFAULT_CATCHUP_MS,
                pause_on_failure: false,
                keep_original_workflow_id: false,
            };
        };
        let catchup_ms = match p.catchup_window.as_ref().map(duration_ms) {
            None | Some(0) => DEFAULT_CATCHUP_MS,
            Some(ms) => ms.max(MIN_CATCHUP_MS),
        };
        Policies {
            overlap: Overlap::from_proto(p.overlap_policy, Overlap::Skip),
            catchup_ms,
            pause_on_failure: p.pause_on_failure,
            keep_original_workflow_id: p.keep_original_workflow_id,
        }
    }
}

/// A start waiting for its turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BufferedStart {
    pub nominal_ms: i64,
    pub actual_ms: i64,
    pub overlap: Overlap,
    /// A trigger or a backfill: not counted against limited actions.
    pub manual: bool,
}

/// A workflow the schedule started that has not been seen to close.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Running {
    pub workflow_id: String,
    pub run_id: String,
    /// A cancel was requested by `CANCEL_OTHER` (asked once).
    #[serde(default)]
    pub cancel_requested: bool,
}

/// One action taken, for DescribeSchedule's `recent_actions`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentAction {
    /// The scheduled (jittered) time.
    pub schedule_ms: i64,
    /// When the action was taken.
    pub actual_ms: i64,
    pub workflow_id: String,
    pub run_id: String,
    pub status: i32,
}

/// The state the gateway keeps beside the `Schedule` proto.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Runtime {
    /// The last nominal time processed: the next pass looks after it.
    pub cursor_ms: i64,
    #[serde(default)]
    pub buffer: Vec<BufferedStart>,
    #[serde(default)]
    pub running: Vec<Running>,
    #[serde(default)]
    pub recent: Vec<RecentAction>,
    #[serde(default)]
    pub action_count: i64,
    #[serde(default)]
    pub missed_catchup_window: i64,
    #[serde(default)]
    pub overlap_skipped: i64,
    #[serde(default)]
    pub buffer_dropped: i64,
    pub create_time_ms: i64,
    pub update_time_ms: i64,
    /// When the armed timer fires (None: nothing to wait for).
    #[serde(default)]
    pub next_wake_ms: Option<i64>,
}

impl Runtime {
    /// A new schedule's state: it looks for times after `now`.
    pub fn new(now_ms: i64) -> Runtime {
        Runtime {
            cursor_ms: now_ms,
            create_time_ms: now_ms,
            update_time_ms: now_ms,
            ..Default::default()
        }
    }

    /// Adds a start to the buffer, dropping it when the buffer is full.
    pub fn push(&mut self, b: BufferedStart) {
        if self.buffer.len() >= MAX_BUFFER {
            self.buffer_dropped += 1;
        } else {
            self.buffer.push(b);
        }
    }

    /// Records a started workflow.
    pub fn started(&mut self, b: &BufferedStart, now_ms: i64, workflow_id: &str, run_id: &str) {
        self.action_count += 1;
        self.running.push(Running {
            workflow_id: workflow_id.to_string(),
            run_id: run_id.to_string(),
            cancel_requested: false,
        });
        self.recent.push(RecentAction {
            schedule_ms: b.actual_ms,
            actual_ms: now_ms,
            workflow_id: workflow_id.to_string(),
            run_id: run_id.to_string(),
            status: status::RUNNING,
        });
        if self.recent.len() > RECENT_ACTIONS {
            let extra = self.recent.len() - RECENT_ACTIONS;
            self.recent.drain(..extra);
        }
    }

    /// Records that a running workflow closed with `status`. Answers whether
    /// it was one of ours.
    pub fn closed(&mut self, workflow_id: &str, run_id: &str, status: i32) -> bool {
        let before = self.running.len();
        self.running
            .retain(|r| !(r.workflow_id == workflow_id && r.run_id == run_id));
        for a in &mut self.recent {
            if a.workflow_id == workflow_id && a.run_id == run_id {
                a.status = status;
            }
        }
        self.running.len() != before
    }
}

/// Whether a closed workflow's status pauses a schedule with
/// `pause_on_failure`.
pub fn is_failure(status: i32) -> bool {
    status == status::FAILED || status == status::TIMED_OUT
}

/// What [`collect_due`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Due {
    /// When the next spec time is due (its actual time), if any.
    pub next_wake_ms: Option<i64>,
    /// Starts buffered by this pass.
    pub buffered: usize,
}

/// Buffers every spec time due by `now` after the cursor: a time whose
/// actual time is older than the catch-up window is counted as missed
/// instead. A paused schedule skips its times. A schedule with limited
/// actions and none left buffers nothing more.
pub fn collect_due(
    rt: &mut Runtime,
    spec: &CompiledSpec,
    seed: &str,
    now_ms: i64,
    policies: &Policies,
    paused: bool,
    actions_left: Option<i64>,
) -> Due {
    if paused {
        rt.cursor_ms = rt.cursor_ms.max(now_ms);
        return Due {
            next_wake_ms: None,
            buffered: 0,
        };
    }
    let mut buffered = 0usize;
    // Automatic starts already waiting count against the limit.
    let waiting = rt.buffer.iter().filter(|b| !b.manual).count() as i64;
    let mut left = actions_left.map(|n| n - waiting);
    for _ in 0..MAX_TIMES_PER_PASS {
        if left.is_some_and(|n| n <= 0) {
            rt.cursor_ms = rt.cursor_ms.max(now_ms);
            return Due {
                next_wake_ms: None,
                buffered,
            };
        }
        let Some(n) = spec.next(seed, rt.cursor_ms) else {
            rt.cursor_ms = rt.cursor_ms.max(now_ms);
            return Due {
                next_wake_ms: None,
                buffered,
            };
        };
        if n.actual_ms > now_ms {
            return Due {
                next_wake_ms: Some(n.actual_ms),
                buffered,
            };
        }
        rt.cursor_ms = n.nominal_ms;
        if now_ms - n.actual_ms > policies.catchup_ms {
            rt.missed_catchup_window += 1;
            continue;
        }
        rt.push(BufferedStart {
            nominal_ms: n.nominal_ms,
            actual_ms: n.actual_ms,
            overlap: policies.overlap,
            manual: false,
        });
        buffered += 1;
        if let Some(l) = left.as_mut() {
            *l -= 1;
        }
    }
    // Too many times in one pass: come back at once for the rest.
    Due {
        next_wake_ms: Some(now_ms),
        buffered,
    }
}

/// What to do with the buffer now.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// Start these, in order.
    pub starts: Vec<BufferedStart>,
    /// What stays buffered.
    pub buffer: Vec<BufferedStart>,
    /// Starts dropped by `SKIP` or `BUFFER_ONE`, or superseded.
    pub overlap_skipped: i64,
    /// Request cancellation of the running workflows (`CANCEL_OTHER`).
    pub cancel_running: bool,
    /// Terminate the running workflows (`TERMINATE_OTHER`), then plan again.
    pub terminate_running: bool,
}

/// Decides, for each buffered start in order, by its overlap policy:
///
/// - `ALLOW_ALL`: start now, whatever runs.
/// - `SKIP`: start if nothing runs or waits; otherwise drop it.
/// - `BUFFER_ONE`: start if nothing runs or waits; otherwise keep it if
///   nothing is kept yet, else drop it.
/// - `BUFFER_ALL`: start if nothing runs or waits; otherwise keep it.
/// - `CANCEL_OTHER`: start if nothing runs or waits; otherwise ask the
///   running workflows to cancel and keep it (replacing an earlier kept
///   `CANCEL_OTHER` start); it starts once they have closed.
/// - `TERMINATE_OTHER`: as `CANCEL_OTHER` but terminate; the driver then
///   plans again, so it starts at once.
///
/// Only one non-`ALLOW_ALL` start begins per plan while nothing else runs,
/// so buffered starts run one after another.
pub fn plan(buffer: &[BufferedStart], running: bool) -> Plan {
    let mut p = Plan::default();
    let mut busy = running;
    for b in buffer {
        let waiting = busy || !p.buffer.is_empty();
        match b.overlap {
            Overlap::AllowAll => p.starts.push(*b),
            Overlap::Skip => {
                if waiting {
                    p.overlap_skipped += 1;
                } else {
                    p.starts.push(*b);
                    busy = true;
                }
            }
            Overlap::BufferOne => {
                if !waiting {
                    p.starts.push(*b);
                    busy = true;
                } else if p.buffer.is_empty() {
                    p.buffer.push(*b);
                } else {
                    p.overlap_skipped += 1;
                }
            }
            Overlap::BufferAll => {
                if !waiting {
                    p.starts.push(*b);
                    busy = true;
                } else {
                    p.buffer.push(*b);
                }
            }
            Overlap::CancelOther | Overlap::TerminateOther => {
                if !waiting {
                    p.starts.push(*b);
                    busy = true;
                } else {
                    let before = p.buffer.len();
                    p.buffer.retain(|x| x.overlap != b.overlap);
                    p.overlap_skipped += (before - p.buffer.len()) as i64;
                    p.buffer.push(*b);
                    if running {
                        if b.overlap == Overlap::CancelOther {
                            p.cancel_running = true;
                        } else {
                            p.terminate_running = true;
                        }
                    }
                }
            }
        }
    }
    p
}

/// The workflow ID of a scheduled start: the action's ID, and unless the
/// policies keep it, `-` and the nominal time in RFC 3339 (UTC, seconds), as
/// Temporal names them.
pub fn scheduled_workflow_id(base: &str, nominal_ms: i64, keep_original: bool) -> String {
    if keep_original {
        return base.to_string();
    }
    let t = DateTime::<Utc>::from_timestamp_millis(nominal_ms)
        .map(|t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_else(|| nominal_ms.to_string());
    format!("{base}-{t}")
}

/// `ms` as RFC 3339 in UTC (`2026-10-27T09:00:00Z`, with a fraction only
/// when there is one): how Datetime search attributes are written.
pub fn rfc3339(ms: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(ms)
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true))
        .unwrap_or_default()
}

/// The starts a backfill of `[from, to]` buffers (manual: not counted
/// against limited actions, not subject to the catch-up window).
pub fn backfill(
    spec: &CompiledSpec,
    seed: &str,
    from_ms: i64,
    to_ms: i64,
    overlap: Overlap,
) -> Vec<BufferedStart> {
    spec.times_between(seed, from_ms, to_ms, MAX_BUFFER)
        .into_iter()
        .map(|t| BufferedStart {
            nominal_ms: t.nominal_ms,
            actual_ms: t.actual_ms,
            overlap,
            manual: true,
        })
        .collect()
}

/// The start a trigger buffers.
pub fn trigger(now_ms: i64, scheduled_ms: Option<i64>, overlap: Overlap) -> BufferedStart {
    BufferedStart {
        nominal_ms: scheduled_ms.unwrap_or(now_ms),
        actual_ms: now_ms,
        overlap,
        manual: true,
    }
}

/// The seed of a schedule's jitter.
pub fn jitter_seed(namespace_id: &str, schedule_id: &str) -> String {
    format!("{namespace_id}/{schedule_id}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use loams_temporal_proto::temporal::api::schedule::v1::ScheduleSpec;

    fn b(nominal_ms: i64, overlap: Overlap) -> BufferedStart {
        BufferedStart {
            nominal_ms,
            actual_ms: nominal_ms,
            overlap,
            manual: false,
        }
    }

    fn every_minute() -> CompiledSpec {
        CompiledSpec::compile(&ScheduleSpec {
            cron_string: vec!["@every 1m".into()],
            ..Default::default()
        })
        .unwrap()
    }

    fn policies(overlap: Overlap, catchup_ms: i64) -> Policies {
        Policies {
            overlap,
            catchup_ms,
            pause_on_failure: false,
            keep_original_workflow_id: false,
        }
    }

    #[test]
    fn skip_starts_one_and_drops_the_rest() {
        let p = plan(&[b(1, Overlap::Skip), b(2, Overlap::Skip)], false);
        assert_eq!(p.starts, vec![b(1, Overlap::Skip)]);
        assert_eq!(p.overlap_skipped, 1);
        assert!(p.buffer.is_empty());
        let p = plan(&[b(1, Overlap::Skip)], true);
        assert!(p.starts.is_empty());
        assert_eq!(p.overlap_skipped, 1);
    }

    #[test]
    fn buffer_one_keeps_one() {
        let p = plan(
            &[
                b(1, Overlap::BufferOne),
                b(2, Overlap::BufferOne),
                b(3, Overlap::BufferOne),
            ],
            true,
        );
        assert!(p.starts.is_empty());
        assert_eq!(p.buffer, vec![b(1, Overlap::BufferOne)]);
        assert_eq!(p.overlap_skipped, 2);
        // Once the run closes, the kept one starts.
        let p = plan(&p.buffer, false);
        assert_eq!(p.starts, vec![b(1, Overlap::BufferOne)]);
    }

    #[test]
    fn buffer_all_runs_them_one_by_one() {
        let all = [
            b(1, Overlap::BufferAll),
            b(2, Overlap::BufferAll),
            b(3, Overlap::BufferAll),
        ];
        let p = plan(&all, false);
        assert_eq!(p.starts, vec![all[0]]);
        assert_eq!(p.buffer, all[1..].to_vec());
        assert_eq!(p.overlap_skipped, 0);
        let p = plan(&p.buffer, true);
        assert!(p.starts.is_empty());
        assert_eq!(p.buffer.len(), 2);
    }

    #[test]
    fn cancel_other_cancels_and_waits() {
        let p = plan(
            &[b(1, Overlap::CancelOther), b(2, Overlap::CancelOther)],
            true,
        );
        assert!(p.cancel_running && !p.terminate_running);
        assert!(p.starts.is_empty());
        assert_eq!(p.buffer, vec![b(2, Overlap::CancelOther)]);
        assert_eq!(p.overlap_skipped, 1);
        let p = plan(&p.buffer, false);
        assert_eq!(p.starts, vec![b(2, Overlap::CancelOther)]);
        assert!(!p.cancel_running);
    }

    #[test]
    fn terminate_other_terminates_then_starts() {
        let p = plan(&[b(1, Overlap::TerminateOther)], true);
        assert!(p.terminate_running && p.starts.is_empty());
        // The driver terminates and plans again with nothing running.
        let p = plan(&p.buffer, false);
        assert_eq!(p.starts, vec![b(1, Overlap::TerminateOther)]);
    }

    #[test]
    fn allow_all_ignores_everything() {
        let p = plan(
            &[
                b(1, Overlap::AllowAll),
                b(2, Overlap::Skip),
                b(3, Overlap::AllowAll),
            ],
            true,
        );
        assert_eq!(
            p.starts,
            vec![b(1, Overlap::AllowAll), b(3, Overlap::AllowAll)]
        );
        assert_eq!(p.overlap_skipped, 1);
    }

    #[test]
    fn collect_due_buffers_due_times_and_wakes_for_the_next() {
        let spec = every_minute();
        let mut rt = Runtime::new(0);
        let d = collect_due(
            &mut rt,
            &spec,
            "s",
            150_000,
            &policies(Overlap::Skip, DEFAULT_CATCHUP_MS),
            false,
            None,
        );
        assert_eq!(d.buffered, 2);
        assert_eq!(d.next_wake_ms, Some(180_000));
        assert_eq!(rt.cursor_ms, 120_000);
        assert_eq!(
            rt.buffer.iter().map(|b| b.nominal_ms).collect::<Vec<_>>(),
            [60_000, 120_000]
        );
        // A second pass at the same time finds nothing new.
        let d = collect_due(
            &mut rt,
            &spec,
            "s",
            150_000,
            &policies(Overlap::Skip, DEFAULT_CATCHUP_MS),
            false,
            None,
        );
        assert_eq!((d.buffered, d.next_wake_ms), (0, Some(180_000)));
    }

    #[test]
    fn catchup_window_counts_missed_times() {
        let spec = every_minute();
        let mut rt = Runtime::new(0);
        // Ten minutes late with a 150 s window: 7 missed, 3 caught up.
        let d = collect_due(
            &mut rt,
            &spec,
            "s",
            600_000,
            &policies(Overlap::BufferAll, 150_000),
            false,
            None,
        );
        assert_eq!(rt.missed_catchup_window, 7);
        assert_eq!(d.buffered, 3);
        assert_eq!(rt.buffer.first().map(|b| b.nominal_ms), Some(480_000));
    }

    #[test]
    fn paused_schedules_skip_their_times() {
        let spec = every_minute();
        let mut rt = Runtime::new(0);
        let d = collect_due(
            &mut rt,
            &spec,
            "s",
            600_000,
            &policies(Overlap::Skip, DEFAULT_CATCHUP_MS),
            true,
            None,
        );
        assert_eq!(
            d,
            Due {
                next_wake_ms: None,
                buffered: 0
            }
        );
        assert_eq!(rt.cursor_ms, 600_000);
        assert!(rt.buffer.is_empty());
    }

    #[test]
    fn limited_actions_stop_buffering() {
        let spec = every_minute();
        let mut rt = Runtime::new(0);
        let d = collect_due(
            &mut rt,
            &spec,
            "s",
            600_000,
            &policies(Overlap::BufferAll, DEFAULT_CATCHUP_MS),
            false,
            Some(2),
        );
        assert_eq!(d.buffered, 2);
        assert_eq!(d.next_wake_ms, None);
        // Waiting automatic starts count too.
        let d = collect_due(
            &mut rt,
            &spec,
            "s",
            700_000,
            &policies(Overlap::BufferAll, DEFAULT_CATCHUP_MS),
            false,
            Some(2),
        );
        assert_eq!(d.buffered, 0);
    }

    #[test]
    fn long_outages_are_bounded_per_pass() {
        let spec = CompiledSpec::compile(&ScheduleSpec {
            cron_string: vec!["@every 1s".into()],
            ..Default::default()
        })
        .unwrap();
        let mut rt = Runtime::new(0);
        let d = collect_due(
            &mut rt,
            &spec,
            "s",
            86_400_000,
            &policies(Overlap::Skip, 5_000),
            false,
            None,
        );
        assert_eq!(d.next_wake_ms, Some(86_400_000));
        assert_eq!(
            rt.missed_catchup_window as usize + d.buffered,
            MAX_TIMES_PER_PASS
        );
    }

    #[test]
    fn runtime_tracks_starts_and_closes() {
        let mut rt = Runtime::new(0);
        for i in 0..12 {
            rt.started(
                &b(i, Overlap::AllowAll),
                i,
                &format!("wf-{i}"),
                &format!("run-{i}"),
            );
        }
        assert_eq!(rt.action_count, 12);
        assert_eq!(rt.recent.len(), RECENT_ACTIONS);
        assert_eq!(rt.recent[0].workflow_id, "wf-2");
        assert!(rt.closed("wf-5", "run-5", status::FAILED));
        assert!(!rt.closed("wf-5", "run-5", status::FAILED));
        assert_eq!(rt.running.len(), 11);
        assert_eq!(
            rt.recent
                .iter()
                .find(|a| a.workflow_id == "wf-5")
                .unwrap()
                .status,
            status::FAILED
        );
        assert!(is_failure(status::TIMED_OUT) && !is_failure(status::TERMINATED));
        let mut full = Runtime::new(0);
        for i in 0..(MAX_BUFFER as i64 + 5) {
            full.push(b(i, Overlap::BufferAll));
        }
        assert_eq!((full.buffer.len(), full.buffer_dropped), (MAX_BUFFER, 5));
    }

    #[test]
    fn ids_policies_backfill_and_trigger() {
        assert_eq!(
            scheduled_workflow_id("report", 1_792_800_000_000, false),
            "report-2026-10-24T00:00:00Z"
        );
        assert_eq!(scheduled_workflow_id("report", 1, true), "report");
        assert_eq!(rfc3339(1_792_800_000_000), "2026-10-24T00:00:00Z");
        assert_eq!(rfc3339(1_792_800_000_250), "2026-10-24T00:00:00.250Z");
        let p = Policies::from_proto(Some(&SchedulePolicies {
            overlap_policy: 0,
            catchup_window: Some(prost_types::Duration {
                seconds: 1,
                nanos: 0,
            }),
            ..Default::default()
        }));
        assert_eq!((p.overlap, p.catchup_ms), (Overlap::Skip, MIN_CATCHUP_MS));
        assert_eq!(Policies::from_proto(None).catchup_ms, DEFAULT_CATCHUP_MS);
        for o in [
            Overlap::Skip,
            Overlap::BufferOne,
            Overlap::BufferAll,
            Overlap::CancelOther,
            Overlap::TerminateOther,
            Overlap::AllowAll,
        ] {
            assert_eq!(Overlap::from_proto(o.to_proto(), Overlap::Skip), o);
        }
        let spec = every_minute();
        let starts = backfill(&spec, "s", 60_000, 180_000, Overlap::BufferAll);
        assert_eq!(
            starts.iter().map(|b| b.nominal_ms).collect::<Vec<_>>(),
            [60_000, 120_000, 180_000]
        );
        assert!(starts.iter().all(|b| b.manual));
        let t = trigger(5, None, Overlap::AllowAll);
        assert_eq!((t.nominal_ms, t.actual_ms, t.manual), (5, 5, true));
    }
}
