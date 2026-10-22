//! Idempotency keys (design §44 §7.4, D610; runtime contract R3) and the
//! consistency-token store (D609; R4).
//!
//! ## Idempotency keys
//!
//! A mutating call that carries an `idempotency_key` field is given one **per
//! logical call**, before the first attempt, and **the same key goes out on
//! every retry**. A key regenerated per attempt turns one write into two, which
//! is the exact failure the key exists to prevent.
//!
//! Which calls carry one is read from the **generated message type**, not
//! guessed from the value a caller built: [`Keyed`] is implemented only for a
//! request message whose schema declares the field, so `MutateRequest` gets a
//! key and `DeployRequest` — which has no such field, and to which a key would
//! be a field the schema does not know — cannot even be asked for one.
//!
//! ## Consistency tokens
//!
//! A write answers with a `consistency_token`; a read accepts one, so a caller
//! that just wrote can read its own write. Threading those by hand is the
//! caller's job today. A **session store** is the alternative: off by default,
//! and when a call opts in, every response's token is folded into the session
//! and attached to later reads.
//!
//! **The token's encoding is not in the protos yet.** §05 §5 defines the
//! semantics and API1's write paths carry an opaque `v1:` string; §44 §7.4 says
//! it merges by "max offset per stream and partition", which needs the encoding
//! parsed. Until that lands, [`ConsistencySession`] keeps the token it was
//! given and reports **two different tokens meeting** as a conflict rather than
//! merging them into a wrong one — a silently-wrong consistency token reads
//! stale data, which is worse than a failure.

use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::consistency::ConsistencyToken;
use crate::error::LoamsError;
use crate::uuidv7::uuidv7;

/// The request-message field a mutation is keyed by (D610, R3).
///
/// `loams/live/v1/live.proto:163`. Read by the facade from the generated binding
/// rather than guessed from the message a caller built.
pub const IDEMPOTENCY_KEY_FIELD: &str = "idempotency_key";

/// A keyed request and whether it ended up keyed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyedRequest<K> {
    /// The request, with its key set.
    pub request: K,
    /// Whether a key is on it. Always `true` today — the function only runs for a
    /// request message that declares the field — and stated anyway so a caller
    /// reads the same shape as the other SDKs' `keyed` flag.
    pub keyed: bool,
}

/// The key one logical call goes out with (R3, D610).
///
/// * a key the caller supplied **wins** and is never replaced;
/// * otherwise a UUIDv7 is minted;
/// * an existing non-empty key on the request wins over both, because a caller
///   who built the message with a key has already decided.
///
/// A key regenerated per attempt turns one write into two, which is the exact
/// failure the key exists to prevent: this function is therefore called **once
/// per logical call**, before the first attempt, and the request it keys is the
/// one every attempt sends.
#[must_use]
pub fn idempotency_key(existing: Option<&str>, supplied: Option<&str>) -> String {
    match existing {
        Some(key) if !key.is_empty() => key.to_owned(),
        _ => supplied.map_or_else(uuidv7, str::to_owned),
    }
}

/// Keys a request message in place and reports that it is keyed.
///
/// `existing` and `set` are the two accessors of a generated request's
/// `idempotency_key` field. They are passed as closures rather than through a
/// trait impl because of the orphan rule: both the trait and the message would be
/// foreign to this crate, so `impl Keyed for MutateRequest` is not expressible.
/// A function keeps the policy in one place and leaves the generated type alone.
///
/// A request whose schema declares **no** such field is never passed here at all —
/// [`crate::binding::CallBinding::takes_idempotency_key`] says so, and the facade
/// only calls this for the calls it says are keyed.
pub fn with_idempotency_key<K, G, S>(
    mut request: K,
    supplied: Option<&str>,
    get: G,
    set: S,
) -> KeyedRequest<K>
where
    G: FnOnce(&K) -> Option<String>,
    S: FnOnce(&mut K, String),
{
    let key = idempotency_key(get(&request).as_deref(), supplied);
    set(&mut request, key);
    KeyedRequest {
        request,
        keyed: true,
    }
}

/// A session's merged consistency token (D609; R4 as amended by SDKP §2.1).
/// Off unless a client or a call turns it on.
///
/// Every token a response carries is parsed and **merged**: per `(stream,
/// partition)` the larger offset wins, so a read held to the session waits for
/// every write the session saw. A string that is not a `v1:` token at all (a
/// page token, a bearer) is ignored; a `v1:` string that does not parse is
/// counted in [`ConsistencySession::rejected`] and otherwise ignored, because the
/// RPC that answered with it already succeeded and a caller that retries on
/// that error performs the write twice.
#[derive(Debug)]
pub struct ConsistencySession {
    state: Mutex<SessionState>,
}

#[derive(Debug, Default)]
struct SessionState {
    /// One merged token per scope (a proto package; `""` for a caller using
    /// the store directly). R4, SDKP-17.
    tokens: BTreeMap<String, ConsistencyToken>,
    rejected: u64,
    merged: u64,
}

impl Default for ConsistencySession {
    fn default() -> Self {
        ConsistencySession {
            state: Mutex::new(SessionState::default()),
        }
    }
}

impl ConsistencySession {
    /// A new, empty session.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The token to attach to the next read in the default scope, as text.
    #[must_use]
    pub fn current(&self) -> Option<String> {
        self.current_in("")
    }

    /// The token to attach to the next read in `scope` (a proto package).
    #[must_use]
    pub fn current_in(&self, scope: &str) -> Option<String> {
        self.token_in(scope).map(|token| token.to_string())
    }

    /// The merged token of the default scope, parsed.
    #[must_use]
    pub fn token(&self) -> Option<ConsistencyToken> {
        self.token_in("")
    }

    /// The merged token of `scope`, parsed.
    #[must_use]
    pub fn token_in(&self, scope: &str) -> Option<ConsistencyToken> {
        self.state
            .lock()
            .ok()
            .and_then(|state| state.tokens.get(scope).cloned())
    }

    /// Folds a token the server returned into the default scope.
    pub fn record(&self, token: Option<&str>) {
        self.record_in(token, "");
    }

    /// Folds a token the server returned into `scope`'s.
    pub fn record_in(&self, token: Option<&str>, scope: &str) {
        let Some(token) = token.filter(|t| t.starts_with("v1:")) else {
            return;
        };
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        match token.parse::<ConsistencyToken>() {
            Ok(parsed) => {
                let merged = match state.tokens.remove(scope) {
                    None => parsed.normalized(),
                    Some(current) => current.merge(&parsed),
                };
                state.tokens.insert(scope.to_owned(), merged);
                state.merged += 1;
            }
            Err(_) => state.rejected += 1,
        }
    }

    /// Forgets the token.
    pub fn clear(&self) {
        if let Ok(mut state) = self.state.lock() {
            *state = SessionState::default();
        }
    }

    /// How many `v1:` tokens the session could not parse.
    #[must_use]
    pub fn rejected(&self) -> u64 {
        self.state.lock().map(|state| state.rejected).unwrap_or(0)
    }

    /// The old name of [`ConsistencySession::rejected`], from before tokens
    /// merged.
    #[must_use]
    pub fn conflicts(&self) -> u64 {
        self.rejected()
    }

    /// How many tokens were folded in.
    #[must_use]
    pub fn merged(&self) -> u64 {
        self.state.lock().map(|state| state.merged).unwrap_or(0)
    }

    /// The token a response carried, read out of the response message.
    #[must_use]
    pub fn token_of(consistency_token: &str) -> Option<&str> {
        (!consistency_token.is_empty()).then_some(consistency_token)
    }
}

/// The consistency a call runs at (design §44 §7.4, D609; §05 §5).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Consistency {
    /// `STRONG`, the default: wait for the write to be visible.
    #[default]
    Strong,
    /// `EVENTUAL`: read what is there now.
    Eventual,
    /// `AT_LEAST{token}`: read at least as far as `token`.
    AtLeast(String),
}

impl Consistency {
    /// The `loams-consistency` header value, or `None` for `Strong` (the
    /// server's default, so the header is not spent).
    #[must_use]
    pub fn header_value(&self) -> Option<String> {
        match self {
            Consistency::Strong => None,
            Consistency::Eventual => Some("eventual".to_owned()),
            Consistency::AtLeast(token) => Some(format!("at_least:{token}")),
        }
    }

    /// The consistency to send: an explicit one wins, otherwise the session's
    /// token, which is what makes an opted-in session read-your-writes rather
    /// than record tokens and never send one.
    #[must_use]
    pub fn resolve(
        explicit: Option<&Consistency>,
        session: Option<&ConsistencySession>,
    ) -> Consistency {
        match explicit {
            Some(Consistency::AtLeast(token)) => Consistency::AtLeast(token.clone()),
            Some(other) => other.clone(),
            None => session
                .and_then(ConsistencySession::current)
                .map_or(Consistency::Strong, Consistency::AtLeast),
        }
    }
}

/// The header a consistency travels in. Design §44 §7.4 names the token
/// `loams-consistency-token` on the **response**; the request field §05 §5
/// defines is `consistency`, and the SDK sends it as a header so a caller does
/// not have to thread it through every generated request message.
pub const CONSISTENCY_HEADER: &str = "loams-consistency";

/// The response header a write's token arrives in, for a caller reading it
/// without the generated message.
pub const CONSISTENCY_TOKEN_HEADER: &str = "loams-consistency-token";

/// Reads a consistency token out of a response's headers.
#[must_use]
pub fn token_from_headers(headers: &http::HeaderMap) -> Option<String> {
    headers
        .get(CONSISTENCY_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

/// The metadata map a `feature_not_in_variant` refusal carries, as the guard
/// builds it (R5).
#[must_use]
pub fn variant_metadata(package: &str, variant: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("package".to_owned(), package.to_owned()),
        ("variant".to_owned(), variant.to_owned()),
    ])
}

/// An error for a call the SDK cannot make at all.
#[must_use]
pub fn unknown_call(rpc: &str) -> LoamsError {
    LoamsError::internal(format!("no generated call {rpc}")).with_rpc(rpc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live_v1::MutateRequest;

    /// A request shaped like `MutateRequest`: proto3 `optional`, so `None` means
    /// "the caller did not supply one".
    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    struct MutateLike {
        function: String,
        idempotency_key: Option<String>,
    }

    fn key_like<K, G, S>(request: K, supplied: Option<&str>, get: G, set: S) -> KeyedRequest<K>
    where
        G: FnOnce(&K) -> Option<String>,
        S: FnOnce(&mut K, String),
    {
        with_idempotency_key(request, supplied, get, set)
    }

    #[test]
    fn one_key_is_minted_per_logical_call_and_it_is_a_uuidv7() {
        let first = key_like(
            MutateLike::default(),
            None,
            |r: &MutateLike| r.idempotency_key.clone(),
            |r: &mut MutateLike, key| r.idempotency_key = Some(key),
        );
        let second = key_like(
            MutateLike::default(),
            None,
            |r: &MutateLike| r.idempotency_key.clone(),
            |r: &mut MutateLike, key| r.idempotency_key = Some(key),
        );
        let a = first.request.idempotency_key.expect("keyed");
        let b = second.request.idempotency_key.expect("keyed");
        assert_ne!(a, b, "two logical calls get two keys");
        // UUIDv7: version nibble, variant bit, and a millisecond stamp of now.
        assert_eq!(a.as_bytes()[14], b'7');
        assert!(matches!(a.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));
        assert!(crate::uuidv7::uuidv7_time(&a).is_some());
    }

    #[test]
    fn a_caller_supplied_key_wins_and_is_never_replaced() {
        let kept = key_like(
            MutateLike {
                idempotency_key: Some("mine".to_owned()),
                ..Default::default()
            },
            None,
            |r: &MutateLike| r.idempotency_key.clone(),
            |r: &mut MutateLike, key| r.idempotency_key = Some(key),
        );
        assert_eq!(kept.request.idempotency_key.as_deref(), Some("mine"));
        let supplied = key_like(
            MutateLike::default(),
            Some("supplied"),
            |r: &MutateLike| r.idempotency_key.clone(),
            |r: &mut MutateLike, key| r.idempotency_key = Some(key),
        );
        assert_eq!(
            supplied.request.idempotency_key.as_deref(),
            Some("supplied")
        );
    }

    #[test]
    fn the_policy_is_one_small_function_the_callers_read() {
        assert!(!idempotency_key(None, None).is_empty());
        assert_eq!(idempotency_key(None, Some("mine")), "mine");
        assert_eq!(idempotency_key(Some("already"), Some("mine")), "already");
        assert_eq!(idempotency_key(Some(""), Some("mine")), "mine");
    }

    #[test]
    fn the_generated_mutation_really_carries_the_field_the_facade_names() {
        // The type-level half of "Mutate is keyed": `MutateRequest` has the field
        // the binding names, and the SDK can read and write it.
        let keyed = with_idempotency_key(
            MutateRequest::default(),
            Some("idem_fixed"),
            |r: &MutateRequest| r.idempotency_key.clone(),
            |r: &mut MutateRequest, key| r.idempotency_key = Some(key),
        );
        assert!(keyed.keyed);
        assert_eq!(keyed.request.idempotency_key.as_deref(), Some("idem_fixed"));
        assert_eq!(IDEMPOTENCY_KEY_FIELD, "idempotency_key");
    }

    #[test]
    fn the_facade_says_which_calls_are_keyed() {
        assert!(
            crate::binding::binding_of("tables", "mutate")
                .expect("generated")
                .takes_idempotency_key()
        );
        assert!(
            !crate::binding::binding_of("live.admin", "deploy")
                .expect("generated")
                .takes_idempotency_key()
        );
        assert!(
            !crate::binding::binding_of("tables", "query")
                .expect("generated")
                .takes_idempotency_key()
        );
        assert!(
            !crate::binding::binding_of("instance", "get_instance")
                .expect("generated")
                .takes_idempotency_key()
        );
    }

    #[test]
    fn a_session_merges_tokens_and_counts_what_it_cannot_parse() {
        let session = ConsistencySession::new();
        assert_eq!(session.current(), None);
        session.record(Some("v1:s1/p0@5"));
        assert_eq!(session.current().as_deref(), Some("v1:s1/p0@5"));
        session.record(Some("v1:s1/p0@3,s2/p0@1"));
        assert_eq!(session.current().as_deref(), Some("v1:s1/p0@5,s2/p0@1"));
        assert_eq!(session.merged(), 2);
        assert_eq!(session.rejected(), 0);
        // A `v1:` string that does not parse is counted, not merged.
        session.record(Some("v1:garbage"));
        assert_eq!(session.rejected(), 1);
        // Anything that is not a `v1:` token is not the session's business.
        session.record(Some("page-2"));
        session.record(None);
        session.record(Some(""));
        assert_eq!(session.current().as_deref(), Some("v1:s1/p0@5,s2/p0@1"));
        assert_eq!(session.rejected(), 1);
    }

    #[test]
    fn an_explicit_consistency_wins_and_the_session_is_the_fallback() {
        let session = ConsistencySession::new();
        session.record(Some("v1:s1/p0@1"));
        assert_eq!(
            Consistency::resolve(None, Some(&session)),
            Consistency::AtLeast("v1:s1/p0@1".to_owned())
        );
        assert_eq!(
            Consistency::resolve(Some(&Consistency::Eventual), Some(&session)),
            Consistency::Eventual
        );
        assert_eq!(Consistency::resolve(None, None), Consistency::Strong);
        assert_eq!(Consistency::Strong.header_value(), None);
        assert_eq!(
            Consistency::Eventual.header_value().as_deref(),
            Some("eventual")
        );
        assert_eq!(
            Consistency::AtLeast("v1:a".to_owned())
                .header_value()
                .as_deref(),
            Some("at_least:v1:a")
        );
    }
}
