-- Partition pruning.
SELECT id, e, s FROM part26 WHERE p = 2 AND id < 2003 ORDER BY id
