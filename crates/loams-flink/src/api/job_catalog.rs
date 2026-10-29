//! FL-a's [`JobCatalog`] over the saved-job store: what the session-cluster
//! controller stops with savepoints and restores across an upgrade, a
//! resize or a JobManager restart.
//!
//! A managed job is a saved job in `running`. Its name is the job's
//! `pipeline.name` (`loams:<id>:<name>`), which is also the Flink job's name,
//! so the controller can match live jobs by name. Its setup is the full
//! session bootstrap (the instance's spec merged with the job's, secrets
//! resolved), then the job's own setup, then `SET 'pipeline.name'`: what a
//! fresh session needs before the statement. The rendered setup holds
//! secret values and lives only in the controller's memory; the store keeps
//! references (D1985).

use std::sync::Arc;

use async_trait::async_trait;

use super::jobs::{JobState, JobStore, SavedJob, StoreError};
use super::{FlinkTarget, SecretResolver};
use crate::catalog::RenderCtx;
use crate::catalog::quote::SetOption;
use crate::controller::{JobCatalog, ManagedJob};
use crate::engine::EngineError;
use crate::rest::JobId;
use crate::storage::TenantId;

/// Which namespace and instance a controller tenant is, and its target.
#[async_trait]
pub trait TenantInstances: Send + Sync + 'static {
    /// `(namespace, instance, target)` for `tenant`.
    async fn locate(
        &self,
        tenant: &TenantId,
    ) -> Result<(String, String, Arc<FlinkTarget>), EngineError>;
}

/// [`JobCatalog`] on a [`JobStore`].
pub struct StoreJobCatalog {
    store: Arc<dyn JobStore>,
    tenants: Arc<dyn TenantInstances>,
    secrets: Arc<dyn SecretResolver>,
}

impl std::fmt::Debug for StoreJobCatalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoreJobCatalog").finish_non_exhaustive()
    }
}

/// How many times a record is retried when a concurrent API operation
/// changed the job in between.
const CAS_RETRIES: usize = 5;

fn store_err(e: StoreError) -> EngineError {
    match e {
        StoreError::NotFound(id) => EngineError::NotFound(id),
        other => EngineError::Runtime(other.to_string()),
    }
}

/// The job id in a pipeline name `loams:<id>:<name>`.
pub fn job_id_of(pipeline_name: &str) -> Option<&str> {
    let rest = pipeline_name.strip_prefix("loams:")?;
    let (id, _) = rest.split_once(':')?;
    id.starts_with("fj_").then_some(id)
}

impl StoreJobCatalog {
    pub fn new(
        store: Arc<dyn JobStore>,
        tenants: Arc<dyn TenantInstances>,
        secrets: Arc<dyn SecretResolver>,
    ) -> Self {
        Self {
            store,
            tenants,
            secrets,
        }
    }

    async fn setup_of(
        &self,
        namespace: &str,
        target: &FlinkTarget,
        job: &SavedJob,
    ) -> Result<Vec<String>, EngineError> {
        let script = target
            .session
            .merged(&job.session)
            .script()
            .map_err(|e| EngineError::Invalid(format!("job {}: {e}", job.id)))?;
        let refs = script.secret_refs();
        let values = if refs.is_empty() {
            Default::default()
        } else {
            self.secrets
                .resolve(namespace, &refs)
                .await
                .map_err(|e| EngineError::Provision(format!("job {}: {}", job.id, e.message)))?
        };
        let mut setup = script
            .render(&RenderCtx::resolved(&values))
            .map_err(|e| EngineError::Invalid(format!("job {}: {e}", job.id)))?;
        setup.extend(job.setup.iter().cloned());
        setup.push(
            SetOption::new("pipeline.name", job.pipeline_name())
                .render(&RenderCtx::redacted())
                .map_err(|e| EngineError::Invalid(e.to_string()))?,
        );
        Ok(setup)
    }

    /// Apply `change` to the job named `pipeline_name`, retrying when an API
    /// operation changed it in between.
    async fn record(
        &self,
        tenant: &TenantId,
        pipeline_name: &str,
        change: impl Fn(&mut SavedJob),
    ) -> Result<(), EngineError> {
        let (namespace, _, _) = self.tenants.locate(tenant).await?;
        let id = job_id_of(pipeline_name).ok_or_else(|| {
            EngineError::Invalid(format!("{pipeline_name:?} is not a Loams saved job"))
        })?;
        for _ in 0..CAS_RETRIES {
            let Some(mut job) = self.store.get(&namespace, id).await.map_err(store_err)? else {
                return Err(EngineError::NotFound(id.to_string()));
            };
            change(&mut job);
            match self.store.update(job).await {
                Ok(_) => return Ok(()),
                Err(StoreError::Conflict(_)) => continue,
                Err(e) => return Err(store_err(e)),
            }
        }
        Err(EngineError::Runtime(format!(
            "job {id} kept changing; gave up after {CAS_RETRIES} tries"
        )))
    }
}

#[async_trait]
impl JobCatalog for StoreJobCatalog {
    async fn managed_jobs(&self, tenant: &TenantId) -> Result<Vec<ManagedJob>, EngineError> {
        let (namespace, instance, target) = self.tenants.locate(tenant).await?;
        let jobs = self
            .store
            .list(&namespace, &instance)
            .await
            .map_err(store_err)?;
        let mut out = Vec::new();
        for job in jobs.iter().filter(|j| j.state == JobState::Running) {
            let flink_job = match &job.flink_job_id {
                Some(id) => Some(JobId::new(id.clone()).map_err(EngineError::from)?),
                None => None,
            };
            out.push(ManagedJob {
                name: job.pipeline_name(),
                setup: self.setup_of(&namespace, &target, job).await?,
                statement: job.statement.clone(),
                flink_job,
                last_savepoint: job.savepoint.clone(),
            });
        }
        Ok(out)
    }

    async fn record_stopped(
        &self,
        tenant: &TenantId,
        name: &str,
        savepoint: Option<String>,
    ) -> Result<(), EngineError> {
        // The controller stops a running job to restore it right after
        // (an upgrade or a resize): the job stays `running` in the store,
        // with the savepoint it will restore from and no live Flink job.
        self.record(tenant, name, |job| {
            job.savepoint.clone_from(&savepoint);
            job.flink_job_id = None;
        })
        .await
    }

    async fn record_started(
        &self,
        tenant: &TenantId,
        name: &str,
        job_id: JobId,
    ) -> Result<(), EngineError> {
        let id = job_id.as_str().to_string();
        self.record(tenant, name, |job| {
            job.state = JobState::Running;
            job.flink_job_id = Some(id.clone());
            job.message = None;
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::StaticSecrets;
    use crate::api::jobs::{MemoryJobStore, NewJob};
    use crate::catalog::SessionSpec;
    use std::collections::BTreeMap;

    #[test]
    fn job_ids_from_pipeline_names() {
        assert_eq!(job_id_of("loams:fj_abc:clicks: per minute"), Some("fj_abc"));
        assert_eq!(job_id_of("other:fj_abc:x"), None);
        assert_eq!(job_id_of("loams:nope:x"), None);
        assert_eq!(job_id_of("loams:fj_abc"), None);
    }

    struct OneTenant(Arc<FlinkTarget>);

    #[async_trait]
    impl TenantInstances for OneTenant {
        async fn locate(
            &self,
            _tenant: &TenantId,
        ) -> Result<(String, String, Arc<FlinkTarget>), EngineError> {
            Ok(("ns1".into(), "eng_x".into(), self.0.clone()))
        }
    }

    struct NoGateway;

    #[async_trait]
    impl crate::api::backend::FlinkSqlBackend for NoGateway {
        async fn open_session(
            &self,
            _: &str,
            _: &BTreeMap<String, String>,
        ) -> Result<crate::api::backend::SessionHandle, crate::api::backend::BackendError> {
            Err(crate::api::backend::BackendError::Unavailable(
                "none".into(),
            ))
        }
        async fn close_session(
            &self,
            _: &crate::api::backend::SessionHandle,
        ) -> Result<(), crate::api::backend::BackendError> {
            Ok(())
        }
        async fn heartbeat(
            &self,
            _: &crate::api::backend::SessionHandle,
        ) -> Result<(), crate::api::backend::BackendError> {
            Ok(())
        }
        async fn execute(
            &self,
            _: &crate::api::backend::SessionHandle,
            _: &str,
        ) -> Result<crate::api::backend::OperationHandle, crate::api::backend::BackendError>
        {
            Err(crate::api::backend::BackendError::Unavailable(
                "none".into(),
            ))
        }
        async fn fetch(
            &self,
            _: &crate::api::backend::SessionHandle,
            _: &crate::api::backend::OperationHandle,
            _: u64,
        ) -> Result<crate::api::backend::ResultPage, crate::api::backend::BackendError> {
            Err(crate::api::backend::BackendError::Unavailable(
                "none".into(),
            ))
        }
        async fn cancel(
            &self,
            _: &crate::api::backend::SessionHandle,
            _: &crate::api::backend::OperationHandle,
        ) -> Result<(), crate::api::backend::BackendError> {
            Ok(())
        }
        async fn close_operation(
            &self,
            _: &crate::api::backend::SessionHandle,
            _: &crate::api::backend::OperationHandle,
        ) -> Result<(), crate::api::backend::BackendError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn running_jobs_are_managed_and_records_land_in_the_store() {
        let store = Arc::new(MemoryJobStore::default());
        let target = Arc::new(FlinkTarget {
            backend: Arc::new(NoGateway),
            session: SessionSpec::default(),
        });
        let catalog = StoreJobCatalog::new(
            store.clone(),
            Arc::new(OneTenant(target)),
            Arc::new(StaticSecrets::default()),
        );
        let new = |name: &str| NewJob {
            name: name.into(),
            description: None,
            statement: "INSERT INTO a SELECT 1".into(),
            setup: vec!["CREATE DATABASE IF NOT EXISTS loams.analytics".into()],
            session: SessionSpec::default(),
        };
        let mut running = SavedJob::new("ns1", "eng_x", "user:a", new("running one"));
        running.state = JobState::Running;
        running.flink_job_id = Some("0".repeat(32));
        let draft = SavedJob::new("ns1", "eng_x", "user:a", new("draft one"));
        store.insert(running.clone()).await.expect("insert");
        store.insert(draft).await.expect("insert");

        let tenant = TenantId::new("t1").expect("tenant");
        let managed = catalog.managed_jobs(&tenant).await.expect("managed");
        assert_eq!(managed.len(), 1);
        let m = &managed[0];
        assert_eq!(m.name, running.pipeline_name());
        assert_eq!(m.statement, "INSERT INTO a SELECT 1");
        assert!(
            m.setup[0].starts_with("SET 'execution.runtime-mode'"),
            "{:?}",
            m.setup
        );
        assert!(
            m.setup
                .contains(&"CREATE DATABASE IF NOT EXISTS loams.analytics".to_string())
        );
        assert_eq!(
            m.setup.last().map(String::as_str),
            Some(format!("SET 'pipeline.name' = '{}'", running.pipeline_name()).as_str())
        );

        catalog
            .record_stopped(&tenant, &m.name, Some("s3://b/sp-1".into()))
            .await
            .expect("stopped");
        let j = store
            .get("ns1", &running.id)
            .await
            .expect("get")
            .expect("job");
        assert_eq!(j.savepoint.as_deref(), Some("s3://b/sp-1"));
        assert_eq!(j.flink_job_id, None);
        assert_eq!(j.state, JobState::Running);

        let new_id = JobId::new("a".repeat(32)).expect("job id");
        catalog
            .record_started(&tenant, &m.name, new_id)
            .await
            .expect("started");
        let j = store
            .get("ns1", &running.id)
            .await
            .expect("get")
            .expect("job");
        assert_eq!(j.flink_job_id.as_deref(), Some("a".repeat(32).as_str()));

        assert!(
            catalog
                .record_started(
                    &tenant,
                    "loams:fj_missing:x",
                    JobId::new("b".repeat(32)).expect("id")
                )
                .await
                .is_err()
        );
    }
}
