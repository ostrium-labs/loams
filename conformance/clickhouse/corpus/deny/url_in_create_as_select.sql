-- expect: 344
CREATE TEMPORARY TABLE c AS SELECT * FROM url('http://127.0.0.1:1/x', CSV, 'a String')
