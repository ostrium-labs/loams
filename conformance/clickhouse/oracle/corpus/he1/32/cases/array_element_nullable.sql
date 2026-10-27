-- An out-of-range Nullable element is NULL.
SELECT id, nn[1], nn[9] FROM t32 ORDER BY id
