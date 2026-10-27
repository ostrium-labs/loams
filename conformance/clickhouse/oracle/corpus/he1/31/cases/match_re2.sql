SELECT id, match(s, '^[A-Z]'), match(s, '\\d'), match(s, '(?i)straße'), match(s, '\\bWorld\\b'), match(lc, '^(GET|POST)$') FROM t31 ORDER BY id
