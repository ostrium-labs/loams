-- loams-temporal-store, schema version 2 (D2062, D2063, D2065).
--
-- Applied by PgStore::migrate in the same transaction and advisory lock as
-- version 1. Three changes:
--   1. Retention: executions.closed_at, set when a commit closes the run, so
--      the sweeper finds runs past their namespace's retention by index.
--   2. The run listing: an index for ListWorkflowExecutions' order (newest
--      start first, then workflow ID and run ID) per namespace.
--   3. history_events, the largest table, becomes 16 hash partitions on
--      shard_id: smaller indexes per partition, and vacuum after a retention
--      purge works partition by partition. Existing rows are copied over.

-- 1. Retention.
ALTER TABLE executions ADD COLUMN closed_at timestamptz;
UPDATE executions SET closed_at = last_write_ts WHERE status <> 1;
CREATE INDEX executions_closed ON executions (closed_at) WHERE closed_at IS NOT NULL;

-- The sweeper's two outbox questions: "has this run an undispatched row?"
-- and "which dispatched rows are old?".
CREATE INDEX outbox_run ON outbox (shard_id, namespace_id, workflow_id, run_id);
CREATE INDEX outbox_dispatched ON outbox (dispatched_at) WHERE dispatched_at IS NOT NULL;

-- 2. The run listing.
CREATE INDEX visibility_list ON visibility (namespace_id, start_time DESC, workflow_id, run_id);

-- 3. History partitions. The primary key keeps its name on the new table,
-- so the old one is renamed first (index names are unique per schema).
ALTER TABLE history_events RENAME TO history_events_v1;
ALTER TABLE history_events_v1 RENAME CONSTRAINT history_events_pkey TO history_events_v1_pkey;

CREATE TABLE history_events (
    shard_id             integer     NOT NULL,
    namespace_id         text        NOT NULL,
    workflow_id          text        NOT NULL,
    run_id               text        NOT NULL,
    event_id             bigint      NOT NULL,
    batch_first_event_id bigint      NOT NULL,
    event_type           integer     NOT NULL,
    event_blob           bytea       NOT NULL,
    created_at           timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (shard_id, namespace_id, workflow_id, run_id, event_id)
) PARTITION BY HASH (shard_id);

DO $$
BEGIN
    FOR i IN 0..15 LOOP
        EXECUTE format(
            'CREATE TABLE history_events_p%s PARTITION OF history_events FOR VALUES WITH (MODULUS 16, REMAINDER %s)',
            i, i);
    END LOOP;
END
$$;

INSERT INTO history_events (shard_id, namespace_id, workflow_id, run_id, event_id,
                            batch_first_event_id, event_type, event_blob, created_at)
SELECT shard_id, namespace_id, workflow_id, run_id, event_id,
       batch_first_event_id, event_type, event_blob, created_at
FROM history_events_v1;

DROP TABLE history_events_v1;
