-- loams-temporal-store, schema version 1 (design §74 §5.5).
--
-- Applied by PgStore::migrate in one transaction under an advisory lock.
-- Every per-run table carries shard_id (X1.R6). Payloads are opaque bytes:
-- the store never parses a state blob, an event blob or a memo.

CREATE TABLE schema_version (
    version    integer     PRIMARY KEY,
    -- The shard count, fixed when the database is created (X1.R6).
    shards     integer     NOT NULL CHECK (shards > 0),
    applied_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE namespaces (
    namespace_id       text    PRIMARY KEY,
    loams_namespace_id text    NOT NULL,
    name               text    NOT NULL,
    retention_secs     bigint  NOT NULL,
    state              integer NOT NULL,
    config             jsonb   NOT NULL DEFAULT '{}'::jsonb,
    db_record_version  bigint  NOT NULL DEFAULT 1,
    UNIQUE (loams_namespace_id, name)
);

-- Mutable state, one row per run. db_record_version is the compare-and-set
-- version (§5.5 rule 1).
CREATE TABLE executions (
    shard_id          integer     NOT NULL,
    namespace_id      text        NOT NULL,
    workflow_id       text        NOT NULL,
    run_id            text        NOT NULL,
    status            integer     NOT NULL,
    next_event_id     bigint      NOT NULL,
    db_record_version bigint      NOT NULL,
    state_blob        bytea       NOT NULL,
    last_write_ts     timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (shard_id, namespace_id, workflow_id, run_id)
);

-- The current run of each workflow ID: the ID reuse and conflict checks.
CREATE TABLE current_runs (
    shard_id         integer NOT NULL,
    namespace_id     text    NOT NULL,
    workflow_id      text    NOT NULL,
    run_id           text    NOT NULL,
    status           integer NOT NULL,
    start_request_id text    NOT NULL,
    PRIMARY KEY (shard_id, namespace_id, workflow_id)
);

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
);

-- The transactional outbox (§5.5 rule 2). A relay claims rows with a lease
-- (claimed_by, claimed_until); an expired lease makes a row claimable again,
-- so delivery is at least once and a relay crash never strands a row.
CREATE TABLE outbox (
    id            bigserial   PRIMARY KEY,
    shard_id      integer     NOT NULL,
    namespace_id  text        NOT NULL,
    workflow_id   text        NOT NULL,
    run_id        text        NOT NULL,
    kind          text        NOT NULL,
    resonate_id   text        NOT NULL,
    request       jsonb       NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    dispatched_at timestamptz,
    claimed_by    text,
    claimed_until timestamptz,
    attempts      integer     NOT NULL DEFAULT 0
);

CREATE INDEX outbox_pending ON outbox (shard_id, id) WHERE dispatched_at IS NULL;

-- Task-queue partitions. Unused by DX1's single replica (X1.R8); partition
-- leases land with DX3.
CREATE TABLE task_queues (
    namespace_id text    NOT NULL,
    name         text    NOT NULL,
    kind         integer NOT NULL,
    partitions   integer NOT NULL DEFAULT 1,
    metadata     jsonb   NOT NULL DEFAULT '{}'::jsonb,
    PRIMARY KEY (namespace_id, name, kind)
);

-- Only the columns DescribeWorkflowExecution reads; the list grammar is DX3's.
CREATE TABLE visibility (
    namespace_id   text        NOT NULL,
    workflow_id    text        NOT NULL,
    run_id         text        NOT NULL,
    workflow_type  text        NOT NULL,
    status         integer     NOT NULL,
    start_time     timestamptz NOT NULL,
    close_time     timestamptz,
    execution_time timestamptz NOT NULL,
    task_queue     text        NOT NULL,
    search_attrs   jsonb       NOT NULL DEFAULT '{}'::jsonb,
    memo           bytea,
    PRIMARY KEY (namespace_id, workflow_id, run_id)
);
