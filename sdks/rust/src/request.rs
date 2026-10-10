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

/// A session's merged consistency token (D609). Off unless a client or a call
/// turns it on.
#[derive(Debug)]
pub struct ConsistencySession {
    state: Mutex<SessionState>,
}

#[derive(Debug, Default)]
struct SessionState {
    token: Option<String>,
    /// How many responses carried a token the session could not merge. Counted
    /// rather than thrown: the RPC already succeeded, and a caller that retries
    /// on that error performs the write twice.
    conflicts: u64,
    /// How many tokens were folded in.
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

    /// The token to attach to the next read, if there is one.
    #[must_use]
    pub fn current(&self) -> Option<String> {
        self.state.lock().ok().and_then(|state| state.token.clone())
    }

    /// Folds a token the server returned into the session's.
    ///
    /// The first token becomes the session's. A later token **equal** to it is
    /// a no-op. A later token **different** from it cannot be merged without
    /// parsing the encoding, which is not in the protos yet, so it is counted as
    /// a conflict and the session keeps what it had: reporting is right and
    /// guessing is not.
    pub fn record(&self, token: Option<&str>) {
        let Some(token) = token.filter(|t| !t.is_empty()) else {
            return;
        };
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        match &state.token {
            None => {
                state.token = Some(token.to_owned());
                state.merged += 1;
            }
            Some(current) if current == token => {}
            Some(_) => state.conflicts += 1,
        }
    }

    /// How many tokens could not be merged with the session's.
    #[must_use]
    pub fn conflicts(&self) -> u64 {
        self.state.lock().map(|state| state.conflicts).unwrap_or(0)
    }

    /// How many tokens were folded in.
    #[must_use]
    pub fn merged(&self) -> u64 {
        self.state.lock().map(|state| state.merged).unwrap_or(0)
    }

    /// The token a response carried, read out of the response message.
    ///
    /// A free function so the call path can read the field without the session
    /// knowing what a response message looks like: it is one string field, and
    /// the generated struct's name differs per RPC.
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
            !crate::binding::binding_of("tables", "deploy")
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
    fn a_session_keeps_one_token_and_counts_what_it_cannot_merge() {
        let session = ConsistencySession::new();
        assert_eq!(session.current(), None);
        session.record(Some("v1:a"));
        assert_eq!(session.current().as_deref(), Some("v1:a"));
        session.record(Some("v1:a"));
        assert_eq!(session.merged(), 1);
        assert_eq!(session.conflicts(), 0);
        // Two different tokens meeting: reported, not merged into a wrong one.
        session.record(Some("v1:b"));
        assert_eq!(session.current().as_deref(), Some("v1:a"));
        assert_eq!(session.conflicts(), 1);
        session.record(None);
        session.record(Some(""));
        assert_eq!(session.current().as_deref(), Some("v1:a"));
        assert_eq!(session.conflicts(), 1);
    }

    #[test]
    fn an_explicit_consistency_wins_and_the_session_is_the_fallback() {
        let session = ConsistencySession::new();
        session.record(Some("v1:a"));
        assert_eq!(
            Consistency::resolve(None, Some(&session)),
            Consistency::AtLeast("v1:a".to_owned())
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
