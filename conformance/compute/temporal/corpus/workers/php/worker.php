<?php

declare(strict_types=1);

/*
 * The PHP corpus worker (DX1 TSDK-php), started by RoadRunner (.rr.yaml:
 * `server.command`). RoadRunner's temporal plugin (Go SDK inside the rr
 * binary) talks to the server; this process runs the workflow code
 * (src/Workflows.php) and the activities (src/Activities.php) it hands over
 * the Goridge pipe.
 *
 * One activity at a time, as the Go generator: the parallel fan-out's
 * activities complete in the order they were scheduled, so regenerating is
 * stable. TEMPORAL_API_KEY, when set, is passed to RoadRunner, which sends it
 * as `authorization: Bearer <key>` (TLS stays off without a `temporal.tls`
 * block, the gateway profile's plaintext).
 */

require __DIR__ . '/vendor/autoload.php';

use Corpus\Activities;
use Corpus\Scenarios;
use Corpus\Workflows\All;
use Temporal\Worker\ServiceCredentials;
use Temporal\Worker\WorkerOptions;
use Temporal\WorkerFactory;

$credentials = ServiceCredentials::create();
$key = \getenv('TEMPORAL_API_KEY');
if (\is_string($key) && $key !== '') {
    $credentials = $credentials->withApiKey($key);
}

$factory = WorkerFactory::create(credentials: $credentials);
$worker = $factory->newWorker(
    Scenarios::TASK_QUEUE,
    WorkerOptions::new()->withMaxConcurrentActivityExecutionSize(1),
);
$worker->registerWorkflowTypes(...All::classes());
$worker->registerActivityImplementations(new Activities());
$factory->run();
