-- expect: 344
SELECT * FROM url /* hi */ ('http://127.0.0.1:1/x', CSV, 'a String')
