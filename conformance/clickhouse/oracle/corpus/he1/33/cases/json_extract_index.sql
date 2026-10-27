SELECT id, JSONExtractInt(j, 'arr', 1), JSONExtractInt(j, 'arr', -1), JSONExtractInt(j, 'arr', 4) FROM t33 ORDER BY id
