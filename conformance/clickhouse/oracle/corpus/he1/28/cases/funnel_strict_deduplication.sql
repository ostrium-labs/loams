SELECT uid, windowFunnel(3600, 'strict_deduplication')(ts, ev = 'view', ev = 'cart', ev = 'checkout', ev = 'buy') FROM ev28 GROUP BY uid ORDER BY uid
