-- expect: 344
SELECT 1 WHERE 1 IN (SELECT 1 FROM system.stack_trace)
