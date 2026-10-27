-- arrayDistinct drops NULLs and keeps first occurrences.
SELECT id, arrayDistinct(tags), arrayDistinct(nn), arrayDistinct(lcarr) FROM t32 ORDER BY id
