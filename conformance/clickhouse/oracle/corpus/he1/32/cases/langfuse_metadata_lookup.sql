-- Langfuse: a metadata value by its name.
SELECT id, arrayElement(vals, indexOf(names, 'env')), vals[indexOf(names, 'step')] FROM t32 ORDER BY id
