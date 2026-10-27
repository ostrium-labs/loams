//! Zitadel's event list and user grants, for the identity sync (design 76 §8.1, D1622, D1623;
//! plan IA1, IA3).
//!
//! - [`Zitadel::list_events`]: the admin API's `ListEvents` (`POST /admin/v1/events/_search`,
//!   permission `events.read`), filtered by event type, ascending from a creation date. It is
//!   the sync's primary feed: unlike an Actions v2 webhook, nothing in it is lost.
//! - [`Zitadel::list_user_grants`]: the management API's `ListUserGrants`
//!   (`POST /management/v1/users/grants/_search`) for one project in one organization, which
//!   the nightly reconciliation compares with OpenFGA.
//!
//! Both are written from Zitadel's public documentation and its Apache-2.0 protos
//! (`proto/zitadel/admin.proto`, `event.proto`, `management.proto`, `user.proto`).
//! grpc-gateway answers 64-bit integers as JSON strings, which [`Event::sequence`] accepts.

use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::{Error, Result, Zitadel, decode};

/// The most event types one `ListEvents` call filters by (`max_items: 30` in the proto).
pub const MAX_EVENT_TYPES: usize = 30;

/// One event of Zitadel's event store.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    /// The event type, such as `user.grant.added`.
    pub kind: String,
    /// The aggregate's type (`user`, `usergrant`, `org`, …).
    pub aggregate_type: String,
    /// The aggregate's id (a user's id for `user.*`, a grant's id for `user.grant.*`).
    pub aggregate_id: String,
    /// The organization that owns the aggregate.
    pub resource_owner: String,
    /// The event's sequence within its aggregate.
    pub sequence: u64,
    pub created_at: OffsetDateTime,
    /// The event's payload (`null` for some events, such as `user.deactivated`).
    pub payload: Value,
}

#[derive(Deserialize)]
struct ListEventsResponse {
    #[serde(default)]
    events: Vec<RawEvent>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawEvent {
    aggregate: RawAggregate,
    #[serde(deserialize_with = "u64_or_string")]
    sequence: u64,
    creation_date: String,
    #[serde(default)]
    payload: Value,
    #[serde(rename = "type")]
    kind: Named,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawAggregate {
    id: String,
    #[serde(rename = "type")]
    kind: Named,
    #[serde(default)]
    resource_owner: String,
}

#[derive(Deserialize)]
struct Named {
    #[serde(rename = "type")]
    name: String,
}

fn u64_or_string<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<u64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum N {
        N(u64),
        S(String),
    }
    match N::deserialize(d)? {
        N::N(n) => Ok(n),
        N::S(s) => s.parse().map_err(serde::de::Error::custom),
    }
}

impl TryFrom<RawEvent> for Event {
    type Error = Error;

    fn try_from(raw: RawEvent) -> Result<Self> {
        let created_at = OffsetDateTime::parse(&raw.creation_date, &Rfc3339)
            .map_err(|e| Error::Decode(format!("creationDate {:?}: {e}", raw.creation_date)))?;
        Ok(Self {
            kind: raw.kind.name,
            aggregate_type: raw.aggregate.kind.name,
            aggregate_id: raw.aggregate.id,
            resource_owner: raw.aggregate.resource_owner,
            sequence: raw.sequence,
            created_at,
            payload: raw.payload,
        })
    }
}

/// A user's grant on a project (`zitadel.user.v1.UserGrant`, the fields the sync reads).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserGrant {
    /// The grant's id: the `usergrant` aggregate's id in the event list.
    pub id: String,
    pub user_id: String,
    #[serde(default)]
    pub org_id: String,
    #[serde(default)]
    pub project_id: String,
    #[serde(default)]
    pub role_keys: Vec<String>,
    /// `USER_GRANT_STATE_ACTIVE` or `USER_GRANT_STATE_INACTIVE`.
    #[serde(default)]
    pub state: String,
}

impl UserGrant {
    /// Whether the grant is in force (an inactive grant confers nothing).
    pub fn active(&self) -> bool {
        self.state != "USER_GRANT_STATE_INACTIVE"
    }
}

#[derive(Deserialize)]
struct ListUserGrantsResponse {
    #[serde(default)]
    result: Vec<UserGrant>,
}

impl Zitadel {
    /// Up to `limit` events of `types` (at most [`MAX_EVENT_TYPES`]), oldest first, created
    /// after `from` (all of them when `None`). Zitadel compares `from` strictly, so a caller
    /// that wants the events at `from` itself again asks from a moment before it.
    ///
    /// # Errors
    ///
    /// Zitadel's refusal (403 without `events.read`), a transport error, or an answer that
    /// does not parse.
    pub async fn list_events(
        &self,
        types: &[&str],
        from: Option<OffsetDateTime>,
        limit: u32,
    ) -> Result<Vec<Event>> {
        if types.is_empty() || types.len() > MAX_EVENT_TYPES {
            return Err(Error::Invalid(format!(
                "ListEvents takes 1 to {MAX_EVENT_TYPES} event types, not {}",
                types.len()
            )));
        }
        let mut body = json!({
            "asc": true,
            "limit": limit,
            "eventTypes": types,
        });
        if let Some(from) = from {
            body["from"] = json!(
                from.format(&Rfc3339)
                    .map_err(|e| Error::Invalid(format!("from: {e}")))?
            );
        }
        let res: ListEventsResponse = decode(
            self.send(reqwest::Method::POST, "admin/v1/events/_search", Some(body))
                .await?,
        )?;
        res.events.into_iter().map(Event::try_from).collect()
    }

    /// The grants on `project_id` in organization `org_id`, `limit` at a time from `offset`.
    ///
    /// # Errors
    ///
    /// Zitadel's refusal, a transport error, or an answer that does not parse.
    pub async fn list_user_grants(
        &self,
        org_id: &str,
        project_id: &str,
        offset: u64,
        limit: u32,
    ) -> Result<Vec<UserGrant>> {
        let body = json!({
            "query": {"offset": offset.to_string(), "limit": limit, "asc": true},
            "queries": [{"projectIdQuery": {"projectId": project_id}}],
        });
        let res: ListUserGrantsResponse = decode(
            self.send_in(
                Some(org_id),
                reqwest::Method::POST,
                "management/v1/users/grants/_search",
                Some(body),
            )
            .await?,
        )?;
        Ok(res.result)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn events_parse_as_grpc_gateway_answers_them() {
        // The shape the admin API answers (sequences as strings, a null payload).
        let res: ListEventsResponse = serde_json::from_value(json!({"events": [
            {"editor": {"userId": "1"}, "aggregate": {"id": "g1", "type": {"type": "usergrant"},
             "resourceOwner": "o1"}, "sequence": "2", "creationDate": "2026-10-26T09:11:38.897485Z",
             "payload": {"roleKeys": ["admin"], "userId": "u1"}, "type": {"type": "user.grant.changed"}},
            {"aggregate": {"id": "u1", "type": {"type": "user"}, "resourceOwner": "o1"},
             "sequence": 3, "creationDate": "2026-10-26T09:11:38Z", "payload": null,
             "type": {"type": "user.deactivated"}}
        ]}))
        .unwrap();
        let events: Vec<Event> = res
            .events
            .into_iter()
            .map(Event::try_from)
            .collect::<Result<_>>()
            .unwrap();
        assert_eq!(events[0].kind, "user.grant.changed");
        assert_eq!(events[0].aggregate_type, "usergrant");
        assert_eq!(events[0].sequence, 2);
        assert_eq!(events[0].created_at.nanosecond(), 897_485_000);
        assert_eq!(events[1].sequence, 3);
        assert!(events[1].payload.is_null());
    }

    #[test]
    fn an_inactive_grant_is_not_in_force() {
        let g: UserGrant = serde_json::from_value(json!({
            "id": "g", "userId": "u", "roleKeys": ["member"], "state": "USER_GRANT_STATE_INACTIVE"
        }))
        .unwrap();
        assert!(!g.active());
    }
}
