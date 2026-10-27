-- oracle: order = unordered
SELECT id FROM (SELECT id FROM t37 UNION SELECT id FROM t37) SETTINGS union_default_mode = 'DISTINCT'
