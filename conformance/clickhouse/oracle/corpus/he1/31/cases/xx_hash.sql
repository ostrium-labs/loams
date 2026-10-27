SELECT id, xxHash32(s), xxHash64(s), xxHash32(lc), xxHash32(s, lc), xxHash64(s, lc) FROM t31 ORDER BY id
