<?php

declare(strict_types=1);

namespace Loams\Tests;

use Loams\Builders\Filter;
use Loams\Builders\Fusion;
use Loams\Builders\Schema;
use Loams\Builders\Search;
use Loams\Builders\Sort;

/**
 * Drives the builders from the builder corpus's neutral notation
 * (`sdks/fixtures/builders/README.md`): `{"op": verb, "args": [...]}`, where
 * an argument with an `op` is itself a call and a trailing plain object is the
 * call's options (named arguments, `snake_case` mapped to `camelCase`).
 */
final class BuilderCorpus
{
    private const FUSIONS = ['rrf', 'dbsf', 'weighted'];
    private const SORTS = ['score', 'pk', 'field'];
    /** Calls whose trailing object is an argument, not options. */
    private const NO_OPTIONS = ['ids'];

    /** The fixtures directory: `LOAMS_FIXTURES_DIR`, or `sdks/fixtures`. */
    public static function dir(): string
    {
        $env = getenv('LOAMS_FIXTURES_DIR');
        return $env !== false && $env !== '' ? $env : dirname(__DIR__, 2) . '/fixtures';
    }

    /** @return list<array<string, mixed>> */
    public static function cases(): array
    {
        $files = glob(self::dir() . '/builders/*.json') ?: [];
        sort($files);
        return array_map(static fn (string $f) => json_decode((string) file_get_contents($f), true), $files);
    }

    /** @param array<string, mixed> $case */
    public static function build(array $case): mixed
    {
        switch ($case['builder']) {
            case 'schema':
                $schema = Schema::new();
                foreach ($case['spec'] as $call) {
                    $schema = self::apply($schema, $call);
                }
                return $schema->toArray();
            case 'filter':
                return self::evaluate($case['spec'])->toArray();
            case 'search':
                $search = Search::on($case['spec']['on']);
                foreach ($case['spec']['calls'] as $call) {
                    $search = self::apply($search, $call);
                }
                return $search->build();
        }
        throw new \RuntimeException('unknown builder ' . $case['builder']);
    }

    /** @param array<string, mixed> $call */
    private static function evaluate(array $call): mixed
    {
        $op = $call['op'];
        $class = in_array($op, self::FUSIONS, true) ? Fusion::class : (in_array($op, self::SORTS, true) ? Sort::class : Filter::class);
        return self::apply($class, $call);
    }

    /** @param array<string, mixed> $call */
    private static function apply(object|string $target, array $call): mixed
    {
        $op = $call['op'];
        $raw = $call['args'] ?? [];
        $args = array_map(static fn ($a) => is_array($a) && isset($a['op']) ? self::evaluate($a) : $a, $raw);
        $named = [];
        $last = $raw === [] ? null : $raw[count($raw) - 1];
        if (!in_array($op, self::NO_OPTIONS, true) && is_array($last) && !array_is_list($last) && !isset($last['op'])) {
            array_pop($args);
            foreach ($last as $k => $v) {
                $named[lcfirst(str_replace('_', '', ucwords((string) $k, '_')))] = $v;
            }
        }
        $method = lcfirst(str_replace('_', '', ucwords($op, '_')));
        return is_string($target) ? $target::$method(...$args, ...$named) : $target->$method(...$args, ...$named);
    }

    /** Proto3 JSON compared as values: numbers by value, key order ignored. */
    public static function normalize(mixed $v): mixed
    {
        if (is_array($v)) {
            $out = array_map([self::class, 'normalize'], $v);
            if (!array_is_list($out)) {
                ksort($out);
            }
            return $out;
        }
        return is_int($v) ? (float) $v : $v;
    }
}
