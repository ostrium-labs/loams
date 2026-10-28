SELECT name, engine, sorting_key, primary_key, partition_key FROM system.tables WHERE database = currentDatabase() ORDER BY name
