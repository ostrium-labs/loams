-- HE1 Task 26: lake-table reads. The Rust engine reads these tables from
-- House-written lake fixtures (`lake/`, `loams-house-engine-dev --lake`),
-- which shadow the seeded copies; the fixtures hold the same rows, written
-- with loams-house-lake from this setup's rows
-- (`scripts/house/lake-fixtures-26.sh`).
DROP TABLE IF EXISTS types26;
CREATE TABLE types26 (
    id UInt32,
    u8 UInt8, u16 UInt16, u32 UInt32, u64 UInt64,
    i8 Int8, i16 Int16, i32 Int32, i64 Int64,
    f32 Float32, f64 Float64,
    b Bool,
    dec Decimal(18, 4),
    s String, fs FixedString(3),
    d Date, d32 Date32,
    dt DateTime('UTC'), dt64 DateTime64(3, 'UTC'), dt64n DateTime64(9, 'UTC'),
    uuid UUID, ip4 IPv4, ip6 IPv6,
    e Enum8('a' = 1, 'b' = 2), e16 Enum16('x' = 300, 'y' = -1),
    ns Nullable(String), ni Nullable(Int32),
    lc LowCardinality(String), lcn LowCardinality(Nullable(String)),
    arr Array(String), arrn Array(Nullable(UInt8)),
    m Map(String, UInt64),
    tup Tuple(UInt8, String), ntup Tuple(a UInt8, b String)
) ENGINE = MergeTree ORDER BY id;
INSERT INTO types26 VALUES
    (1, 0, 0, 0, 0, -128, -32768, -2147483648, -9223372036854775808, 0.1, 1e21, true, 1.5, 'a\tb', 'ab', '2026-10-23', '1900-01-01', '2026-10-23 01:02:03', '2026-10-23 01:02:03.5', '2026-10-23 01:02:03.123456789', '61f0c404-5cb3-11e7-907b-a6006ad3dba0', '1.2.3.4', '2001:db8::1', 'a', 'x', NULL, NULL, 'x', NULL, ['a', 'b'], [NULL, 1], {'k': 1}, (1, 'x'), (2, 'y')),
    (2, 255, 65535, 4294967295, 18446744073709551615, 127, 32767, 2147483647, 9223372036854775807, -0.0, -1e-7, false, -0.0001, 'it''s "q"', 'xyz', '1970-01-01', '2149-06-06', '1970-01-01 00:00:00', '1969-12-31 23:59:59.5', '2262-04-11 23:47:16', '00000000-0000-0000-0000-000000000000', '0.0.0.0', '::ffff:1.2.3.4', 'b', 'y', '', 0, '', '', [], [], {}, (0, ''), (0, '')),
    (3, 7, 7, 7, 9223372036854775808, 7, 7, 7, 7, 7.5, 7.25, true, 7, 'seven', 'sev', '2000-02-29', '2000-02-29', '2000-02-29 12:00:00', '2000-02-29 12:00:00.007', '2000-02-29 12:00:00.000000007', 'ffffffff-ffff-ffff-ffff-ffffffffffff', '255.255.255.255', '::', 'a', 'y', 'n', -7, 'lc', 'lcn', ['c'], [7], {'a': 1, 'b': 2}, (7, 's'), (7, 's'));

-- Columns added by ALTER read their DEFAULT for older rows; an ALIAS is
-- computed; a renamed column reads its older files.
DROP TABLE IF EXISTS alter26;
CREATE TABLE alter26 (id UInt32, s String, plus UInt64 ALIAS id + 1) ENGINE = MergeTree ORDER BY id;
INSERT INTO alter26 (id, s) VALUES (1, 'a'), (2, 'b');
ALTER TABLE alter26 ADD COLUMN n UInt32 DEFAULT id * 10;
ALTER TABLE alter26 ADD COLUMN z String;
ALTER TABLE alter26 RENAME COLUMN s TO label;
INSERT INTO alter26 (id, label, n, z) VALUES (3, 'c', 7, 'zz');

-- A delete TTL: expired rows are hidden. `ttl26_src` keeps every row (the
-- fixture's), `ttl26` is ClickHouse's after its merge.
DROP TABLE IF EXISTS ttl26_src;
CREATE TABLE ttl26_src (id UInt32, ts DateTime('UTC')) ENGINE = MergeTree ORDER BY id;
INSERT INTO ttl26_src VALUES (1, '2001-09-09 01:46:40'), (2, '2096-10-02 07:06:40'), (3, '2020-01-01 00:00:00'), (4, '2100-01-01 00:00:00');
DROP TABLE IF EXISTS ttl26;
CREATE TABLE ttl26 (id UInt32, ts DateTime('UTC')) ENGINE = MergeTree ORDER BY id TTL ts;
INSERT INTO ttl26 SELECT * FROM ttl26_src;
OPTIMIZE TABLE ttl26 FINAL;

-- Partitioned, one file per insert: pruned reads answer as unpruned ones.
DROP TABLE IF EXISTS part26;
CREATE TABLE part26 (p UInt8, id UInt64, e Enum8('a' = 1, 'b' = 2), s String) ENGINE = MergeTree PARTITION BY p ORDER BY id;
INSERT INTO part26 SELECT 0, number, 'a', toString(number) FROM numbers(100);
INSERT INTO part26 SELECT 1, 1000 + number, 'b', toString(number) FROM numbers(100);
INSERT INTO part26 SELECT 2, 2000 + number, 'a', toString(number) FROM numbers(100);
INSERT INTO part26 SELECT 3, 3000 + number, 'b', toString(number) FROM numbers(100);
