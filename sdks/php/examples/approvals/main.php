<?php

declare(strict_types=1);

require __DIR__ . '/../bootstrap.php';

use Loams\Client;
use Loams\LoamsException;
use Loams\Pagination;

$client = new Client(loams_endpoint());

try {
    // Page through every approval; Pagination::iterate follows nextPageToken.
    $count = 0;
    $pages = static fn (string $token): array => (function () use ($client, $token): array {
        $res = $client->approvals->listApprovals($token === '' ? [] : ['pageToken' => $token]);
        return ['items' => $res['approvals'] ?? [], 'nextPageToken' => $res['nextPageToken'] ?? ''];
    })();
    foreach (Pagination::iterate($pages) as $approval) {
        $count++;
        printf("%s  %s\n", $approval['approvalId'] ?? $approval['id'] ?? '?', $approval['state'] ?? '');
    }
    echo "$count approval(s)\n";
} catch (LoamsException $e) {
    // Typed errors map the server's reason (e.g. UnauthenticatedException).
    if (str_contains($e->getMessage(), 'no such route')) {
        echo "the approvals service is not served by this engine\n";
        exit(0);
    }
    fwrite(STDERR, get_class($e) . ': ' . $e->getMessage() . "\n");
    exit(1);
}
