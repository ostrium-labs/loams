-- expect: 344
WITH (SELECT count() FROM url('http://127.0.0.1:1/x', CSV, 'a String')) AS n SELECT n
