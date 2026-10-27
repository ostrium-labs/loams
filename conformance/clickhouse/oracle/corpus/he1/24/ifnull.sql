SELECT ifNull(n,CAST(7,'Int32')) AS value,toTypeName(ifNull(n,CAST(7,'Int32'))) AS type FROM he24.typed ORDER BY id
