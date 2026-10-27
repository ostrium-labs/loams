//! The spike's authorizer: OpenFGA over gRPC (`openfga-client` 0.6.1) plus an allow-only decision
//! cache (design 76 §9.3, D1611). This is the shape IA2's `loams-authz` takes, not its code.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use openfga_client::client::{
    BatchCheckItem, CheckRequestTupleKey, ConsistencyPreference, OpenFgaClient,
    OpenFgaServiceClient, ReadRequestTupleKey, RelationshipCondition, TupleKey,
    TupleKeyWithoutCondition, WriteOptions, batch_check_single_result::CheckResult,
};
use openfga_client::prost_wkt_types::Struct;
use openfga_client::tonic::transport::Channel;
use tokio::sync::Mutex;

/// The object types a subject can hold a tuple on, for "delete every tuple of this subject".
pub const SUBJECT_OBJECT_TYPES: &[&str] = &[
    "platform",
    "org",
    "team",
    "agent",
    "project",
    "environment",
    "deployment",
    "database",
    "graph",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Consistency {
    MinimizeLatency,
    HigherConsistency,
}

#[derive(Clone, Debug)]
pub struct Decision {
    pub allowed: bool,
    pub cached: bool,
    pub elapsed: Duration,
}

type CacheKey = (String, String, String);

#[derive(Clone)]
pub struct Fga {
    fast: OpenFgaClient<Channel>,
    strong: OpenFgaClient<Channel>,
    cache_ttl: Duration,
    cache: Arc<Mutex<HashMap<CacheKey, Instant>>>,
}

pub fn context(v: serde_json::Value) -> Result<Struct> {
    serde_json::from_value(v).context("context must be a JSON object")
}

impl Fga {
    pub async fn connect(
        endpoint: &str,
        store: &str,
        model: &str,
        cache_ttl: Duration,
    ) -> Result<Self> {
        let svc = OpenFgaServiceClient::connect(endpoint.to_owned())
            .await
            .context("connect to OpenFGA gRPC")?;
        let base = OpenFgaClient::new(svc, store, model);
        Ok(Self {
            fast: base
                .clone()
                .set_consistency(ConsistencyPreference::MinimizeLatency),
            strong: base.set_consistency(ConsistencyPreference::HigherConsistency),
            cache_ttl,
            cache: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    fn client(&self, c: Consistency) -> &OpenFgaClient<Channel> {
        match c {
            Consistency::MinimizeLatency => &self.fast,
            Consistency::HigherConsistency => &self.strong,
        }
    }

    /// One check. `HigherConsistency` bypasses the cache (destructive, admin and
    /// credential-issuing calls, D1611); only allows are cached.
    pub async fn check(
        &self,
        user: &str,
        relation: &str,
        object: &str,
        consistency: Consistency,
        ctx: Option<Struct>,
    ) -> Result<Decision> {
        let start = Instant::now();
        let key = (user.to_owned(), relation.to_owned(), object.to_owned());
        let use_cache = !self.cache_ttl.is_zero()
            && consistency == Consistency::MinimizeLatency
            && ctx.is_none();
        if use_cache {
            let cache = self.cache.lock().await;
            if let Some(at) = cache.get(&key)
                && at.elapsed() < self.cache_ttl
            {
                return Ok(Decision {
                    allowed: true,
                    cached: true,
                    elapsed: start.elapsed(),
                });
            }
        }
        let tk = CheckRequestTupleKey {
            user: user.into(),
            relation: relation.into(),
            object: object.into(),
        };
        let allowed = self.client(consistency).check(tk, None, ctx, false).await?;
        if use_cache {
            let mut cache = self.cache.lock().await;
            if allowed {
                cache.insert(key, Instant::now());
            } else {
                cache.remove(&key);
            }
        }
        Ok(Decision {
            allowed,
            cached: false,
            elapsed: start.elapsed(),
        })
    }

    pub async fn batch_check(
        &self,
        checks: &[(String, String, String)],
        consistency: Consistency,
    ) -> Result<Vec<bool>> {
        let items: Vec<BatchCheckItem> = checks
            .iter()
            .enumerate()
            .map(|(i, (u, r, o))| BatchCheckItem {
                tuple_key: Some(CheckRequestTupleKey {
                    user: u.clone(),
                    relation: r.clone(),
                    object: o.clone(),
                }),
                contextual_tuples: None,
                context: None,
                correlation_id: format!("c{i}"),
            })
            .collect();
        let res = self.client(consistency).batch_check(items).await?;
        Ok((0..checks.len())
            .map(|i| matches!(res.get(&format!("c{i}")), Some(CheckResult::Allowed(true))))
            .collect())
    }

    pub async fn list_objects(&self, user: &str, relation: &str, ty: &str) -> Result<Vec<String>> {
        let res = self
            .fast
            .list_objects(ty, relation, user, None, None)
            .await?;
        Ok(res.into_inner().objects)
    }

    /// Idempotent write: a duplicate write and a missing delete both succeed (D1608).
    pub async fn write(
        &self,
        writes: Vec<TupleKey>,
        deletes: Vec<TupleKeyWithoutCondition>,
    ) -> Result<()> {
        self.fast
            .write_with_options(writes, deletes, WriteOptions::new_idempotent())
            .await?;
        // A write or delete can turn a cached allow stale; the spike drops the whole cache. IA2
        // invalidates by key from ReadChanges instead.
        self.cache.lock().await.clear();
        Ok(())
    }

    /// Writes with the default conflict behaviour, so type-restriction errors surface (used by
    /// `agent_cannot_write_grants`).
    pub async fn write_strict(&self, writes: Vec<TupleKey>) -> Result<()> {
        self.fast.write(writes, None).await?;
        Ok(())
    }

    /// Deletes every tuple whose user is `subject` (`agent:<id>`), in batches of at most 100.
    pub async fn delete_subject(&self, subject: &str) -> Result<usize> {
        let mut deletes = Vec::new();
        for ty in SUBJECT_OBJECT_TYPES {
            let key = ReadRequestTupleKey {
                user: subject.into(),
                relation: String::new(),
                object: format!("{ty}:"),
            };
            for t in self.strong.read_all_pages(Some(key), 100, 100).await? {
                if let Some(k) = t.key {
                    deletes.push(TupleKeyWithoutCondition {
                        user: k.user,
                        relation: k.relation,
                        object: k.object,
                    });
                }
            }
        }
        let n = deletes.len();
        for chunk in deletes.chunks(100) {
            self.write(Vec::new(), chunk.to_vec()).await?;
        }
        Ok(n)
    }

    pub async fn read_all(&self) -> Result<Vec<TupleKey>> {
        let tuples = self
            .strong
            .read_all_pages(None::<ReadRequestTupleKey>, 100, 10_000)
            .await?;
        Ok(tuples.into_iter().filter_map(|t| t.key).collect())
    }
}

pub fn tuple(user: &str, relation: &str, object: &str) -> TupleKey {
    TupleKey {
        user: user.into(),
        relation: relation.into(),
        object: object.into(),
        condition: None,
    }
}

pub fn tuple_with(
    user: &str,
    relation: &str,
    object: &str,
    condition: &str,
    ctx: Struct,
) -> TupleKey {
    TupleKey {
        user: user.into(),
        relation: relation.into(),
        object: object.into(),
        condition: Some(RelationshipCondition {
            name: condition.into(),
            context: Some(ctx),
        }),
    }
}

pub fn key(user: &str, relation: &str, object: &str) -> TupleKeyWithoutCondition {
    TupleKeyWithoutCondition {
        user: user.into(),
        relation: relation.into(),
        object: object.into(),
    }
}
