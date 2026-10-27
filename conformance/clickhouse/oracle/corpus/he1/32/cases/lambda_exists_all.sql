-- HyperDX and Langfuse: arrayExists; Plausible: arrayAll over two arrays.
SELECT id, arrayExists(x -> x = 'b', tags), arrayAll(x -> x > 5, nums), arrayAll((k, v) -> length(k) > 0, names, vals), arrayExists(nums), arrayAll(nums) FROM t32 ORDER BY id
