SELECT uid, windowFunnel(1800)(ts, ev = 'view', ev = 'cart', ev = 'checkout', ev = 'buy') AS level FROM ev28 GROUP BY uid ORDER BY uid
