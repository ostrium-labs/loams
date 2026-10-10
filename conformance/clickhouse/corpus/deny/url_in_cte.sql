-- expect: 344
WITH c AS (SELECT * FROM url('http://127.0.0.1:1/x', CSV, 'a String')) SELECT * FROM c
