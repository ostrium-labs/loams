SELECT id, ifNull(n, 'none'), ifNull(ni, 0), toTypeName(ifNull(ni, 0)), ifNull(s, 'x') FROM t33 ORDER BY id
