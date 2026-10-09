-- expect: 62
SELECT * FROM system.disks FORMAT JSONEachRow SETTINGS max_threads = 1
