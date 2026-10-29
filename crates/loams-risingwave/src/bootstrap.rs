//! The service users of a new cluster. RisingWave starts with two
//! superusers, `root` and `postgres`, without passwords, and its `password`
//! auth method lets a user without one in. Before the cluster is handed out
//! the controller logs in as `root` (inside the instance's network only) and:
//!
//! - sets `root`'s password (the controller's own user from then on),
//! - sets a random, discarded password on `postgres`,
//! - creates or updates the gateway's user, which loams-gateway logs in as
//!   on behalf of authenticated Loams principals (D1957).

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio_postgres::SimpleQueryMessage;

use crate::backend::{PASSWORD_LEN, Secret};
use crate::probe::connect;
use crate::provision::{quote_ident, quote_literal};

/// The user loams-gateway logs in as.
pub const GATEWAY_USER: &str = "loams_gateway";

/// The credentials the controller sets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceUsers {
    pub root_password: Secret,
    pub gateway_user: String,
    pub gateway_password: Secret,
}

impl ServiceUsers {
    pub fn generate() -> Self {
        Self {
            root_password: Secret::generate(PASSWORD_LEN),
            gateway_user: GATEWAY_USER.to_owned(),
            gateway_password: Secret::generate(PASSWORD_LEN),
        }
    }
}

/// The statements, given which users exist already.
pub fn bootstrap_sql(users: &ServiceUsers, existing: &[String]) -> Vec<String> {
    let mut sql = vec![format!(
        "ALTER USER root WITH PASSWORD {}",
        quote_literal(users.root_password.expose())
    )];
    if existing.iter().any(|u| u == "postgres") {
        sql.push(format!(
            "ALTER USER postgres WITH PASSWORD {}",
            quote_literal(Secret::generate(PASSWORD_LEN).expose())
        ));
    }
    let gw = quote_ident(&users.gateway_user);
    let pw = quote_literal(users.gateway_password.expose());
    if existing.contains(&users.gateway_user) {
        sql.push(format!("ALTER USER {gw} WITH PASSWORD {pw}"));
    } else {
        sql.push(format!(
            "CREATE USER {gw} WITH SUPERUSER CREATEDB LOGIN PASSWORD {pw}"
        ));
    }
    sql
}

/// Logs in as `root` (with its password, or without one on a fresh
/// cluster) and applies [`bootstrap_sql`]. Idempotent.
pub async fn ensure_service_users(
    host: &str,
    port: u16,
    users: &ServiceUsers,
    timeout: Duration,
) -> Result<(), String> {
    let client = match connect(host, port, "root", Some(&users.root_password), timeout).await {
        Ok(c) => c,
        Err(with_password) => connect(host, port, "root", None, timeout)
            .await
            .map_err(|e| {
                format!("root login failed with the password ({with_password}) and without ({e})")
            })?,
    };
    let rows = client
        .simple_query("SELECT usename FROM pg_catalog.pg_user")
        .await
        .map_err(|e| e.to_string())?;
    let existing: Vec<String> = rows
        .iter()
        .filter_map(|m| match m {
            SimpleQueryMessage::Row(r) => r.get(0).map(str::to_owned),
            _ => None,
        })
        .collect();
    for stmt in bootstrap_sql(users, &existing) {
        client
            .simple_query(&stmt)
            .await
            .map_err(|e| format!("{}: {e}", stmt.split(" PASSWORD").next().unwrap_or("")))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_cluster() {
        let users = ServiceUsers {
            root_password: Secret::new("r00t"),
            gateway_user: GATEWAY_USER.into(),
            gateway_password: Secret::new("gw"),
        };
        let sql = bootstrap_sql(&users, &["root".into(), "postgres".into()]);
        assert_eq!(sql[0], "ALTER USER root WITH PASSWORD 'r00t'");
        assert!(sql[1].starts_with("ALTER USER postgres WITH PASSWORD '"));
        assert_eq!(
            sql[2],
            "CREATE USER \"loams_gateway\" WITH SUPERUSER CREATEDB LOGIN PASSWORD 'gw'"
        );
    }

    #[test]
    fn rerun_rotates() {
        let users = ServiceUsers::generate();
        let sql = bootstrap_sql(&users, &["root".into(), GATEWAY_USER.into()]);
        assert_eq!(sql.len(), 2);
        assert!(sql[1].starts_with("ALTER USER \"loams_gateway\" WITH PASSWORD '"));
        assert!(users.root_password.is_url_safe());
    }
}
