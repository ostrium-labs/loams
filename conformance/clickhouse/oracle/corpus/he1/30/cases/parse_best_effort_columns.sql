SELECT id, parseDateTimeBestEffort(s), parseDateTimeBestEffortOrNull(s), parseDateTime64BestEffort(s, 3) FROM t30 ORDER BY id
