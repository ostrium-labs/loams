-- expect: 344
ATTACH TABLE a (x String) ENGINE = File(CSV, '/etc/hostname')
