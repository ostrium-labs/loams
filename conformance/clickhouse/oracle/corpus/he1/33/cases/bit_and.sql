SELECT id, bitAnd(u, 15), bitOr(u, 1), bitXor(u, 255), bitNot(u), toTypeName(bitAnd(u, toInt16(1))) FROM t33 ORDER BY id
