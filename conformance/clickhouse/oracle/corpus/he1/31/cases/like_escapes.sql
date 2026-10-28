SELECT 'a%b' LIKE 'a\\%b', 'axb' LIKE 'a\\%b', 'a_b' LIKE 'a\\_b', 'a\\bc' LIKE 'a\\bc', 'abc' LIKE 'a\\bc', 'ä' LIKE '_', 'a\nb' LIKE 'a_b', 'a.b' LIKE 'a.b', 'axb' LIKE 'a.b'
