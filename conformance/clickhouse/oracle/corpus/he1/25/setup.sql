-- HE1 Task 25: name resolution over two small tables.
DROP TABLE IF EXISTS t25;
CREATE TABLE t25 (a Int64, b String, c Float64, number UInt64, tp Tuple(x UInt8, y String)) ENGINE = MergeTree ORDER BY a;
INSERT INTO t25 VALUES (1, 'x', 1.5, 10, (1, 'p')), (2, 'y', 2.5, 20, (2, 'q')), (3, 'x', 3.5, 30, (3, 'r'));
DROP TABLE IF EXISTS u25;
CREATE TABLE u25 (a Int64, d String, k Int64) ENGINE = MergeTree ORDER BY a;
INSERT INTO u25 VALUES (1, 'one', 7), (2, 'two', 8), (4, 'four', 9);
