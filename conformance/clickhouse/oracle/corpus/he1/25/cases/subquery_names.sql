SELECT (SELECT max(a) FROM t25), (SELECT min(a) FROM t25) AS s, (SELECT max(k) FROM u25)
