-- Langfuse: tag options.
SELECT id, arrayFilter(value -> length(value) > 0, arrayDistinct(tags)), arrayFilter(x -> x > 15, nums), arrayFilter(x -> x IS NOT NULL, nn) FROM t32 ORDER BY id
