SELECT sumIf(u8, u8 > 1) AS s, COUNT() AS c, array_agg(id), last_value(g), first_value(g), any_value(g) FROM t28
