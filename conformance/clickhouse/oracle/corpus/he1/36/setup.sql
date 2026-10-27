-- HE1 Task 36: every output format over the Task 21 types table (same shape
-- and values as he1/21/setup.sql), seeded into the engine.
DROP TABLE IF EXISTS types36;
CREATE TABLE types36 (
    id UInt32,
    u8 UInt8, u16 UInt16, u32 UInt32, u64 UInt64,
    i8 Int8, i16 Int16, i32 Int32, i64 Int64,
    f32 Float32, f64 Float64,
    b Bool,
    dec Decimal(18, 4), dec64 Decimal64(2),
    s String, fs FixedString(3),
    d Date, d32 Date32,
    dt DateTime('UTC'), dt64 DateTime64(3, 'UTC'), dt64n DateTime64(9, 'UTC'),
    uuid UUID, ip4 IPv4, ip6 IPv6,
    e Enum8('a' = 1, 'b' = 2),
    ns Nullable(String), ni Nullable(Int32),
    lc LowCardinality(String), lcn LowCardinality(Nullable(String)),
    arr Array(String), arrn Array(Nullable(UInt8)),
    m Map(String, UInt64),
    tup Tuple(UInt8, String), ntup Tuple(a UInt8, b String)
) ENGINE = MergeTree ORDER BY id;
INSERT INTO types36 VALUES
    (1, 0, 0, 0, 0, -128, -32768, -2147483648, -9223372036854775808, 0.1, 1e21, true, 1.5, 0.05, 'a\tb', 'ab', '2026-10-23', '1900-01-01', '2026-10-23 01:02:03', '2026-10-23 01:02:03.5', '2026-10-23 01:02:03.123456789', '61f0c404-5cb3-11e7-907b-a6006ad3dba0', '1.2.3.4', '2001:db8::1', 'a', NULL, NULL, 'x', NULL, ['a', 'b'], [NULL, 1], {'k': 1}, (1, 'x'), (2, 'y')),
    (2, 255, 65535, 4294967295, 18446744073709551615, 127, 32767, 2147483647, 9223372036854775807, -0.0, -1e-7, false, -0.0001, 12345.67, 'it''s "q"', 'xyz', '1970-01-01', '2149-06-06', '1970-01-01 00:00:00', '1969-12-31 23:59:59.5', '2262-04-11 23:47:16', '00000000-0000-0000-0000-000000000000', '0.0.0.0', '::ffff:1.2.3.4', 'b', '', 0, '', '', [], [], {}, (0, ''), (0, ''));
