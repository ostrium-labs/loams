-- expect: 344
SELECT 1 WHERE 'x' IN (SELECT a FROM url('http://127.0.0.1:1/x', CSV, 'a String'))
