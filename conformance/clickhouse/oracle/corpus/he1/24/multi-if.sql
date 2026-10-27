SELECT multiIf(id=1,CAST(255,'UInt8'),id=2,CAST(-1,'Int8'),CAST(0,'UInt16')) AS value FROM he24.typed ORDER BY id
