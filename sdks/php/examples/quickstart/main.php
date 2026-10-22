<?php

declare(strict_types=1);

require __DIR__ . '/../bootstrap.php';

use Loams\Client;

$client = new Client(loams_endpoint());

$instance = $client->instances->getInstance();
printf("instance %s (%s) server %s\n", $instance['instanceId'] ?? '?', $instance['edition'] ?? '?', $instance['serverVersion'] ?? '?');
echo 'api versions: ', implode(', ', $instance['apiVersions'] ?? []), "\n";

$me = $client->instances->whoAmI();
echo 'whoami: ', json_encode($me), "\n";
