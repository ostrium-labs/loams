-- loams-temporal-visibility (D2075). Applied by PgVisibility::migrate under
-- its own advisory lock; every statement is idempotent. The `visibility`
-- table itself belongs to loams-temporal-store (0001_init.sql); this adds
-- only indexes for the list queries and the search-attribute registry.

CREATE TABLE IF NOT EXISTS temporal_search_attributes (
    namespace_id text        NOT NULL,
    name         text        NOT NULL,
    -- temporal.api.enums.v1.IndexedValueType
    value_type   integer     NOT NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (namespace_id, name)
);

CREATE INDEX IF NOT EXISTS visibility_ns_start
    ON visibility (namespace_id, start_time DESC, run_id);
CREATE INDEX IF NOT EXISTS visibility_ns_status
    ON visibility (namespace_id, status);
CREATE INDEX IF NOT EXISTS visibility_ns_type
    ON visibility (namespace_id, workflow_type);
CREATE INDEX IF NOT EXISTS visibility_search_attrs
    ON visibility USING gin (search_attrs);
