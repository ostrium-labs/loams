<?php

declare(strict_types=1);

/*
 * corpus_replays_php (DX1 TSDK-php): every PHP history in a directory
 * replays with the pinned SDK's WorkflowReplayer and zero nondeterminism
 * errors. The gateway's PHP leg points it at the histories the gateway
 * wrote; generate-php.sh at the ones it just generated.
 *
 *   CORPUS_HISTORIES=dir php replay.php     # default: ../../histories/php
 *
 * The PHP SDK's WorkflowReplayer is a RoadRunner RPC client: RoadRunner's
 * temporal plugin replays the history with the Go SDK's replayer, driving
 * this worker's PHP workflow code (corpus-php.sh starts `rr serve` first).
 * replayFromJSON hands RoadRunner the path of the protojson file the Go
 * exporter wrote, which the Go replayer reads natively. The workflow type is
 * the scenario's name (the file name without `.run<k>.json`).
 *
 * RoadRunner's replay does not take the original workflow ID (the Go
 * replayer's default is used), so the PHP workflows never read their own ID:
 * the child scenarios build the child's ID from the workflow type
 * (Workflows.php, Child).
 */

require __DIR__ . '/vendor/autoload.php';

use Temporal\Testing\Replay\Exception\ReplayerException;
use Temporal\Testing\Replay\WorkflowReplayer;

$dir = \getenv('CORPUS_HISTORIES') ?: __DIR__ . '/../../histories/php';
$files = \glob(\rtrim($dir, '/') . '/*.json') ?: [];
\sort($files);
if ($files === []) {
    \fwrite(\STDERR, "no histories in {$dir}\n");
    exit(1);
}

$replayer = new WorkflowReplayer();
$failures = [];
foreach ($files as $f) {
    $type = (string) \preg_replace('/(\.run\d+)?\.json$/', '', \basename($f));
    // RoadRunner answers FAILED_PRECONDITION until its workflow pool is up.
    for ($try = 1; ; $try++) {
        try {
            $replayer->replayFromJSON($type, \realpath($f));
            echo "ok   ", \basename($f), "\n";
            break;
        } catch (ReplayerException $e) {
            if ($try < 30 && \str_contains($e->getMessage(), 'not initialized')) {
                \sleep(1);
                continue;
            }
            $failures[] = \basename($f) . ': ' . $e::class . ': ' . $e->getMessage();
            echo "FAIL ", \end($failures), "\n";
            break;
        }
    }
}
\printf("corpus_replays_php: %d/%d replayed\n", \count($files) - \count($failures), \count($files));
exit($failures === [] ? 0 : 1);
