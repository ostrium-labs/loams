-- expect: 344
INSERT INTO FUNCTION file('/tmp/loams-deny-x', CSV, 'a String') SELECT 'x'
