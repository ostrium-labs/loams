SELECT id, nullIf(s, ''), nullIf(u, 0), toTypeName(nullIf(u, 0)) FROM t33 ORDER BY id
