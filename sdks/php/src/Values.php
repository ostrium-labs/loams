<?php

declare(strict_types=1);

namespace Loams;

use Loams\Builders\Json;

/**
 * Conversions between PHP values and the proto3 JSON the app modules send
 * and answer.
 */
final class Values
{
    /**
     * A primary key as a `DocumentId`'s JSON: a non-negative int is `uint`,
     * a string is `string`, and `['uuid' => …]` (any array naming its arm)
     * passes through.
     *
     * @return array<string, string>
     */
    public static function documentId(int|string|array $id): array
    {
        if (is_int($id)) {
            if ($id < 0) {
                throw InvalidArgumentException::client('a document id is a non-negative integer, a string or [\'uuid\' => …]');
            }
            return ['uint' => (string) $id];
        }
        if (is_string($id)) {
            return ['string' => $id];
        }
        return $id;
    }

    /**
     * A `DocumentId`'s JSON back as a PHP value: an int (a string past
     * PHP_INT_MAX), a string, or `['uuid' => …]`.
     *
     * @param array<string, string> $id
     */
    public static function fromDocumentId(array $id): int|string|array
    {
        if (isset($id['uint'])) {
            $n = $id['uint'];
            return (strlen($n) < 19 || (strlen($n) === 19 && strcmp($n, (string) PHP_INT_MAX) <= 0)) ? (int) $n : $n;
        }
        if (isset($id['string'])) {
            return $id['string'];
        }
        return $id;
    }

    /** An associative array as a JSON object: an empty one encodes as `{}`. */
    public static function object(array $value): array|\stdClass
    {
        return $value === [] ? Json::empty() : $value;
    }

    /**
     * A PHP value as a `loams.graph.v1.Value`'s JSON, for GQL parameters.
     *
     * @return array<string, mixed>
     */
    public static function graphValue(mixed $value): array
    {
        return match (true) {
            $value === null => ['null' => 'NULL_VALUE'],
            is_bool($value) => ['boolean' => $value],
            is_int($value) => ['int64' => (string) $value],
            is_float($value) => ['float64' => $value],
            is_string($value) => ['string' => $value],
            is_array($value) && array_is_list($value) => ['list' => ['values' => array_map([self::class, 'graphValue'], $value)]],
            is_array($value) => ['map' => ['entries' => array_map([self::class, 'graphValue'], $value)]],
            default => throw InvalidArgumentException::client('no graph value for ' . get_debug_type($value)),
        };
    }

    /**
     * A `loams.graph.v1.Value`'s JSON as a PHP value. Nodes, relationships,
     * paths and temporal values stay as their JSON.
     *
     * @param array<string, mixed> $value
     */
    public static function fromGraphValue(array $value): mixed
    {
        if ($value === []) {
            return null;
        }
        $kind = array_key_first($value);
        $inner = $value[$kind];
        return match ($kind) {
            'null' => null,
            'boolean', 'float64', 'string' => $inner,
            'int64', 'uint64' => is_numeric($inner) && abs((float) $inner) < 9.2e18 ? (int) $inner : $inner,
            'list' => array_map([self::class, 'fromGraphValue'], $inner['values'] ?? []),
            'map' => array_map([self::class, 'fromGraphValue'], $inner['entries'] ?? []),
            default => $value,
        };
    }
}
