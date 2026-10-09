-- expect: 344
SELECT * FROM numbers(1) AS n CROSS JOIN url('http://127.0.0.1:1/x', CSV, 'a String') AS u
