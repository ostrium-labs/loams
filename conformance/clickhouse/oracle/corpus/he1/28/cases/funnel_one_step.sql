SELECT windowFunnel(10)(ts, ev = 'buy'), windowFunnel(10, 'strict_order')(ts, ev = 'buy') FROM ev28 WHERE uid = 3
