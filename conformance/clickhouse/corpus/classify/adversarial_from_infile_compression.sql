-- kind: error 344
INSERT INTO t FROM INFILE '/etc/passwd' COMPRESSION 'gzip' FORMAT TSV
