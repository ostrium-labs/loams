-- expect: 344
INSERT INTO t VALUES ((SELECT a FROM url('http://127.0.0.1:1/x', CSV, 'a String')))
