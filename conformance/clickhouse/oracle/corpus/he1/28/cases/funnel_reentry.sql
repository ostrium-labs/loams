SELECT uid, windowFunnel(2400, 'strict_order', 'allow_reentry')(ts, ev = 'view', ev = 'cart', ev = 'buy') FROM ev28 GROUP BY uid ORDER BY uid
