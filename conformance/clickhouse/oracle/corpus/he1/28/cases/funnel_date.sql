SELECT uid, windowFunnel(2)(day, ev = 'view', ev = 'buy') FROM ev28 GROUP BY uid ORDER BY uid
