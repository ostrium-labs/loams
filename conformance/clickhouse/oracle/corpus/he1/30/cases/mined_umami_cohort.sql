SELECT id, dateDiff(_CAST('day', 'String'), toDate(parseDateTimeBestEffort(s)), toDate(parseDateTimeBestEffort(formatDateTime(date_trunc('month', dt), '%Y-%m-%d')))) AS x FROM t30 ORDER BY id
