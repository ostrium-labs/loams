use loams_imperial::{Lease, LeaseError};

#[test]
fn takeover_fences_old_holder_even_after_serialization() {
    let mut lease = Lease::new();
    let first = lease.claim("leader-a", 100, 20).unwrap();
    assert_eq!(lease.claim("leader-b", 119, 20), Err(LeaseError::Held));
    let mut lease: Lease = serde_json::from_slice(&serde_json::to_vec(&lease).unwrap()).unwrap();
    let second = lease.claim("leader-b", 120, 20).unwrap();
    assert!(second.epoch() > first.epoch());
    assert_eq!(lease.check(&first, 121), Err(LeaseError::Fenced));
    assert_eq!(lease.check(&second, 121), Ok(()));
}

#[test]
fn a_heartbeat_without_new_progress_cannot_renew() {
    let mut lease = Lease::new();
    let token = lease.claim("worker", 1, 10).unwrap();
    assert_eq!(lease.renew(&token, 2, 10, 0), Err(LeaseError::NoProgress));
    lease.renew(&token, 3, 10, 1).unwrap();
    assert_eq!(lease.renew(&token, 4, 10, 1), Err(LeaseError::NoProgress));
    assert_eq!(lease.check(&token, 12), Ok(()));
    assert_eq!(lease.check(&token, 13), Err(LeaseError::Expired));
}

#[test]
fn revoke_fences_before_reassignment() {
    let mut lease = Lease::new();
    let old = lease.claim("worker", 0, 100).unwrap();
    lease.revoke(&old, 1).unwrap();
    assert_eq!(lease.check(&old, 1), Err(LeaseError::Fenced));
    let next = lease.claim("worker", 2, 100).unwrap();
    assert!(next.epoch() > old.epoch());
    assert_eq!(lease.renew(&old, 3, 100, 1), Err(LeaseError::Fenced));
}

#[test]
fn expired_lease_cannot_be_resurrected_by_renewal() {
    let mut lease = Lease::new();
    let token = lease.claim("worker", 1, 2).unwrap();
    assert_eq!(lease.renew(&token, 3, 10, 1), Err(LeaseError::Expired));
    assert_eq!(lease.revoke(&token, 3), Err(LeaseError::Expired));
    let next = lease.claim("worker", 3, 2).unwrap();
    assert!(next.epoch() > token.epoch());
}

#[test]
fn invalid_deadlines_and_clock_regression_are_rejected() {
    let mut lease = Lease::new();
    assert_eq!(lease.claim("", 0, 1), Err(LeaseError::InvalidHolder));
    assert_eq!(lease.claim("x", 0, 0), Err(LeaseError::InvalidTtl));
    assert_eq!(lease.claim("x", u64::MAX, 2), Err(LeaseError::TimeOverflow));
    let token = lease.claim("x", 10, 10).unwrap();
    assert_eq!(
        lease.renew(&token, 9, 10, 1),
        Err(LeaseError::ClockRegression)
    );
    assert_eq!(lease.check(&token, 9), Err(LeaseError::ClockRegression));
}

#[test]
fn stale_tokens_cannot_revoke_a_new_holder() {
    let mut lease = Lease::new();
    let first = lease.claim("a", 10, 1).unwrap();
    let second = lease.claim("b", 11, 10).unwrap();
    assert_eq!(lease.revoke(&first, 12), Err(LeaseError::Fenced));
    assert_eq!(lease.check(&second, 12), Ok(()));
}
