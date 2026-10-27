SELECT id, formatDateTime(DATE_TRUNC('hour', dt), '%Y/%m/%d %H') AS label FROM t30 WHERE dt >= parseDateTimeBestEffort('2024-01-01 00:00:00') ORDER BY id
