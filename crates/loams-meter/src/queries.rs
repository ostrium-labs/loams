//! `MeterService.QueryMeter` (design §63 §8.5, §17.1; ME1 Task 8): the
//! Connect face of the meter query. The work is
//! [`loams_meter_house::query_meter`], the same function OpenMeter's
//! `GET /api/v1/meters/{slug}/query` runs ([`loams_meter_house::query`]);
//! this module adds the authorization and the meter lookup.
//!
//! Who may query (§63 §17.1): a `meter_admin` (or the operator) may query
//! any subject; anyone else must name the subjects, and must be a
//! `billing_viewer` of the org of each subject's customer. Service
//! identities and subjects with no customer are refused.

use std::sync::Arc;

use loams_meter_core::{Slug, SubjectKey};
use loams_meter_house::query::{MeterQuery, QueryError, query_spec};
use loams_meter_house::sql::QueryAnswer;
use loams_meter_house::{HouseSql, query_meter};

use crate::authz::{Caller, MeterAuthz};
use crate::error::{MeterError, Result};
use crate::store::MeterStore;

/// A query's answer, with what the response echoes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryAnswered {
    pub query: MeterQuery,
    /// The meter dimensions the rows are grouped by, in the rows' order.
    pub group_by: Vec<String>,
    pub answer: QueryAnswer,
}

/// The meter queries, over the meter store and the House.
pub struct MeterQueries<H> {
    store: Arc<dyn MeterStore>,
    authz: MeterAuthz,
    house: Arc<H>,
}

impl<H> std::fmt::Debug for MeterQueries<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeterQueries")
            .field("authz", &self.authz)
            .finish_non_exhaustive()
    }
}

impl<H> Clone for MeterQueries<H> {
    fn clone(&self) -> Self {
        MeterQueries {
            store: self.store.clone(),
            authz: self.authz.clone(),
            house: self.house.clone(),
        }
    }
}

fn from_query_error(e: QueryError) -> MeterError {
    match e {
        QueryError::BadRequest(message) => MeterError::invalid("query", message),
        QueryError::NotFound(slug) => MeterError::not_found("meter", slug),
        QueryError::House(e) => MeterError::Unavailable(format!("House: {e}")),
    }
}

impl<H: HouseSql> MeterQueries<H> {
    pub fn new(store: Arc<dyn MeterStore>, authz: MeterAuthz, house: Arc<H>) -> Self {
        MeterQueries {
            store,
            authz,
            house,
        }
    }

    /// Refuses a caller who may not see every subject `query` names.
    async fn authorize(&self, caller: &Caller, query: &MeterQuery) -> Result<()> {
        if self.authz.is_meter_admin(caller).await? {
            return Ok(());
        }
        if let Caller::Service(id) = caller {
            return Err(MeterError::denied(
                "service_not_allowed",
                format!("service identity {id} may not query meters"),
            ));
        }
        if query.subjects.is_empty() {
            return Err(MeterError::denied(
                "subjects_required",
                "only a meter admin may query every subject: name the subjects",
            ));
        }
        for subject in &query.subjects {
            let key = SubjectKey::new(subject.as_str())
                .map_err(|e| MeterError::invalid("subjects", e.to_string()))?;
            let Some(customer) = self.store.customer_by_subject(&key).await? else {
                // Not found and not allowed look the same to a non-admin.
                return Err(MeterError::denied(
                    "billing_viewer_required",
                    format!("subject {subject} is not one of your orgs' customers"),
                ));
            };
            self.authz
                .require_billing_view(caller, &customer.org)
                .await?;
        }
        Ok(())
    }

    /// `QueryMeter`: `query` on the meter `slug`, for `caller`.
    pub async fn query(
        &self,
        caller: &Caller,
        slug: &str,
        query: MeterQuery,
    ) -> Result<QueryAnswered> {
        if query.from >= query.to {
            return Err(MeterError::invalid("from", "from must be before to"));
        }
        self.authorize(caller, &query).await?;
        let key = Slug::new(slug).map_err(|e| MeterError::invalid("meter", e.to_string()))?;
        let meter = self
            .store
            .meter(&key)
            .await?
            .ok_or_else(|| MeterError::not_found("meter", slug))?;
        let spec = query_spec(&meter, &query).map_err(from_query_error)?;
        let answer = query_meter(&*self.house, &meter, &query)
            .await
            .map_err(from_query_error)?;
        Ok(QueryAnswered {
            query,
            group_by: spec.group_by,
            answer,
        })
    }
}
