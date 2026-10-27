SELECT count(), sum(u8), sum(f64), avg(f64), min(g), max(dt), any(i64), anyLast(e), argMax(g, u8), groupArray(u8), sumMap(ks, vs) FROM t28 WHERE id > 1000
