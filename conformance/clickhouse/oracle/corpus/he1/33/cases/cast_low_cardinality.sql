SELECT toTypeName(CAST(s AS LowCardinality(String))), CAST(lc AS String) FROM t33 ORDER BY id
