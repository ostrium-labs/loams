//! `LeaseService` (ME1 Task 17): service identities only, owners only, and
//! grants fenced by the owner epoch.

use std::sync::Arc;

use async_trait::async_trait;
use loams_authz::Subject;
use loams_meter::authz::Caller;
use loams_meter::error::Result;
use loams_meter::lease_service::{AllocatorInputs, EpochFence, LeaseInputs, LeaseServer};
use loams_meter::owner::{METER_PARTITIONS, partition_of};
use loams_meter_core::{
    AllocatorConfig, Amount, FeatureKey, LeasePolicy, Period, RefusalReason, SubjectKey, UtcNanos,
};

const NOW: UtcNanos = UtcNanos(1_000 * UtcNanos::SECOND);

/// `org_a` may use 1,000 units of `house_cpu` this period; nothing else.
#[derive(Debug)]
struct Inputs;

#[async_trait]
impl LeaseInputs for Inputs {
    async fn inputs(
        &self,
        subject: &SubjectKey,
        feature: &FeatureKey,
        _now: UtcNanos,
    ) -> Result<Option<AllocatorInputs>> {
        if subject.as_str() != "org_a" || feature.as_str() != "house_cpu" {
            return Ok(None);
        }
        Ok(Some(AllocatorInputs {
            period: Period {
                start: UtcNanos(0),
                end: UtcNanos(30 * UtcNanos::DAY),
            },
            allowance: Amount::from_units(1_000),
            min_chunk: Amount::from_units(1),
            policy: LeasePolicy::HardLimit,
            counted: Amount::ZERO,
        }))
    }
}

fn server() -> LeaseServer {
    LeaseServer::new(
        METER_PARTITIONS,
        AllocatorConfig::default(),
        Arc::new(Inputs),
        vec!["house".into()],
        Arc::new(|| NOW),
    )
}

fn house() -> Caller {
    Caller::Service("house".into())
}

fn org_a() -> SubjectKey {
    SubjectKey::new("org_a").expect("subject")
}

fn cpu() -> FeatureKey {
    FeatureKey::new("house_cpu").expect("feature")
}

#[tokio::test]
async fn stale_epoch_grant_ignored() {
    let p = partition_of("org_a", METER_PARTITIONS);
    // `old` owned the partition at epoch 1 and was fenced without noticing;
    // `new` took it over at epoch 2.
    let old = server();
    old.gained(p, 1);
    let new = server();
    new.gained(p, 2);
    let fence = EpochFence::new();

    let fresh = new
        .acquire(
            &house(),
            org_a(),
            cpu(),
            "node-1".into(),
            Amount::from_units(1),
            vec![],
            fence.known(p),
        )
        .await
        .expect("new owner grants");
    assert_eq!(fresh.partition, p);
    assert_eq!(fresh.grant.owner_epoch, 2);
    assert!(fresh.grant.has_access);
    assert!(fence.admit(p, fresh.grant.owner_epoch));

    // The gate now knows epoch 2: the stale owner refuses rather than grant.
    let err = old
        .acquire(
            &house(),
            org_a(),
            cpu(),
            "node-1".into(),
            Amount::from_units(1),
            vec![],
            fence.known(p),
        )
        .await
        .expect_err("stale owner");
    assert_eq!(err.reason(), "not_owner");

    // A grant from the stale owner that raced in anyway is dropped.
    let stale = old
        .acquire(
            &house(),
            org_a(),
            cpu(),
            "node-2".into(),
            Amount::from_units(1),
            vec![],
            0,
        )
        .await
        .expect("a gate that has not seen epoch 2");
    assert_eq!(stale.grant.owner_epoch, 1);
    assert!(
        !fence.admit(p, stale.grant.owner_epoch),
        "older epoch ignored"
    );
    assert!(fence.admit(p, 2));
    assert!(fence.admit(p, 3), "a newer owner is accepted");
}

#[tokio::test]
async fn only_owners_and_service_identities_lease() {
    let s = server();
    let err = s
        .acquire(
            &house(),
            org_a(),
            cpu(),
            "n".into(),
            Amount::from_units(1),
            vec![],
            0,
        )
        .await
        .expect_err("not owned");
    assert_eq!(err.reason(), "not_owner");

    let p = partition_of("org_a", METER_PARTITIONS);
    s.gained(p, 1);
    let person = Caller::Principal(Subject::user("mia"));
    let err = s
        .acquire(
            &person,
            org_a(),
            cpu(),
            "n".into(),
            Amount::from_units(1),
            vec![],
            0,
        )
        .await
        .expect_err("a person");
    assert_eq!(err.reason(), "service_identity_required");
    let err = s
        .acquire(
            &Caller::Service("other".into()),
            org_a(),
            cpu(),
            "n".into(),
            Amount::from_units(1),
            vec![],
            0,
        )
        .await
        .expect_err("an unknown service");
    assert_eq!(err.reason(), "service_identity_required");

    // A feature without an entitlement is a cached refusal, not an error.
    let g = s
        .acquire(
            &house(),
            org_a(),
            FeatureKey::new("llm_tokens").expect("feature"),
            "n".into(),
            Amount::from_units(1),
            vec![],
            0,
        )
        .await
        .expect("refusal");
    assert!(!g.grant.has_access);
    assert_eq!(g.grant.refusal, Some(RefusalReason::NotEntitled));

    // Losing the partition stops the grants.
    s.lost(p, 1);
    assert!(
        s.acquire(
            &house(),
            org_a(),
            cpu(),
            "n".into(),
            Amount::from_units(1),
            vec![],
            0
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn a_return_at_another_epoch_is_ignored() {
    let s = server();
    let p = partition_of("org_a", METER_PARTITIONS);
    s.gained(p, 4);
    let g = s
        .acquire(
            &house(),
            org_a(),
            cpu(),
            "n".into(),
            Amount::from_units(1),
            vec![],
            0,
        )
        .await
        .expect("grant");
    let id = g.grant.id.clone().expect("granted");
    s.return_lease(&house(), &org_a(), &cpu(), &id, Amount::ZERO, 3)
        .expect("ignored, not an error");
    s.return_lease(&house(), &org_a(), &cpu(), &id, Amount::ZERO, 4)
        .expect("returned");
}
