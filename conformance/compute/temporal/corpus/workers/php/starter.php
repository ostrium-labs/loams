<?php

declare(strict_types=1);

/*
 * The PHP corpus's starter (DX1 TSDK-php): drives every scenario (or the
 * comma-separated ones given) against a PHP worker that RoadRunner runs
 * (corpus-php.sh starts it first) and writes the observations.
 *
 *   php starter.php <address> <namespace> <out.json> [scenario,...]
 */

require __DIR__ . '/vendor/autoload.php';

use Corpus\Driver;

[, $address, $namespace, $out] = $argv + [null, null, null, null];
$only = $argv[4] ?? null;
if ($address === null || $namespace === null || $out === null) {
    \fwrite(\STDERR, "usage: starter.php <address> <namespace> <out.json> [scenarios]\n");
    exit(2);
}
Driver::connect($address, $namespace)->run($out, $only);
