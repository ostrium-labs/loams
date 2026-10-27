SELECT id, ifNotFinite(f, 0), ifNotFinite(f32, -1), ifNotFinite(u, 1) FROM t33 ORDER BY id
