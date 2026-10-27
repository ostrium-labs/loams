-- Synthetic migrations in the shape of an application's (HE0 Task 2 fixture).
CREATE TABLE IF NOT EXISTS traces (
    id String,
    timestamp DateTime64(3),
    project_id String,
    name String DEFAULT '',
    tags Array(String),
    metadata Map(LowCardinality(String), String),
    level Enum8('DEBUG' = 1, 'ERROR' = 2),
    cost Nullable(Decimal(18, 4)),
    event_ts DateTime64(3),
    is_deleted UInt8,
    INDEX idx_id id TYPE bloom_filter(0.001) GRANULARITY 1
) ENGINE = ReplacingMergeTree(event_ts, is_deleted)
PARTITION BY toYYYYMM(timestamp)
PRIMARY KEY (project_id)
ORDER BY (project_id, toDate(timestamp), id)
TTL toDateTime(timestamp) + INTERVAL 30 DAY
SETTINGS index_granularity = 8192;

CREATE TABLE IF NOT EXISTS events (id UUID, created_at DateTime, name LowCardinality(String)) ENGINE = MergeTree ORDER BY created_at;

CREATE TABLE IF NOT EXISTS blobs (id String, body String CODEC(ZSTD(3))) ENGINE = MergeTree PARTITION BY id ORDER BY id;

ALTER TABLE traces ADD INDEX IF NOT EXISTS idx_meta_keys mapKeys(metadata) TYPE bloom_filter(0.01) GRANULARITY 1;

ALTER TABLE traces {CLICKHOUSE_CLUSTER_CLAUSE} ADD COLUMN IF NOT EXISTS t1 String; ALTER TABLE traces {CLICKHOUSE_CLUSTER_CLAUSE} ADD COLUMN IF NOT EXISTS t2 String;
