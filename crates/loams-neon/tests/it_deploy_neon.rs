//! `loams-neon` against `deploy/neon` at its pinned digests (PG2 Task 2):
//! attach a tenant, create a timeline, branch it, list both, delete them.
//! Ignored by default; `pg2-e2e.yml` runs it with the stack up:
//!
//! ```text
//! (cd deploy/neon && docker compose up -d rustfs create-bucket storage_broker pageserver)
//! cargo test -p loams-neon --test it_deploy_neon -- --ignored
//! ```
//!
//! `NEON_PAGESERVER` overrides the pageserver URL (default
//! `http://127.0.0.1:9898`).
#![allow(clippy::unwrap_used)]

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use loams_neon::pageserver::{NeonClient, NeonEndpoints, TenantConfig, TimelineCreate};
use loams_neon::{TenantId, TimelineId};

/// A fresh 16-byte id per run: the time and a tag, so runs never collide.
fn fresh(tag: u8) -> [u8; 16] {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut id = [0u8; 16];
    id[..15].copy_from_slice(&nanos.to_be_bytes()[1..]);
    id[15] = tag;
    id
}

#[tokio::test]
#[ignore = "needs deploy/neon running"]
async fn it_deploy_neon_tenant_timeline_branch() {
    let url = std::env::var("NEON_PAGESERVER").unwrap_or_else(|_| "http://127.0.0.1:9898".into());
    let client = NeonClient::new(
        NeonEndpoints {
            pageserver: url.parse().unwrap(),
            storcon: None,
        },
        None,
    )
    .unwrap();
    let t = TenantId(fresh(1));
    let main = TimelineId(fresh(2));
    let branch = TimelineId(fresh(3));
    let conf = TenantConfig {
        pitr_interval: Some(Duration::from_secs(24 * 3600)),
        ..TenantConfig::default()
    };

    client.attach_tenant(t, 1, &conf).await.unwrap();
    let info = client
        .create_timeline(t, &TimelineCreate::bootstrap(main, 17))
        .await
        .unwrap();
    assert_eq!((info.timeline_id, info.pg_version), (main, 17));
    // A retried create of the same timeline is accepted, not a conflict.
    client
        .create_timeline(t, &TimelineCreate::bootstrap(main, 17))
        .await
        .unwrap();

    let at = client.timeline(t, main).await.unwrap().last_record_lsn;
    let b = client
        .create_timeline(t, &TimelineCreate::branch(branch, main, Some(at)))
        .await
        .unwrap();
    assert_eq!(
        (b.ancestor_timeline_id, b.ancestor_lsn),
        (Some(main), Some(at))
    );

    let mut ids: Vec<TimelineId> = client
        .list_timelines(t)
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.timeline_id)
        .collect();
    ids.sort();
    let mut want = vec![main, branch];
    want.sort();
    assert_eq!(ids, want);

    // The same new id with other parameters is already_exists.
    let e = client
        .create_timeline(t, &TimelineCreate::bootstrap(branch, 17))
        .await
        .unwrap_err();
    assert_eq!(e.reason(), "already_exists", "{e}");

    // A timeline with a child cannot go first.
    let e = client.delete_timeline(t, main).await.unwrap_err();
    assert_eq!(e.reason(), "branch_has_children", "{e}");

    // A patch keeps the other settings.
    client.tenant_config(t, &conf).await.unwrap();

    for tl in [branch, main] {
        client.delete_timeline(t, tl).await.unwrap();
        let mut gone = false;
        for _ in 0..100 {
            match client.timeline(t, tl).await {
                Err(e) if e.reason() == "not_found" => {
                    gone = true;
                    break;
                }
                _ => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
        assert!(gone, "{tl} was not deleted");
    }
}
