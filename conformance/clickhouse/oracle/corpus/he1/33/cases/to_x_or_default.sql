SELECT id, toFloat64OrDefault(s), toUInt8OrDefault(s, toUInt8(9)), toInt64OrDefault(s, toInt64(-1)) FROM t33 ORDER BY id
