SELECT if(id=1,CAST(255,'UInt8'),CAST(-1,'Int8')) AS value,toTypeName(if(id=1,CAST(255,'UInt8'),CAST(-1,'Int8'))) AS type FROM he24.typed ORDER BY id
