SELECT id, arrayEnumerate(tags), arrayEnumerateUniq(tags), arrayEnumerateUniq(tags, lcarr) FROM t32 ORDER BY id
