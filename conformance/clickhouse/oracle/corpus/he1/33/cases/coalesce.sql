SELECT id, coalesce(n, s), coalesce(ni, i), coalesce(NULL, n), toTypeName(coalesce(ni, i)), toTypeName(coalesce(NULL, n)) FROM t33 ORDER BY id
