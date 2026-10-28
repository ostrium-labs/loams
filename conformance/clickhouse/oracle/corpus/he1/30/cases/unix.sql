SELECT id, toUnixTimestamp(dt), toUnixTimestamp(d), toUnixTimestamp(dtz), toUnixTimestamp(s), toUnixTimestamp(s, 'Asia/Kolkata') FROM t30 ORDER BY id
