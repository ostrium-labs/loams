-- HE1 Task 31: strings, regular expressions, hashes and UUIDs over a small
-- table with UTF-8 edges, empty strings and NULLs.
DROP TABLE IF EXISTS t31;
CREATE TABLE t31 (id UInt32, s String, lc LowCardinality(String), ns Nullable(String), fs FixedString(4), attrs Map(LowCardinality(String), String), bounds Array(Float64), u UUID) ENGINE = MergeTree ORDER BY id;
INSERT INTO t31 VALUES (1, 'Hello, World', 'GET', 'abc', 'ab', {'service.name':'api','env':'prod'}, [0.5, 1, 2.5], '61f0c404-5cb3-11e7-907b-a6006ad3dba0'), (2, '', 'POST', NULL, '', {}, [], '00000000-0000-0000-0000-000000000000'), (3, 'Straße ÄÖÜ İstanbul', 'get', '', 'äb', {'k':''}, [1e10], 'ffffffff-ffff-ffff-ffff-ffffffffffff'), (4, 'ΣΊΣΥΦΟΣ año 日本語', 'Put', 'x%y_z', 'abcd', {'a':'1','b':'2','c':'3'}, [-1.5, 0], '123e4567-e89b-12d3-a456-426614174000'), (5, 'a\\b%c_d', 'GET', 'A.B', 'z', {'service.name':'web'}, [nan, inf], 'a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11');
