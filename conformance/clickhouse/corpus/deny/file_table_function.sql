-- expect: 344
SELECT * FROM file('/etc/hostname', LineAsString)
