//! SigNoz has no panels: its health is its unauthenticated `/api/v2/healthz` at
//! the configured URL's origin, whatever path the UI is on (D1666).

use url::Url;

use super::Connection;
use crate::error::OpError;
use crate::upstream::UpstreamRequest;

pub(super) async fn probe(conn: &Connection) -> Result<(), OpError> {
    let url = Url::parse(&conn.url)
        .and_then(|u| u.join("/api/v2/healthz"))
        .map_err(|e| OpError::other(e.to_string()))?;
    let response = conn
        .upstream
        .send(UpstreamRequest {
            method: "GET",
            url: url.to_string(),
            headers: Vec::new(),
        })
        .await?;
    if (200..300).contains(&response.status) {
        Ok(())
    } else {
        Err(OpError::http(
            response.status,
            format!("health check answered {}", response.status),
        ))
    }
}
