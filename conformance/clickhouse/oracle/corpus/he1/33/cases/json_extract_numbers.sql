SELECT id, JSONExtractInt(j, 'n', 'k'), JSONExtractUInt(j, 'n', 'k'), JSONExtractFloat(j, 'n', 'f'), JSONExtractBool(j, 't') FROM t33 ORDER BY id
