-- OpenPanel, HyperDX: arrayFirstIndex; Umami: arrayFirst.
SELECT id, arrayFirstIndex(x -> x = 'b', tags), arrayFirst(x -> x > 15, nums), arrayFirstIndex(x -> x > 1, arrayEnumerateUniq(tags)), arrayFirst(x -> 1, tags) FROM t32 ORDER BY id
