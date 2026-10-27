SELECT id, substringUTF8(s, 2, 3), substringUTF8(s, -4), substring(lc, 2), substring(lc, -2, 1) FROM t31 ORDER BY id
