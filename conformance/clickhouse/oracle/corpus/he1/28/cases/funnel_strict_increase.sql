SELECT uid, windowFunnel(900, 'strict_increase')(ts, ev = 'view', ev = 'view', ev = 'cart') FROM ev28 GROUP BY uid ORDER BY uid
