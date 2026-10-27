-- Langfuse: truncate metadata values.
SELECT id, arrayMap(v -> substring(v, 1, 2), vals), arrayMap(x -> x * 2, nums), arrayMap(x -> isNull(x), nn) FROM t32 ORDER BY id
