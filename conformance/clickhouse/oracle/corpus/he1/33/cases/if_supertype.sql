SELECT toTypeName(if(u > 0, u, i)), toTypeName(if(u > 0, u, toInt8(1))), toTypeName(if(u > 0, f32, toInt16(1))) FROM t33 WHERE id = 1
