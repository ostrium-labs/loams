-- HE1 Task 22: the front end over a small table.
DROP TABLE IF EXISTS t22;
CREATE TABLE t22 (a Int64, b String, d Date) ENGINE = MergeTree ORDER BY a;
INSERT INTO t22 VALUES (1, 'x', '2026-01-01'), (2, 'x', '2026-01-02'), (3, 'y', '2026-01-03');
