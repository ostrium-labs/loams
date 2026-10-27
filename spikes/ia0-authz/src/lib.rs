//! IA0 spike: Zitadel JWT -> OpenFGA `Check` (design 76, plan IA1 IA0 Tasks 3-5).
//!
//! Not production code. IA2 builds `crates/loams-authz` from what this proves.

pub mod config;
pub mod fga;
pub mod jwt;
pub mod server;
pub mod zitadel;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::Mutex;

use crate::config::Config;
use crate::server::{Inner, Kind, Principal};

/// Builds the service state from `.env.ia0` and the environment.
pub async fn state(cfg: &Config) -> Result<server::AppState> {
    let verifier = jwt::Verifier::new(
        cfg.req("IA0_ISSUER")?,
        cfg.req("IA0_AUDIENCE")?,
        cfg.get("IA0_ZITADEL_INTERNAL"),
    );
    let fga = fga::Fga::connect(
        cfg.req("IA0_FGA_GRPC")?,
        cfg.req("IA0_STORE")?,
        cfg.req("IA0_MODEL")?,
        server::cache_ttl(cfg.or("IA0_CACHE_TTL_MS", "5000")),
    )
    .await?;
    let mut principals = HashMap::new();
    for id in cfg.or("IA0_AGENT", "").split(',').filter(|s| !s.is_empty()) {
        principals.insert(
            id.to_owned(),
            Principal {
                kind: Kind::Agent,
                disabled: false,
            },
        );
    }
    Ok(Arc::new(Inner {
        verifier,
        fga,
        admin_token: cfg.or("IA0_ADMIN_TOKEN", "").to_owned(),
        event_key_file: cfg.get("IA0_EVENT_KEY_FILE").map(Into::into),
        principals: Mutex::new(principals),
        events: Mutex::new(Vec::new()),
        applied_seq: Mutex::new(HashMap::new()),
        fail_next: Mutex::new(0),
    }))
}

/// Percentiles over microsecond samples.
pub fn percentiles(samples: &mut [u128]) -> (f64, f64, f64, f64) {
    samples.sort_unstable();
    let at = |q: f64| {
        let i = ((samples.len() as f64 - 1.0) * q).round() as usize;
        samples[i] as f64 / 1000.0
    };
    let mean = samples.iter().sum::<u128>() as f64 / samples.len() as f64 / 1000.0;
    (at(0.50), at(0.95), at(0.99), mean)
}
