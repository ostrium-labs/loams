SELECT id, nested, arrayMap(a -> length(a), nested), arrayMap(a -> arraySum(a), nested), arrayConcat(nested, [[9]]) FROM t32 ORDER BY id
