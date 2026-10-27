-- OpenPanel slices events; GatherUtils' offset and length rules.
SELECT id, arraySlice(nums, 2), arraySlice(nums, -2), arraySlice(nums, 2, 1), arraySlice(nums, -3, 2), arraySlice(nums, 1, -1), arraySlice(nums, 0) FROM t32 ORDER BY id
