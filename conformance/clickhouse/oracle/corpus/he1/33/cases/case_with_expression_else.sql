SELECT id, CASE u WHEN 200 THEN 'two hundred' WHEN 0 THEN 'zero' ELSE 'other' END FROM t33 ORDER BY id
