SELECT id, d - toDate('2024-01-01'), dt - toDateTime('2024-01-01 00:00:00'), dt64 - toDateTime64('2024-01-01 00:00:00', 3), dtz - dt FROM t30 ORDER BY id
