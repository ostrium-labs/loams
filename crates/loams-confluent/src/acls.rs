//! Kafka ACLs through the cluster's REST endpoint (Kafka v3 Admin API,
//! `GET /kafka/v3/clusters/{id}/acls`), and the sanity check `loams byoc
//! confluent check` runs on Loams' data principal (design 80 §7.1, D1826).
//!
//! Build a [`Client`] on the cluster's REST endpoint ([`crate::clusters::Cluster::rest`])
//! with a Kafka API key that may describe the cluster's ACLs. The data key
//! usually may not: then the check is skipped, not failed.

use reqwest::Method;
use serde_json::Value;

use crate::{Client, Error};

/// One ACL binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Acl {
    /// `TOPIC`, `GROUP`, `TRANSACTIONAL_ID`, `CLUSTER`.
    pub resource_type: String,
    pub resource_name: String,
    /// `LITERAL` or `PREFIXED`.
    pub pattern_type: String,
    pub principal: String,
    pub host: String,
    /// `READ`, `WRITE`, `DESCRIBE`, …, `ALL`.
    pub operation: String,
    /// `ALLOW` or `DENY`.
    pub permission: String,
}

/// The ACLs that name `principal` (`User:sa-…`) on cluster `cluster`.
pub async fn list(c: &Client, cluster: &str, principal: &str) -> Result<Vec<Acl>, Error> {
    let what = format!("describe the ACLs of {principal} on {cluster}");
    let path = format!(
        "/kafka/v3/clusters/{cluster}/acls?principal={}",
        principal.replace(':', "%3A")
    );
    let v = c.call(Method::GET, &path, None, &what).await?;
    let s = |x: &Value| x.as_str().unwrap_or_default().to_string();
    Ok(v["data"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|a| Acl {
            resource_type: s(&a["resource_type"]),
            resource_name: s(&a["resource_name"]),
            pattern_type: s(&a["pattern_type"]),
            principal: s(&a["principal"]),
            host: s(&a["host"]),
            operation: s(&a["operation"]),
            permission: s(&a["permission"]),
        })
        .collect())
}

/// How bad a finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Wider than D1826 allows: Loams' key could change the cluster.
    Fail,
    /// Narrower than Loams needs, or worth a look.
    Warn,
}

/// One finding of [`audit`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub level: Level,
    pub message: String,
}

/// The operations D1826 grants the data principal.
const ALLOWED_OPS: &[&str] = &[
    "READ",
    "WRITE",
    "DESCRIBE",
    "DESCRIBE_CONFIGS",
    "IDEMPOTENT_WRITE",
];

fn describe(a: &Acl) -> String {
    format!(
        "{} {} {}:{} ({})",
        a.permission, a.operation, a.resource_type, a.resource_name, a.pattern_type
    )
}

/// Checks the data principal's ALLOW bindings against the module's set
/// (D1826): topic READ/WRITE/DESCRIBE/DESCRIBE_CONFIGS on named topics,
/// group READ/DESCRIBE and transactional id WRITE/DESCRIBE on Loams'
/// prefixes, cluster IDEMPOTENT_WRITE. Anything that could create, delete or
/// reconfigure is a failure; a wildcard or an empty prefix is a failure; a
/// missing piece Loams needs is a warning.
#[must_use]
pub fn audit(acls: &[Acl]) -> Vec<Finding> {
    let mut out = Vec::new();
    let allow: Vec<&Acl> = acls
        .iter()
        .filter(|a| a.permission.eq_ignore_ascii_case("ALLOW"))
        .collect();
    for a in &allow {
        let op = a.operation.to_ascii_uppercase();
        if !ALLOWED_OPS.contains(&op.as_str()) {
            out.push(Finding {
                level: Level::Fail,
                message: format!(
                    "{}: Loams' key may {op}, which D1826 never grants",
                    describe(a)
                ),
            });
            continue;
        }
        let wildcard = a.resource_name == "*"
            || (a.pattern_type.eq_ignore_ascii_case("PREFIXED") && a.resource_name.is_empty());
        if wildcard && !a.resource_type.eq_ignore_ascii_case("CLUSTER") {
            out.push(Finding {
                level: Level::Fail,
                message: format!(
                    "{}: every {} on the cluster, not the install's own",
                    describe(a),
                    a.resource_type.to_ascii_lowercase()
                ),
            });
        }
        if a.resource_type.eq_ignore_ascii_case("CLUSTER") && op != "IDEMPOTENT_WRITE" {
            out.push(Finding {
                level: Level::Fail,
                message: format!("{}: only IDEMPOTENT_WRITE on the cluster", describe(a)),
            });
        }
    }
    let has = |ty: &str, op: &str| {
        allow.iter().any(|a| {
            a.resource_type.eq_ignore_ascii_case(ty) && a.operation.eq_ignore_ascii_case(op)
        })
    };
    for (ty, op, why) in [
        ("TOPIC", "READ", "no topic to read: sources will fail"),
        ("TOPIC", "WRITE", "no topic to write: sinks will fail"),
        (
            "GROUP",
            "READ",
            "no consumer group grant: sources cannot commit offsets",
        ),
        (
            "CLUSTER",
            "IDEMPOTENT_WRITE",
            "no IDEMPOTENT_WRITE: idempotent and exactly-once sinks will fail",
        ),
    ] {
        if !has(ty, op) {
            out.push(Finding {
                level: Level::Warn,
                message: why.to_string(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acl(ty: &str, name: &str, pattern: &str, op: &str) -> Acl {
        Acl {
            resource_type: ty.into(),
            resource_name: name.into(),
            pattern_type: pattern.into(),
            principal: "User:sa-test01".into(),
            host: "*".into(),
            operation: op.into(),
            permission: "ALLOW".into(),
        }
    }

    fn module_set() -> Vec<Acl> {
        let mut v = Vec::new();
        for op in ["READ", "WRITE", "DESCRIBE", "DESCRIBE_CONFIGS"] {
            v.push(acl("TOPIC", "orders", "LITERAL", op));
        }
        for op in ["READ", "DESCRIBE"] {
            v.push(acl("GROUP", "loams-prod-", "PREFIXED", op));
        }
        for op in ["WRITE", "DESCRIBE"] {
            v.push(acl("TRANSACTIONAL_ID", "loams-prod-", "PREFIXED", op));
        }
        v.push(acl(
            "CLUSTER",
            "kafka-cluster",
            "LITERAL",
            "IDEMPOTENT_WRITE",
        ));
        v
    }

    #[test]
    fn the_module_set_is_clean() {
        assert_eq!(audit(&module_set()), Vec::new());
    }

    #[test]
    fn wider_grants_fail() {
        for extra in [
            acl("TOPIC", "orders", "LITERAL", "ALL"),
            acl("TOPIC", "orders", "LITERAL", "CREATE"),
            acl("TOPIC", "orders", "LITERAL", "ALTER_CONFIGS"),
            acl("TOPIC", "*", "LITERAL", "READ"),
            acl("TOPIC", "", "PREFIXED", "WRITE"),
            acl("CLUSTER", "kafka-cluster", "LITERAL", "DESCRIBE"),
            acl("CLUSTER", "kafka-cluster", "LITERAL", "ALTER"),
        ] {
            let mut v = module_set();
            v.push(extra.clone());
            let f = audit(&v);
            assert!(
                f.iter().any(|x| x.level == Level::Fail),
                "{extra:?} passed: {f:?}"
            );
        }
    }

    #[test]
    fn deny_bindings_are_not_grants() {
        let mut v = module_set();
        let mut d = acl("TOPIC", "*", "LITERAL", "ALL");
        d.permission = "DENY".into();
        v.push(d);
        assert_eq!(audit(&v), Vec::new());
    }

    #[test]
    fn missing_pieces_warn() {
        let v: Vec<Acl> = module_set()
            .into_iter()
            .filter(|a| a.resource_type != "CLUSTER" && a.operation != "WRITE")
            .collect();
        let f = audit(&v);
        assert!(f.iter().all(|x| x.level == Level::Warn), "{f:?}");
        assert!(f.iter().any(|x| x.message.contains("IDEMPOTENT_WRITE")));
        assert!(f.iter().any(|x| x.message.contains("sinks")));
    }
}
