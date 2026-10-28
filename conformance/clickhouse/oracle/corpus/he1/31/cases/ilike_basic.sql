SELECT id, s ILIKE '%world%', lc ILIKE 'get', s NOT ILIKE '%STRASSE%', s ILIKE '%straße%', ilike(s, '%σίσυφος%') FROM t31 ORDER BY id
