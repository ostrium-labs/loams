SELECT id, JSON_QUERY(j, '$.n'), JSON_EXISTS(j, '$.arr') FROM t33 ORDER BY id
