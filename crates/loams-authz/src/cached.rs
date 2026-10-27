//! [`CachedAuthorizer`]: a short-TTL cache of allow decisions in front of any authorizer
//! (D66, D1611).
//!
//! - Only allows are cached, so a new grant is seen at once and a revocation within the TTL.
//! - The TTL is clamped to [`MAX_CACHE_TTL`] (5 s). Run OpenFGA's own check cache off, or the
//!   two TTLs stack (IA0 report, F6).
//! - `HigherConsistency` requests, and requests with condition context or contextual tuples,
//!   bypass the cache in both directions.
//! - Each allow remembers the model id that decided it; when the backend answers with another
//!   model, every cached allow is dropped, so a model change never serves an old answer.

use std::time::Duration;

use async_trait::async_trait;
use moka::future::Cache;

use crate::{Authorizer, CheckRequest, Decision, ObjectRef, Result, Subject};

/// The longest an allow may be served from the cache (D66).
pub const MAX_CACHE_TTL: Duration = Duration::from_secs(5);

/// `(user, relation, object)`; the value is the model id that allowed it.
type Key = (String, String, String);

pub struct CachedAuthorizer<A> {
    inner: A,
    allows: Cache<Key, Option<String>>,
    /// The model id the backend answered with last. An allow from another model is never
    /// served: when the model changes, the cache is dropped (D1611's key includes the model).
    model: std::sync::Mutex<Option<String>>,
    ttl: Duration,
}

impl<A> std::fmt::Debug for CachedAuthorizer<A> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedAuthorizer")
            .field("ttl", &self.ttl)
            .field("entries", &self.allows.entry_count())
            .finish_non_exhaustive()
    }
}

impl<A: Authorizer> CachedAuthorizer<A> {
    /// `ttl` is clamped to [`MAX_CACHE_TTL`]; `max_entries` bounds memory.
    pub fn new(inner: A, ttl: Duration, max_entries: u64) -> Self {
        let ttl = ttl.min(MAX_CACHE_TTL);
        Self {
            inner,
            allows: Cache::builder()
                .max_capacity(max_entries)
                .time_to_live(ttl)
                .build(),
            model: std::sync::Mutex::new(None),
            ttl,
        }
    }

    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    pub fn inner(&self) -> &A {
        &self.inner
    }

    /// Drops one cached allow (IA3's `ReadChanges` feed calls this for a deleted tuple).
    pub async fn invalidate(&self, subject: &Subject, relation: &str, object: &ObjectRef) {
        self.allows
            .invalidate(&key(&subject.fga_user(), relation, object))
            .await;
    }

    pub fn invalidate_all(&self) {
        self.allows.invalidate_all();
    }

    fn current_model(&self) -> Option<String> {
        self.model
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Records the model that answered; a different one drops every cached allow.
    fn observe_model(&self, model: &Option<String>) {
        let mut cur = self
            .model
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *cur != *model {
            *cur = model.clone();
            self.allows.invalidate_all();
        }
    }
}

fn key(user: &str, relation: &str, object: &ObjectRef) -> Key {
    (user.to_owned(), relation.to_owned(), object.to_string())
}

#[async_trait]
impl<A: Authorizer> Authorizer for CachedAuthorizer<A> {
    async fn check(&self, req: CheckRequest) -> Result<Decision> {
        if !req.cacheable() {
            let d = self.inner.check(req).await?;
            self.observe_model(&d.model_id);
            return Ok(d);
        }
        let k = key(&req.subject.fga_user(), &req.relation, &req.object);
        if let Some(model_id) = self.allows.get(&k).await
            && model_id == self.current_model()
        {
            return Ok(Decision {
                allowed: true,
                model_id,
                cached: true,
            });
        }
        let decision = self.inner.check(req).await?;
        self.observe_model(&decision.model_id);
        if decision.allowed {
            self.allows.insert(k, decision.model_id.clone()).await;
        }
        Ok(decision)
    }

    async fn batch_check(&self, reqs: Vec<CheckRequest>) -> Result<Vec<Decision>> {
        futures::future::try_join_all(reqs.into_iter().map(|r| self.check(r))).await
    }

    async fn list_objects(
        &self,
        subject: &Subject,
        relation: &str,
        ty: &str,
    ) -> Result<Vec<ObjectRef>> {
        self.inner.list_objects(subject, relation, ty).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::*;
    use crate::Consistency;

    /// Allows while `allow` is true, counting calls.
    #[derive(Default)]
    struct Switch {
        allow: AtomicBool,
        calls: AtomicUsize,
    }

    #[async_trait]
    impl Authorizer for Switch {
        async fn check(&self, _req: CheckRequest) -> Result<Decision> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(Decision {
                allowed: self.allow.load(Ordering::SeqCst),
                model_id: Some("m1".into()),
                cached: false,
            })
        }

        async fn list_objects(&self, _: &Subject, _: &str, _: &str) -> Result<Vec<ObjectRef>> {
            Ok(Vec::new())
        }
    }

    fn req() -> CheckRequest {
        CheckRequest::new(
            Subject::agent("a1"),
            "can_view",
            ObjectRef::new("deployment", "d1"),
        )
    }

    #[tokio::test]
    async fn cached_allows_only() {
        let inner = Arc::new(Switch::default());
        let c = CachedAuthorizer::new(inner.clone(), Duration::from_secs(5), 1000);
        // A deny is not cached: the grant that follows is seen at once.
        assert!(!c.check(req()).await.unwrap().allowed);
        inner.allow.store(true, Ordering::SeqCst);
        let d = c.check(req()).await.unwrap();
        assert!(d.allowed && !d.cached);
        // The allow is now served from the cache, with its model id, even after revocation.
        inner.allow.store(false, Ordering::SeqCst);
        let d = c.check(req()).await.unwrap();
        assert!(d.allowed && d.cached);
        assert_eq!(d.model_id.as_deref(), Some("m1"));
        assert_eq!(inner.calls.load(Ordering::SeqCst), 2);
        c.invalidate(
            &Subject::agent("a1"),
            "can_view",
            &ObjectRef::new("deployment", "d1"),
        )
        .await;
        assert!(
            !c.check(req()).await.unwrap().allowed,
            "invalidated allow is re-checked"
        );
    }

    /// Allows with whatever model id it holds.
    struct Model(std::sync::Mutex<&'static str>);

    #[async_trait]
    impl Authorizer for Model {
        async fn check(&self, _req: CheckRequest) -> Result<Decision> {
            Ok(Decision::allow(Some((*self.0.lock().unwrap()).to_owned())))
        }

        async fn list_objects(&self, _: &Subject, _: &str, _: &str) -> Result<Vec<ObjectRef>> {
            Ok(Vec::new())
        }
    }

    #[tokio::test]
    async fn model_change_drops_cached_allows() {
        let inner = Arc::new(Model(std::sync::Mutex::new("m1")));
        let c = CachedAuthorizer::new(inner.clone(), Duration::from_secs(5), 10);
        let other = || {
            CheckRequest::new(
                Subject::user("u"),
                "can_view",
                ObjectRef::new("deployment", "d2"),
            )
        };
        assert!(!c.check(req()).await.unwrap().cached);
        assert!(c.check(req()).await.unwrap().cached);
        // The backend now answers with model m2 (a model upgrade): every m1 allow is dropped.
        *inner.0.lock().unwrap() = "m2";
        assert_eq!(
            c.check(other()).await.unwrap().model_id.as_deref(),
            Some("m2")
        );
        let d = c.check(req()).await.unwrap();
        assert!(
            !d.cached && d.model_id.as_deref() == Some("m2"),
            "re-checked under the new model"
        );
    }

    #[tokio::test]
    async fn cache_ttl_is_clamped() {
        let c = CachedAuthorizer::new(Switch::default(), Duration::from_secs(60), 10);
        assert_eq!(c.ttl(), MAX_CACHE_TTL);
        let c = CachedAuthorizer::new(Switch::default(), Duration::from_millis(50), 10);
        assert_eq!(c.ttl(), Duration::from_millis(50));
    }

    #[tokio::test]
    async fn cached_allow_expires() {
        let inner = Arc::new(Switch::default());
        inner.allow.store(true, Ordering::SeqCst);
        let c = CachedAuthorizer::new(inner.clone(), Duration::from_millis(100), 10);
        assert!(c.check(req()).await.unwrap().allowed);
        inner.allow.store(false, Ordering::SeqCst);
        assert!(c.check(req()).await.unwrap().cached);
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(
            !c.check(req()).await.unwrap().allowed,
            "denied once the TTL passes"
        );
    }

    #[tokio::test]
    async fn higher_consistency_bypasses_cache() {
        let inner = Arc::new(Switch::default());
        inner.allow.store(true, Ordering::SeqCst);
        let c = CachedAuthorizer::new(inner.clone(), Duration::from_secs(5), 10);
        assert!(c.check(req()).await.unwrap().allowed);
        inner.allow.store(false, Ordering::SeqCst);
        let d = c
            .check(req().consistency(Consistency::HigherConsistency))
            .await
            .unwrap();
        assert!(!d.allowed && !d.cached);
    }

    #[tokio::test]
    async fn context_bypasses_cache() {
        let inner = Arc::new(Switch::default());
        inner.allow.store(true, Ordering::SeqCst);
        let c = CachedAuthorizer::new(inner.clone(), Duration::from_secs(5), 10);
        let with_ctx = || req().context("session_id", "s-1");
        assert!(c.check(with_ctx()).await.unwrap().allowed);
        inner.allow.store(false, Ordering::SeqCst);
        assert!(
            !c.check(with_ctx()).await.unwrap().allowed,
            "a session-scoped check is never cached"
        );
        assert!(
            !c.check(req()).await.unwrap().allowed,
            "and it did not seed the plain key"
        );
    }
}
