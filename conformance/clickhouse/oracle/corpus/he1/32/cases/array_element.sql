-- arrayElement: 1-based, negative, out of range gives the default.
SELECT id, nums[1], nums[-1], nums[5], nums[0], tags[2], nn[2], nested[1], lcarr[1] FROM t32 ORDER BY id
