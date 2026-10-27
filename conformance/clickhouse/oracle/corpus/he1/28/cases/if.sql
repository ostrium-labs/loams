SELECT sumIf(u8, i16 > 0), countIf(u8 > 2), avgIf(f64, b), minIf(g, id > 3), maxIf(d, e = 'b'), groupArrayIf(g, u8 > 4), argMaxIf(g, u8, u8 < 5) FROM t28
