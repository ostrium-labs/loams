-- HE1 Task 28: aggregates, combinators, parametric aggregates and windowFunnel.
-- t28: one row per id, every Tier 0 argument type, values chosen so float sums
-- are exact and no topK count ties at a cut.
DROP TABLE IF EXISTS t28;
CREATE TABLE t28 (
    id UInt32,
    g String,
    lc LowCardinality(String),
    u8 UInt8,
    i16 Int16,
    i64 Int64,
    u64 UInt64,
    f32 Float32,
    f64 Float64,
    d Decimal(9, 2),
    b Bool,
    e Enum8('a' = 1, 'b' = 2, 'c' = 3),
    day Date,
    dt DateTime,
    dt3 DateTime64(3),
    n Nullable(Int32),
    ns Nullable(String),
    arr Array(UInt8),
    arrn Array(Nullable(UInt8)),
    ks Array(String),
    vs Array(Int8),
    m Map(String, UInt8)
) ENGINE = MergeTree ORDER BY id;
INSERT INTO t28 VALUES
    (1, 'x', 'p', 1, -3, -100, 18446744073709551615, 0.5, 1.25, 1.10, true, 'a', '2026-01-01', '2026-01-01 00:00:00', '2026-01-01 00:00:00.125', 10, 'k', [1, 2], [1, NULL], ['b', 'a'], [1, 2], {'b': 1, 'a': 2}),
    (2, 'x', 'p', 2, 5, 200, 2, 1.5, 2.5, 2.20, false, 'b', '2026-01-03', '2026-01-02 00:00:00', '2026-01-02 00:00:00.250', NULL, NULL, [3], [NULL], ['a', 'c'], [-2, 0], {'a': 0, 'c': 5}),
    (3, 'y', 'q', 3, 7, 300, 3, 2.25, -0.75, 3.30, true, 'c', '2026-01-04', '2026-01-03 00:00:00', '2026-01-03 00:00:00.500', 30, 'm', [4, 5, 6], [2, 3], ['c'], [3], {'c': 1}),
    (4, 'x', 'q', 4, -1, -400, 4, -0.25, 4.0, 4.40, false, 'a', '2026-01-02', '2026-01-04 00:00:00', '2026-01-04 00:00:00.750', NULL, 'z', [], [], [], [], {}),
    (5, 'z', 'p', 5, 2, 500, 5, 3.0, 0.125, 5.50, true, 'b', '2026-01-05', '2026-01-05 12:00:00', '2026-01-05 00:00:00.001', 50, NULL, [7, 8], [NULL, 9], ['a'], [4], {'a': 3, 'd': 1}),
    (6, 'y', 'p', 6, 9, 600, 6, 4.5, 6.5, 6.60, true, 'c', '2026-01-06', '2026-01-06 00:00:00', '2026-01-06 00:00:00.999', 60, 'a', [9], [10], ['b'], [5], {'b': 2}),
    (7, 'x', 'q', 7, 0, 700, 7, 5.75, -2.0, 7.70, false, 'a', '2026-01-07', '2026-01-07 00:00:00', '2026-01-07 00:00:00.000', NULL, 'b', [10, 11], [11, NULL], ['d', 'a'], [6, -1], {'d': 4}),
    (8, 'y', 'p', 8, -4, 800, 8, 6.0, 8.25, 8.80, true, 'b', '2026-01-08', '2026-01-08 00:00:00', '2026-01-08 00:00:00.500', 80, 'c', [12], [12], ['a'], [7], {'a': 1});
-- ev: generated event streams for windowFunnel, 40 users over two hours.
DROP TABLE IF EXISTS ev28;
CREATE TABLE ev28 (
    uid UInt32,
    ts DateTime,
    t32 UInt32,
    day Date,
    ev String
) ENGINE = MergeTree ORDER BY (uid, ts, ev);
INSERT INTO ev28
SELECT
    toUInt32(number % 40) AS uid,
    toDateTime('2026-03-01 00:00:00') + toIntervalSecond(cityHash64(number, 1) % 7200) AS ts,
    toUInt32(cityHash64(number, 1) % 7200) AS t32,
    toDate('2026-03-01') + toIntervalDay(cityHash64(number, 2) % 9) AS day,
    ['view', 'cart', 'checkout', 'buy', 'other'][1 + cityHash64(number, 3) % 5] AS ev
FROM numbers(800);
