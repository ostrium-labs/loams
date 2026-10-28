//! The system objects every stack has, made by `migrate` (ME1 Task 4, design
//! §63 §7.1, §54 §6.1).
//!
//! These are not migrations: a migration adopts or changes data a release
//! already has, and a new stack runs none (it is stamped at its release). A
//! system object has to exist on a new stack and on an upgraded one alike, so
//! [`ensure_system_streams`] runs at the end of every successful
//! `migrate run` ([`crate::runner::Runner::run`]), whatever it applied. A
//! process that runs without the `migrate` service (`loams dev`, the
//! desktop) gets the stream from the `meter` role, which calls
//! [`ensure_meter_stream`] when it starts. Each step is idempotent: a second
//! run finds what the first made and changes nothing.
//!
//! | Object | Where | Shape |
//! |---|---|---|
//! | Loams Meter's usage stream [`METER_STREAM`] | the `_system` namespace | [`METER_STREAM_PARTITIONS`] partitions, class `Standard`, 14 days' retention ([`system_stream_retention`]); records keyed by the event's `subject` |
//!
//! The stream is reserved ([`loams_common::meta::SYSTEM_STREAMS`]): nobody
//! creates it as an ordinary stream, and the API refuses an outside
//! producer's append to it.

use loams_common::StreamId;
use loams_common::meta::{
    ApplyError, Consistency, METER_STREAM, METER_STREAM_PARTITIONS, MetaError, MetaStore,
    SYSTEM_NAMESPACE, system_stream_retention,
};

use crate::ledger::Error;

/// The `_system` namespace's id, creating the namespace when it does not
/// exist yet (as [`crate::ledger::Ledger::open`] does).
async fn system_namespace(meta: &dyn MetaStore) -> Result<loams_common::NamespaceId, Error> {
    match meta.create_namespace(SYSTEM_NAMESPACE).await {
        Ok(id) | Err(MetaError::Rejected(ApplyError::NamespaceExists(id))) => Ok(id),
        Err(e) => Err(e.into()),
    }
}

/// Loams Meter's usage stream, `_system/_meter`: found as it is, or created
/// with [`METER_STREAM_PARTITIONS`] partitions and 14 days' retention. An
/// existing stream's partitions are never changed (the counter owners and the
/// House pipe's cursors follow them).
pub async fn ensure_meter_stream(meta: &dyn MetaStore) -> Result<StreamId, Error> {
    let namespace = system_namespace(meta).await?;
    let id = meta.ensure_system_stream(namespace, METER_STREAM).await?;
    if let Some(stream) = meta.stream(Consistency::Linearizable, id).await?
        && stream.partitions != METER_STREAM_PARTITIONS
    {
        tracing::warn!(
            stream = %METER_STREAM,
            partitions = stream.partitions,
            expected = METER_STREAM_PARTITIONS,
            "the meter stream has a partition count this release did not create; kept as it is"
        );
    }
    Ok(id)
}

/// Every system object of §63 §7.1, made if missing; returns the names of the
/// system streams, as `<namespace>/<stream>`.
pub async fn ensure_system_streams(meta: &dyn MetaStore) -> Result<Vec<String>, Error> {
    ensure_meter_stream(meta).await?;
    Ok(vec![format!("{SYSTEM_NAMESPACE}/{METER_STREAM}")])
}

/// The retention [`ensure_meter_stream`] creates the stream with.
pub fn meter_stream_retention() -> loams_common::meta::Retention {
    system_stream_retention(METER_STREAM)
}
