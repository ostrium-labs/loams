SELECT g, groupUniqArray(g), groupUniqArray(lc), groupUniqArray(1)(e) FROM t28 WHERE id IN (1, 2, 3, 6) GROUP BY g ORDER BY g
