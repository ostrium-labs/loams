SELECT id, multiIf(u > 250, 'a', u > 100, 'b', u = 0, 'c', 'd') FROM t33 ORDER BY id
