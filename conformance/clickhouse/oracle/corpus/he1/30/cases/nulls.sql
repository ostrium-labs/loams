SELECT id, toDate(ns), toStartOfHour(ns), ns + INTERVAL 1 DAY, dateDiff('day', ns, dt), toYYYYMM(ns), toUnixTimestamp(ns), formatDateTime(ns, '%F') FROM t30 ORDER BY id
