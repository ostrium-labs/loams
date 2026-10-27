-- OpenPanel builds pair indexes with range.
SELECT id, range(length(nums)), range(1, length(nums) + 1), range(10, 0, -3) FROM t32 ORDER BY id
