<?php

declare(strict_types=1);

require __DIR__ . '/../bootstrap.php';

use Loams\Client;

// A small health/feature probe: exits 0 if the engine is reachable, non-zero otherwise.
// Usage: php status.php [feature ...]   (exit 2 if a named feature flag is off)
$client = new Client(loams_endpoint(), null, 1);
try {
    $info = $client->instances->getInstance();
} catch (Throwable $e) {
    fwrite(STDERR, 'unreachable: ' . $e->getMessage() . "\n");
    exit(1);
}
$features = $info['features'] ?? [];
ksort($features);
foreach ($features as $name => $on) {
    printf("%-24s %s\n", $name, $on ? 'on' : 'off');
}
foreach (array_slice($argv, 1) as $want) {
    if (empty($features[$want])) {
        fwrite(STDERR, "feature $want is off\n");
        exit(2);
    }
}
