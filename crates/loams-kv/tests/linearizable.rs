//! The one-shot operations are linearizable (KV1 plan KV0 Task 2, review
//! focus 2): 8 tasks run 200 random `get`, `put`, `delete` and
//! `compare_and_put` operations each on 4 keys, every operation's invoke and
//! completion stamped from one logical clock, and each key's history is
//! checked as a register with compare-and-put by the Wing–Gong–Lowe checker
//! `loams-sim` re-exports (`loams_meta_conformance::linearizability`). On
//! the embedded store always, and on TiKV when `LOAMS_TEST_PD` is set.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use loams_kv::{Store, TxnError, testing};
use loams_meta_conformance::linearizability::{Model, Op, Outcome, check};
use rand::{Rng, SeedableRng, rngs::StdRng};

const TASKS: u32 = 8;
const OPS_PER_TASK: u32 = 200;
const KEYS: usize = 4;

/// A register of bytes with compare-and-put, the sequential specification
/// of one key under the one-shot operations.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
struct Register(Option<String>);

#[derive(Clone, Debug, PartialEq, Eq)]
enum Input {
    Get,
    Put(String),
    Delete,
    Cas {
        expected: Option<String>,
        new: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Output {
    Got(Option<String>),
    Done,
    Swapped,
    Mismatch(Option<String>),
}

impl Model for Register {
    type Input = Input;
    type Output = Output;

    fn step(&self, input: &Input) -> (Self, Output) {
        match input {
            Input::Get => (self.clone(), Output::Got(self.0.clone())),
            Input::Put(v) => (Register(Some(v.clone())), Output::Done),
            Input::Delete => (Register(None), Output::Done),
            Input::Cas { expected, new } => {
                if self.0 == *expected {
                    (Register(new.clone()), Output::Swapped)
                } else {
                    (self.clone(), Output::Mismatch(self.0.clone()))
                }
            }
        }
    }
}

fn text(v: Option<Vec<u8>>) -> Option<String> {
    v.map(|b| String::from_utf8(b).expect("the test writes UTF-8"))
}

/// Runs `input` on `key`. `None`: the operation certainly did not apply
/// (a conflict the runner's attempts did not outlast), so it is left out of
/// the history; `Some(Indeterminate)`: it may have.
async fn apply(store: &Store, key: &[u8], input: &Input) -> Option<Outcome<Output>> {
    let result: Result<Output, TxnError> = match input {
        Input::Get => store.get(key).await.map(|v| Output::Got(text(v))),
        Input::Put(v) => store
            .put(key, v.as_bytes().to_vec())
            .await
            .map(|_| Output::Done),
        Input::Delete => store.delete(key).await.map(|_| Output::Done),
        Input::Cas { expected, new } => store
            .compare_and_put(
                key,
                expected.as_deref().map(str::as_bytes),
                new.as_deref().map(str::as_bytes),
            )
            .await
            .map(|r| match r {
                Ok(_) => Output::Swapped,
                Err(current) => Output::Mismatch(text(current)),
            }),
    };
    match result {
        Ok(out) => Some(Outcome::Ok(out)),
        Err(TxnError::Conflict | TxnError::NotApplied(_)) => None,
        Err(TxnError::Undetermined { .. } | TxnError::Deadline) => Some(Outcome::Indeterminate),
        Err(e) => panic!("{input:?} failed: {e}"),
    }
}

async fn history(store: Store, seed: u64) -> Vec<Vec<Op<Input, Output>>> {
    let clock = Arc::new(AtomicU64::new(0));
    let mut tasks = Vec::new();
    for client in 0..TASKS {
        let (store, clock) = (store.clone(), clock.clone());
        tasks.push(tokio::spawn(async move {
            let mut rng = StdRng::seed_from_u64(seed ^ u64::from(client));
            // The last value this client saw per key, so its CASes often match.
            let mut seen: Vec<Option<String>> = vec![None; KEYS];
            let mut ops = Vec::new();
            for i in 0..OPS_PER_TASK {
                let k = rng.random_range(0..KEYS);
                let fresh = format!("c{client}-{i}");
                let input = match rng.random_range(0..10) {
                    0..=2 => Input::Get,
                    3..=4 => Input::Put(fresh),
                    5 => Input::Delete,
                    _ => Input::Cas {
                        expected: seen[k].clone(),
                        new: (rng.random_range(0..5) > 0).then_some(fresh),
                    },
                };
                let key = format!("key-{k}");
                let invoke = clock.fetch_add(1, Ordering::SeqCst);
                let Some(outcome) = apply(&store, key.as_bytes(), &input).await else {
                    continue;
                };
                let complete = match outcome {
                    Outcome::Ok(_) => clock.fetch_add(1, Ordering::SeqCst),
                    Outcome::Indeterminate => u64::MAX,
                };
                match (&input, &outcome) {
                    (_, Outcome::Ok(Output::Got(v) | Output::Mismatch(v))) => seen[k] = v.clone(),
                    (Input::Put(v), Outcome::Ok(_)) => seen[k] = Some(v.clone()),
                    (Input::Delete, Outcome::Ok(_)) => seen[k] = None,
                    (Input::Cas { new, .. }, Outcome::Ok(Output::Swapped)) => seen[k] = new.clone(),
                    _ => {}
                }
                ops.push((
                    k,
                    Op {
                        client,
                        invoke,
                        complete,
                        input,
                        outcome,
                    },
                ));
            }
            ops
        }));
    }
    let mut per_key: Vec<Vec<Op<Input, Output>>> = (0..KEYS).map(|_| Vec::new()).collect();
    for task in tasks {
        for (k, op) in task.await.expect("a client task") {
            per_key[k].push(op);
        }
    }
    per_key
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn compare_and_put_is_linearizable() {
    let seed: u64 = rand::rng().random();
    for store in testing::stores("compare_and_put_is_linearizable").await {
        let backend = store.backend();
        let per_key = history(store, seed).await;
        let mut swapped = 0;
        for (k, ops) in per_key.iter().enumerate() {
            swapped += ops
                .iter()
                .filter(|op| op.outcome == Outcome::Ok(Output::Swapped))
                .count();
            if let Err(v) = check(Register::default(), ops) {
                panic!(
                    "{backend:?}, key-{k}, seed {seed}: not linearizable: {v}\nhistory: {ops:#?}"
                );
            }
        }
        // The workload exercised the interesting path.
        assert!(
            swapped > 0,
            "{backend:?}, seed {seed}: no compare_and_put matched"
        );
    }
}
