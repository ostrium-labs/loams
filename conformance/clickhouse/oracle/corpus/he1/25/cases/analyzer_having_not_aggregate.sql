SELECT a % 2 AS n, count() FROM t25 GROUP BY n HAVING a > 1
