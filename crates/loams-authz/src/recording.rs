//! [`RecordingAuthorizer`]: records every request and its outcome, allows and denials alike
//! (tests, and the shape of the decision log, §11).

use std::sync::Mutex;

use async_trait::async_trait;

use crate::{Authorizer, CheckRequest, Decision, ObjectRef, Result, Subject};

#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub request: CheckRequest,
    /// `None` when the backend failed.
    pub decision: Option<Decision>,
}

pub struct RecordingAuthorizer<A> {
    inner: A,
    records: Mutex<Vec<Record>>,
}

impl<A> std::fmt::Debug for RecordingAuthorizer<A> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecordingAuthorizer")
            .finish_non_exhaustive()
    }
}

impl<A: Authorizer> RecordingAuthorizer<A> {
    pub fn new(inner: A) -> Self {
        Self {
            inner,
            records: Mutex::new(Vec::new()),
        }
    }

    pub fn records(&self) -> Vec<Record> {
        self.records
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn denials(&self) -> Vec<Record> {
        self.records()
            .into_iter()
            .filter(|r| r.decision.as_ref().is_none_or(|d| !d.allowed))
            .collect()
    }

    fn push(&self, record: Record) {
        self.records
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(record);
    }
}

#[async_trait]
impl<A: Authorizer> Authorizer for RecordingAuthorizer<A> {
    async fn check(&self, req: CheckRequest) -> Result<Decision> {
        let res = self.inner.check(req.clone()).await;
        self.push(Record {
            request: req,
            decision: res.as_ref().ok().cloned(),
        });
        res
    }

    async fn batch_check(&self, reqs: Vec<CheckRequest>) -> Result<Vec<Decision>> {
        let res = self.inner.batch_check(reqs.clone()).await;
        match &res {
            Ok(ds) => reqs.into_iter().zip(ds).for_each(|(r, d)| {
                self.push(Record {
                    request: r,
                    decision: Some(d.clone()),
                })
            }),
            Err(_) => reqs.into_iter().for_each(|r| {
                self.push(Record {
                    request: r,
                    decision: None,
                })
            }),
        }
        res
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
    use super::*;

    struct OnlyDev;

    #[async_trait]
    impl Authorizer for OnlyDev {
        async fn check(&self, req: CheckRequest) -> Result<Decision> {
            Ok(if req.object.id == "dev" {
                Decision::allow(None)
            } else {
                Decision::deny(None)
            })
        }

        async fn list_objects(&self, _: &Subject, _: &str, _: &str) -> Result<Vec<ObjectRef>> {
            Ok(vec![ObjectRef::new("environment", "dev")])
        }
    }

    #[tokio::test]
    async fn recording_records_denials() {
        let r = RecordingAuthorizer::new(OnlyDev);
        let req = |id: &str| {
            CheckRequest::new(
                Subject::agent("a"),
                "can_view",
                ObjectRef::new("environment", id),
            )
        };
        assert!(r.check(req("dev")).await.unwrap().allowed);
        assert!(!r.check(req("prod")).await.unwrap().allowed);
        let ds = r.batch_check(vec![req("dev"), req("stage")]).await.unwrap();
        assert_eq!(
            ds.iter().map(|d| d.allowed).collect::<Vec<_>>(),
            [true, false]
        );
        assert_eq!(r.records().len(), 4);
        let denied: Vec<String> = r
            .denials()
            .into_iter()
            .map(|d| d.request.object.id)
            .collect();
        assert_eq!(denied, ["prod", "stage"]);
    }
}
