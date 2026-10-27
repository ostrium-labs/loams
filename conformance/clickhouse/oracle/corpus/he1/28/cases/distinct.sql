SELECT sumDistinct(g = 'x'), sum(DISTINCT u8 > 3), countDistinct(e), avgDistinct(i16 > 0), sumDistinctIf(b, u8 > 2), sumIfDistinct(b, u8 > 2), groupArrayDistinct(e) FROM t28 WHERE id < 3
