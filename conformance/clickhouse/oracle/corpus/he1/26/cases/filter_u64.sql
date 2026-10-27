-- Filters on restored types: UInt64 from decimal(20, 0), Enum from names, IPv4 from text.
SELECT id, u64, e, ip4 FROM types26 WHERE u64 > 9223372036854775807 ORDER BY id
