<?php

declare(strict_types=1);

namespace Loams\Builders;

/**
 * One `loams.collection.v1.Query` (SDKP §2.4): the filter of a search, the
 * query of a text retriever, and the typed `where` of the document reads.
 *
 *     Filter::and(Filter::eq('tags', 'policy'), Filter::range('n', gte: 2))
 */
final class Filter implements \JsonSerializable
{
    /** @param array<string, mixed> $json */
    private function __construct(private readonly array $json)
    {
    }

    /** @return array<string, mixed> */
    public function toArray(): array
    {
        return $this->json;
    }

    public function jsonSerialize(): mixed
    {
        return $this->json;
    }

    public static function all(): self
    {
        return new self(['matchAll' => Json::empty()]);
    }

    public static function none(): self
    {
        return new self(['matchNone' => Json::empty()]);
    }

    /** `field` equals `value` (a term query). */
    public static function eq(string $field, mixed $value): self
    {
        return new self(['term' => ['field' => $field, 'value' => $value]]);
    }

    /** `field` is one of `values` (a terms query). @param list<mixed> $values */
    public static function in(string $field, array $values): self
    {
        return new self(['terms' => ['field' => $field, 'values' => array_values($values)]]);
    }

    public static function range(string $field, mixed $gt = null, mixed $gte = null, mixed $lt = null, mixed $lte = null): self
    {
        return new self(['range' => Json::compact(['field' => $field, 'gt' => $gt, 'gte' => $gte, 'lt' => $lt, 'lte' => $lte])]);
    }

    public static function exists(string $field): self
    {
        return new self(['exists' => ['field' => $field]]);
    }

    public static function isNull(string $field): self
    {
        return new self(['isNull' => ['field' => $field]]);
    }

    public static function isEmpty(string $field): self
    {
        return new self(['isEmpty' => ['field' => $field]]);
    }

    public static function prefix(string $field, string $value): self
    {
        return new self(['prefix' => ['field' => $field, 'value' => $value]]);
    }

    public static function wildcard(string $field, string $pattern): self
    {
        return new self(['wildcard' => ['field' => $field, 'pattern' => $pattern]]);
    }

    /** `$edits` is a number of edits or `'auto'`; null leaves the server's default. */
    public static function fuzzy(string $field, string $value, int|string|null $edits = null): self
    {
        return new self(['fuzzy' => Json::compact(['field' => $field, 'value' => $value, 'fuzziness' => self::fuzziness($edits)])]);
    }

    /** Full-text match; `$operator` is `or` (default) or `and`; `$fuzziness` edits or `'auto'`. */
    public static function match(string $field, string $text, ?string $operator = null, int|string|null $minimumShouldMatch = null, int|string|null $fuzziness = null, ?string $analyzer = null): self
    {
        return new self(['match' => Json::compact([
            'field' => $field, 'text' => $text, 'operator' => $operator,
            'minimumShouldMatch' => $minimumShouldMatch === null ? null : (string) $minimumShouldMatch,
            'fuzziness' => self::fuzziness($fuzziness), 'analyzer' => $analyzer,
        ])]);
    }

    public static function phrase(string $field, string $text, ?int $slop = null): self
    {
        return new self(['matchPhrase' => Json::compact(['field' => $field, 'text' => $text, 'slop' => $slop])]);
    }

    /**
     * One text over several fields: names, `'name^boost'`, or `[name => boost]`.
     * `$kind` is `best_fields`, `most_fields`, `cross_fields`, `phrase` or
     * `phrase_prefix`; `$operator` is `or` or `and`.
     *
     * @param array<int|string, mixed> $fields
     */
    public static function multiMatch(array $fields, string $text, ?string $kind = null, ?string $operator = null, ?float $tieBreaker = null): self
    {
        $list = [];
        foreach ($fields as $k => $v) {
            $list[] = is_string($k) ? ['field' => $k, 'boost' => (float) $v] : self::boosted((string) $v);
        }
        return new self(['multiMatch' => Json::compact([
            'fields' => $list, 'text' => $text,
            'kind' => $kind === null ? null : 'MULTI_MATCH_KIND_' . strtoupper($kind),
            'operator' => $operator === null ? null : self::boolOperator($operator),
            'tieBreaker' => $tieBreaker,
        ])]);
    }

    /** @param ?list<string> $fields the default fields */
    public static function queryString(string $query, ?array $fields = null, ?string $operator = null): self
    {
        return new self(['queryString' => Json::compact([
            'query' => $query,
            'defaultFields' => $fields === null || $fields === [] ? null : array_values($fields),
            'defaultOperator' => $operator === null ? null : self::boolOperator($operator),
        ])]);
    }

    /** Documents by primary key: integers, strings or `['uuid' => …]`. */
    public static function ids(int|string|array ...$ids): self
    {
        return new self(['ids' => array_values(array_map([self::class, 'idValue'], $ids))]);
    }

    /**
     * A primary key as `Query.ids` spells it: an int is `['uint' => '…']`, a
     * string stays a string, and `['uuid' => …]` passes through.
     */
    public static function idValue(int|string|array $id): string|array
    {
        if (is_int($id)) {
            if ($id < 0) {
                throw new \InvalidArgumentException('a numeric document id is non-negative');
            }
            return ['uint' => (string) $id];
        }
        return $id;
    }

    /** Every clause must match (`bool.must`). */
    public static function and(self|array ...$clauses): self
    {
        return new self(['bool' => ['must' => self::list($clauses)]]);
    }

    /** Any clause matches (`bool.should`). */
    public static function or(self|array ...$clauses): self
    {
        return new self(['bool' => ['should' => self::list($clauses)]]);
    }

    /** At least `$minimumShouldMatch` of `$clauses` match. @param list<self|array<string, mixed>> $clauses */
    public static function anyOf(array $clauses, int|string $minimumShouldMatch): self
    {
        return new self(['bool' => ['should' => self::list($clauses), 'minimumShouldMatch' => (string) $minimumShouldMatch]]);
    }

    /** No clause matches (`bool.mustNot`). */
    public static function not(self|array ...$clauses): self
    {
        return new self(['bool' => ['mustNot' => self::list($clauses)]]);
    }

    /** Every clause matches, without scoring (`bool.filter`). */
    public static function filter(self|array ...$clauses): self
    {
        return new self(['bool' => ['filter' => self::list($clauses)]]);
    }

    public static function boost(self|array $clause, float $boost): self
    {
        return new self(['boost' => ['query' => Json::of($clause), 'boost' => $boost]]);
    }

    public static function constantScore(self|array $clause, float $score): self
    {
        return new self(['constantScore' => ['query' => Json::of($clause), 'score' => $score]]);
    }

    /** @return ?array<string, mixed> */
    private static function fuzziness(int|string|null $edits): ?array
    {
        return match (true) {
            $edits === null => null,
            $edits === 'auto' => ['auto' => Json::empty()],
            default => ['edits' => (int) $edits],
        };
    }

    /** @return array<string, mixed> */
    private static function boosted(string $spec): array
    {
        $at = strrpos($spec, '^');
        if ($at !== false && $at > 0 && is_numeric(substr($spec, $at + 1))) {
            return ['field' => substr($spec, 0, $at), 'boost' => (float) substr($spec, $at + 1)];
        }
        return ['field' => $spec];
    }

    private static function boolOperator(string $operator): string
    {
        return $operator === 'and' ? 'BOOL_OPERATOR_AND' : 'BOOL_OPERATOR_OR';
    }

    /**
     * @param array<int, self|array<string, mixed>> $clauses
     * @return list<mixed>
     */
    private static function list(array $clauses): array
    {
        return array_values(array_map(static fn ($c) => Json::of($c), $clauses));
    }
}
