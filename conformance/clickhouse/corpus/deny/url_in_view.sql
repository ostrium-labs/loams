-- expect: 344
SELECT * FROM view(SELECT * FROM url('http://127.0.0.1:1/x', CSV, 'a String'))
