-- A DateTime64(9) from a long of nanoseconds (R9.61).
SELECT id, dt64n FROM types26 WHERE dt64n > toDateTime64('2000-01-01 00:00:00', 9, 'UTC') ORDER BY id
