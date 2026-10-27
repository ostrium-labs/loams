-- SigNoz: attribute maps by key; a missing key gives the default.
SELECT id, attrs['service.name'], attrs['missing'], usage['input'], usage['none'] FROM t32 ORDER BY id
