<?php

declare(strict_types=1);

namespace Loams;

/**
 * A consistency token (runtime contract R4): `v1:s<stream>/p<partition>@<offset>,…`,
 * the encoding `crates/loams-collection/src/token.rs` defines.
 *
 * A write answers with one and a read accepts one as `at_least`. Two tokens
 * merge by keeping the largest offset per `(stream, partition)`. Offsets are
 * u64, so they are kept as decimal strings and compared as such.
 */
final class ConsistencyToken implements \Stringable
{
    private const PREFIX = 'v1:';
    private const U32_MAX = '4294967295';
    private const U64_MAX = '18446744073709551615';

    /** @var list<array{0: string, 1: string, 2: string}> in the order the token spells them */
    private array $items;

    /** @param list<array{0: string, 1: string, 2: string}> $items */
    private function __construct(array $items)
    {
        $this->items = $items;
    }

    /** The `(stream, partition, offset)` triples, as decimal strings. @return list<array{0: string, 1: string, 2: string}> */
    public function items(): array
    {
        return $this->items;
    }

    /** Sorted by stream then partition, one item per pair (the largest offset). */
    public function normalized(): self
    {
        return new self(self::normalize($this->items));
    }

    /**
     * Parses `$text`, throwing `InvalidArgumentException` (client-side, no
     * network call) when it is not a `v1:` token.
     */
    public static function parse(string $text): self
    {
        if (!str_starts_with($text, self::PREFIX)) {
            throw self::invalid($text);
        }
        $body = substr($text, strlen(self::PREFIX));
        if ($body === '') {
            return new self([]);
        }
        $items = [];
        foreach (explode(',', $body) as $item) {
            $parsed = self::parseItem($item);
            if ($parsed === null) {
                throw self::invalid($text);
            }
            $items[] = $parsed;
        }
        return new self($items);
    }

    public static function isValid(string $text): bool
    {
        try {
            self::parse($text);
            return true;
        } catch (InvalidArgumentException) {
            return false;
        }
    }

    public function __toString(): string
    {
        return self::PREFIX . implode(',', array_map(
            static fn (array $i): string => "s{$i[0]}/p{$i[1]}@{$i[2]}",
            $this->items
        ));
    }

    /** A new token holding the larger offset of the two for every (stream, partition). */
    public function merge(self|string $other): self
    {
        $other = is_string($other) ? self::parse($other) : $other;
        return new self(self::normalize(array_merge($this->items, $other->items)));
    }

    /** The offset this token holds for one partition, as a decimal string, or null. */
    public function offset(int|string $stream, int|string $partition): ?string
    {
        $best = null;
        foreach ($this->items as [$s, $p, $o]) {
            if ($s === (string) $stream && $p === (string) $partition && ($best === null || self::cmp($o, $best) > 0)) {
                $best = $o;
            }
        }
        return $best;
    }

    public function isEmpty(): bool
    {
        return $this->items === [];
    }

    public function equals(self $other): bool
    {
        return $this->items === $other->items;
    }

    /** @return ?array{0: string, 1: string, 2: string} */
    private static function parseItem(string $item): ?array
    {
        if (!preg_match('/\As(0|[1-9][0-9]*)\/p(0|[1-9][0-9]*)@(0|[1-9][0-9]*)\z/', $item, $m)) {
            return null;
        }
        if (!self::fits($m[1], self::U64_MAX) || !self::fits($m[2], self::U32_MAX) || !self::fits($m[3], self::U64_MAX)) {
            return null;
        }
        return [$m[1], $m[2], $m[3]];
    }

    private static function fits(string $n, string $max): bool
    {
        return self::cmp($n, $max) <= 0;
    }

    /** Compares two canonical decimal strings. */
    private static function cmp(string $a, string $b): int
    {
        return strlen($a) <=> strlen($b) ?: strcmp($a, $b) <=> 0;
    }

    /**
     * @param list<array{0: string, 1: string, 2: string}> $items
     * @return list<array{0: string, 1: string, 2: string}>
     */
    private static function normalize(array $items): array
    {
        usort($items, static fn (array $x, array $y): int =>
            self::cmp($x[0], $y[0]) ?: self::cmp($x[1], $y[1]) ?: self::cmp($x[2], $y[2]));
        $out = [];
        foreach ($items as $item) {
            $last = count($out) - 1;
            if ($last >= 0 && $out[$last][0] === $item[0] && $out[$last][1] === $item[1]) {
                $out[$last] = $item;
            } else {
                $out[] = $item;
            }
        }
        return $out;
    }

    private static function invalid(string $text): InvalidArgumentException
    {
        return InvalidArgumentException::client('invalid consistency token ' . json_encode($text), 'invalid_consistency_token');
    }
}

/**
 * The session store (R4, `sessionConsistency: true`): every token an answer
 * carries is merged in, and every read that leaves `consistency` unset is
 * sent with `atLeast` set to the session's token.
 *
 * One token per proto package: a graph's tokens name the graph's own stream
 * and a graph read refuses a token of another stream.
 */
final class SessionStore
{
    /** The response fields a token is read from, in proto3 JSON. */
    public const TOKEN_FIELDS = ['token', 'readToken', 'consistencyToken'];

    /** @var array<string, ConsistencyToken> */
    private array $tokens = [];

    public function token(string $scope = 'loams.collection.v1'): ?ConsistencyToken
    {
        return $this->tokens[$scope] ?? null;
    }

    /** Merges `$text` into the session; a token that does not parse is ignored. */
    public function observe(string $scope, ?string $text): void
    {
        if ($text === null || $text === '' || !ConsistencyToken::isValid($text)) {
            return;
        }
        $parsed = ConsistencyToken::parse($text);
        $current = $this->tokens[$scope] ?? null;
        $this->tokens[$scope] = $current === null ? $parsed : $current->merge($parsed);
    }

    /** @param array<string, mixed> $answer */
    public function observeAnswer(string $scope, array $answer): void
    {
        foreach (self::TOKEN_FIELDS as $field) {
            if (isset($answer[$field]) && is_string($answer[$field])) {
                $this->observe($scope, $answer[$field]);
            }
        }
    }

    /**
     * Sets `consistency.atLeast` when the request leaves `consistency` unset.
     *
     * @param array<string, mixed> $request
     * @return array<string, mixed>
     */
    public function apply(string $scope, array $request): array
    {
        if (array_key_exists('consistency', $request) && $request['consistency'] !== null) {
            return $request;
        }
        $current = $this->tokens[$scope] ?? null;
        if ($current === null || $current->isEmpty()) {
            return $request;
        }
        $request['consistency'] = ['atLeast' => (string) $current];
        return $request;
    }
}
