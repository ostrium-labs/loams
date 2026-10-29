SELECT id, JSONExtractString(j, 'a'), JSONExtractString(j, 'n'), JSONExtractString(j, 'missing') FROM t33 ORDER BY id
