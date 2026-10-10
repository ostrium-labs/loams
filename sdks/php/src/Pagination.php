<?php

declare(strict_types=1);

namespace Loams;

class Pagination
{
    /**
     * @template T
     * @param callable(string): array{items: list<T>, nextPageToken?: ?string} $fetcher
     * @return \Generator<int, T>
     */
    public static function iterate(callable $fetcher): \Generator
    {
        $pageToken = '';
        $index = 0;

        do {
            $page = $fetcher($pageToken);
            $items = $page['items'] ?? [];
            foreach ($items as $item) {
                yield $index++ => $item;
            }

            $next = $page['nextPageToken'] ?? '';
            if (empty($next) || $next === $pageToken) {
                break;
            }
            $pageToken = (string) $next;
        } while (true);
    }
}
