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
    let spec = &v["spec"];
    let s = |x: &serde_json::Value| x.as_str().unwrap_or_default().to_string();
    let cluster = Cluster {
        id: s(&v["id"]),
        display_name: s(&spec["display_name"]),
        cloud: s(&spec["cloud"]).to_ascii_lowercase(),
        region: s(&spec["region"]),
        kind: s(&spec["config"]["kind"]),
        bootstrap: s(&spec["kafka_bootstrap_endpoint"])
            .trim_start_matches("SASL_SSL://")
            .to_string(),
    };
    if cluster.id.is_empty() || cluster.region.is_empty() {
        return Err(Error::Decode {
            what,
            message: "no id or region in the response".into(),
        });
    }
    Ok(cluster)
}
