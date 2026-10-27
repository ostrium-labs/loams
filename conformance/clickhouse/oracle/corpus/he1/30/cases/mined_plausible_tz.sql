SELECT id, toDate(toTimeZone(dt, _CAST('America/New_York', 'String'))) AS time, toStartOfMonth(toTimeZone(dt, 'Asia/Kolkata')) AS month FROM t30 ORDER BY id
