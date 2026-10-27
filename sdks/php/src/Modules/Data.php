<?php

declare(strict_types=1);

namespace Loams\Modules;

use Loams\Builders\Json;
use Loams\Builders\Search as SearchBuilder;
use Loams\Subscription;

/** `sql`: one read-only statement over a namespace's collections, with `$1…$n` parameters. */
final class Sql extends Base
{
    /**
     * `$params` bind `$1…$n` in order; `$consistency` is `strong`, `eventual`
     * or a token. The answer's `rows` are arrays in `columns` order; see `records()`.
     *
     * @param list<mixed> $params
     * @param array<string, mixed> $options
     * @return array<string, mixed>
     */
    public function query(string $sql, array $params = [], ?int $maxRows = null, string|\Stringable|null $consistency = null, ?string $namespace = null, array $options = []): array
    {
        return $this->rpc('sql.query', self::request($sql, $params, $maxRows, $consistency, $namespace), $options);
    }

    /**
     * The same statement, answered as Arrow IPC bytes (`arrowIpc`, decoded).
     *
     * @param list<mixed> $params
     * @param array<string, mixed> $options
     */
    public function queryArrow(string $sql, array $params = [], ?int $maxRows = null, string|\Stringable|null $consistency = null, ?string $namespace = null, array $options = []): string
    {
        $answer = $this->rpc('sql.query_arrow', self::request($sql, $params, $maxRows, $consistency, $namespace), $options);
        return base64_decode((string) ($answer['arrowIpc'] ?? ''), true) ?: '';
    }

    /**
     * A `QueryResponse`'s rows as arrays, column name to value.
     *
     * @param array<string, mixed> $response
     * @return list<array<string, mixed>>
     */
    public static function records(array $response): array
    {
        $names = array_map(static fn ($c) => (string) $c['name'], $response['columns'] ?? []);
        $out = [];
        foreach ($response['rows'] ?? [] as $row) {
            $out[] = array_combine($names, array_pad(array_slice($row, 0, count($names)), count($names), null));
        }
        return $out;
    }

    /**
     * @param list<mixed> $params
     * @return array<string, mixed>
     */
    private static function request(string $sql, array $params, ?int $maxRows, string|\Stringable|null $consistency, ?string $namespace): array
    {
        return self::ns($namespace) + Json::compact([
            'sql' => $sql,
            'params' => $params === [] ? null : array_values($params),
            'maxRows' => $maxRows,
            'consistency' => SearchBuilder::consistencyJson($consistency),
        ]);
    }
}

/**
 * `streams`: a namespace's logs. Records are bytes on the wire; the module takes
 * strings (bytes as they are) or arrays (JSON, UTF-8) and answers bytes.
 */
final class Streams extends Base
{
    /** @param ?array{maxAgeMs?: int, maxBytes?: int} $retention @param array<string, mixed> $options @return array<string, mixed> */
    public function create(string $name, int $partitions = 1, ?array $retention = null, ?string $namespace = null, array $options = []): array
    {
        return $this->rpc('streams.create', self::ns($namespace) + Json::compact([
            'name' => $name, 'partitions' => $partitions === 0 ? null : $partitions,
            'retention' => $retention === null ? null : array_map('strval', $retention),
        ]), $options);
    }

    /** @param array<string, mixed> $options @return array<string, mixed> */
    public function get(string $stream, ?string $namespace = null, array $options = []): array
    {
        return $this->rpc('streams.get', self::ns($namespace) + ['stream' => $stream], $options);
    }

    /** @param array<string, mixed> $options @return array<string, mixed> */
    public function list(?int $pageSize = null, ?string $pageToken = null, ?string $namespace = null, array $options = []): array
    {
        return $this->rpc('streams.list', self::ns($namespace) + Json::compact(['pageSize' => $pageSize, 'pageToken' => $pageToken]), $options);
    }

    /** @param array<string, mixed> $options */
    public function listAll(?int $pageSize = null, ?string $namespace = null, array $options = []): \Generator
    {
        return $this->paged('streams.list', self::ns($namespace) + Json::compact(['pageSize' => $pageSize]), 'streams', $options);
    }

    /**
     * Appends `$records` to one partition. A record is a value (a string, or an
     * array sent as JSON) or `['key' => …, 'value' => …, 'headers' => [k => v]]`.
     * With `$producerId` and `$producerSeq` a repeat is deduplicated (`duplicate`).
     *
     * @param list<mixed> $records
     * @param array<string, mixed> $options
     * @return array<string, mixed>
     */
    public function produce(string $stream, array $records, int $partition = 0, ?string $producerId = null, ?int $producerSeq = null, ?string $namespace = null, array $options = []): array
    {
        return $this->rpc('streams.produce', self::ns($namespace) + Json::compact([
            'stream' => $stream,
            'partition' => $partition === 0 ? null : $partition,
            'records' => array_map([self::class, 'record'], array_values($records)),
            'producerId' => $producerId,
            'producerSeq' => $producerSeq === null ? null : (string) $producerSeq,
        ]), $options);
    }

    /**
     * Records at and after `$offset`; see `records()` for them decoded.
     *
     * @param array<string, mixed> $options
     * @return array<string, mixed>
     */
    public function fetch(string $stream, int $partition = 0, int $offset = 0, ?int $maxRecords = null, ?int $maxBytes = null, ?int $maxWaitMs = null, ?string $namespace = null, array $options = []): array
    {
        return $this->rpc('streams.fetch', self::ns($namespace) + Json::compact([
            'stream' => $stream,
            'partition' => $partition === 0 ? null : $partition,
            'offset' => $offset === 0 ? null : (string) $offset,
            'maxBytes' => $maxBytes,
            'maxRecords' => $maxRecords,
            'maxWaitMs' => $maxWaitMs,
        ]), $options);
    }

    /**
     * CloudEvents (arrays) appended as one JSON batch.
     *
     * @param list<array<string, mixed>> $events
     * @param array<string, mixed> $options
     * @return array<string, mixed>
     */
    public function produceEvents(string $stream, array $events, ?int $partition = null, ?string $namespace = null, array $options = []): array
    {
        return $this->rpc('streams.produce_events', self::ns($namespace) + Json::compact([
            'stream' => $stream,
            'partition' => $partition,
            'jsonBatch' => base64_encode(Json::encode(array_values($events))),
        ]), $options);
    }

    /**
     * CloudEvents at and after `$offset`, decoded from the answer's JSON batch.
     *
     * @param array<string, mixed> $options
     * @return list<array<string, mixed>>
     */
    public function fetchEvents(string $stream, int $partition = 0, int $offset = 0, ?int $maxBytes = null, ?int $maxWaitMs = null, ?string $namespace = null, array $options = []): array
    {
        $answer = $this->rpc('streams.fetch_events', self::ns($namespace) + Json::compact([
            'stream' => $stream,
            'partition' => $partition === 0 ? null : $partition,
            'offset' => $offset === 0 ? null : (string) $offset,
            'maxBytes' => $maxBytes,
            'maxWaitMs' => $maxWaitMs,
        ]), $options);
        $batch = base64_decode((string) ($answer['jsonBatch'] ?? ''), true);
        return $batch === false || $batch === '' ? [] : (json_decode($batch, true) ?? []);
    }

    /**
     * R14: every record from `$fromOffset` on, as a loop over `fetch` (see
     * `Subscription`): iterate it, `cancel()` it, resume from `offset()`.
     *
     * @param array<string, mixed> $options
     */
    public function subscribe(string $stream, int $partition = 0, int $fromOffset = 0, ?int $maxRecords = null, ?int $maxWaitMs = null, ?string $namespace = null, array $options = []): Subscription
    {
        return new Subscription(
            fn (int $offset) => self::records($this->fetch($stream, $partition, $offset, $maxRecords, null, $maxWaitMs, $namespace, $options)),
            $fromOffset
        );
    }

    /**
     * A `FetchResponse`'s records, decoded: `offset` (int), `key` and `value`
     * (bytes, or null), `headers` (name to bytes) and `timestampMs`.
     *
     * @param array<string, mixed> $response
     * @return list<array<string, mixed>>
     */
    public static function records(array $response): array
    {
        $out = [];
        foreach ($response['records'] ?? [] as $r) {
            $headers = [];
            foreach ($r['headers'] ?? [] as $h) {
                $headers[(string) $h['key']] = isset($h['value']) ? base64_decode((string) $h['value']) : null;
            }
            $out[] = [
                'offset' => (int) ($r['offset'] ?? 0),
                'key' => isset($r['key']) ? base64_decode((string) $r['key']) : null,
                'value' => isset($r['value']) ? base64_decode((string) $r['value']) : null,
                'headers' => $headers,
                'timestampMs' => (int) ($r['timestampMs'] ?? 0),
            ];
        }
        return $out;
    }

    /** One record as a `Record`'s JSON. @return array<string, mixed> */
    private static function record(mixed $record): array
    {
        if (!is_array($record) || !(array_key_exists('value', $record) || array_key_exists('key', $record))) {
            return ['value' => base64_encode(self::bytes($record))];
        }
        $headers = [];
        foreach ($record['headers'] ?? [] as $k => $v) {
            $headers[] = Json::compact(['key' => (string) $k, 'value' => $v === null ? null : base64_encode(self::bytes($v))]);
        }
        return Json::compact([
            'key' => array_key_exists('key', $record) && $record['key'] !== null ? base64_encode(self::bytes($record['key'])) : null,
            'value' => array_key_exists('value', $record) && $record['value'] !== null ? base64_encode(self::bytes($record['value'])) : null,
            'headers' => $headers === [] ? null : $headers,
        ]);
    }

    private static function bytes(mixed $value): string
    {
        return is_string($value) ? $value : Json::encode($value);
    }
}
