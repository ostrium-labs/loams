SELECT uid, windowFunnel(1800, 'strict_order')(ts, ev = 'view', ev = 'cart', ev = 'checkout') FROM ev28 GROUP BY uid ORDER BY uid
