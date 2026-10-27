-- A column ALTER added reads its DEFAULT for older rows; an ALIAS; a renamed column.
SELECT id, label, plus, n, z FROM alter26 ORDER BY id
