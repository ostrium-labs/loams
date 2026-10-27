//! Seeded, deterministic simulation of gates leasing from a counter owner
//! (ME1 Task 16, D2167): node kills, gate↔owner partitions, stream lag and
//! owner failover with learning mode. There is no `loams-detsim` crate yet
//! (D2160), so the schedule is driven here by a splitmix64 generator; the
//! same seed always replays the same run. These scenarios move to
//! `loams-detsim` when it lands.

use loams_meter_core::lease::{Allocator, AllocatorConfig, LeasePolicy, LeaseReport, LeaseRequest};
use loams_meter_core::{Amount, FeatureKey, LeaseId, SubjectKey, UtcNanos};

/// One simulated step.
const STEP: i64 = 100 * 1_000_000;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }

    fn chance(&mut self, per_million: u64) -> bool {
        self.below(1_000_000) < per_million
    }
}

struct Local {
    id: LeaseId,
    granted: i64,
    spent: i64,
    expires_at: i64,
}

struct Node {
    name: String,
    alive: bool,
    dead_until: i64,
    partitioned_until: i64,
    lease: Option<Local>,
    epoch_seen: u64,
}

struct Pending {
    deliver_at: i64,
    lease: LeaseId,
}

#[derive(Clone, Copy)]
struct Scenario {
    seed: u64,
    nodes: usize,
    allowance: i64,
    min_chunk: i64,
    steps: i64,
    /// Per node per step, in millionths.
    kill_ppm: u64,
    partition_ppm: u64,
    failover_at_step: Option<i64>,
}

struct Outcome {
    spent: i64,
    full_chunk: i64,
}

fn units(a: Amount) -> i64 {
    i64::try_from(a.trunc_units()).expect("small amounts")
}

fn run(s: Scenario) -> Outcome {
    let cfg = AllocatorConfig::default();
    let mut rng = Rng(s.seed);
    let allowance = Amount::from_units(s.allowance);
    let min_chunk = Amount::from_units(s.min_chunk);
    let mut owner = Allocator::new(
        cfg,
        1,
        allowance,
        min_chunk,
        LeasePolicy::HardLimit,
        Amount::ZERO,
    );
    let full_chunk = units(owner.full_chunk());
    let mut nodes: Vec<Node> = (0..s.nodes)
        .map(|i| Node {
            name: format!("node-{i}"),
            alive: true,
            dead_until: 0,
            partitioned_until: 0,
            lease: None,
            epoch_seen: 0,
        })
        .collect();
    let mut stream: Vec<Pending> = Vec::new();
    let mut delivered: i64 = 0;
    let mut spent: i64 = 0;
    let subject = SubjectKey::new("org_1").expect("subject");
    let feature = FeatureKey::new("api_requests").expect("feature");

    for step in 0..s.steps {
        let now = step * STEP;

        // The stream delivers what is due, in append order.
        let mut kept = Vec::with_capacity(stream.len());
        for p in stream.drain(..) {
            if p.deliver_at <= now {
                owner.on_usage(Some(&p.lease), Amount::from_units(1));
                delivered += 1;
            } else {
                kept.push(p);
            }
        }
        stream = kept;
        owner.tick(UtcNanos(now));

        if s.failover_at_step == Some(step) {
            // The new owner rebuilds from what reached the stream's
            // consumer (House + tail) and learns for one TTL.
            owner = Allocator::after_failover(
                cfg,
                owner.epoch() + 1,
                allowance,
                min_chunk,
                LeasePolicy::HardLimit,
                Amount::from_units(delivered),
                nodes.iter().map(|n| n.name.clone()),
                UtcNanos(now),
            );
        }

        for node in nodes.iter_mut() {
            if !node.alive {
                if now >= node.dead_until {
                    node.alive = true;
                }
                continue;
            }
            if rng.chance(s.kill_ppm) {
                node.alive = false;
                node.lease = None; // a killed gate loses its local state
                node.dead_until = now + (50 + rng.below(250) as i64) * STEP;
                continue;
            }
            if rng.chance(s.partition_ppm) {
                node.partitioned_until = now + (50 + rng.below(550) as i64) * STEP;
            }
            if rng.below(10) >= 8 {
                continue; // no work this step
            }

            let usable = |l: &Local| now < l.expires_at && l.spent < l.granted;
            if !node.lease.as_ref().is_some_and(usable) {
                if now < node.partitioned_until {
                    continue; // cannot reach the owner: fail closed
                }
                let held: Vec<LeaseReport> = node
                    .lease
                    .iter()
                    .map(|l| LeaseReport {
                        lease: l.id.clone(),
                        granted: Amount::from_units(l.granted),
                        spent: Amount::from_units(l.spent),
                    })
                    .collect();
                let grant = owner.acquire(&LeaseRequest {
                    subject: subject.clone(),
                    feature: feature.clone(),
                    node: node.name.clone(),
                    want: Amount::from_units(1),
                    held,
                    now: UtcNanos(now),
                });
                if grant.owner_epoch < node.epoch_seen {
                    continue; // a stale owner's grant is ignored
                }
                node.epoch_seen = grant.owner_epoch;
                if let Some(id) = grant.id.filter(|_| grant.amount.is_positive()) {
                    node.lease = Some(Local {
                        id,
                        granted: units(grant.amount),
                        spent: 0,
                        expires_at: grant.expires_at.0,
                    });
                }
            }
            if let Some(l) = node.lease.as_mut().filter(|l| usable(&**l)) {
                l.spent += 1;
                spent += 1;
                let lag = rng.below(21) as i64 * STEP;
                stream.push(Pending {
                    deliver_at: now + lag,
                    lease: l.id.clone(),
                });
            }
        }
    }
    Outcome { spent, full_chunk }
}

fn base(seed: u64) -> Scenario {
    Scenario {
        seed,
        nodes: 10,
        allowance: 10_000,
        min_chunk: 1,
        steps: 6_000,
        kill_ppm: 0,
        partition_ppm: 0,
        failover_at_step: None,
    }
}

#[test]
fn no_faults_spends_exactly_the_allowance() {
    for seed in 0..10 {
        let out = run(base(seed));
        assert_eq!(out.spent, 10_000, "seed {seed}");
    }
}

#[test]
fn hard_limit_overshoot_bounded_10_nodes() {
    for seed in 0..20 {
        let s = Scenario {
            kill_ppm: 500,
            partition_ppm: 700,
            ..base(seed)
        };
        let out = run(s);
        // Cost is known before the work here, so leases alone never
        // overshoot; §11.4's bound is one chunk per node.
        assert!(out.spent <= s.allowance, "seed {seed}: spent {}", out.spent);
        assert!(
            out.spent <= s.allowance + s.nodes as i64 * out.full_chunk,
            "seed {seed}"
        );
    }
}

#[test]
fn owner_failover_learning_mode_bounds_overshoot() {
    for seed in 0..20 {
        for failover_at_step in [200, 700, 1_100] {
            let s = Scenario {
                kill_ppm: 300,
                partition_ppm: 300,
                failover_at_step: Some(failover_at_step),
                ..base(seed)
            };
            let out = run(s);
            assert!(
                out.spent <= s.allowance + s.nodes as i64 * out.full_chunk,
                "seed {seed}, failover at {failover_at_step}: spent {}",
                out.spent
            );
        }
    }
}

#[test]
fn exact_mode_spends_a_tiny_allowance_without_overshoot() {
    for seed in 0..10 {
        let s = Scenario {
            allowance: 40,
            min_chunk: 5,
            steps: 2_000,
            ..base(seed)
        };
        let out = run(s);
        assert_eq!(out.spent, 40, "seed {seed}");
    }
}

#[test]
fn same_seed_same_run() {
    let s = Scenario {
        kill_ppm: 500,
        partition_ppm: 700,
        failover_at_step: Some(500),
        ..base(7)
    };
    assert_eq!(run(s).spent, run(s).spent);
}
