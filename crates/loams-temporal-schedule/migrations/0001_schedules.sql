-- loams-temporal-schedule: Temporal Schedules (D2072).
--
-- Applied by PgScheduleStore::migrate under its own advisory lock. Every
-- statement is idempotent, so the schema needs no version ledger of its own
-- and never touches loams-temporal-store's schema_version.

CREATE TABLE IF NOT EXISTS temporal_schedules (
    namespace_id      text        NOT NULL,
    schedule_id       text        NOT NULL,
    -- A new value per create: a re-created schedule never acts on the old
    -- one's timers (D2070).
    generation        text        NOT NULL,
    -- The compare-and-set version; also the API's conflict token.
    version           bigint      NOT NULL,
    request_id        text        NOT NULL DEFAULT '',
    -- temporal.api.schedule.v1.Schedule, canonical spec.
    schedule          bytea       NOT NULL,
    memo              bytea,
    search_attributes bytea,
    -- The runtime state (buffer, running workflows, counters, cursor).
    runtime           jsonb       NOT NULL,
    -- When the schedule's timer fires; the sweeper reads overdue rows.
    next_wake_ms      bigint,
    paused            boolean     NOT NULL DEFAULT false,
    updated_at        timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (namespace_id, schedule_id)
);

CREATE INDEX IF NOT EXISTS temporal_schedules_wake
    ON temporal_schedules (next_wake_ms)
    WHERE next_wake_ms IS NOT NULL;
