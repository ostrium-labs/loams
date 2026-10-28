SELECT id, formatDateTime(date_trunc(_CAST('day', 'String'), dt, _CAST('Asia/Kolkata', 'String')), _CAST('%Y-%m-%d %H:%i:%S', 'String')) AS t FROM t30 ORDER BY id
