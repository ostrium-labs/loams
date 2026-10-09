-- expect: 344
SELECT * FROM s3('http://127.0.0.1:1/b/k', CSV, 'a String') AS `x, table_function_name: numbers`
