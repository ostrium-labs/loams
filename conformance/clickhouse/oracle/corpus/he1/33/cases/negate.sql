SELECT id, negate(u), negate(i), negate(f), negate(dec), toTypeName(negate(u)), toTypeName(negate(toUInt64(1))) FROM t33 ORDER BY id
