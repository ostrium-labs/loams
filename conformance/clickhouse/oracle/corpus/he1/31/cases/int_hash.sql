SELECT id, intHash64(id), intHash32(id), intHash64(toInt8(-1)), intHash32(toUInt64(18446744073709551615)) FROM t31 ORDER BY id
