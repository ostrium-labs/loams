//! Namespace updates and the worker-versioning read methods (TMP-api group
//! E, D2088).
//!
//! The gateway routes tasks by task queue only. It records the build ID
//! each worker reports, but has no version sets or assignment rules. So
//! the versioning reads answer "no sets, no rules, every build reachable",
//! which keeps an SDK that asks from erroring. The versioning writes stay
//! `UNIMPLEMENTED`.

use loams_temporal_history::time::dur_ms;
use loams_temporal_proto::temporal::api::enums::v1::{NamespaceState, TaskReachability};
use loams_temporal_proto::temporal::api::taskqueue::v1::{
    BuildIdReachability, TaskQueueReachability,
};
use loams_temporal_proto::temporal::api::workflowservice::v1::{
    DeprecateNamespaceRequest, DeprecateNamespaceResponse, GetWorkerTaskReachabilityRequest,
    GetWorkerTaskReachabilityResponse, UpdateNamespaceRequest, UpdateNamespaceResponse,
};
use loams_temporal_store::NamespaceRecord;
use tonic::Status;

use crate::auth::Principal;
use crate::engine::store_status;
use crate::service::Frontend;

/// Applies an `UpdateNamespace` request's fields to `ns`.
pub(crate) fn apply_update(ns: &mut NamespaceRecord, r: &UpdateNamespaceRequest) {
    if !ns.config.is_object() {
        ns.config = serde_json::json!({});
    }
    if let Some(info) = &r.update_info {
        if !info.description.is_empty() {
            ns.config["description"] = info.description.clone().into();
        }
        if !info.owner_email.is_empty() {
            ns.config["owner_email"] = info.owner_email.clone().into();
        }
        if !info.data.is_empty() {
            let data = ns.config["data"].as_object().cloned().unwrap_or_default();
            let mut data = data;
            for (k, v) in &info.data {
                data.insert(k.clone(), v.clone().into());
            }
            ns.config["data"] = serde_json::Value::Object(data);
        }
        if info.state != 0 {
            ns.state = info.state;
        }
    }
    if let Some(c) = &r.config
        && let Some(ttl) = &c.workflow_execution_retention_ttl
    {
        let secs = dur_ms(Some(ttl)) / 1000;
        if secs > 0 {
            ns.retention_secs = secs;
        }
    }
}

/// `GetWorkerTaskReachability`: every build ID can reach new and existing
/// workflows on every queue asked about (no versioning rules exist).
pub(crate) fn reachability(
    r: &GetWorkerTaskReachabilityRequest,
) -> GetWorkerTaskReachabilityResponse {
    let kinds = match TaskReachability::try_from(r.reachability).unwrap_or_default() {
        TaskReachability::Unspecified => vec![
            TaskReachability::NewWorkflows as i32,
            TaskReachability::ExistingWorkflows as i32,
        ],
        k => vec![k as i32],
    };
    GetWorkerTaskReachabilityResponse {
        build_id_reachability: r
            .build_ids
            .iter()
            .map(|b| BuildIdReachability {
                build_id: b.clone(),
                task_queue_reachability: r
                    .task_queues
                    .iter()
                    .map(|q| TaskQueueReachability {
                        task_queue: q.clone(),
                        reachability: kinds.clone(),
                    })
                    .collect(),
            })
            .collect(),
    }
}

impl Frontend {
    async fn admin_namespace(&self, p: &Principal, name: &str) -> Result<NamespaceRecord, Status> {
        if !p.admin {
            return Err(Status::permission_denied(
                "changing a namespace needs the admin role",
            ));
        }
        self.namespace(p, name).await
    }

    async fn save_namespace(&self, ns: &NamespaceRecord) -> Result<(), Status> {
        self.store()
            .update_namespace(ns.clone())
            .await
            .map_err(store_status)?;
        self.forget_namespace(&ns.namespace_id);
        Ok(())
    }

    /// `UpdateNamespace` (admin only).
    pub(crate) async fn update_namespace(
        &self,
        p: &Principal,
        r: UpdateNamespaceRequest,
    ) -> Result<UpdateNamespaceResponse, Status> {
        let mut ns = self.admin_namespace(p, &r.namespace).await?;
        apply_update(&mut ns, &r);
        self.save_namespace(&ns).await?;
        let d = Self::describe_namespace_response(&ns);
        Ok(UpdateNamespaceResponse {
            namespace_info: d.namespace_info,
            config: d.config,
            ..Default::default()
        })
    }

    /// `DeprecateNamespace` (admin only; deprecated upstream too).
    pub(crate) async fn deprecate_namespace(
        &self,
        p: &Principal,
        r: DeprecateNamespaceRequest,
    ) -> Result<DeprecateNamespaceResponse, Status> {
        let mut ns = self.admin_namespace(p, &r.namespace).await?;
        ns.state = NamespaceState::Deprecated as i32;
        self.save_namespace(&ns).await?;
        Ok(DeprecateNamespaceResponse {})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loams_temporal_proto::temporal::api::namespace::v1::{
        NamespaceConfig, UpdateNamespaceInfo,
    };

    fn ns() -> NamespaceRecord {
        NamespaceRecord {
            namespace_id: "id".into(),
            loams_namespace_id: "l".into(),
            name: "default".into(),
            retention_secs: 3600,
            state: NamespaceState::Registered as i32,
            config: serde_json::json!({ "description": "old" }),
        }
    }

    #[test]
    fn an_update_changes_only_what_it_names() {
        let mut n = ns();
        apply_update(
            &mut n,
            &UpdateNamespaceRequest {
                namespace: "default".into(),
                update_info: Some(UpdateNamespaceInfo {
                    owner_email: "ops@ostriumlabs.org".into(),
                    data: [("team".to_string(), "durable".to_string())].into(),
                    ..Default::default()
                }),
                config: Some(NamespaceConfig {
                    workflow_execution_retention_ttl: Some(prost_types::Duration {
                        seconds: 86_400,
                        nanos: 0,
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        assert_eq!(n.config["description"], "old");
        assert_eq!(n.config["owner_email"], "ops@ostriumlabs.org");
        assert_eq!(n.config["data"]["team"], "durable");
        assert_eq!(n.retention_secs, 86_400);
        assert_eq!(n.state, NamespaceState::Registered as i32);
    }

    #[test]
    fn every_build_id_is_reachable() {
        let r = reachability(&GetWorkerTaskReachabilityRequest {
            build_ids: vec!["b1".into(), "b2".into()],
            task_queues: vec!["q".into()],
            ..Default::default()
        });
        assert_eq!(r.build_id_reachability.len(), 2);
        assert_eq!(
            r.build_id_reachability[0].task_queue_reachability[0].reachability,
            [
                TaskReachability::NewWorkflows as i32,
                TaskReachability::ExistingWorkflows as i32
            ]
        );
    }
}
