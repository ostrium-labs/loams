-- Langfuse: hasAll on tags.
SELECT id, hasAll(tags, ['a']), hasAll(tags, []), hasAny(tags, ['z', 'a']), hasAny(tags, []) FROM t32 ORDER BY id
