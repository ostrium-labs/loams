SELECT id, positionUTF8(s, 'İ'), positionUTF8(s, '本'), positionCaseInsensitiveUTF8(s, 'straSSe'), positionCaseInsensitiveUTF8(s, 'äöü') FROM t31 ORDER BY id
