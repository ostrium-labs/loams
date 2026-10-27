-- Langfuse: metadata as a map, newest value of a name first.
SELECT id, mapFromArrays(arrayReverse(names), arrayReverse(vals)), mapFromArrays(names, vals)['env'] FROM t32 ORDER BY id
