SELECT id FROM t30 WHERE toStartOfHour(dt) <= toStartOfHour(subtractHours(now(), _CAST(1, 'Int64'))) ORDER BY id
