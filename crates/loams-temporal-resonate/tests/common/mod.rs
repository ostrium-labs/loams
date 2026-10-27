//! The profile scenarios of D1591, written once against the port and run on
//! [`FakeDurability`] (`tests/fake.rs`, manual clock) and on a real Resonate
//! server (`tests/profile.rs`, wall clock), so the fake is checked against
//! the server it stands for.
#![allow(clippy::unwrap_used, clippy::expect_used, dead_code)]

use std::time::Duration;

use loams_temporal_resonate::ids::{PromiseId, TimeoutKind};
use loams_temporal_resonate::{
    CreatePromise, Delivery, Durability, DurabilityError, FakeDurability, PromiseStatus, SettleAs,
};
use tokio::sync::mpsc::Receiver;

/// Time, as the scenario sees it.
pub enum Clock {
    /// The fake's manual clock: sleeping advances it.
    Fake(FakeDurability),
    /// The wall clock, for a real server.
    Wall,
}

impl Clock {
    pub fn now(&self) -> i64 {
        match self {
            Clock::Fake(f) => f.now(),
            Clock::Wall => std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64,
        }
    }

    pub async fn sleep(&self, ms: i64) {
        match self {
            Clock::Fake(f) => f.advance(ms),
            Clock::Wall => tokio::time::sleep(Duration::from_millis(ms as u64)).await,
        }
    }
}

/// A scenario's names: unique per run, so a real server's database can be
/// reused.
pub struct Names {
    pub u: String,
}

impl Names {
    pub fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self {
            u: format!("{:x}", nanos ^ (std::process::id() as u128) << 64),
        }
    }

    pub fn group(&self, name: &str) -> String {
        format!("t6-{}-{name}", self.u)
    }

    pub fn ns(&self) -> String {
        format!("01jtest{}.default", self.u)
    }

    pub fn wft(
        &self,
        wf: &str,
        event: i64,
    ) -> (String, std::collections::BTreeMap<String, String>) {
        PromiseId::Wft {
            ns: self.ns(),
            wf: wf.into(),
            run: "0f8fad5b-d9cb-469f-a165-70867728950e".into(),
            scheduled_event_id: event,
        }
        .encode()
    }

    pub fn tmr(
        &self,
        wf: &str,
        event: i64,
    ) -> (String, std::collections::BTreeMap<String, String>) {
        PromiseId::Tmr {
            ns: self.ns(),
            wf: wf.into(),
            run: "0f8fad5b-d9cb-469f-a165-70867728950e".into(),
            started_event_id: event,
        }
        .encode()
    }

    pub fn to(&self, wf: &str, event: i64) -> (String, std::collections::BTreeMap<String, String>) {
        PromiseId::To {
            ns: self.ns(),
            wf: wf.into(),
            run: "0f8fad5b-d9cb-469f-a165-70867728950e".into(),
            event_id: event,
            kind: TimeoutKind::StartToClose,
            attempt: 1,
        }
        .encode()
    }
}

/// The next delivery that is not a [`Delivery::Gap`], within `max_ms`.
pub async fn next(rx: &mut Receiver<Delivery>, clock: &Clock, max_ms: i64) -> Option<Delivery> {
    let step = match clock {
        Clock::Fake(_) => 10,
        Clock::Wall => 50,
    };
    let mut waited = 0;
    loop {
        while let Ok(d) = rx.try_recv() {
            if !matches!(d, Delivery::Gap { .. }) {
                return Some(d);
            }
        }
        if waited >= max_ms {
            return None;
        }
        clock.sleep(step).await;
        waited += step;
    }
}

fn ready(d: Option<Delivery>) -> (String, i64) {
    match d {
        Some(Delivery::TaskReady {
            task_id,
            version,
            promise_id,
        }) => {
            assert_eq!(task_id, promise_id, "a task's ID is its promise's ID");
            (task_id, version)
        }
        other => panic!("expected TaskReady, got {other:?}"),
    }
}

fn settled(d: Option<Delivery>) -> (String, PromiseStatus) {
    match d {
        Some(Delivery::Settled { promise_id, status }) => (promise_id, status),
        other => panic!("expected Settled, got {other:?}"),
    }
}

/// A timer resolves at its deadline and an awaitable non-timer promise is
/// rejected as timed out; each one's listener is told.
pub async fn timer_times_out_and_notifies(d: &dyn Durability, c: &Clock, n: &Names) {
    let g = n.group("timer");
    let mut rx = d.subscribe(&g).await.unwrap();
    let now = c.now();
    let (tmr, tags) = n.tmr("wf-timer", 5);
    let v = d
        .promise_create(&CreatePromise {
            id: tmr.clone(),
            timeout_at_ms: now + 1_000,
            tags,
            timer: true,
            listener_group: Some(g.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(v.status, PromiseStatus::Pending);
    assert_eq!(
        v.tags.get("resonate:timer").map(String::as_str),
        Some("true")
    );
    let (to, tags) = n.to("wf-timer", 6);
    d.promise_create(&CreatePromise {
        id: to.clone(),
        timeout_at_ms: now + 1_200,
        tags,
        awaitable: true,
        listener_group: Some(g.clone()),
        ..Default::default()
    })
    .await
    .unwrap();
    assert!(
        next(&mut rx, c, 500).await.is_none(),
        "nothing before the deadline"
    );
    let mut got = vec![
        settled(next(&mut rx, c, 15_000).await),
        settled(next(&mut rx, c, 15_000).await),
    ];
    got.sort();
    let mut want = vec![
        (tmr.clone(), PromiseStatus::Resolved),
        (to.clone(), PromiseStatus::RejectedTimedout),
    ];
    want.sort();
    assert_eq!(got, want);
    let v = d.promise_get(&tmr).await.unwrap();
    assert_eq!(v.status, PromiseStatus::Resolved);
    assert_eq!(
        v.settled_at_ms,
        Some(now + 1_000),
        "settled at the deadline"
    );
    assert_eq!(
        d.promise_get(&to).await.unwrap().status,
        PromiseStatus::RejectedTimedout
    );
    assert!(
        next(&mut rx, c, 300).await.is_none(),
        "a listener fires once"
    );
}

/// A task is dispatched at version 0; an acquire moves it to 1; when the
/// lease lapses it is pending at 1 and dispatched again; fulfil settles it.
pub async fn task_lease_expiry_redelivers(d: &dyn Durability, c: &Clock, n: &Names, pid: &str) {
    let g = n.group("lease");
    let mut rx = d.subscribe(&g).await.unwrap();
    let (id, tags) = n.wft("wf-lease", 2);
    d.promise_create(&CreatePromise {
        id: id.clone(),
        timeout_at_ms: c.now() + 600_000,
        tags,
        param: Some(b"input".to_vec()),
        target_group: Some(g.clone()),
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(ready(next(&mut rx, c, 5_000).await), (id.clone(), 0));
    let claim = d.task_acquire(&id, 0, pid, 1_000).await.unwrap();
    assert_eq!(claim.version, 1);
    assert_eq!(claim.promise_id, id);
    assert_eq!(claim.param.as_deref(), Some(&b"input"[..]));
    assert!(matches!(
        d.task_acquire(&id, 0, pid, 1_000).await,
        Err(DurabilityError::Conflict(_))
    ));
    // A heartbeat keeps the lease past its first TTL.
    c.sleep(700).await;
    d.task_heartbeat(pid, &[(id.as_str(), 1)]).await.unwrap();
    c.sleep(600).await;
    assert!(rx.try_recv().is_err(), "the heartbeat extended the lease");
    assert_eq!(ready(next(&mut rx, c, 10_000).await), (id.clone(), 1));
    assert!(matches!(
        d.task_fulfill(&id, 1, SettleAs::Resolved).await,
        Err(DurabilityError::Conflict(_))
    ));
    let claim = d.task_acquire(&id, 1, pid, 60_000).await.unwrap();
    assert_eq!(claim.version, 2);
    d.task_fulfill(&id, 2, SettleAs::Resolved).await.unwrap();
    assert_eq!(
        d.promise_get(&id).await.unwrap().status,
        PromiseStatus::Resolved
    );
    assert!(matches!(
        d.task_acquire(&id, 2, pid, 1_000).await,
        Err(DurabilityError::Conflict(_))
    ));
}

/// A repeated create answers the existing promise and dispatches nothing new.
pub async fn idempotent_create_same_id(d: &dyn Durability, c: &Clock, n: &Names) {
    let g = n.group("same");
    let mut rx = d.subscribe(&g).await.unwrap();
    let (id, tags) = n.wft("wf-same", 2);
    let req = CreatePromise {
        id: id.clone(),
        timeout_at_ms: c.now() + 600_000,
        tags,
        param: Some(b"p".to_vec()),
        target_group: Some(g.clone()),
        ..Default::default()
    };
    let a = d.promise_create(&req).await.unwrap();
    let b = d.promise_create(&req).await.unwrap();
    assert_eq!(a, b);
    assert_eq!(b.status, PromiseStatus::Pending);
    assert_eq!(ready(next(&mut rx, c, 5_000).await), (id.clone(), 0));
    assert!(
        next(&mut rx, c, 1_500).await.is_none(),
        "the repeat dispatched nothing"
    );
    // After it settles, a repeat answers the settled promise.
    d.promise_settle(&id, SettleAs::Resolved).await.unwrap();
    assert_eq!(
        d.promise_create(&req).await.unwrap().status,
        PromiseStatus::Resolved
    );
}

/// A create on an existing ID with another parameter is `IdConflict`, and the
/// promise keeps its first parameter.
pub async fn idempotent_create_conflicting_param(d: &dyn Durability, c: &Clock, n: &Names) {
    let (id, tags) = n.tmr("wf-conflict", 3);
    let mut req = CreatePromise {
        id: id.clone(),
        timeout_at_ms: c.now() + 600_000,
        tags,
        param: Some(b"first".to_vec()),
        timer: true,
        ..Default::default()
    };
    d.promise_create(&req).await.unwrap();
    req.param = Some(b"second".to_vec());
    assert_eq!(
        d.promise_create(&req).await,
        Err(DurabilityError::IdConflict { id: id.clone() })
    );
    req.param = None;
    assert_eq!(
        d.promise_create(&req).await,
        Err(DurabilityError::IdConflict { id: id.clone() })
    );
    assert_eq!(
        d.promise_get(&id).await.unwrap().param.as_deref(),
        Some(&b"first"[..])
    );
}

/// `resonate:delay` (absolute ms) holds a task's dispatch until then.
pub async fn delayed_create(d: &dyn Durability, c: &Clock, n: &Names) {
    let g = n.group("delay");
    let mut rx = d.subscribe(&g).await.unwrap();
    let (id, tags) = n.wft("wf-delay", 2);
    let delay_until = c.now() + 2_000;
    d.promise_create(&CreatePromise {
        id: id.clone(),
        timeout_at_ms: c.now() + 600_000,
        tags,
        target_group: Some(g.clone()),
        delay_until_ms: Some(delay_until),
        ..Default::default()
    })
    .await
    .unwrap();
    assert!(next(&mut rx, c, 1_000).await.is_none(), "dispatched early");
    assert_eq!(ready(next(&mut rx, c, 15_000).await), (id.clone(), 0));
    assert!(c.now() >= delay_until, "dispatched before the delay ended");
    d.task_acquire(&id, 0, "p1", 60_000).await.unwrap();
    // A delay without a target, or past the deadline, is refused.
    let bad = CreatePromise {
        id: format!("{id}x"),
        timeout_at_ms: c.now() + 1_000,
        delay_until_ms: Some(c.now() + 5_000),
        target_group: Some(g),
        ..Default::default()
    };
    assert!(matches!(
        d.promise_create(&bad).await,
        Err(DurabilityError::Rejected { status: 400, .. })
    ));
}

/// A listener hears a settle; one registered on a settled promise is told at
/// once; a promise that is not awaitable takes no listener.
pub async fn listener_delivery(d: &dyn Durability, c: &Clock, n: &Names) {
    let g = n.group("listen");
    let mut rx = d.subscribe(&g).await.unwrap();
    assert!(matches!(
        d.subscribe(&g).await,
        Err(DurabilityError::Conflict(_))
    ));
    let (id, tags) = n.to("wf-listen", 4);
    d.promise_create(&CreatePromise {
        id: id.clone(),
        timeout_at_ms: c.now() + 600_000,
        tags,
        awaitable: true,
        ..Default::default()
    })
    .await
    .unwrap();
    let v = d.promise_register_listener(&id, &g).await.unwrap();
    assert_eq!(v.status, PromiseStatus::Pending);
    d.promise_settle(&id, SettleAs::Resolved).await.unwrap();
    assert_eq!(
        settled(next(&mut rx, c, 5_000).await),
        (id.clone(), PromiseStatus::Resolved)
    );
    let v = d.promise_register_listener(&id, &g).await.unwrap();
    assert_eq!(v.status, PromiseStatus::Resolved);
    assert_eq!(
        settled(next(&mut rx, c, 1_000).await),
        (id.clone(), PromiseStatus::Resolved)
    );
    // Not awaitable: no target, no timer, not `awaitable`.
    let plain = format!("{id}.plain");
    d.promise_create(&CreatePromise {
        id: plain.clone(),
        timeout_at_ms: c.now() + 600_000,
        ..Default::default()
    })
    .await
    .unwrap();
    assert!(matches!(
        d.promise_register_listener(&plain, &g).await,
        Err(DurabilityError::Rejected { status: 422, .. })
    ));
    assert_eq!(
        d.promise_register_listener(&format!("{id}.none"), &g).await,
        Err(DurabilityError::NotFound)
    );
}

/// `rejected_canceled` settles a timer and a task (CancelTimer, terminate);
/// a settled promise answers a later settle unchanged.
pub async fn settle_canceled(d: &dyn Durability, c: &Clock, n: &Names) {
    let g = n.group("cancel");
    let mut rx = d.subscribe(&g).await.unwrap();
    let (tmr, tags) = n.tmr("wf-cancel", 7);
    d.promise_create(&CreatePromise {
        id: tmr.clone(),
        timeout_at_ms: c.now() + 600_000,
        tags,
        timer: true,
        listener_group: Some(g.clone()),
        ..Default::default()
    })
    .await
    .unwrap();
    let v = d
        .promise_settle(&tmr, SettleAs::RejectedCanceled)
        .await
        .unwrap();
    assert_eq!(v.status, PromiseStatus::RejectedCanceled);
    assert_eq!(
        settled(next(&mut rx, c, 5_000).await),
        (tmr.clone(), PromiseStatus::RejectedCanceled)
    );
    let v = d.promise_settle(&tmr, SettleAs::Resolved).await.unwrap();
    assert_eq!(v.status, PromiseStatus::RejectedCanceled, "unchanged");

    let (task, tags) = n.wft("wf-cancel", 2);
    d.promise_create(&CreatePromise {
        id: task.clone(),
        timeout_at_ms: c.now() + 600_000,
        tags,
        target_group: Some(g.clone()),
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(ready(next(&mut rx, c, 5_000).await), (task.clone(), 0));
    d.task_acquire(&task, 0, "p1", 60_000).await.unwrap();
    d.promise_settle(&task, SettleAs::RejectedCanceled)
        .await
        .unwrap();
    // Settling the promise fulfilled its task: the lease holder's fulfil
    // and release are refused.
    assert!(matches!(
        d.task_fulfill(&task, 1, SettleAs::Resolved).await,
        Err(DurabilityError::Conflict(_))
    ));
    assert!(matches!(
        d.task_release(&task, 1).await,
        Err(DurabilityError::Conflict(_))
    ));
    assert_eq!(
        d.promise_settle(&format!("{task}.none"), SettleAs::Resolved)
            .await,
        Err(DurabilityError::NotFound)
    );
}
