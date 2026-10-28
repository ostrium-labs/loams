//! Kafka clusters (`/cmk/v2/clusters`).

use reqwest::Method;

use crate::{Client, Error};

/// What Loams needs to know about a cluster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cluster {
    pub id: String,
    pub display_name: String,
    /// `aws`, `gcp` or `azure`.
    pub cloud: String,
    /// The cloud's own region code (`us-east-1`).
    pub region: String,
    /// `Basic`, `Standard`, `Enterprise`, `Freight` or `Dedicated`.
    pub kind: String,
    /// `host:port`, without the `SASL_SSL://` some answers carry (CC0 R0.12).
    pub bootstrap: String,
    /// The Kafka REST endpoint (`https://pkc-….confluent.cloud:443`), for
    /// the v3 Admin API (ACLs). Empty when the answer has none.
    pub rest: String,
}

/// Reads cluster `id` in `environment`.
pub async fn get(c: &Client, environment: &str, id: &str) -> Result<Cluster, Error> {
    let what = format!("read cluster {id} in {environment}");
    let v = c
        .call(
            Method::GET,
            &format!("/cmk/v2/clusters/{id}?environment={environment}"),
            None,
            &what,
        )
        .await?;
    let cluster = parse(&v);
    if cluster.id.is_empty() || cluster.region.is_empty() {
        return Err(Error::Decode {
            what,
            message: "no id or region in the response".into(),
        });
    }
    Ok(cluster)
}

fn parse(v: &serde_json::Value) -> Cluster {
    let spec = &v["spec"];
    let s = |x: &serde_json::Value| x.as_str().unwrap_or_default().to_string();
    Cluster {
        id: s(&v["id"]),
        display_name: s(&spec["display_name"]),
        cloud: s(&spec["cloud"]).to_ascii_lowercase(),
        region: s(&spec["region"]),
        kind: s(&spec["config"]["kind"]),
        bootstrap: s(&spec["kafka_bootstrap_endpoint"])
            .trim_start_matches("SASL_SSL://")
            .to_string(),
        rest: s(&spec["http_endpoint"]).trim_end_matches('/').to_string(),
    }
}

/// Every Kafka cluster in `environment`, every page.
pub async fn list(c: &Client, environment: &str) -> Result<Vec<Cluster>, Error> {
    let what = format!("list the clusters in {environment}");
    let mut path = format!("/cmk/v2/clusters?environment={environment}&page_size=100");
    let mut out = Vec::new();
    for _ in 0..1000 {
        let v = c.call(Method::GET, &path, None, &what).await?;
        for item in v["data"].as_array().map(Vec::as_slice).unwrap_or_default() {
            out.push(parse(item));
        }
        match v["metadata"]["next"].as_str() {
            Some(next) if !next.is_empty() => path = next.to_string(),
            _ => return Ok(out),
        }
    }
    Err(Error::Decode {
        what,
        message: "more than 1000 pages".into(),
    })
}

/// Sets an elastic cluster's `max_ecku` (the budget guard's throttle, design
/// 80 §8; a CloudClusterAdmin key on that one cluster may, CC0 §7). The
/// cluster's `kind` is read first, since the PATCH names it.
pub async fn set_max_ecku(c: &Client, environment: &str, id: &str, n: u32) -> Result<(), Error> {
    if n == 0 {
        return Err(Error::Usage("max_ecku is at least 1".into()));
    }
    let cluster = get(c, environment, id).await?;
    if cluster.kind.is_empty() {
        return Err(Error::Decode {
            what: format!("read cluster {id}"),
            message: "no config.kind in the response".into(),
        });
    }
    let body = serde_json::json!({
        "spec": {
            "environment": { "id": environment },
            "config": { "kind": cluster.kind, "max_ecku": n },
        }
    });
    let what = format!("set max_ecku of {id} to {n}");
    c.call(
        Method::PATCH,
        &format!("/cmk/v2/clusters/{id}"),
        Some(&body),
        &what,
    )
    .await?;
    Ok(())
}
