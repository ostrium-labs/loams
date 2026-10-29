//! [`OpenFgaAuthorizer`]: OpenFGA over gRPC with `openfga-client` 0.6.1 (Apache-2.0, D66).
//!
//! One store and one pinned model per install (D1612). The client speaks plaintext gRPC to an
//! internal endpoint (OpenFGA is never public, D1617), with an optional pre-shared key sent as
//! a bearer token (`--authn-method preshared`).

use std::time::Duration;

use async_trait::async_trait;
use openfga_client::client::{
    BatchCheckItem, BatchCheckRequest, CheckRequest as PbCheck, CheckRequestTupleKey,
    ConsistencyPreference, ContextualTupleKeys, ListObjectsRequest, OpenFgaServiceClient,
    ReadRequest, ReadRequestTupleKey, RelationshipCondition, TupleKey, TupleKeyWithoutCondition,
    WriteRequest, WriteRequestDeletes, WriteRequestWrites, batch_check_single_result::CheckResult,
};
use openfga_client::prost_wkt_types::Struct;
use openfga_client::tonic::metadata::{Ascii, MetadataValue};
use openfga_client::tonic::service::Interceptor;
use openfga_client::tonic::service::interceptor::InterceptedService;
use openfga_client::tonic::transport::{Channel, Endpoint};
use openfga_client::tonic::{Request, Status};
use serde::{Deserialize, Serialize};

use crate::{
    Authorizer, AuthzError, CheckRequest, Consistency, Decision, ObjectRef, Result, Subject,
};

/// OpenFGA's default `max-checks-per-batch-check` (checked at v1.22.0).
pub const MAX_CHECKS_PER_BATCH: usize = 50;
/// OpenFGA's default `max-tuples-per-write`.
const MAX_TUPLES_PER_WRITE: usize = 100;

/// A relationship tuple, `object#relation@user`, optionally under a named condition.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tuple {
    pub user: String,
    pub relation: String,
    pub object: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<(String, serde_json::Map<String, serde_json::Value>)>,
}

impl Tuple {
    pub fn new(
        user: impl Into<String>,
        relation: impl Into<String>,
        object: impl Into<String>,
    ) -> Self {
        Self {
            user: user.into(),
            relation: relation.into(),
            object: object.into(),
            condition: None,
        }
    }

    #[must_use]
    pub fn with_condition(
        mut self,
        name: impl Into<String>,
        context: serde_json::Map<String, serde_json::Value>,
    ) -> Self {
        self.condition = Some((name.into(), context));
        self
    }

    /// Refuses a condition whose stored context sets `session_id` or `now`. Those come from
    /// the request, and a stored value wins over the request's in OpenFGA, so storing one
    /// would make an `agent_session` grant hold for every session or never expire.
    pub fn check_condition(&self) -> Result<()> {
        if let Some((name, ctx)) = &self.condition
            && let Some(k) = REQUEST_ONLY_PARAMS.iter().find(|k| ctx.contains_key(**k))
        {
            return Err(AuthzError::Invalid(format!(
                "tuple {}#{}@{}: condition {name} may not store {k}, which the request supplies",
                self.object, self.relation, self.user
            )));
        }
        Ok(())
    }
}

/// Condition parameters only a request may supply.
const REQUEST_ONLY_PARAMS: &[&str] = &["session_id", "now"];

#[derive(Clone)]
struct Bearer(Option<MetadataValue<Ascii>>);

impl Interceptor for Bearer {
    fn call(&mut self, mut req: Request<()>) -> std::result::Result<Request<()>, Status> {
        if let Some(v) = &self.0 {
            req.metadata_mut().insert("authorization", v.clone());
        }
        Ok(req)
    }
}

type Client = OpenFgaServiceClient<InterceptedService<Channel, Bearer>>;

fn bearer(preshared_key: Option<&str>) -> Result<Option<MetadataValue<Ascii>>> {
    preshared_key
        .map(|k| {
            format!("Bearer {k}")
                .parse()
                .map_err(|_| AuthzError::Invalid("pre-shared key".into()))
        })
        .transpose()
}

#[derive(Clone)]
pub struct OpenFgaAuthorizer {
    client: Client,
    store_id: String,
    model_id: String,
}

impl std::fmt::Debug for OpenFgaAuthorizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenFgaAuthorizer")
            .field("store_id", &self.store_id)
            .field("model_id", &self.model_id)
            .finish_non_exhaustive()
    }
}

fn unavailable(e: impl std::fmt::Display) -> AuthzError {
    AuthzError::Unavailable(e.to_string())
}

fn status(e: Status) -> AuthzError {
    use openfga_client::tonic::Code;
    match e.code() {
        Code::InvalidArgument | Code::FailedPrecondition | Code::NotFound | Code::AlreadyExists => {
            AuthzError::Invalid(e.message().to_owned())
        }
        // OpenFGA sends its validation errors (codes 2000-2999) as numeric gRPC codes that tonic
        // reads as `Unknown`; their messages name the invalid tuple or request.
        Code::Unknown
            if e.message().starts_with("Invalid") || e.message().contains("type restriction") =>
        {
            AuthzError::Invalid(e.message().to_owned())
        }
        _ => unavailable(format!("{}: {}", e.code(), e.message())),
    }
}

fn consistency(c: Consistency) -> i32 {
    match c {
        Consistency::MinimizeLatency => ConsistencyPreference::MinimizeLatency as i32,
        Consistency::HigherConsistency => ConsistencyPreference::HigherConsistency as i32,
    }
}

fn to_struct(map: &serde_json::Map<String, serde_json::Value>) -> Result<Option<Struct>> {
    if map.is_empty() {
        return Ok(None);
    }
    serde_json::from_value(serde_json::Value::Object(map.clone()))
        .map(Some)
        .map_err(|e| AuthzError::Invalid(format!("context: {e}")))
}

fn to_pb(t: &Tuple) -> Result<TupleKey> {
    t.check_condition()?;
    let condition = match &t.condition {
        Some((name, ctx)) => Some(RelationshipCondition {
            name: name.clone(),
            context: to_struct(ctx)?,
        }),
        None => None,
    };
    Ok(TupleKey {
        user: t.user.clone(),
        relation: t.relation.clone(),
        object: t.object.clone(),
        condition,
    })
}

impl OpenFgaAuthorizer {
    /// Connects to OpenFGA's gRPC endpoint (`http://openfga:8081`), with a pre-shared key when
    /// the server requires one.
    pub async fn connect(
        endpoint: &str,
        store_id: &str,
        model_id: &str,
        preshared_key: Option<&str>,
    ) -> Result<Self> {
        let channel = Endpoint::from_shared(endpoint.to_owned())
            .map_err(unavailable)?
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .connect()
            .await
            .map_err(|e| unavailable(format!("connect {endpoint}: {e}")))?;
        Ok(Self {
            client: OpenFgaServiceClient::new(InterceptedService::new(
                channel,
                Bearer(bearer(preshared_key)?),
            )),
            store_id: store_id.to_owned(),
            model_id: model_id.to_owned(),
        })
    }

    /// Like [`OpenFgaAuthorizer::connect`], but connects on first use, so a process can start
    /// before OpenFGA answers. Until it does, every check fails with
    /// [`AuthzError::Unavailable`], which callers answer as a refusal (503), never an allow.
    pub fn connect_lazy(
        endpoint: &str,
        store_id: &str,
        model_id: &str,
        preshared_key: Option<&str>,
    ) -> Result<Self> {
        let channel = Endpoint::from_shared(endpoint.to_owned())
            .map_err(|e| AuthzError::Invalid(format!("OpenFGA endpoint {endpoint:?}: {e}")))?
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .connect_lazy();
        Ok(Self {
            client: OpenFgaServiceClient::new(InterceptedService::new(
                channel,
                Bearer(bearer(preshared_key)?),
            )),
            store_id: store_id.to_owned(),
            model_id: model_id.to_owned(),
        })
    }

    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    pub fn store_id(&self) -> &str {
        &self.store_id
    }

    fn pb_check(&self, req: &CheckRequest) -> Result<PbCheck> {
        Ok(PbCheck {
            store_id: self.store_id.clone(),
            authorization_model_id: self.model_id.clone(),
            tuple_key: Some(CheckRequestTupleKey {
                user: req.subject.fga_user(),
                relation: req.relation.clone(),
                object: req.object.to_string(),
            }),
            contextual_tuples: contextual(&req.contextual_tuples)?,
            context: to_struct(&req.effective_context())?,
            consistency: consistency(req.consistency),
            trace: false,
        })
    }

    /// Writes and deletes in one transaction each chunk of at most 100, idempotently: a
    /// duplicate write and a missing delete both succeed (D1608).
    pub async fn write(&self, writes: &[Tuple], deletes: &[Tuple]) -> Result<()> {
        let mut writes: Vec<TupleKey> = writes.iter().map(to_pb).collect::<Result<_>>()?;
        let mut deletes: Vec<TupleKeyWithoutCondition> = deletes
            .iter()
            .map(|t| TupleKeyWithoutCondition {
                user: t.user.clone(),
                relation: t.relation.clone(),
                object: t.object.clone(),
            })
            .collect();
        while !writes.is_empty() || !deletes.is_empty() {
            let w: Vec<_> = writes
                .drain(..writes.len().min(MAX_TUPLES_PER_WRITE))
                .collect();
            let d: Vec<_> = deletes
                .drain(..deletes.len().min(MAX_TUPLES_PER_WRITE - w.len()))
                .collect();
            let req = WriteRequest {
                store_id: self.store_id.clone(),
                authorization_model_id: self.model_id.clone(),
                writes: (!w.is_empty()).then(|| WriteRequestWrites {
                    tuple_keys: w,
                    on_duplicate: "ignore".into(),
                }),
                deletes: (!d.is_empty()).then(|| WriteRequestDeletes {
                    tuple_keys: d,
                    on_missing: "ignore".into(),
                }),
            };
            self.send_write(req).await?;
        }
        Ok(())
    }

    /// The stored tuples that match: `object` is a full object (`org:o1`) or, with `user`, a
    /// type alone (`org:`); `user` and `relation` narrow it when given. Every page is read
    /// (OpenFGA's `Read`, 100 per page), with `HigherConsistency`, so a sync or a
    /// reconciliation never acts on a stale read. Conditions are not returned.
    pub async fn read(
        &self,
        user: Option<&str>,
        relation: Option<&str>,
        object: &str,
    ) -> Result<Vec<Tuple>> {
        let mut out = Vec::new();
        let mut token = String::new();
        loop {
            let req = ReadRequest {
                store_id: self.store_id.clone(),
                tuple_key: Some(ReadRequestTupleKey {
                    user: user.unwrap_or_default().to_owned(),
                    relation: relation.unwrap_or_default().to_owned(),
                    object: object.to_owned(),
                }),
                page_size: Some(MAX_TUPLES_PER_WRITE as i32),
                continuation_token: token,
                consistency: ConsistencyPreference::HigherConsistency as i32,
            };
            let res = self
                .client
                .clone()
                .read(req)
                .await
                .map_err(status)?
                .into_inner();
            out.extend(res.tuples.into_iter().filter_map(|t| t.key).map(|k| Tuple {
                user: k.user,
                relation: k.relation,
                object: k.object,
                condition: None,
            }));
            if res.continuation_token.is_empty() {
                return Ok(out);
            }
            token = res.continuation_token;
        }
    }

    /// A strict write: an existing tuple or a type restriction is an error. Grants go through
    /// this, so `agent` on `billing_admin` is refused by the model (D1607).
    pub async fn write_strict(&self, writes: &[Tuple]) -> Result<()> {
        let tuple_keys: Vec<TupleKey> = writes.iter().map(to_pb).collect::<Result<_>>()?;
        let req = WriteRequest {
            store_id: self.store_id.clone(),
            authorization_model_id: self.model_id.clone(),
            writes: Some(WriteRequestWrites {
                tuple_keys,
                on_duplicate: String::new(),
            }),
            deletes: None,
        };
        self.send_write(req).await?;
        Ok(())
    }
}

impl OpenFgaAuthorizer {
    /// Sends one write, retrying a write the datastore did not commit because it was busy
    /// (SQLite's lock, `busy_timeout` 100 ms in OpenFGA) or the server was briefly unavailable.
    /// Up to 4 attempts, 50-400 ms apart; validation errors are never retried.
    async fn send_write(&self, req: WriteRequest) -> Result<()> {
        let mut delay = Duration::from_millis(50);
        for attempt in 1.. {
            match self.client.clone().write(req.clone()).await {
                Ok(_) => return Ok(()),
                Err(e) if attempt < 4 && transient(&e) => {
                    tracing::debug!(target: "loams_authz", attempt, error = %e, "retrying an OpenFGA write");
                    tokio::time::sleep(delay).await;
                    delay *= 2;
                }
                Err(e) => return Err(status(e)),
            }
        }
        unreachable!("the loop returns")
    }
}

fn transient(e: &Status) -> bool {
    use openfga_client::tonic::Code;
    let m = e.message();
    matches!(
        e.code(),
        Code::Unavailable | Code::ResourceExhausted | Code::Aborted
    ) || m.contains("database is locked")
        || m.contains("SQLITE_BUSY")
}

fn contextual(tuples: &[Tuple]) -> Result<Option<ContextualTupleKeys>> {
    if tuples.is_empty() {
        return Ok(None);
    }
    Ok(Some(ContextualTupleKeys {
        tuple_keys: tuples.iter().map(to_pb).collect::<Result<_>>()?,
    }))
}

#[async_trait]
impl Authorizer for OpenFgaAuthorizer {
    async fn check(&self, req: CheckRequest) -> Result<Decision> {
        let res = self
            .client
            .clone()
            .check(self.pb_check(&req)?)
            .await
            .map_err(status)?;
        let model_id = Some(self.model_id.clone());
        Ok(if res.get_ref().allowed {
            Decision::allow(model_id)
        } else {
            Decision::deny(model_id)
        })
    }

    /// `BatchCheck` in chunks of [`MAX_CHECKS_PER_BATCH`]; each chunk uses the strongest
    /// consistency any of its requests asks for.
    async fn batch_check(&self, reqs: Vec<CheckRequest>) -> Result<Vec<Decision>> {
        let mut out = Vec::with_capacity(reqs.len());
        for chunk in reqs.chunks(MAX_CHECKS_PER_BATCH) {
            let strongest = chunk
                .iter()
                .map(|r| r.consistency)
                .max_by_key(|c| *c == Consistency::HigherConsistency);
            let checks = chunk
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    Ok(BatchCheckItem {
                        tuple_key: Some(CheckRequestTupleKey {
                            user: r.subject.fga_user(),
                            relation: r.relation.clone(),
                            object: r.object.to_string(),
                        }),
                        contextual_tuples: contextual(&r.contextual_tuples)?,
                        context: to_struct(&r.effective_context())?,
                        correlation_id: format!("c{i}"),
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let req = BatchCheckRequest {
                store_id: self.store_id.clone(),
                authorization_model_id: self.model_id.clone(),
                checks,
                consistency: consistency(strongest.unwrap_or_default()),
            };
            let res = self
                .client
                .clone()
                .batch_check(req)
                .await
                .map_err(status)?
                .into_inner();
            for i in 0..chunk.len() {
                let allowed = match res
                    .result
                    .get(&format!("c{i}"))
                    .and_then(|r| r.check_result.as_ref())
                {
                    Some(CheckResult::Allowed(a)) => *a,
                    // One failed item fails the batch: never guess an allow.
                    Some(CheckResult::Error(e)) => {
                        return Err(unavailable(format!("batch item: {}", e.message)));
                    }
                    None => return Err(unavailable("batch item missing from the response")),
                };
                let model_id = Some(self.model_id.clone());
                out.push(if allowed {
                    Decision::allow(model_id)
                } else {
                    Decision::deny(model_id)
                });
            }
        }
        Ok(out)
    }

    async fn list_objects(
        &self,
        subject: &Subject,
        relation: &str,
        ty: &str,
    ) -> Result<Vec<ObjectRef>> {
        let req = ListObjectsRequest {
            store_id: self.store_id.clone(),
            authorization_model_id: self.model_id.clone(),
            r#type: ty.to_owned(),
            relation: relation.to_owned(),
            user: subject.fga_user(),
            contextual_tuples: None,
            context: None,
            consistency: consistency(Consistency::MinimizeLatency),
        };
        let res = self
            .client
            .clone()
            .list_objects(req)
            .await
            .map_err(status)?
            .into_inner();
        let mut objects: Vec<ObjectRef> = res
            .objects
            .iter()
            .map(|o| o.parse())
            .collect::<Result<_>>()?;
        objects.sort();
        Ok(objects)
    }
}
