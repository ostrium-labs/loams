SELECT name FROM system.tables WHERE database = currentDatabase() AND name LIKE 't38%' ORDER BY name
