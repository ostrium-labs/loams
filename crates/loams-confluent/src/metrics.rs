//! The Metrics API (`POST /v2/metrics/cloud/query` on
//! `api.telemetry.confluent.cloud`): hourly numbers per cluster, for the
//! budget guard (CC2) and the nightly's checks.
//!
//! Build a [`Client`] with [`TELEMETRY_BASE`] for these calls. From CC0 (§8 of
//! the spike report):
//! - `received_bytes`, `sent_bytes` and `retained_bytes` break down by topic
//!   and partition only, never by principal;
//! - `request_bytes` and `response_bytes` break down by `principal_id` on
//!   Basic and Standard, and are what the guard's per-principal estimate sums
//!   (they read 2 to 7 % high, the safe side for a budget);
//! - data lags 1 to 3 minutes, is kept 7 days, and the limit is 300 requests
//!   a minute per IP;
//! - a MetricsViewer role bound at environment scope is enough.

use reqwest::Method;
use serde_json::{Value, json};

use crate::{Client, Error};

/// The Metrics API.
pub const TELEMETRY_BASE: &str = "https://api.telemetry.confluent.cloud";

/// The longest interval the API keeps.
pub const MAX_INTERVAL_SECS: i64 = 7 * 86_400;

/// A Kafka metric Loams reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Metric {
    ReceivedBytes,
    SentBytes,
    RetainedBytes,
    PartitionCount,
    /// Per principal on Basic and Standard (CC0 R0.7).
    RequestBytes,
    /// Per principal on Basic and Standard (CC0 R0.7).
    ResponseBytes,
    /// Any other `io.confluent.kafka.server/…` metric, by its full name.
    Other(String),
}

impl Metric {
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Metric::ReceivedBytes => "io.confluent.kafka.server/received_bytes",
            Metric::SentBytes => "io.confluent.kafka.server/sent_bytes",
            Metric::RetainedBytes => "io.confluent.kafka.server/retained_bytes",
            Metric::PartitionCount => "io.confluent.kafka.server/partition_count",
            Metric::RequestBytes => "io.confluent.kafka.server/request_bytes",
            Metric::ResponseBytes => "io.confluent.kafka.server/response_bytes",
            Metric::Other(n) => n,
        }
    }

    /// The labels the metric may be grouped by (CC0 §8).
    #[must_use]
    pub fn allows_group_by(&self, label: &str) -> bool {
        match self {
            Metric::ReceivedBytes | Metric::SentBytes | Metric::RetainedBytes => {
                matches!(label, "metric.topic" | "metric.partition")
            }
            Metric::RequestBytes | Metric::ResponseBytes => matches!(
                label,
                "metric.type" | "metric.principal_id" | "metric.principal_name"
            ),
            Metric::PartitionCount => false,
            Metric::Other(_) => true,
        }
    }
}

/// One query: a metric for one cluster over `[start, end)` (unix seconds),
/// hourly unless `granularity` says otherwise.
#[derive(Debug, Clone)]
pub struct Query {
    pub metric: Metric,
    pub cluster: String,
    pub start: i64,
    pub end: i64,
    /// ISO 8601 duration: `PT1M`, `PT1H`, `P1D`, …
    pub granularity: String,
    /// Labels to group by, such as `metric.principal_id`.
    pub group_by: Vec<String>,
}

impl Query {
    #[must_use]
    pub fn hourly(metric: Metric, cluster: &str, start: i64, end: i64) -> Self {
        Self {
            metric,
            cluster: cluster.to_string(),
            start,
            end,
            granularity: "PT1H".into(),
            group_by: Vec::new(),
        }
    }

    #[must_use]
    pub fn group_by(mut self, label: &str) -> Self {
        self.group_by.push(label.to_string());
        self
    }

    fn validate(&self) -> Result<(), Error> {
        let id = self.cluster.strip_prefix("lkc-").unwrap_or_default();
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(Error::Usage(format!(
                "'{}' is not a cluster id (lkc-…)",
                self.cluster
            )));
        }
        if self.end <= self.start {
            return Err(Error::Usage(
                "the interval's end is not after its start".into(),
            ));
        }
        if self.end - self.start > MAX_INTERVAL_SECS {
            return Err(Error::Usage(
                "the Metrics API keeps 7 days; ask for at most 7 days".into(),
            ));
        }
        if let Some(l) = self
            .group_by
            .iter()
            .find(|l| !self.metric.allows_group_by(l))
        {
            return Err(Error::Usage(format!(
                "{} cannot be grouped by {l} (CC0 §8)",
                self.metric.name()
            )));
        }
        Ok(())
    }

    /// The request body.
    #[must_use]
    pub fn body(&self) -> Value {
        let mut b = json!({
            "aggregations": [{ "metric": self.metric.name() }],
            "filter": { "field": "resource.kafka.id", "op": "EQ", "value": self.cluster },
            "granularity": self.granularity,
            "intervals": [format!("{}/{}", rfc3339(self.start), rfc3339(self.end))],
            "limit": 1000,
        });
        if !self.group_by.is_empty() {
            b["group_by"] = json!(self.group_by);
        }
        b
    }
}

/// One point: its time, value and the group-by labels' values.
#[derive(Debug, Clone, PartialEq)]
pub struct Point {
    /// As Confluent sends it (RFC 3339, UTC).
    pub timestamp: String,
    pub value: f64,
    pub labels: Vec<(String, String)>,
}

impl Point {
    #[must_use]
    pub fn label(&self, name: &str) -> Option<&str> {
        self.labels
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// Runs `q`, every page. `c` is a [`Client`] on [`TELEMETRY_BASE`].
pub async fn query(c: &Client, q: &Query) -> Result<Vec<Point>, Error> {
    q.validate()?;
    let what = format!("query {} for {}", q.metric.name(), q.cluster);
    let body = q.body();
    let mut out = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..1000 {
        let path = match &token {
            Some(t) => format!("/v2/metrics/cloud/query?page_token={}", encode(t)),
            None => "/v2/metrics/cloud/query".to_string(),
        };
        let v = c.call(Method::POST, &path, Some(&body), &what).await?;
        for p in v["data"].as_array().map(Vec::as_slice).unwrap_or_default() {
            let labels = q
                .group_by
                .iter()
                .map(|l| {
                    let val = match &p[l.as_str()] {
                        Value::String(s) => s.clone(),
                        Value::Null => String::new(),
                        other => other.to_string(),
                    };
                    (l.clone(), val)
                })
                .collect();
            out.push(Point {
                timestamp: p["timestamp"].as_str().unwrap_or_default().to_string(),
                value: p["value"].as_f64().unwrap_or(0.0),
                labels,
            });
        }
        match v["meta"]["pagination"]["next_page_token"].as_str() {
            Some(t) if !t.is_empty() => token = Some(t.to_string()),
            _ => return Ok(out),
        }
    }
    Err(Error::Decode {
        what,
        message: "more than 1000 pages".into(),
    })
}

/// The sum of the points' values per value of `label` (all points under ""
/// when the label is absent).
#[must_use]
pub fn sum_by(points: &[Point], label: &str) -> std::collections::BTreeMap<String, f64> {
    let mut out = std::collections::BTreeMap::new();
    for p in points {
        *out.entry(p.label(label).unwrap_or_default().to_string())
            .or_insert(0.0) += p.value;
    }
    out
}

/// Percent-encodes a page token for the query string.
fn encode(s: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// `unix_seconds` as `YYYY-MM-DDTHH:MM:SSZ`.
#[must_use]
pub fn rfc3339(unix_seconds: i64) -> String {
    let day = crate::costs::Day::from_unix(unix_seconds);
    let secs = unix_seconds.rem_euclid(86_400);
    format!(
        "{day}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_has_the_shape_the_api_takes() {
        let q = Query::hourly(
            Metric::RequestBytes,
            "lkc-test01",
            1_800_000_000,
            1_800_003_600,
        )
        .group_by("metric.principal_id");
        let b = q.body();
        assert_eq!(
            b["aggregations"][0]["metric"],
            "io.confluent.kafka.server/request_bytes"
        );
        assert_eq!(b["filter"]["value"], "lkc-test01");
        assert_eq!(b["granularity"], "PT1H");
        assert_eq!(
            b["intervals"][0],
            "2027-01-15T08:00:00Z/2027-01-15T09:00:00Z"
        );
        assert_eq!(b["group_by"][0], "metric.principal_id");
        assert!(q.validate().is_ok());
    }

    #[test]
    fn refuses_what_cc0_found_the_api_refuses() {
        let q = Query::hourly(Metric::ReceivedBytes, "lkc-test01", 0, 3600)
            .group_by("metric.principal_id");
        assert!(
            q.validate().is_err(),
            "received_bytes has no principal label"
        );
        let q = Query::hourly(Metric::SentBytes, "lkc-test01", 0, 8 * 86_400);
        assert!(q.validate().is_err(), "more than 7 days");
        let q = Query::hourly(Metric::SentBytes, "lkc-x;y", 0, 3600);
        assert!(q.validate().is_err(), "bad cluster id");
    }

    #[test]
    fn sums_by_label() {
        let p = |v: f64, sa: &str| Point {
            timestamp: String::new(),
            value: v,
            labels: vec![("metric.principal_id".into(), sa.into())],
        };
        let s = sum_by(
            &[p(1.0, "sa-a"), p(2.0, "sa-b"), p(3.0, "sa-a")],
            "metric.principal_id",
        );
        assert_eq!(s.get("sa-a").copied(), Some(4.0));
        assert_eq!(s.get("sa-b").copied(), Some(2.0));
    }

    #[test]
    fn encodes_page_tokens() {
        assert_eq!(encode("a b/c=="), "a%20b%2Fc%3D%3D");
    }
}
