SELECT uid, windowFunnel(3000, 'strict_deduplication', 'strict_increase', 'strict_once')(ts, ev = 'view', ev = 'checkout', ev = 'buy') FROM ev28 GROUP BY uid ORDER BY uid
