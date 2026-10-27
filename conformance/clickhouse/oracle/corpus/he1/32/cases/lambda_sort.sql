-- Langfuse: rank (tag, count) pairs.
SELECT id, arraySort(pair -> pair.2, arrayZip(names, vals)), arraySort(x -> -x, nums), arraySort((x, k) -> k, tags, arrayEnumerate(tags)) FROM t32 ORDER BY id
