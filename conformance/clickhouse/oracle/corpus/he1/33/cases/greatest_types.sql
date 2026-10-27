SELECT toTypeName(greatest(u, toInt8(1))), toTypeName(least(f32, u)) FROM t33 WHERE id = 1
