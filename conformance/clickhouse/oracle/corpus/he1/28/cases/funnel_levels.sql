SELECT level, count() AS users FROM (SELECT uid, windowFunnel(600)(ts, ev = 'view', ev = 'cart', ev = 'buy') AS level FROM ev28 GROUP BY uid) GROUP BY level ORDER BY level
