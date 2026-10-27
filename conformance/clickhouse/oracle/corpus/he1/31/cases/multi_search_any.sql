SELECT id, multiSearchAny(s, ['World', 'año']), multiSearchAny(lc, ['ET', 'OS']), multiSearchAny(s, ['']), multiSearchAnyCaseInsensitive(s, ['world']) FROM t31 ORDER BY id
