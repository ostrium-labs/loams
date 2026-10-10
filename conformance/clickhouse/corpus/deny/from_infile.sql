-- expect: 344
INSERT INTO t FROM INFILE '/etc/hostname' FORMAT LineAsString
