SELECT uid, windowFunnel(1200, 'strict_once')(ts, ev IN ('view', 'cart'), ev = 'cart', ev IN ('cart', 'buy')) FROM ev28 GROUP BY uid ORDER BY uid
