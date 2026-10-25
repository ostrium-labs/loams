-- The select corpus's table (HS1 Task 11, `differential_select_corpus_on_lake`):
-- 1000 generated rows, the same as the House worker test's `corpus_rows()`.
DROP TABLE IF EXISTS t;
CREATE TABLE t (k Int64, g LowCardinality(String), v Float64, d Date, flag UInt8) ENGINE = MergeTree ORDER BY k;
INSERT INTO t SELECT
    toInt64(number) AS k,
    ['alpha', 'beta', 'gamma', 'delta'][(number % 4) + 1] AS g,
    toFloat64((number * 37) % 101) + number / 8 AS v,
    toDate('2024-01-01') + toIntervalDay(number % 60) AS d,
    toUInt8(number % 3 = 0) AS flag
FROM numbers(1000);
