<?php

declare(strict_types=1);

namespace Loams;

use Loams\Builders\Json;

/**
 * What a `writeBatch` applied: the merged consistency token (null when no
 * chunk answered one), the chunks and the rows.
 */
final class BatchResult
{
    public function __construct(
        public readonly ?string $token,
        public readonly int $chunks,
        public readonly int $rows
    ) {
    }
}

/**
 * A `writeBatch` stopped at a failed chunk (R13): the chunks before it were
 * applied, and `token` is their merged consistency token.
 */
final class BatchException extends LoamsException
{
    /** @var array<string, string> `chunks_applied`, `rows_applied` and `cause_reason` */
    public readonly array $metadata;

    public function __construct(
        public readonly LoamsException $cause,
        public readonly int $chunksApplied,
        public readonly int $rowsApplied,
        public readonly ?string $token
    ) {
        $this->metadata = ['chunks_applied' => (string) $chunksApplied, 'rows_applied' => (string) $rowsApplied]
            + ($cause->reason === null ? [] : ['cause_reason' => $cause->reason]);
        parent::__construct(
            sprintf(
                'writeBatch stopped at chunk %d: %d chunks (%d operations) were applied before it; %s',
                $chunksApplied + 1,
                $chunksApplied,
                $rowsApplied,
                $cause->getMessage()
            ),
            // SDKP-23: registered under `aborted`; the failed chunk's error is the cause.
            'aborted',
            'batch_partially_applied',
            null,
            $cause->httpStatus,
            $cause->rawDetails,
            $cause
        );
    }
}

/**
 * R13: a large write as a sequence of `WriteDocuments` chunks. One chunk is
 * in flight at a time; a chunk closes at `$chunkRows` ops or when the next op
 * would take it past `$chunkBytes` of JSON. Each chunk gets its own
 * idempotency key, which the transport reuses on that chunk's retries.
 */
final class Batch
{
    public const DEFAULT_CHUNK_ROWS = 10000;
    public const DEFAULT_CHUNK_BYTES = 4 * 1024 * 1024;

    /**
     * @param iterable<array<string, mixed>> $ops `WriteOp` JSON
     * @param callable(list<array<string, mixed>>, string): array<string, mixed> $write
     */
    public static function run(iterable $ops, int $chunkRows, int $chunkBytes, callable $write): BatchResult
    {
        if ($chunkRows <= 0 || $chunkBytes <= 0) {
            throw new \InvalidArgumentException('chunkRows and chunkBytes must be positive');
        }
        $token = null;
        $applied = 0;
        $rows = 0;
        $chunk = [];
        $size = 0;
        $flush = static function () use (&$chunk, &$size, &$token, &$applied, &$rows, $write): void {
            if ($chunk === []) {
                return;
            }
            try {
                $res = $write($chunk, Idempotency::mintKey());
            } catch (LoamsException $e) {
                throw new BatchException($e, $applied, $rows, $token === null ? null : (string) $token);
            }
            $text = $res['token'] ?? '';
            if (is_string($text) && $text !== '' && ConsistencyToken::isValid($text)) {
                $parsed = ConsistencyToken::parse($text);
                $token = $token === null ? $parsed->normalized() : $token->merge($parsed);
            }
            $applied++;
            $rows += count($chunk);
            $chunk = [];
            $size = 0;
        };
        foreach ($ops as $op) {
            $bytes = strlen(Json::encode($op));
            if ($chunk !== [] && (count($chunk) >= $chunkRows || $size + $bytes > $chunkBytes)) {
                $flush();
            }
            $chunk[] = $op;
            $size += $bytes;
        }
        $flush();
        return new BatchResult($token === null ? null : (string) $token, $applied, $rows);
    }
}

/**
 * R14: `streams.subscribe`, a loop over unary `Fetch` (SDKP-2).
 *
 * The next offset is the last record's offset + 1. An empty fetch backs off
 * 100 ms, doubling up to 1 s. `offset()` is where the next fetch starts, so a
 * caller can resume from it; `cancel()` stops the loop.
 *
 *     $sub = new Subscription(fn (int $offset) => $fetch($offset), fromOffset: 1);
 *     foreach ($sub as $record) { …; if ($done) $sub->cancel(); }
 */
final class Subscription implements \IteratorAggregate
{
    public const INITIAL_BACKOFF = 0.1;
    public const MAX_BACKOFF = 1.0;

    /** @var callable(int): iterable<array<string, mixed>> */
    private $fetch;
    /** @var callable(float): void */
    private $sleeper;
    private bool $cancelled = false;

    /**
     * @param callable(int): iterable<array<string, mixed>> $fetch the records at and after an offset, each with an `offset`
     * @param ?callable(float): void $sleeper
     */
    public function __construct(callable $fetch, private int $fromOffset = 0, ?callable $sleeper = null)
    {
        $this->fetch = $fetch;
        $this->sleeper = $sleeper ?? static function (float $s): void {
            usleep((int) ($s * 1_000_000));
        };
    }

    public function offset(): int
    {
        return $this->fromOffset;
    }

    public function cancel(): void
    {
        $this->cancelled = true;
    }

    public function getIterator(): \Generator
    {
        $backoff = self::INITIAL_BACKOFF;
        while (!$this->cancelled) {
            $records = ($this->fetch)($this->fromOffset);
            $any = false;
            foreach ($records as $record) {
                if ($this->cancelled) {
                    return;
                }
                $any = true;
                $this->fromOffset = (int) $record['offset'] + 1;
                yield $record;
            }
            if ($any) {
                $backoff = self::INITIAL_BACKOFF;
                continue;
            }
            ($this->sleeper)($backoff);
            $backoff = min($backoff * 2, self::MAX_BACKOFF);
        }
    }
}
