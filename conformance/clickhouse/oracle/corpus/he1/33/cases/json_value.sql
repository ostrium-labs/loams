SELECT id, JSON_VALUE(j, '$.a'), JSON_VALUE(j, '$.n.k'), JSON_VALUE(j, '$.arr[0]'), JSON_VALUE(j, '$.n') FROM t33 ORDER BY id
