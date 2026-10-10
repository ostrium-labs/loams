//! The idempotency ledger (PG2 Task 5; Task 0 ruling 3: no existing store
//! fits). `(principal, rpc, idempotency_key)` maps to the first answer for
//! [`LEDGER_TTL`], in [`IdempotencyRec`]s at `I/<digest>`.
//!
//! The entry is written in the same transaction as the records the call
//! wrote ([`Batch::put_derived`], so it holds the versions that transaction
//! assigned). So an entry exists exactly when the call's writes applied: a
//! call that met `Undetermined` or a conflict looks here, and a retry finds
//! the first answer here, never `already_exists` (R3.14). A key reused for
//! another request (another fingerprint) is `invalid_argument`, as
//! `connect_idempotency.rs` rules. An empty key is never recorded.
//!
//! **Answers are format-tagged JSON** ([`ANSWER_JSON`] then the JSON
//! body), not postcard: JSON names its fields, so a record that gains a
//! field (with `serde(default)`) still decodes the answers recorded before
//! it, for the 24 h they live. A change JSON cannot absorb takes a new tag,
//! with a decoder for the old one kept for 24 h; an entry under a tag this
//! build does not know answers `failed_precondition` (use a new key), never
//! a wrong answer.
//!
//! Nothing here may hold a secret (R1.10): Task 5's answers carry none, and
//! the secret-issuing RPCs (Task 6) record only that a secret was issued.

use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};

use super::{Caller, ServiceError};
use crate::model::{AllIdempotency, IdempotencyKey, IdempotencyRec, Record};
use crate::store::{Batch, KvControlStore, MAX_PAGE_SIZE, Page, PgControlStore, StoreError};

/// The tag of a JSON answer.
pub const ANSWER_JSON: u8 = 1;

/// How long a replay answers the first response (AP0; the proto header).
pub const LEDGER_TTL: Duration = Duration::from_secs(24 * 3600);

/// The durable idempotency ledger on the control store.
#[derive(Debug, Clone)]
pub struct IdempotencyLedger {
    store: KvControlStore,
}

/// A fresh call's place in the ledger: what its entry will say, and the
/// version of the expired entry it replaces.
#[derive(Debug, Clone)]
pub(crate) struct Claim {
    principal: String,
    rpc: &'static str,
    key: String,
    fingerprint: [u8; 32],
    expected: Option<u64>,
}

/// A replay's first answer, or a fresh call's claim (`None`: no key).
pub(crate) enum Begin<T> {
    Replay(T),
    Fresh(Option<Claim>),
}

/// What a lookup found.
enum Found<T> {
    Absent,
    Expired(u64),
    Answer(T),
}

impl IdempotencyLedger {
    pub fn new(store: KvControlStore) -> Self {
        IdempotencyLedger { store }
    }

    /// The first answer to this call, if the ledger holds one; otherwise the
    /// claim the call's batch records.
    pub(crate) async fn begin<T: DeserializeOwned, Q: Serialize>(
        &self,
        caller: &Caller,
        rpc: &'static str,
        key: &str,
        request: &Q,
        now_ms: u64,
    ) -> Result<Begin<T>, ServiceError> {
        if key.is_empty() {
            return Ok(Begin::Fresh(None));
        }
        let fingerprint = fingerprint(rpc, request)?;
        let mut claim = Claim {
            principal: caller.principal.clone(),
            rpc,
            key: key.to_string(),
            fingerprint,
            expected: None,
        };
        match self.lookup(&claim, now_ms).await? {
            Found::Answer(first) => Ok(Begin::Replay(first)),
            Found::Expired(version) => {
                claim.expected = Some(version);
                Ok(Begin::Fresh(Some(claim)))
            }
            Found::Absent => Ok(Begin::Fresh(Some(claim))),
        }
    }

    /// The answer recorded under `claim`'s key since it was made: after a
    /// conflict or an unknown outcome.
    pub(crate) async fn find<T: DeserializeOwned>(
        &self,
        claim: &Claim,
        now_ms: u64,
    ) -> Result<Option<T>, ServiceError> {
        Ok(match self.lookup(claim, now_ms).await? {
            Found::Answer(first) => Some(first),
            Found::Absent | Found::Expired(_) => None,
        })
    }

    async fn lookup<T: DeserializeOwned>(
        &self,
        claim: &Claim,
        now_ms: u64,
    ) -> Result<Found<T>, ServiceError> {
        let id = IdempotencyKey::of(&claim.principal, claim.rpc, &claim.key);
        let Some(entry) = self.store.get::<IdempotencyRec>(&id).await? else {
            return Ok(Found::Absent);
        };
        let rec = entry.record;
        if rec.expires_at_ms <= now_ms {
            return Ok(Found::Expired(entry.version));
        }
        if rec.principal != claim.principal
            || rec.rpc != claim.rpc
            || rec.key != claim.key
            || rec.fingerprint != claim.fingerprint
        {
            return Err(ServiceError::invalid(
                "idempotency_key",
                "this idempotency_key was used for another request in the last 24 h",
            ));
        }
        decode_answer(&rec.answer).map(Found::Answer)
    }

    /// Adds `claim`'s entry to `batch`, its answer made (in the
    /// transaction) from the outcomes of the operations before it.
    pub(crate) fn record<T: Serialize>(
        &self,
        batch: &mut Batch,
        claim: &Claim,
        now_ms: u64,
        answer: impl Fn(&[Option<u64>]) -> T + Send + Sync + 'static,
    ) -> Result<usize, ServiceError> {
        let id = IdempotencyKey::of(&claim.principal, claim.rpc, &claim.key);
        let claim = claim.clone();
        let ttl_ms = u64::try_from(LEDGER_TTL.as_millis()).unwrap_or(u64::MAX);
        Ok(
            batch.put_derived::<IdempotencyRec>(&id, claim.expected, move |out| {
                IdempotencyRec {
                    principal: claim.principal.clone(),
                    rpc: claim.rpc.to_string(),
                    key: claim.key.clone(),
                    fingerprint: claim.fingerprint,
                    answer: encode_answer(&answer(out)),
                    created_at_ms: now_ms,
                    expires_at_ms: now_ms.saturating_add(ttl_ms),
                }
            })?,
        )
    }

    /// Deletes every entry expired at `now_ms`; returns how many. Run it
    /// periodically (Task 9 wires the timer); an expired entry that is not
    /// pruned yet is already ignored.
    ///
    /// # Errors
    ///
    /// The store's, on a read or a delete that failed.
    pub async fn prune(&self, now_ms: u64) -> Result<usize, StoreError> {
        let writer = self.store.api_writer();
        let mut pruned = 0;
        let mut page = Page::first(MAX_PAGE_SIZE);
        loop {
            let (entries, next) = self
                .store
                .list::<IdempotencyRec>(&AllIdempotency, page)
                .await?;
            for entry in entries {
                if entry.record.expires_at_ms > now_ms {
                    continue;
                }
                match writer
                    .delete::<IdempotencyRec>(&entry.record.key(), entry.version)
                    .await
                {
                    Ok(()) => pruned += 1,
                    // Replaced or pruned meanwhile.
                    Err(StoreError::Conflict { .. } | StoreError::NotFound) => {}
                    Err(e) => return Err(e),
                }
            }
            match next {
                Some(token) => page = Page::after(MAX_PAGE_SIZE, token),
                None => return Ok(pruned),
            }
        }
    }
}

/// [`ANSWER_JSON`] and the answer's JSON. An answer that does not encode
/// is recorded as the bare tag, which a replay reports as corrupt; the
/// service's answers always encode.
fn encode_answer<T: Serialize>(answer: &T) -> Vec<u8> {
    let mut out = vec![ANSWER_JSON];
    if let Err(e) = serde_json::to_writer(&mut out, answer) {
        tracing::error!(error = %e, "an idempotency answer does not encode");
    }
    out
}

fn decode_answer<T: DeserializeOwned>(answer: &[u8]) -> Result<T, ServiceError> {
    match answer.split_first() {
        Some((&ANSWER_JSON, body)) => serde_json::from_slice(body).map_err(|e| {
            StoreError::Corrupt(format!("an idempotency answer does not decode: {e}")).into()
        }),
        _ => Err(ServiceError::failed_precondition(
            "the first answer under this idempotency_key was recorded in a format this \
             server does not read; retry with a new key",
        )),
    }
}

/// SHA-256 of the RPC's name and the request (its key included: the key is
/// the same for every request it is compared with).
fn fingerprint<Q: Serialize>(rpc: &str, request: &Q) -> Result<[u8; 32], ServiceError> {
    let body = postcard::to_stdvec(request).map_err(|e| {
        tracing::error!(error = %e, rpc, "a request does not encode");
        ServiceError::new(super::Reason::Internal, "internal error")
    })?;
    let mut h = Sha256::new();
    h.update(rpc.as_bytes());
    h.update([0]);
    h.update(&body);
    Ok(h.finalize().into())
}
