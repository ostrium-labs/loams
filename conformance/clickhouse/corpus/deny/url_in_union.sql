-- expect: 344
SELECT 'x' UNION ALL SELECT a FROM url('http://127.0.0.1:1/x', CSV, 'a String')
