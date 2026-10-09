-- kind: CreateTable
CREATE TABLE t (k UInt64, v String) ENGINE = ReplacingMergeTree ORDER BY k PARTITION BY k % 4
