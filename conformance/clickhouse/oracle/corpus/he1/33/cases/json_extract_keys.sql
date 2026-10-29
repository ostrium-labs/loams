SELECT id, JSONExtractKeys(j), JSONExtractArrayRaw(j, 'arr') FROM t33 ORDER BY id
