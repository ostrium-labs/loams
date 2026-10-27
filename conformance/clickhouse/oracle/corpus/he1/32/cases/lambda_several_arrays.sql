-- Langfuse: metadata truncated, over values and their indexes.
SELECT id, arrayExists((v, i) -> ((length(v) > 3) AND (arrayFirstIndex(n -> (n = (names[i])), names) = i)), vals, arrayEnumerate(vals)), arrayMap((n, v) -> concat(n, '=', v), names, vals) FROM t32 ORDER BY id
