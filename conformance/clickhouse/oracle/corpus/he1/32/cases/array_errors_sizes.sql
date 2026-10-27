-- Arrays of different sizes: 190.
SELECT arrayMap((a, b) -> a, nums, tags) FROM t32 WHERE id = 1
