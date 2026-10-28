-- Langfuse v3: usage detail sums through mapFilter with a tuple lambda.
SELECT id, arraySum(mapValues(mapFilter(x -> (positionCaseInsensitive(x.1, 'input') > 0), usage))), mapFilter((k, v) -> v > 4, usage) FROM t32 ORDER BY id
