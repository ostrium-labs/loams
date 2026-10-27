-- HE1 Task 38: metadata, system tables, SHOW / DESCRIBE / EXISTS, numbers().
DROP TABLE IF EXISTS v38;
DROP TABLE IF EXISTS t38;
DROP TABLE IF EXISTS t38_parts;
CREATE TABLE t38 (id UInt64, name String COMMENT 'the name', ts DateTime, n Nullable(Int32) CODEC(ZSTD(1))) ENGINE = MergeTree PARTITION BY toYYYYMM(ts) ORDER BY (id, ts);
INSERT INTO t38 VALUES (1, 'a', '2026-01-01 00:00:00', 1), (2, 'b', '2026-02-01 00:00:00', NULL), (3, 'c', '2026-02-02 00:00:00', 3);
CREATE TABLE t38_parts (a UInt64, b String) ENGINE = MergeTree ORDER BY a;
INSERT INTO t38_parts VALUES (1, 'x'), (2, 'y');
CREATE VIEW v38 AS SELECT id, name FROM t38;
