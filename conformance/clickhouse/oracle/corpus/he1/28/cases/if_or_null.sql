SELECT sumIfOrNull(u8, u8 > 100), sumOrNullIf(u8, u8 > 100), avgIfOrDefault(f64, u8 > 100), countIfOrNull(u8 > 100), sumIfOrNull(u8, u8 > 1) FROM t28
