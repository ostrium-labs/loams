//! The Kubernetes resolver (platform design 01 MB2, design §63 §21, ME1
//! Task 30): the node's pods, watched from the API server, map a pod
//! cgroup's UID ([`crate::cgroup::pod_uid_from_path`]) to the tenant in the
//! pod's labels.
//!
//! | Label | Meaning |
//! |---|---|
//! | `loams.dev/org` | The org billed (required) |
//! | `loams.dev/namespace` | The Loams namespace (required) |
//! | `loams.dev/function` | The function, when the pod runs one |
//! | `loams.dev/tier` | `t0`, `t1`, `t2` or `t3` |
//!
//! A pod without the two required labels is not a tenant's: its usage is
//! metered under `_platform`. A deleted pod stays resolvable for a
//! retention window (10 minutes by default), because its cgroup's final
//! read can come after the API server has already deleted the pod.
//!
//! [`PodIndex`] is sans-I/O; [`watch`] keeps one up to date
//! from a list and a watch on `spec.nodeName=<node>`.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use crate::cgroup::{CgroupSample, pod_uid_from_path};
use crate::event::{Attribution, Tier};
use crate::resolver::TenantResolver;

/// The org label.
pub const ORG_LABEL: &str = "loams.dev/org";
/// The namespace label.
pub const NAMESPACE_LABEL: &str = "loams.dev/namespace";
/// The function label.
pub const FUNCTION_LABEL: &str = "loams.dev/function";
/// The tier label.
pub const TIER_LABEL: &str = "loams.dev/tier";
/// How long a deleted pod stays resolvable, in milliseconds.
pub const DEFAULT_RETENTION_MS: i64 = 10 * 60 * 1000;

/// The tenant in a pod's labels, if it has one.
pub fn attribution_from_labels(labels: &BTreeMap<String, String>) -> Option<Attribution> {
    let org = labels.get(ORG_LABEL).filter(|v| !v.is_empty())?;
    let namespace = labels.get(NAMESPACE_LABEL).filter(|v| !v.is_empty())?;
    let tier = labels.get(TIER_LABEL).and_then(|t| match t.as_str() {
        "t0" => Some(Tier::T0),
        "t1" => Some(Tier::T1),
        "t2" => Some(Tier::T2),
        "t3" => Some(Tier::T3),
        _ => None,
    });
    Some(Attribution {
        org: org.clone(),
        namespace: namespace.clone(),
        function: labels
            .get(FUNCTION_LABEL)
            .filter(|v| !v.is_empty())
            .cloned(),
        tier,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    attribution: Option<Attribution>,
    deleted_at_ms: Option<i64>,
}

/// The node's pods by UID.
#[derive(Debug, Clone, Default)]
pub struct PodIndex {
    pods: BTreeMap<String, Entry>,
    retention_ms: i64,
}

impl PodIndex {
    /// An empty index with the default retention.
    pub fn new() -> Self {
        Self::with_retention(DEFAULT_RETENTION_MS)
    }

    /// An empty index keeping deleted pods for `retention_ms`.
    pub fn with_retention(retention_ms: i64) -> Self {
        Self {
            pods: BTreeMap::new(),
            retention_ms,
        }
    }

    /// A pod was added or changed.
    pub fn upsert(&mut self, uid: &str, labels: &BTreeMap<String, String>) {
        self.pods.insert(
            uid.to_string(),
            Entry {
                attribution: attribution_from_labels(labels),
                deleted_at_ms: None,
            },
        );
    }

    /// A pod was deleted at `now_ms`; it stays resolvable for the retention.
    pub fn delete(&mut self, uid: &str, now_ms: i64) {
        if let Some(entry) = self.pods.get_mut(uid) {
            entry.deleted_at_ms.get_or_insert(now_ms);
        }
    }

    /// A full list replaced the index at `now_ms`: pods missing from it are
    /// treated as deleted then.
    pub fn replace(&mut self, pods: &[(String, BTreeMap<String, String>)], now_ms: i64) {
        let listed: std::collections::BTreeSet<&str> =
            pods.iter().map(|(u, _)| u.as_str()).collect();
        let gone: Vec<String> = self
            .pods
            .keys()
            .filter(|u| !listed.contains(u.as_str()))
            .cloned()
            .collect();
        for uid in gone {
            self.delete(&uid, now_ms);
        }
        for (uid, labels) in pods {
            self.upsert(uid, labels);
        }
    }

    /// Forgets pods deleted more than the retention before `now_ms`.
    pub fn expire(&mut self, now_ms: i64) {
        let retention = self.retention_ms;
        self.pods
            .retain(|_, e| e.deleted_at_ms.is_none_or(|at| now_ms - at <= retention));
    }

    /// The tenant of pod `uid`; `None` if unknown or not a tenant's.
    pub fn get(&self, uid: &str) -> Option<&Attribution> {
        self.pods.get(uid)?.attribution.as_ref()
    }

    /// Pods known, including deleted ones still retained.
    pub fn len(&self) -> usize {
        self.pods.len()
    }

    /// Whether no pod is known.
    pub fn is_empty(&self) -> bool {
        self.pods.is_empty()
    }
}

/// Resolves pod cgroups from a shared [`PodIndex`], and anything else (the
/// T0 slices, the T1 host, pods the index does not know) from a fallback
/// resolver.
#[derive(Debug, Clone)]
pub struct PodResolver<F> {
    index: Arc<RwLock<PodIndex>>,
    fallback: F,
}

impl<F: TenantResolver> PodResolver<F> {
    /// A resolver over `index`, falling back to `fallback`.
    pub fn new(index: Arc<RwLock<PodIndex>>, fallback: F) -> Self {
        Self { index, fallback }
    }

    /// The shared index.
    pub fn index(&self) -> &Arc<RwLock<PodIndex>> {
        &self.index
    }
}

impl<F: TenantResolver> TenantResolver for PodResolver<F> {
    fn resolve(&self, sample: &CgroupSample) -> Option<Attribution> {
        // A pod the index knows as a tenant's wins; anything else (the T0
        // slices, the T1 host, a pod with static rules) uses the fallback.
        let from_pod = pod_uid_from_path(&sample.path).and_then(|uid| {
            let index = self.index.read().unwrap_or_else(|p| p.into_inner());
            index.get(&uid).cloned()
        });
        from_pod.or_else(|| self.fallback.resolve(sample))
    }
}

/// Keeps `index` up to date with the pods on `node`: a list, then a watch
/// from its resource version, and a fresh list whenever the watch fails or
/// the version expires. Runs until the task is dropped.
pub async fn watch(client: ::kube::Client, node: String, index: Arc<RwLock<PodIndex>>) {
    use ::kube::api::{Api, ListParams, WatchEvent, WatchParams};
    use futures::StreamExt;
    use k8s_openapi::api::core::v1::Pod;
    use tracing::{debug, warn};

    let pods: Api<Pod> = Api::all(client);
    let selector = format!("spec.nodeName={node}");
    let labels_of = |pod: &Pod| pod.metadata.labels.clone().unwrap_or_default();
    let mut backoff = std::time::Duration::from_millis(500);
    loop {
        let list = match pods.list(&ListParams::default().fields(&selector)).await {
            Ok(list) => list,
            Err(e) => {
                warn!(error = %e, "listing the node's pods failed; retrying");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(std::time::Duration::from_secs(30));
                continue;
            }
        };
        backoff = std::time::Duration::from_millis(500);
        let listed: Vec<(String, BTreeMap<String, String>)> = list
            .items
            .iter()
            .filter_map(|p| Some((p.metadata.uid.clone()?, labels_of(p))))
            .collect();
        {
            let mut index = index.write().unwrap_or_else(|p| p.into_inner());
            index.replace(&listed, now_ms());
            index.expire(now_ms());
        }
        let mut version = list.metadata.resource_version.unwrap_or_default();
        'watch: loop {
            let params = WatchParams::default().fields(&selector).timeout(290);
            let stream = match pods.watch(&params, &version).await {
                Ok(stream) => stream,
                Err(e) => {
                    warn!(error = %e, "watching the node's pods failed; listing again");
                    break 'watch;
                }
            };
            let mut stream = std::pin::pin!(stream);
            while let Some(event) = stream.next().await {
                let event = match event {
                    Ok(event) => event,
                    Err(e) => {
                        debug!(error = %e, "pod watch ended");
                        continue 'watch;
                    }
                };
                let mut index = index.write().unwrap_or_else(|p| p.into_inner());
                match event {
                    WatchEvent::Added(pod) | WatchEvent::Modified(pod) => {
                        if let Some(uid) = pod.metadata.uid.as_deref() {
                            index.upsert(uid, &labels_of(&pod));
                        }
                        if let Some(v) = pod.metadata.resource_version {
                            version = v;
                        }
                    }
                    WatchEvent::Deleted(pod) => {
                        if let Some(uid) = pod.metadata.uid.as_deref() {
                            index.delete(uid, now_ms());
                        }
                        if let Some(v) = pod.metadata.resource_version {
                            version = v;
                        }
                    }
                    WatchEvent::Bookmark(b) => version = b.metadata.resource_version,
                    WatchEvent::Error(status) => {
                        // 410 Gone: the version expired; list again.
                        debug!(status = ?status, "pod watch error; listing again");
                        break 'watch;
                    }
                }
                index.expire(now_ms());
            }
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolver::StaticResolver;

    fn labels(org: &str) -> BTreeMap<String, String> {
        BTreeMap::from([
            (ORG_LABEL.to_string(), org.to_string()),
            (NAMESPACE_LABEL.to_string(), "prod".to_string()),
            (TIER_LABEL.to_string(), "t2".to_string()),
            (FUNCTION_LABEL.to_string(), "chat".to_string()),
        ])
    }

    fn sample(path: &str) -> CgroupSample {
        CgroupSample {
            path: path.into(),
            inode: 1,
            cpu_usec: 0,
            memory_current: None,
            memory_peak: None,
            populated: true,
        }
    }

    const UID: &str = "0e8f2a1c-3b4d-4e5f-8a9b-0c1d2e3f4a5b";
    const POD: &str = "kubepods.slice/kubepods-burstable.slice/\
                       kubepods-burstable-pod0e8f2a1c_3b4d_4e5f_8a9b_0c1d2e3f4a5b.slice";

    #[test]
    fn labels_make_the_attribution() {
        let a = attribution_from_labels(&labels("org_a")).unwrap();
        assert_eq!(a.org, "org_a");
        assert_eq!(a.namespace, "prod");
        assert_eq!(a.tier, Some(Tier::T2));
        assert_eq!(a.function.as_deref(), Some("chat"));
        let mut partial = labels("org_a");
        partial.remove(NAMESPACE_LABEL);
        assert!(attribution_from_labels(&partial).is_none());
    }

    #[test]
    fn deleted_pod_resolves_until_retention_ends() {
        let index = Arc::new(RwLock::new(PodIndex::with_retention(1_000)));
        let resolver = PodResolver::new(index.clone(), StaticResolver::default());
        index.write().unwrap().upsert(UID, &labels("org_a"));
        assert_eq!(resolver.resolve(&sample(POD)).unwrap().org, "org_a");
        index.write().unwrap().delete(UID, 5_000);
        index.write().unwrap().expire(5_900);
        assert_eq!(resolver.resolve(&sample(POD)).unwrap().org, "org_a");
        index.write().unwrap().expire(6_001);
        assert!(resolver.resolve(&sample(POD)).is_none());
    }

    #[test]
    fn relist_marks_missing_pods_deleted() {
        let mut index = PodIndex::with_retention(1_000);
        index.upsert(UID, &labels("org_a"));
        index.replace(&[("other".to_string(), labels("org_b"))], 10_000);
        assert!(index.get(UID).is_some(), "retained after the relist");
        index.expire(11_001);
        assert!(index.get(UID).is_none());
        assert_eq!(index.get("other").unwrap().org, "org_b");
    }

    #[test]
    fn non_pod_cgroups_use_the_fallback() {
        let index = Arc::new(RwLock::new(PodIndex::new()));
        let fallback = StaticResolver::from_json(
            r#"{"rules": [{"prefix": "loams.slice/tenant-org_t.slice", "org": "org_t", "namespace": "dev"}]}"#,
        )
        .unwrap();
        let resolver = PodResolver::new(index, fallback);
        assert_eq!(
            resolver
                .resolve(&sample("loams.slice/tenant-org_t.slice"))
                .unwrap()
                .org,
            "org_t"
        );
        assert!(resolver.resolve(&sample(POD)).is_none(), "unknown pod");
    }
}
