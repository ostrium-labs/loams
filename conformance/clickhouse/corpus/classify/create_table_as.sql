-- kind: CreateTableAs
CREATE TABLE t ENGINE = MergeTree ORDER BY n AS SELECT number AS n FROM numbers(3)
