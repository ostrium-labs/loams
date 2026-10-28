SELECT id, multiIf(i < 0, 'neg', i = 0, 'zero', 'pos') AS sign FROM t33 ORDER BY id
