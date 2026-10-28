SELECT id, cityHash64(attrs), cityHash64(bounds), cityHash64(attrs, attrs, bounds) FROM t31 ORDER BY id
