SELECT id, JSONExtract(j, 'arr', 'Array(UInt8)'), JSONExtract(j, 'a', 'Nullable(String)'), JSONExtract(j, 'n', 'k', 'Int64') FROM t33 ORDER BY id
