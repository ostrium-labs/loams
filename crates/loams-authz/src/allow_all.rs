//! [`AllowAllAuthorizer`]: developer mode only. It can be built only when every listener is on
//! loopback, as `LOAMS_AUTH_MODE=off` is today (§9.1).

use std::net::SocketAddr;

use async_trait::async_trait;

use crate::{Authorizer, CheckRequest, Decision, ObjectRef, Result, Subject};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("AllowAllAuthorizer refused: listener {0} is not on loopback")]
pub struct NotLoopback(pub SocketAddr);

#[derive(Debug)]
pub struct AllowAllAuthorizer(());

impl AllowAllAuthorizer {
    /// Fails unless every address in `listeners` is a loopback address. An empty list is
    /// refused too: the caller must say where it listens.
    pub fn loopback_only(listeners: &[SocketAddr]) -> Result<Self, NotLoopback> {
        match listeners.iter().find(|a| !a.ip().is_loopback()) {
            Some(a) => Err(NotLoopback(*a)),
            None if listeners.is_empty() => Err(NotLoopback(SocketAddr::from(([0, 0, 0, 0], 0)))),
            None => Ok(Self(())),
        }
    }
}

#[async_trait]
impl Authorizer for AllowAllAuthorizer {
    async fn check(&self, _req: CheckRequest) -> Result<Decision> {
        Ok(Decision::allow(None))
    }

    /// Developer mode cannot enumerate objects; listings fall back to the database.
    async fn list_objects(&self, _: &Subject, _: &str, _: &str) -> Result<Vec<ObjectRef>> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_all_refuses_non_loopback() {
        let lo: SocketAddr = "127.0.0.1:7780".parse().unwrap();
        let lo6: SocketAddr = "[::1]:7780".parse().unwrap();
        let any: SocketAddr = "0.0.0.0:7780".parse().unwrap();
        let lan: SocketAddr = "192.168.1.10:7780".parse().unwrap();
        assert!(AllowAllAuthorizer::loopback_only(&[lo, lo6]).is_ok());
        assert_eq!(
            AllowAllAuthorizer::loopback_only(&[lo, any]).unwrap_err(),
            NotLoopback(any)
        );
        assert_eq!(
            AllowAllAuthorizer::loopback_only(&[lan]).unwrap_err(),
            NotLoopback(lan)
        );
        assert!(AllowAllAuthorizer::loopback_only(&[]).is_err());
    }
}
