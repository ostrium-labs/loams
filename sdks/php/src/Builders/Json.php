<?php

declare(strict_types=1);

namespace Loams\Builders;

/**
 * The proto3 JSON helpers the builders share. A builder's `toArray()` is the
 * request's proto3 JSON as PHP arrays; an empty message is `new \stdClass()`
 * so that it encodes as `{}` and never as `[]`.
 */
final class Json
{
    /**
     * Drops the null values, so an unset optional field is absent.
     *
     * @param array<string, mixed> $a
     * @return array<string, mixed>
     */
    public static function compact(array $a): array
    {
        return array_filter($a, static fn ($v): bool => $v !== null);
    }

    /** A builder, an array or null, as proto3 JSON. */
    public static function of(mixed $value): mixed
    {
        if ($value instanceof Filter || $value instanceof Search || $value instanceof Schema) {
            return $value->toArray();
        }
        return $value;
    }

    public static function empty(): \stdClass
    {
        return new \stdClass();
    }

    public static function encode(mixed $value): string
    {
        return json_encode($value, JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE | JSON_PRESERVE_ZERO_FRACTION | JSON_THROW_ON_ERROR);
    }
}
