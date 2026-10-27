SELECT id, toTimeZone(dt, 'Asia/Kolkata'), toTimezone(dt64u, 'America/New_York'), toTimeZone(dny, 'UTC') FROM t30 ORDER BY id
