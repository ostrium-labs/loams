SELECT id, if(u > 100, NULL, u), toTypeName(if(u > 100, NULL, u)) FROM t33 ORDER BY id
