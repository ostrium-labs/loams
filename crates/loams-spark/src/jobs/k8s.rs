//! The Kubernetes runner (feature `kubernetes`; D2001): creates the rendered
//! driver pod, follows its phase, reads its logs, deletes it to cancel.
//!
//! The driver pod runs `spark-submit` in client mode (Kubernetes backend) or
//! the Loams Connect driver (Connect backend); either way one pod is one
//! attempt, with `restartPolicy: Never`, so Kubernetes never retries behind
//! the supervisor's back. Executor pods are owned by the driver pod and go
//! with it.

use async_trait::async_trait;
use k8s_openapi::api::core::v1::{ConfigMap, Pod};
use kube::api::{Api, DeleteParams, LogParams, PostParams};

use super::JobsError;
use super::render::{RenderContext, render_pod};
use super::run::{AttemptOutcome, LogLink};
use super::runner::{Attempt, AttemptHandle, AttemptPhase, Runner};

/// The most log lines [`K8sRunner::logs`] returns.
pub const LOG_TAIL_LINES: i64 = 5_000;

/// Builds the render context of an attempt (the namespace's images, secrets,
/// endpoint and catalog), from the host's configuration.
pub trait ContextFactory: Send + Sync + std::fmt::Debug {
    /// The context of `attempt`.
    fn context(&self, attempt: &Attempt) -> RenderContext;
}

/// A fixed template: every attempt gets `template` with its run and attempt.
#[derive(Debug, Clone)]
pub struct TemplateContext(pub RenderContext);

impl ContextFactory for TemplateContext {
    fn context(&self, attempt: &Attempt) -> RenderContext {
        let mut ctx = self.0.clone();
        ctx.run_id = attempt.run_id.clone();
        ctx.attempt = attempt.attempt;
        ctx
    }
}

/// Runs attempts as driver pods.
pub struct K8sRunner<F: ContextFactory> {
    client: kube::Client,
    contexts: F,
}

// `kube::Client` has no `Debug`.
impl<F: ContextFactory> std::fmt::Debug for K8sRunner<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("K8sRunner")
            .field("contexts", &self.contexts)
            .finish_non_exhaustive()
    }
}

fn kube_err(e: kube::Error) -> JobsError {
    JobsError::Backend(e.to_string())
}

/// `<k8s namespace>/<pod>`.
fn split_id(id: &str) -> Result<(&str, &str), JobsError> {
    id.split_once('/')
        .ok_or_else(|| JobsError::Backend(format!("not a pod id: {id}")))
}

impl<F: ContextFactory> K8sRunner<F> {
    /// A runner over `client`.
    pub fn new(client: kube::Client, contexts: F) -> Self {
        Self { client, contexts }
    }

    /// A runner with the in-cluster (or kubeconfig) client.
    pub async fn try_default(contexts: F) -> Result<Self, JobsError> {
        let client = kube::Client::try_default()
            .await
            .map_err(|e| JobsError::Unavailable(format!("no Kubernetes client: {e}")))?;
        Ok(Self::new(client, contexts))
    }

    fn pods(&self, ns: &str) -> Api<Pod> {
        Api::namespaced(self.client.clone(), ns)
    }
}

/// The attempt's phase from its pod.
pub fn phase_of(pod: &Pod) -> AttemptPhase {
    let Some(status) = &pod.status else {
        return AttemptPhase::Pending;
    };
    let terminated = status
        .container_statuses
        .as_ref()
        .and_then(|cs| cs.iter().find(|c| c.name == "driver"))
        .and_then(|c| c.state.as_ref())
        .and_then(|s| s.terminated.as_ref());
    match status.phase.as_deref() {
        Some("Running") => AttemptPhase::Running,
        Some("Succeeded") => AttemptPhase::Done(AttemptOutcome::Succeeded),
        Some("Failed") => {
            let exit_code = terminated.map(|t| t.exit_code);
            let reason = status
                .reason
                .clone()
                .or_else(|| terminated.and_then(|t| t.reason.clone()));
            if reason.as_deref() == Some("DeadlineExceeded") {
                return AttemptPhase::Done(AttemptOutcome::Canceled { timed_out: true });
            }
            let message = match (reason, terminated.and_then(|t| t.message.clone())) {
                (Some(r), Some(m)) => format!("{r}: {m}"),
                (Some(r), None) => r,
                (None, Some(m)) => m,
                (None, None) => match exit_code {
                    Some(c) => format!("the driver exited with {c}"),
                    None => "the driver pod failed".to_string(),
                },
            };
            AttemptPhase::Done(AttemptOutcome::Failed { exit_code, message })
        }
        _ => AttemptPhase::Pending,
    }
}

#[async_trait]
impl<F: ContextFactory> Runner for K8sRunner<F> {
    async fn start(&self, attempt: &Attempt) -> Result<AttemptHandle, JobsError> {
        let ctx = self.contexts.context(attempt);
        let rendered = render_pod(&attempt.spec, &ctx);
        if let Some(cm) = rendered.config_map {
            let cm: ConfigMap = serde_json::from_value(cm)
                .map_err(|e| JobsError::Backend(format!("config map: {e}")))?;
            let api: Api<ConfigMap> = Api::namespaced(self.client.clone(), &ctx.k8s_namespace);
            match api.create(&PostParams::default(), &cm).await {
                Ok(_) => {}
                // Shared and immutable: one that exists is the same.
                Err(kube::Error::Api(e)) if e.code == 409 => {}
                Err(e) => return Err(kube_err(e)),
            }
        }
        let pod: Pod = serde_json::from_value(rendered.pod)
            .map_err(|e| JobsError::Backend(format!("pod: {e}")))?;
        let name = ctx.pod_name(&attempt.spec);
        match self
            .pods(&ctx.k8s_namespace)
            .create(&PostParams::default(), &pod)
            .await
        {
            Ok(_) => {}
            // The same attempt started twice (a resumed supervisor): the pod
            // name is the attempt's, so it is the same pod.
            Err(kube::Error::Api(e)) if e.code == 409 => {}
            Err(e) => return Err(kube_err(e)),
        }
        Ok(AttemptHandle {
            id: format!("{}/{name}", ctx.k8s_namespace),
            log: LogLink {
                attempt: attempt.attempt,
                url: format!("/api/v1/namespaces/{}/pods/{name}/log", ctx.k8s_namespace),
            },
        })
    }

    async fn poll(&self, handle: &AttemptHandle) -> Result<AttemptPhase, JobsError> {
        let (ns, name) = split_id(&handle.id)?;
        match self.pods(ns).get_opt(name).await.map_err(kube_err)? {
            Some(pod) => Ok(phase_of(&pod)),
            None => Ok(AttemptPhase::Done(AttemptOutcome::Failed {
                exit_code: None,
                message: "the driver pod is gone".to_string(),
            })),
        }
    }

    async fn cancel(&self, handle: &AttemptHandle) -> Result<(), JobsError> {
        let (ns, name) = split_id(&handle.id)?;
        match self.pods(ns).delete(name, &DeleteParams::default()).await {
            Ok(_) => Ok(()),
            Err(kube::Error::Api(e)) if e.code == 404 => Ok(()),
            Err(e) => Err(kube_err(e)),
        }
    }

    async fn logs(&self, handle: &AttemptHandle) -> Result<String, JobsError> {
        let (ns, name) = split_id(&handle.id)?;
        let lp = LogParams {
            container: Some("driver".to_string()),
            tail_lines: Some(LOG_TAIL_LINES),
            ..LogParams::default()
        };
        self.pods(ns).logs(name, &lp).await.map_err(kube_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pod(json: serde_json::Value) -> Pod {
        serde_json::from_value(json).expect("pod")
    }

    #[test]
    fn phases() {
        assert_eq!(phase_of(&pod(serde_json::json!({}))), AttemptPhase::Pending);
        assert_eq!(
            phase_of(&pod(serde_json::json!({"status": {"phase": "Running"}}))),
            AttemptPhase::Running
        );
        assert_eq!(
            phase_of(&pod(serde_json::json!({"status": {"phase": "Succeeded"}}))),
            AttemptPhase::Done(AttemptOutcome::Succeeded)
        );
        let failed = pod(serde_json::json!({"status": {
            "phase": "Failed",
            "containerStatuses": [{
                "name": "driver", "image": "x", "imageID": "", "ready": false, "restartCount": 0,
                "state": {"terminated": {"exitCode": 1, "reason": "Error"}}
            }]
        }}));
        assert_eq!(
            phase_of(&failed),
            AttemptPhase::Done(AttemptOutcome::Failed {
                exit_code: Some(1),
                message: "Error".into()
            })
        );
        let deadline = pod(serde_json::json!({"status": {
            "phase": "Failed", "reason": "DeadlineExceeded"
        }}));
        assert_eq!(
            phase_of(&deadline),
            AttemptPhase::Done(AttemptOutcome::Canceled { timed_out: true })
        );
    }

    #[test]
    fn rendered_pods_deserialize_as_pods() {
        use crate::jobs::RunId;
        use crate::jobs::spec::{SparkApp, SparkJobSpec};
        let spec = SparkJobSpec::new(
            "etl",
            SparkApp::Jar {
                file: "s3://b/a.jar".into(),
                main_class: "Main".into(),
                jars: vec![],
            },
        );
        let ctx = RenderContext::new(1, RunId::new(), 1);
        let out = render_pod(&spec, &ctx);
        let p: Pod = serde_json::from_value(out.pod).expect("a valid Pod");
        assert_eq!(
            p.spec.expect("spec").restart_policy.as_deref(),
            Some("Never")
        );
    }
}
