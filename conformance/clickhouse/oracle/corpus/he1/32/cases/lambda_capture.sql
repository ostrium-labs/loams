SELECT id, arrayMap(x -> x + id, nums), arrayFilter(t -> t != tags[1], tags) FROM t32 ORDER BY id
