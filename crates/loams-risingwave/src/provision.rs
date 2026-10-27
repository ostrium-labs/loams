//! The provisioners a cluster's stores come from (D1951, D1952).
//!
//! - [`MetaDbProvisioner`]: the instance's meta database and owner role on
//!   Loams Postgres. [`PostgresMetaDb`] creates them over SQL as an admin
//!   role (the compose stack's `cloud_admin` on `compute1`, or a
//!   `pg-control` endpoint's admin role); `pg-control`'s role and database
//!   RPCs are the production path once ENG-UI wires them.
//! - [`ObjectStoreProvisioner`]: the instance's prefix and credentials.
//!   [`SharedObjectStore`] hands every instance the same access key (the
//!   single-tenant compose stack, ENG81 R3: the prefix keeps instances
//!   apart, the key does not); a prefix-scoped key per instance (R5) needs
//!   RustFS's or the cloud's IAM API and comes with the k8s deploy (RW1).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Mutex;
use tokio_postgres::NoTls;

use crate::backend::{MetaDb, ObjectStore, PASSWORD_LEN, Secret};
use crate::model::InstanceKey;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProvisionError {
    #[error("postgres: {0}")]
    Postgres(String),
    #[error("object storage: {0}")]
    ObjectStore(String),
    #[error("not supported by this provisioner: {0}")]
    Unsupported(&'static str),
}

/// Creates and drops a instance's meta database.
#[async_trait]
pub trait MetaDbProvisioner: Send + Sync {
    /// Creates the database and its owner role if missing, sets a fresh
    /// password, and says how to reach it. Idempotent.
    async fn ensure(&self, key: &InstanceKey) -> Result<MetaDb, ProvisionError>;

    /// Drops the database and the role. Idempotent.
    async fn drop_db(&self, key: &InstanceKey) -> Result<(), ProvisionError>;
}

/// Grants, revokes and empties a instance's object-storage prefix.
#[async_trait]
pub trait ObjectStoreProvisioner: Send + Sync {
    /// The instance's prefix and credentials that reach it. Idempotent.
    /// `prefix` is the instance's state prefix (`engines/<ns>/<id>`).
    async fn ensure(&self, key: &InstanceKey, prefix: &str) -> Result<ObjectStore, ProvisionError>;

    /// Revokes the instance's credentials. Idempotent.
    async fn revoke(&self, key: &InstanceKey) -> Result<(), ProvisionError>;

    /// Deletes every object under the instance's prefix. Idempotent.
    async fn purge(&self, key: &InstanceKey, prefix: &str) -> Result<(), ProvisionError>;
}

/// The statements that create a instance's meta role and database, in order.
/// `CREATE DATABASE` cannot run in a transaction or a `DO` block, so the
/// caller checks `pg_database` first ([`PostgresMetaDb`] does).
pub fn create_role_sql(name: &str, password: &Secret) -> String {
    format!(
        "DO $$ BEGIN \
         IF NOT EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = {lit}) THEN \
         CREATE ROLE {ident} LOGIN PASSWORD {pw}; \
         ELSE ALTER ROLE {ident} LOGIN PASSWORD {pw}; END IF; END $$",
        lit = quote_literal(name),
        ident = quote_ident(name),
        pw = quote_literal(password.expose()),
    )
}

/// `CREATE DATABASE` for the role of the same name.
pub fn create_database_sql(name: &str) -> String {
    let ident = quote_ident(name);
    format!("CREATE DATABASE {ident} OWNER {ident}")
}

/// Drops the database (closing its sessions) and then the role.
pub fn drop_sql(name: &str) -> [String; 2] {
    let ident = quote_ident(name);
    [
        format!("DROP DATABASE IF EXISTS {ident} WITH (FORCE)"),
        format!("DROP ROLE IF EXISTS {ident}"),
    ]
}

/// `"name"` with inner quotes doubled.
pub fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// `'value'` with inner quotes doubled (standard_conforming_strings on).
pub fn quote_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// How [`PostgresMetaDb`] reaches Loams Postgres.
#[derive(Debug, Clone)]
pub struct PostgresAdmin {
    /// The host and port the meta nodes use too (the direct endpoint).
    pub host: String,
    pub port: u16,
    pub admin_user: String,
    pub admin_password: Secret,
    /// The database the admin connects to (`postgres`).
    pub admin_database: String,
    /// `RW_SQL_URL_PARAMS` for the meta nodes, such as `sslmode=require`.
    pub url_params: Option<String>,
    pub connect_timeout: Duration,
}

/// Creates meta databases over SQL as an admin role.
#[derive(Debug, Clone)]
pub struct PostgresMetaDb {
    admin: PostgresAdmin,
}

impl PostgresMetaDb {
    pub fn new(admin: PostgresAdmin) -> Self {
        Self { admin }
    }

    async fn connect(&self) -> Result<tokio_postgres::Client, ProvisionError> {
        let a = &self.admin;
        let mut config = tokio_postgres::Config::new();
        config
            .host(&a.host)
            .port(a.port)
            .user(&a.admin_user)
            .password(a.admin_password.expose())
            .dbname(&a.admin_database)
            .application_name("loams-risingwave")
            .connect_timeout(a.connect_timeout);
        let (client, connection) = config.connect(NoTls).await.map_err(pg)?;
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                tracing::debug!(error = %e, "meta db admin connection closed");
            }
        });
        Ok(client)
    }
}

fn pg(e: tokio_postgres::Error) -> ProvisionError {
    ProvisionError::Postgres(e.to_string())
}

#[async_trait]
impl MetaDbProvisioner for PostgresMetaDb {
    async fn ensure(&self, key: &InstanceKey) -> Result<MetaDb, ProvisionError> {
        let name = MetaDb::name_for(key);
        let password = Secret::generate(PASSWORD_LEN);
        let client = self.connect().await?;
        client
            .batch_execute(&create_role_sql(&name, &password))
            .await
            .map_err(pg)?;
        let exists = client
            .query_opt(
                "SELECT 1 FROM pg_catalog.pg_database WHERE datname = $1",
                &[&name],
            )
            .await
            .map_err(pg)?
            .is_some();
        if !exists {
            client
                .batch_execute(&create_database_sql(&name))
                .await
                .map_err(pg)?;
        }
        Ok(MetaDb {
            host: self.admin.host.clone(),
            port: self.admin.port,
            database: name.clone(),
            username: name,
            password,
            url_params: self.admin.url_params.clone(),
        })
    }

    async fn drop_db(&self, key: &InstanceKey) -> Result<(), ProvisionError> {
        let name = MetaDb::name_for(key);
        let client = self.connect().await?;
        for stmt in drop_sql(&name) {
            client.batch_execute(&stmt).await.map_err(pg)?;
        }
        Ok(())
    }
}

fn checked_prefix(state_prefix: &str) -> Result<String, ProvisionError> {
    ObjectStore::prefix_from_state(state_prefix).ok_or_else(|| {
        ProvisionError::ObjectStore(format!(
            "state prefix {state_prefix:?} is not a safe prefix"
        ))
    })
}

/// One bucket and access key for every instance, each under its own prefix.
#[derive(Debug, Clone)]
pub struct SharedObjectStore {
    pub bucket: String,
    pub endpoint: Option<String>,
    pub region: String,
    pub force_path_style: bool,
    pub access_key_id: String,
    pub secret_access_key: Secret,
}

#[async_trait]
impl ObjectStoreProvisioner for SharedObjectStore {
    async fn ensure(
        &self,
        _key: &InstanceKey,
        prefix: &str,
    ) -> Result<ObjectStore, ProvisionError> {
        Ok(ObjectStore {
            bucket: self.bucket.clone(),
            endpoint: self.endpoint.clone(),
            region: self.region.clone(),
            force_path_style: self.force_path_style,
            prefix: checked_prefix(prefix)?,
            access_key_id: self.access_key_id.clone(),
            secret_access_key: self.secret_access_key.clone(),
        })
    }

    async fn revoke(&self, _key: &InstanceKey) -> Result<(), ProvisionError> {
        // The access key is shared: nothing per instance to revoke.
        Ok(())
    }

    async fn purge(&self, _key: &InstanceKey, prefix: &str) -> Result<(), ProvisionError> {
        let prefix = checked_prefix(prefix)?;
        let store = s3_client(
            &self.bucket,
            self.endpoint.as_deref(),
            &self.region,
            self.force_path_style,
            &self.access_key_id,
            &self.secret_access_key,
        )?;
        purge_prefix(&store, &prefix).await
    }
}

/// An S3 client for a bucket (RustFS, MinIO or AWS).
pub fn s3_client(
    bucket: &str,
    endpoint: Option<&str>,
    region: &str,
    force_path_style: bool,
    access_key_id: &str,
    secret_access_key: &Secret,
) -> Result<object_store::aws::AmazonS3, ProvisionError> {
    let mut b = object_store::aws::AmazonS3Builder::new()
        .with_bucket_name(bucket)
        .with_region(region)
        .with_access_key_id(access_key_id)
        .with_secret_access_key(secret_access_key.expose())
        .with_virtual_hosted_style_request(!force_path_style);
    if let Some(e) = endpoint {
        b = b.with_endpoint(e).with_allow_http(e.starts_with("http://"));
    }
    b.build()
        .map_err(|e| ProvisionError::ObjectStore(e.to_string()))
}

/// Deletes every object under `prefix/` (the trailing `/` keeps
/// `engines/ns/eng_a` from matching `engines/ns/eng_ab`). Idempotent.
pub async fn purge_prefix(
    store: &dyn object_store::ObjectStore,
    prefix: &str,
) -> Result<(), ProvisionError> {
    use futures::TryStreamExt;
    use object_store::ObjectStoreExt;

    let os = |e: object_store::Error| ProvisionError::ObjectStore(e.to_string());
    let path = object_store::path::Path::parse(prefix)
        .map_err(|e| ProvisionError::ObjectStore(e.to_string()))?;
    // `list` matches whole path segments, so `a/b` never lists `a/bc/...`.
    let objects: Vec<object_store::path::Path> = store
        .list(Some(&path))
        .map_ok(|meta| meta.location)
        .try_collect()
        .await
        .map_err(os)?;
    for location in objects {
        match store.delete(&location).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => {}
            Err(e) => return Err(os(e)),
        }
    }
    Ok(())
}

/// In-memory provisioners for tests and the controller's unit tests.
#[derive(Debug, Default, Clone)]
pub struct MemoryProvisioner {
    pub log: Arc<Mutex<Vec<String>>>,
    /// Fail every call whose log line starts with this.
    pub fail: Arc<Mutex<Option<String>>>,
}

impl MemoryProvisioner {
    async fn record(&self, line: String) -> Result<(), ProvisionError> {
        let fail = self.fail.lock().await.clone();
        let failing = fail.is_some_and(|f| line.starts_with(&f));
        self.log.lock().await.push(line.clone());
        if failing {
            Err(ProvisionError::Postgres(format!("injected: {line}")))
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl MetaDbProvisioner for MemoryProvisioner {
    async fn ensure(&self, key: &InstanceKey) -> Result<MetaDb, ProvisionError> {
        self.record(format!("meta.ensure {key}")).await?;
        let name = MetaDb::name_for(key);
        Ok(MetaDb {
            host: "compute1".into(),
            port: 55433,
            database: name.clone(),
            username: name,
            password: Secret::generate(PASSWORD_LEN),
            url_params: None,
        })
    }

    async fn drop_db(&self, key: &InstanceKey) -> Result<(), ProvisionError> {
        self.record(format!("meta.drop {key}")).await
    }
}

#[async_trait]
impl ObjectStoreProvisioner for MemoryProvisioner {
    async fn ensure(&self, key: &InstanceKey, prefix: &str) -> Result<ObjectStore, ProvisionError> {
        self.record(format!("store.ensure {key}")).await?;
        Ok(ObjectStore {
            bucket: "loams-engines".into(),
            endpoint: Some("http://rustfs:9000".into()),
            region: "us-east-1".into(),
            force_path_style: true,
            prefix: checked_prefix(prefix)?,
            access_key_id: format!("AK{key}"),
            secret_access_key: Secret::generate(PASSWORD_LEN),
        })
    }

    async fn revoke(&self, key: &InstanceKey) -> Result<(), ProvisionError> {
        self.record(format!("store.revoke {key}")).await
    }

    async fn purge(&self, key: &InstanceKey, _prefix: &str) -> Result<(), ProvisionError> {
        self.record(format!("store.purge {key}")).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_is_quoted() {
        let pw = Secret::new("abc");
        let sql = create_role_sql("rw_acme", &pw);
        assert!(sql.contains("rolname = 'rw_acme'"));
        assert!(sql.contains("CREATE ROLE \"rw_acme\" LOGIN PASSWORD 'abc'"));
        assert!(sql.contains("ALTER ROLE \"rw_acme\" LOGIN PASSWORD 'abc'"));
        assert_eq!(
            create_database_sql("rw_acme"),
            "CREATE DATABASE \"rw_acme\" OWNER \"rw_acme\""
        );
        assert_eq!(quote_literal("o'k"), "'o''k'");
        assert_eq!(quote_ident("a\"b"), "\"a\"\"b\"");
        let [db, role] = drop_sql("rw_acme");
        assert!(db.starts_with("DROP DATABASE IF EXISTS \"rw_acme\""));
        assert_eq!(role, "DROP ROLE IF EXISTS \"rw_acme\"");
    }

    #[tokio::test]
    async fn purge_deletes_only_the_prefix() {
        use object_store::ObjectStoreExt;
        use object_store::memory::InMemory;
        use object_store::path::Path;

        let store = InMemory::new();
        for p in [
            "engines/dev/eng_a/hummock/1.sst",
            "engines/dev/eng_a/backup/m",
            "engines/dev/eng_ab/hummock/1.sst",
            "engines/dev/eng_b/x",
        ] {
            store
                .put(&Path::from(p), b"x".to_vec().into())
                .await
                .unwrap();
        }
        purge_prefix(&store, "engines/dev/eng_a").await.unwrap();
        purge_prefix(&store, "engines/dev/eng_a").await.unwrap();
        assert!(
            store
                .head(&Path::from("engines/dev/eng_a/backup/m"))
                .await
                .is_err()
        );
        assert!(
            store
                .head(&Path::from("engines/dev/eng_ab/hummock/1.sst"))
                .await
                .is_ok()
        );
        assert!(store.head(&Path::from("engines/dev/eng_b/x")).await.is_ok());
    }

    #[tokio::test]
    async fn shared_store_prefixes_per_instance() {
        let shared = SharedObjectStore {
            bucket: "loams-engines".into(),
            endpoint: None,
            region: "eu-west-1".into(),
            force_path_style: false,
            access_key_id: "k".into(),
            secret_access_key: Secret::new("s"),
        };
        let a = shared
            .ensure(&InstanceKey::new("a").unwrap(), "engines/dev/eng_a/")
            .await
            .unwrap();
        let b = shared
            .ensure(&InstanceKey::new("b").unwrap(), "engines/dev/eng_b/")
            .await
            .unwrap();
        assert_eq!(a.prefix, "engines/dev/eng_a");
        assert_ne!(a.prefix, b.prefix);
        assert!(
            shared
                .ensure(&InstanceKey::new("c").unwrap(), "../x")
                .await
                .is_err()
        );
        assert!(
            shared
                .purge(&InstanceKey::new("a").unwrap(), "/")
                .await
                .is_err()
        );
    }
}
