SELECT id, arrayCount(x -> x > 10, nums), arrayCount(nums), arraySum(x -> x * 2, nums), arraySum(v -> length(v), vals) FROM t32 ORDER BY id
