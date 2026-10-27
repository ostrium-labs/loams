//! The [`ConnectorFactory`] of the Kafka-protocol family: `kafka` itself, and
//! every target profile whose `module` is `kafka` (Redpanda, Azure Event
//! Hubs). A profile's `defaults` are merged under the instance config, and
//! its `narrows` are enforced: a narrowed direction is not offered, and
//! `delivery: exactly_once` is refused when the profile narrows
//! `sink.transactional`.

use std::collections::BTreeSet;
use std::sync::{Mutex, OnceLock};

use loams_connector_sdk::{
    ConnectorContext, ConnectorError, ConnectorFactory, Delivery, Profile, ProfileKind,
    SinkFactory, SourceFactory, SplitEnumerator,
};

use crate::config::{Connection, KafkaConfig, RecordFormat, SinkDelivery};
use crate::sink::KafkaSinkFactory;
use crate::source::{KafkaEnumerator, KafkaSourceFactory};

/// The family's module name in profiles (`module: kafka`).
pub const MODULE: &str = "kafka";

/// The Kafka-protocol connector factory.
#[derive(Debug, Clone, Copy)]
pub struct Kafka {
    id: &'static str,
}

impl Default for Kafka {
    fn default() -> Self {
        Self::new()
    }
}

/// Profile ids as `&'static str`, interned once each (the factory's `id`).
fn intern(id: &str) -> &'static str {
    static IDS: OnceLock<Mutex<BTreeSet<&'static str>>> = OnceLock::new();
    let mut ids = IDS
        .get_or_init(|| Mutex::new(BTreeSet::new()))
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    if let Some(found) = ids.get(id) {
        return found;
    }
    let leaked: &'static str = Box::leak(id.to_owned().into_boxed_str());
    ids.insert(leaked);
    leaked
}

impl Kafka {
    /// The `kafka` manifest's factory.
    pub fn new() -> Self {
        Self { id: "kafka" }
    }

    /// The factory of a target profile on this family (`redpanda`,
    /// `azure-event-hubs`); its id is the profile's, which is the manifest's.
    pub fn for_profile(profile: &Profile) -> Result<Self, ConnectorError> {
        check_profile(profile)?;
        Ok(Self {
            id: intern(&profile.id),
        })
    }

    /// The strongest sink delivery an instance may declare under `profile`:
    /// `exactly_once` through transactions, unless the profile narrows them
    /// (Event Hubs, R0.19), when the idempotent producer gives
    /// `effectively_once`.
    pub fn sink_delivery(profile: Option<&Profile>) -> Delivery {
        if profile.is_some_and(|p| narrows(p, "sink.transactional")) {
            Delivery::EffectivelyOnce
        } else {
            Delivery::ExactlyOnce
        }
    }

    fn connection(
        &self,
        ctx: &ConnectorContext,
    ) -> Result<std::sync::Arc<Connection>, ConnectorError> {
        let config = match &ctx.profile {
            Some(p) => {
                check_profile(p)?;
                p.config_with_defaults(&ctx.config)
            }
            None => ctx.config.clone(),
        };
        Connection::new(self.id, ctx, &config)
    }
}

fn check_profile(p: &Profile) -> Result<(), ConnectorError> {
    if p.kind != ProfileKind::Target || p.module != MODULE {
        return Err(ConnectorError::fatal(format!(
            "profile {} is not a target profile of the {MODULE} family",
            p.id
        )));
    }
    Ok(())
}

fn narrows(p: &Profile, path: &str) -> bool {
    p.narrows.iter().any(|n| n == path)
}

impl ConnectorFactory for Kafka {
    fn id(&self) -> &'static str {
        self.id
    }

    fn source(
        &self,
        ctx: &ConnectorContext,
    ) -> Result<Option<(Box<dyn SplitEnumerator>, Box<dyn SourceFactory>)>, ConnectorError> {
        if ctx.profile.as_ref().is_some_and(|p| narrows(p, "source")) {
            return Ok(None);
        }
        let conn = self.connection(ctx)?;
        Ok(Some((
            Box::new(KafkaEnumerator::new(conn.clone())),
            Box::new(KafkaSourceFactory::new(conn)),
        )))
    }

    fn sink(&self, ctx: &ConnectorContext) -> Result<Option<Box<dyn SinkFactory>>, ConnectorError> {
        let merged = match &ctx.profile {
            Some(p) => p.config_with_defaults(&ctx.config),
            None => ctx.config.clone(),
        };
        if KafkaConfig::parse(&merged)?.format != RecordFormat::Plain {
            return Err(ConnectorError::fatal(
                "kafka config: format applies to the source; a sink writes CloudEvents (D270)",
            ));
        }
        if let Some(p) = &ctx.profile {
            if narrows(p, "sink") {
                return Ok(None);
            }
            let cfg = KafkaConfig::parse(&merged)?;
            if cfg.delivery == SinkDelivery::ExactlyOnce && narrows(p, "sink.transactional") {
                return Err(ConnectorError::fatal(format!(
                    "{} narrows sink.transactional: its sink is effectively_once (the idempotent producer), so delivery: exactly_once is refused",
                    p.id
                )));
            }
        }
        let conn = self.connection(ctx)?;
        Ok(Some(Box::new(KafkaSinkFactory::new(conn)?)))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use loams_connector_sdk::{InstanceKey, Scrubber, StaticSecrets};
    use serde_json::{Value, json};

    use super::*;
    use crate::config::SecurityProtocol;

    fn profile(id: &str) -> Profile {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../../connectors/profiles/{id}.yaml"));
        let text = std::fs::read_to_string(&path).expect("profile file");
        serde_norway::from_str(&text).expect("profile parses")
    }

    /// The Confluent Cloud instance shape lane B's secrets back (D1827).
    fn confluent_instance() -> Value {
        json!({
            "brokers": "SASL_SSL://127.0.0.1:9092",
            "topics": ["orders"],
            "group_id_prefix": "loams-main-",
            "sasl": {
                "username_secret": "confluent-main/username",
                "password": "confluent-main/password",
            },
        })
    }

    fn ctx(config: Value, profile: &Profile) -> (ConnectorContext, Arc<Scrubber>) {
        let scrubber = Scrubber::new();
        let secrets = StaticSecrets::new()
            .with("confluent-main/username", "TESTKEYID0000001")
            .with("confluent-main/password", "test-only-api-secret");
        let mut ctx = ConnectorContext::new(
            InstanceKey::new(0, "orders-in"),
            config,
            Arc::new(secrets),
            scrubber.clone(),
        );
        ctx.profile = Some(profile.clone());
        (ctx, scrubber)
    }

    #[tokio::test]
    async fn confluent_profile_defaults_apply() {
        let p = profile("confluent-cloud");
        let factory = Kafka::for_profile(&p).expect("a kafka target profile");
        assert_eq!(factory.id(), "confluent-cloud");
        assert!(p.narrows.is_empty(), "CC0 R0.1: nothing is narrowed");
        assert_eq!(Kafka::sink_delivery(Some(&p)), Delivery::ExactlyOnce);

        let cfg = KafkaConfig::parse(&p.config_with_defaults(&confluent_instance()))
            .expect("the merged config parses");
        assert_eq!(cfg.security_protocol, SecurityProtocol::SaslSsl);
        let sasl = cfg.sasl.as_ref().expect("sasl");
        assert_eq!(sasl.mechanism, "plain");
        let tls = cfg.tls.as_ref().expect("tls");
        assert!(tls.enabled && tls.verify_full);
        assert!(
            tls.ca_pem.is_none(),
            "no CA shipped: the system roots (R0.3)"
        );
        assert_eq!(cfg.max_record_bytes, 8_000_000);
        assert!(!cfg.create_checkpoint_topic);
        assert_eq!(cfg.brokers, "127.0.0.1:9092", "the scheme is stripped");

        // What the user sets wins over a default.
        let mut own = confluent_instance();
        own["max_record_bytes"] = json!(1_000_000);
        let cfg = KafkaConfig::parse(&p.config_with_defaults(&own)).expect("parse");
        assert_eq!(cfg.max_record_bytes, 1_000_000);

        // The client librdkafka gets: SASL_SSL, PLAIN, the key id and secret
        // resolved from the store (and recorded for scrubbing), probed roots,
        // hostname verification.
        let (ctx, scrubber) = ctx(confluent_instance(), &p);
        let conn = factory.connection(&ctx).expect("connection");
        let client = conn.base().await.expect("client config");
        assert_eq!(client.get("bootstrap.servers"), Some("127.0.0.1:9092"));
        assert_eq!(client.get("security.protocol"), Some("sasl_ssl"));
        assert_eq!(client.get("sasl.mechanism"), Some("PLAIN"));
        assert_eq!(client.get("sasl.username"), Some("TESTKEYID0000001"));
        assert_eq!(client.get("sasl.password"), Some("test-only-api-secret"));
        assert_eq!(client.get("ssl.ca.location"), Some("probe"));
        assert_eq!(
            client.get("ssl.endpoint.identification.algorithm"),
            Some("https")
        );
        let scrubbed = scrubber.scrub("auth as TESTKEYID0000001 with test-only-api-secret");
        assert!(
            !scrubbed.contains("TESTKEYID0000001") && !scrubbed.contains("test-only-api-secret"),
            "{scrubbed}"
        );
    }

    #[test]
    fn confluent_profile_requires_sasl_secret() {
        let p = profile("confluent-cloud");
        let factory = Kafka::for_profile(&p).expect("profile");
        let refused = |config: Value| {
            let (ctx, _) = ctx(config.clone(), &p);
            let err = factory
                .connection(&ctx)
                .expect_err("the instance must be refused");
            // A refusal names fields, never a secret's value.
            assert!(!err.to_string().contains("test-only-api-secret"));
            err
        };
        let mut no_sasl = confluent_instance();
        no_sasl.as_object_mut().expect("object").remove("sasl");
        refused(no_sasl);
        refused(json!({
            "brokers": "a:9092", "topics": ["t"],
            "sasl": {"username_secret": "confluent-main/username"},
        }));
        let err = refused(json!({
            "brokers": "a:9092", "topics": ["t"],
            "sasl": {"password": "confluent-main/password"},
        }));
        assert!(err.to_string().contains("username"), "{err}");
        refused(json!({
            "brokers": "a:9092", "topics": ["t"],
            "sasl": {
                "username": "KEY", "username_secret": "confluent-main/username",
                "password": "confluent-main/password",
            },
        }));
        // A literal key id is allowed beside the secret's name for the secret.
        let (ctx, _) = ctx(
            json!({
                "brokers": "a:9092", "topics": ["t"],
                "sasl": {"username": "KEY", "password": "confluent-main/password"},
            }),
            &p,
        );
        assert!(factory.connection(&ctx).is_ok());
    }
}
