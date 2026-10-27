SELECT g, count() AS c, sum(u8), avg(f64), min(i16), max(lc), any(e), argMax(id, f64) FROM t28 GROUP BY g ORDER BY g
