-- expect: 344
CREATE DICTIONARY d (k UInt64, v String) PRIMARY KEY k SOURCE(HTTP(url 'http://127.0.0.1:1/' format 'TSV')) LAYOUT(FLAT()) LIFETIME(0)
