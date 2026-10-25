//! The Kafka connector against Confluent Cloud (CC0 Task 0 of plan CC1,
//! §80 §4.2): the roundtrip suite and the exactly-once (transactions) cases
//! on a real Basic or Standard cluster. Skips unless `LOAMS_CONFLUENT_BOOTSTRAP`
//! is set; never runs in CI (it needs a cluster and a key, see
//! `Target::confluent`). Run from an owner's machine:
//!
//! ```text
//! LOAMS_CONFLUENT_BOOTSTRAP=… LOAMS_CONFLUENT_API_KEY=… LOAMS_CONFLUENT_API_SECRET=… \
//!   cargo test -p loams-connector-kafka --test confluent -j 2 -- --test-threads 1
//! ```

mod common;

use common::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn confluent_suite() {
    let Some(target) = Target::confluent() else {
        return;
    };
    roundtrip_suite(&target).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn confluent_txn_sink_exactly_once_under_crash() {
    let Some(target) = Target::confluent() else {
        return;
    };
    exactly_once_under_crash_case(&target).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn confluent_txn_recover_reads_checkpoint() {
    let Some(target) = Target::confluent() else {
        return;
    };
    recover_reads_checkpoint_case(&target).await;
}
