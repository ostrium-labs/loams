-- An Enum and an IPv4 compared as ClickHouse types.
SELECT id FROM types26 WHERE e = 'a' AND ip4 != toIPv4('0.0.0.0') ORDER BY id
