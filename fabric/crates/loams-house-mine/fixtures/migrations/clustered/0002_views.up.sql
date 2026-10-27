CREATE TABLE IF NOT EXISTS daily_agg ON CLUSTER default (day Date, users AggregateFunction(uniq, String), cost Float64) ENGINE = ReplicatedAggregatingMergeTree ORDER BY day;

CREATE MATERIALIZED VIEW IF NOT EXISTS daily_mv ON CLUSTER default TO daily_agg AS SELECT toDate(timestamp) AS day, uniqState(id) AS users, sum(cost) AS cost FROM traces GROUP BY day;

ALTER TABLE traces ON CLUSTER default ADD COLUMN IF NOT EXISTS version String, MODIFY TTL toDateTime(timestamp) + INTERVAL 90 DAY;

CREATE OR REPLACE FUNCTION he0_channel AS (source) -> lower(source);
