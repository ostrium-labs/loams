//! The mock's fake credentials (documented in the crate docs).
//!
//! `Bearer mock-access-<principal>` is a session authenticated now;
//! `Bearer mock-stale-<principal>` one authenticated [`STALE_AGE`] ago.
//! The principal must exist in the seed.
//!
//! A token this mock issued at [`crate::oauth`]'s token endpoint resolves to
//! the device it was issued for as well, which is what `WhoAmI` reports. A
//! hand-written token names a principal only, and then any of that principal's
//! devices will do.

use std::time::{Duration, SystemTime};

use connectrpc::{ConnectError, ErrorCode, RequestContext};

use crate::proto::loams::instance::v1::Principal;
use crate::refuse;
use crate::seed::Seed;
use crate::store::Store;

/// How old a `mock-stale-*` session is: past the 5-minute step-up window.
pub(crate) const STALE_AGE: Duration = Duration::from_secs(10 * 60);

/// The authenticated caller of an RPC.
#[derive(Debug, Clone)]
pub(crate) struct Caller {
    pub(crate) principal: Principal,
    pub(crate) authenticated_at: SystemTime,
    /// The device a token this mock issued was issued for.
    pub(crate) device_id: Option<String>,
}

/// Resolves the caller from the request's bearer token.
pub(crate) fn caller(store: &Store, ctx: &RequestContext) -> Result<Caller, ConnectError> {
    let header = ctx
        .header(http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| ConnectError::unauthenticated("a bearer token is required"))?;
    let mut caller = caller_from_header(&store.seed, header, SystemTime::now())?;
    if let Some(token) = header.strip_prefix("Bearer ") {
        caller.device_id = store.lock().issued.get(token).cloned();
    }
    Ok(caller)
}

pub(crate) fn caller_from_header(
    seed: &Seed,
    header: &str,
    now: SystemTime,
) -> Result<Caller, ConnectError> {
    let token = header
        .strip_prefix("Bearer ")
        .ok_or_else(|| ConnectError::unauthenticated("expected a bearer token"))?;
    let (id, authenticated_at) = if let Some(id) = token.strip_prefix("mock-access-") {
        (id, now)
    } else if let Some(id) = token.strip_prefix("mock-stale-") {
        (id, now.checked_sub(STALE_AGE).unwrap_or(now))
    } else {
        return Err(ConnectError::unauthenticated(
            "unknown token: the mock accepts mock-access-<principal> and mock-stale-<principal>",
        ));
    };
    let principal_id = match (id, seed.principal(id).is_some()) {
        // A hand-written token names the principal directly.
        (id, true) => id.to_owned(),
        // An issued token is `mock-access-<principal>-<discriminator>`, so that two
        // devices of one principal hold distinct tokens and each resolves to its
        // own device in `Store::issued`. Principal ids use `_`, never `-`, so the
        // last `-` is unambiguously the discriminator.
        (id, false) => id
            .rsplit_once('-')
            .map(|(head, _)| head.to_owned())
            .unwrap_or_else(|| id.to_owned()),
    };
    let principal = seed
        .principal(&principal_id)
        .ok_or_else(|| {
            ConnectError::unauthenticated(format!("no seed principal `{principal_id}`"))
        })?;
    if seed.revoked_principals.iter().any(|p| p == &principal_id) {
        return Err(refuse(
            ErrorCode::Unauthenticated,
            "device_revoked",
            "this device was revoked",
        ));
    }
    Ok(Caller {
        principal,
        authenticated_at,
        device_id: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_and_stale_tokens_resolve_seed_principals() {
        let seed = Seed::demo();
        let now = SystemTime::now();
        let fresh = caller_from_header(&seed, "Bearer mock-access-usr_omar", now).unwrap();
        assert_eq!(fresh.principal.id, "usr_omar");
        assert_eq!(fresh.authenticated_at, now);
        let stale = caller_from_header(&seed, "Bearer mock-stale-usr_omar", now).unwrap();
        assert_eq!(
            now.duration_since(stale.authenticated_at).unwrap(),
            STALE_AGE
        );
    }

    #[test]
    fn an_issued_token_suffix_still_resolves_its_principal() {
        // `mock-access-<principal>-<discriminator>` is what the token endpoint
        // hands out; the hand-written `mock-access-<principal>` still works.
        let seed = Seed::demo();
        let now = SystemTime::now();
        let issued = caller_from_header(&seed, "Bearer mock-access-usr_omar-01HQZX9K7T", now).unwrap();
        assert_eq!(issued.principal.id, "usr_omar");
        assert_eq!(issued.authenticated_at, now);
        // And an unknown principal with a suffix is still refused.
        assert!(caller_from_header(&seed, "Bearer mock-access-nobody-01HQZX9K7T", now).is_err());
    }

    #[test]
    fn unknown_tokens_and_principals_are_unauthenticated() {
        let seed = Seed::demo();
        let now = SystemTime::now();
        for header in [
            "Bearer real-looking-token",
            "Basic abc",
            "Bearer mock-access-nobody",
        ] {
            let err = caller_from_header(&seed, header, now).unwrap_err();
            assert_eq!(err.code, ErrorCode::Unauthenticated, "{header}");
        }
    }
}
