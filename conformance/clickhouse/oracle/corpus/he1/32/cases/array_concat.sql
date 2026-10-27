SELECT id, arrayConcat(tags, ['c']), arrayConcat(nums, [1000]), arrayConcat(tags, tags) FROM t32 ORDER BY id
