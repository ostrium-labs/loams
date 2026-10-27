SELECT id, arrayStringConcat(tags), arrayStringConcat(tags, ', '), arrayStringConcat(nums, '-'), arrayStringConcat(nn, '|') FROM t32 ORDER BY id
