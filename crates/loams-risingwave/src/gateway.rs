//! The loams-gateway route of an instance (D1957, adopted as ENG81 R8):
//! clients connect to `<name>.<ns>.<region>.rw.<base-domain>` on the
//! gateway's PG port with TLS and a Loams API key or token as the
//! password; the gateway checks it for the instance's namespace
//! (`can_connect`, R18), logs in to the frontend as
//! [`crate::bootstrap::GATEWAY_USER`] and splices. RisingWave itself is
//! never exposed. In compose, `engine-gateway` serves the first instance on
//! `127.0.0.1:4566` (R7).
//!
//! This module renders the `[[risingwave.endpoints]]` entries of the
//! gateway's routes file (parsed by `loams-gateway`'s `risingwave` module).
//! The gateway reads the upstream password from a file, so the routes file
//! carries no credential.

use serde::{Deserialize, Serialize};

use crate::bootstrap::GATEWAY_USER;
use crate::model::InstanceRef;

/// How gateway hostnames and credential files are named.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayNaming {
    /// `eu1`.
    pub region: String,
    /// `loams.dev`, or a self-hosted stack's own domain.
    pub base_domain: String,
    /// The gateway's PG port clients use (5432; 4566 on compose's
    /// loopback `engine-gateway`).
    #[serde(default = "default_port")]
    pub port: u16,
    /// Where the gateway finds `<key>/password` (a mounted Secret).
    #[serde(default = "default_secrets_dir")]
    pub secrets_dir: String,
}

fn default_port() -> u16 {
    5432
}

fn default_secrets_dir() -> String {
    "/var/run/loams/risingwave".to_owned()
}

impl GatewayNaming {
    /// `<name>.<ns>.<region>.rw.<base-domain>`.
    pub fn hostname(&self, inst: &InstanceRef) -> String {
        format!(
            "{}.{}.{}.rw.{}",
            inst.name, inst.namespace, self.region, self.base_domain
        )
        .to_ascii_lowercase()
    }

    pub fn password_file(&self, inst: &InstanceRef) -> String {
        format!(
            "{}/{}/password",
            self.secrets_dir.trim_end_matches('/'),
            inst.key
        )
    }
}

/// One `[[risingwave.endpoints]]` entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RwEndpoint {
    /// The instance id, `eng_<26>`.
    pub name: String,
    /// The Loams namespace whose principals may connect.
    pub namespace: String,
    /// SNI names, lower-cased; the first is the canonical one.
    pub hostnames: Vec<String>,
    /// Frontend addresses, `host:port`, tried in order.
    pub upstreams: Vec<String>,
    pub upstream_user: String,
    pub upstream_password_file: String,
    /// A loopback port that routes to this instance without SNI (compose's
    /// `engine-gateway`, one instance per port).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

/// The route of an instance whose frontend Service is `frontend`.
pub fn endpoint(inst: &InstanceRef, naming: &GatewayNaming, frontend: &str) -> RwEndpoint {
    RwEndpoint {
        name: inst.id.clone(),
        namespace: inst.namespace.clone(),
        hostnames: vec![naming.hostname(inst)],
        upstreams: vec![frontend.to_owned()],
        upstream_user: GATEWAY_USER.to_owned(),
        upstream_password_file: naming.password_file(inst),
        port: None,
    }
}

#[derive(Serialize)]
struct Section<'a> {
    risingwave: Inner<'a>,
}

#[derive(Serialize)]
struct Inner<'a> {
    endpoints: &'a [RwEndpoint],
}

/// The `[risingwave]` section of a routes file, sorted by name so the
/// file's generation only changes when a route does.
pub fn render_section(endpoints: &[RwEndpoint]) -> Result<String, toml::ser::Error> {
    let mut sorted = endpoints.to_vec();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    toml::to_string(&Section {
        risingwave: Inner { endpoints: &sorted },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "eng_aaaaaaaaaaaaaaaaaaaaaaaaaa";
    const Z: &str = "eng_zzzzzzzzzzzzzzzzzzzzzzzzzz";

    fn naming() -> GatewayNaming {
        GatewayNaming {
            region: "eu1".into(),
            base_domain: "loams.dev".into(),
            port: 5432,
            secrets_dir: "/var/run/loams/risingwave/".into(),
        }
    }

    fn inst(id: &str, name: &str) -> InstanceRef {
        InstanceRef::new(id, name, "dev", &format!("engines/dev/{id}/")).unwrap()
    }

    #[test]
    fn hostnames() {
        let i = inst(A, "Clicks");
        assert_eq!(naming().hostname(&i), "clicks.dev.eu1.rw.loams.dev");
        assert_eq!(
            naming().password_file(&i),
            "/var/run/loams/risingwave/aaaaaaaaaaaaaaaaaaaaaaaaaa/password"
        );
    }

    #[test]
    fn section_round_trips() {
        let z = endpoint(
            &inst(Z, "zed"),
            &naming(),
            "rw-zzzzzzzzzzzzzzzzzzzzzzzzzz-frontend.eng-zzzzzzzzzzzzzzzzzzzzzzzzzz.svc.cluster.local:4566",
        );
        let mut a = endpoint(
            &inst(A, "acme"),
            &naming(),
            "rw-aaaaaaaaaaaaaaaaaaaaaaaaaa-frontend-0:4566",
        );
        a.port = Some(4566);
        let text = render_section(&[z.clone(), a.clone()]).unwrap();
        assert!(text.contains("[[risingwave.endpoints]]"));
        assert!(
            !text.to_lowercase().contains("password ="),
            "no credential in the file"
        );

        #[derive(Deserialize)]
        struct File {
            risingwave: Back,
        }
        #[derive(Deserialize)]
        struct Back {
            endpoints: Vec<RwEndpoint>,
        }
        let back: File = toml::from_str(&text).unwrap();
        assert_eq!(back.risingwave.endpoints, vec![a, z]);
    }
}
