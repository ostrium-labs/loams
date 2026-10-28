SELECT id, cityHash64(s, lc), cityHash64(id), cityHash64(u), cityHash64(tuple(s, id)) FROM t31 ORDER BY id
