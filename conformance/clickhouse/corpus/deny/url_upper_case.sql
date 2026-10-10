-- expect: 344
SELECT * FROM URL('http://127.0.0.1:1/x', CSV, 'a String')
