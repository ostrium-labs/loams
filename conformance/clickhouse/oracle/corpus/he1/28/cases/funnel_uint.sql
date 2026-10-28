SELECT uid, windowFunnel(500)(t32, ev = 'cart', ev = 'buy') FROM ev28 GROUP BY uid ORDER BY uid
