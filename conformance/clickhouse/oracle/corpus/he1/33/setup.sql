-- HE1 Task 33: conversions, conditionals, maths, JSON extraction and the rest,
-- over one table with a column of each family the functions read.
DROP TABLE IF EXISTS t33;
CREATE TABLE t33
(
    id UInt32,
    s String,
    n Nullable(String),
    i Int64,
    u UInt8,
    f Float64,
    f32 Float32,
    d Date,
    dt DateTime('UTC'),
    dt64 DateTime64(3, 'UTC'),
    dec Decimal(18, 4),
    lc LowCardinality(String),
    e Enum8('a' = 1, 'b' = 2),
    j String,
    b Bool,
    ni Nullable(Int32)
)
ENGINE = MergeTree
ORDER BY id;
INSERT INTO t33 VALUES
    (1, '42', '7', -5, 200, 2.5, 1.25, '2026-01-02', '2026-01-02 03:04:05', '2026-01-02 03:04:05.123', 1.2345, 'x', 'a', '{"a":"x","n":{"k":-3,"f":1.5},"arr":[1,2,3],"t":true}', true, 1),
    (2, 'abc', NULL, 300, 0, -1.5, -0.5, '1970-01-01', '1970-01-01 00:00:00', '1970-01-01 00:00:00.000', -9.9999, 'y', 'b', '{"a":"y\\u00e9","n":{"k":18446744073709551615},"arr":[]}', false, NULL),
    (3, '', '', 0, 255, nan, inf, '2149-06-06', '2106-02-07 06:28:15', '2299-12-31 23:59:59.999', 0, 'x', 'a', 'not json', true, -7),
    (4, '2026-10-27 10:00:00', 'b', 9223372036854775807, 1, 1e300, 3.4e38, '2000-02-29', '2000-02-29 12:00:00', '2000-02-29 12:00:00.500', 99999999999999.9999, '', 'b', '{"a":1,"a":2}', false, 0);
