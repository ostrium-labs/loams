SELECT sumOrNull(u8), sumOrDefault(n), avgOrNull(f64), avgOrDefault(f64), maxOrNull(g), countOrNull(), groupArrayOrNull(u8) FROM t28 WHERE id > 1000
