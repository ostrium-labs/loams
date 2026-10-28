SELECT id FROM t30 WHERE ms >= toUnixTimestamp(toDateTime('2024-03-06 00:00:00') - toIntervalMinute(_CAST(30, 'Int64'))) ORDER BY id
