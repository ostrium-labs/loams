-- expect: ok
SELECT count() FROM view(SELECT * FROM generateRandom('a UInt8', 1) LIMIT 3)
