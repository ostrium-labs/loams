-- expect: 344
SELECT * FROM merge(REGEXP('^sys'), '^disks$')
