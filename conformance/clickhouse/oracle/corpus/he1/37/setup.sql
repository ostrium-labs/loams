-- HE1 Task 37: settings on the Rust engine. A zone-less DateTime, a
-- DateTime64, a 64-bit integer past 2^53, a NaN and a slash, so each honoured
-- setting changes the answer.
DROP TABLE IF EXISTS t37;
CREATE TABLE t37 (id Int64, ts DateTime, ts3 DateTime64(3, 'UTC'), big Int64, f Float64, s String) ENGINE = MergeTree ORDER BY id;
INSERT INTO t37 VALUES (1, 1792697523, '2026-10-23 01:02:03.500', 9007199254740993, nan, 'a/b'), (2, 0, '1970-01-01 00:00:00.000', -1, 1.5, 'c');
