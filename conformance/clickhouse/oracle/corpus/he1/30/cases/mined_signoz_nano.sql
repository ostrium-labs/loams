SELECT id, toStartOfInterval(fromUnixTimestamp64Nano(nanos), INTERVAL _CAST(60, 'Int64') SECOND) AS ts FROM t30 WHERE nanos > 0 ORDER BY id
