-- A filter on a column stored as text (Enum): not pruned, still applied.
SELECT id FROM part26 WHERE e = 'b' AND id IN (1001, 2001, 3001) ORDER BY id
