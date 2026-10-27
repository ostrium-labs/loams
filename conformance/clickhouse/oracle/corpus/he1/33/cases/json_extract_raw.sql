SELECT id, JSONExtractRaw(j, 'n'), JSONExtractRaw(j, 'arr'), JSONExtractRaw(j, 'a') FROM t33 ORDER BY id
