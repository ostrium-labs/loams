SELECT id, replaceRegexpAll(s, '[aeiou]', '*'), replaceRegexpOne(s, '(\\w+)', '<\\1>'), replaceRegexpAll(lc, '^(.)(.*)$', '\\2\\1') FROM t31 ORDER BY id
