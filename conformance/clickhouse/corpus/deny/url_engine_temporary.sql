-- expect: 344
CREATE TEMPORARY TABLE c (a String) ENGINE = URL('http://127.0.0.1:1/x', CSV)
