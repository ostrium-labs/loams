-- expect: 344
SELECT arrayMap(x -> file(x), ['/etc/hostname'])
