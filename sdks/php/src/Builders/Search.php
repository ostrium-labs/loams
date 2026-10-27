<?php

declare(strict_types=1);

namespace Loams\Builders;

/**
 * How a hybrid search fuses its retrievers' rankings.
 */
final class Fusion
{
    /** Reciprocal rank fusion; null leaves `k` at the server's default (60). @return array<string, mixed> */
    public static function rrf(?int $k = null): array
    {
        return ['rrf' => $k === null ? Json::empty() : ['k' => $k]];
    }

    /** Distribution-based score fusion. @return array<string, mixed> */
    public static function dbsf(): array
    {
        return ['dbsf' => Json::empty()];
    }

    /** A weighted sum, one weight per retriever. @return array<string, mixed> */
    public static function weighted(float ...$weights): array
    {
        return ['weightedSum' => ['weights' => array_values($weights)]];
    }
}

/**
 * One sort key.
 */
final class Sort
{
    private const ORDERS = ['asc' => 'SORT_ORDER_ASC', 'desc' => 'SORT_ORDER_DESC'];
    private const MISSING = ['first' => 'MISSING_ORDER_FIRST', 'last' => 'MISSING_ORDER_LAST'];

    /** @return array<string, mixed> */
    public static function score(?string $order = null): array
    {
        return ['score' => self::order($order)];
    }

    /** The primary key: the tiebreaker `queryAll` requires. @return array<string, mixed> */
    public static function pk(?string $order = null): array
    {
        return ['pk' => self::order($order)];
    }

    /** @return array<string, mixed> */
    public static function field(string $name, ?string $order = null, ?string $missing = null): array
    {
        return ['field' => Json::compact([
            'field' => $name,
            'order' => $order === null ? null : self::ORDERS[$order],
            'missing' => $missing === null ? null : self::MISSING[$missing],
        ])];
    }

    private static function order(?string $order): array|\stdClass
    {
        return $order === null ? Json::empty() : ['order' => self::ORDERS[$order]];
    }
}

/**
 * A `SearchRequest`, verb by verb (SDKP §2.4):
 *
 *     Search::on('notes')
 *         ->text('refund', field: 'body')
 *         ->vector('embedding', [1, 0, 0, 0])
 *         ->fuse(Fusion::rrf(60))
 *         ->filter(Filter::eq('tags', 'policy'))
 *         ->limit(2)
 */
final class Search implements \JsonSerializable
{
    /** The page size `queryAll` uses when the request sets no `limit`. */
    public const DEFAULT_K = 10;

    /** @var array<string, mixed> */
    private array $req = [];

    public function __construct(?string $collection = null, ?string $namespace = null)
    {
        if ($namespace !== null) {
            $this->req['namespace'] = $namespace;
        }
        if ($collection !== null) {
            $this->req['collection'] = $collection;
        }
    }

    public static function on(string $collection, ?string $namespace = null): self
    {
        return new self($collection, $namespace);
    }

    public function collection(string $name): self
    {
        $this->req['collection'] = $name;
        return $this;
    }

    public function namespace(string $name): self
    {
        $this->req['namespace'] = $name;
        return $this;
    }

    public function namespaceName(): ?string
    {
        return $this->req['namespace'] ?? null;
    }

    /**
     * A text retriever. `$query` is a `Filter` (any query), or a string: a
     * match on one field, a multi-match on several, or a query string when no
     * field is given. `$field` is the one-field spelling of `$fields`.
     *
     * @param ?list<string> $fields
     */
    public function text(Filter|array|string $query, ?array $fields = null, ?string $field = null, ?int $k = null): self
    {
        if ($fields === null && $field !== null) {
            $fields = [$field];
        }
        if (is_string($query)) {
            $query = match (true) {
                $fields === null || $fields === [] => Filter::queryString($query),
                count($fields) === 1 => Filter::match((string) $fields[0], $query),
                default => Filter::multiMatch($fields, $query),
            };
        }
        return $this->retriever('text', Json::compact(['query' => Json::of($query), 'k' => $k]));
    }

    /**
     * A dense-vector (kNN) retriever; the ANN params are `AnnParams`.
     *
     * @param list<int|float> $values
     */
    public function vector(
        string $field,
        array $values,
        ?int $k = null,
        ?bool $exact = null,
        ?int $nprobes = null,
        ?int $refineFactor = null,
        ?int $ef = null,
        ?float $oversampling = null,
        ?string $distance = null,
        Filter|array|null $filter = null
    ): self {
        $params = Json::compact([
            'exact' => $exact, 'nprobes' => $nprobes, 'refineFactor' => $refineFactor, 'ef' => $ef,
            'oversampling' => $oversampling, 'distance' => $distance,
        ]);
        $body = ['field' => $field, 'query' => array_map('floatval', array_values($values))];
        if ($k !== null) {
            $body['k'] = $k;
        }
        if ($params !== []) {
            $body['params'] = $params;
        }
        if ($filter !== null) {
            $body['filter'] = Json::of($filter);
        }
        return $this->retriever('vector', $body);
    }

    /**
     * A sparse-vector retriever; `$terms` is `[index => weight]` or
     * `['indices' => […], 'values' => […]]`.
     *
     * @param array<int|string, mixed> $terms
     */
    public function sparse(string $field, array $terms, ?int $k = null, Filter|array|null $filter = null): self
    {
        $query = isset($terms['indices'])
            ? ['indices' => array_map('intval', $terms['indices']), 'values' => array_map('floatval', $terms['values'])]
            : ['indices' => array_map('intval', array_keys($terms)), 'values' => array_map('floatval', array_values($terms))];
        return $this->retriever('sparse', Json::compact([
            'field' => $field, 'query' => $query, 'k' => $k, 'filter' => $filter === null ? null : Json::of($filter),
        ]));
    }

    /** @param array<string, mixed> $fusion `Fusion::rrf()`, `dbsf()` or `weighted(…)` */
    public function fuse(array $fusion): self
    {
        $this->req['fusion'] = $fusion;
        return $this;
    }

    public function filter(Filter|array $filter): self
    {
        $this->req['filter'] = Json::of($filter);
        return $this;
    }

    /** `Sort::score()`, `Sort::pk()`, `Sort::field()`, or `'score'` / `'pk'`. */
    public function sort(array|string ...$keys): self
    {
        $this->req['sort'] = array_map(static fn ($k) => match ($k) {
            'score' => Sort::score(),
            'pk' => Sort::pk(),
            default => $k,
        }, array_values($keys));
        return $this;
    }

    public function limit(int $count): self
    {
        $this->req['limit'] = $count;
        return $this;
    }

    public function offset(int $count): self
    {
        $this->req['offset'] = $count;
        return $this;
    }

    /** Continue after a hit: its `sortValues`. @param list<mixed> $sortValues */
    public function after(array $sortValues): self
    {
        $this->req['searchAfter'] = array_values($sortValues);
        return $this;
    }

    public function minScore(float $score): self
    {
        $this->req['scoreThreshold'] = $score;
        return $this;
    }

    /**
     * The projection: which `_source` paths, vectors and typed fields to return.
     *
     * @param ?list<string> $include
     * @param ?list<string> $exclude
     * @param ?list<string> $vectors
     * @param ?list<string> $fields
     */
    public function select(?array $include = null, ?array $exclude = null, bool $noSource = false, ?array $vectors = null, ?array $fields = null): self
    {
        $source = Json::compact(['include' => $include, 'exclude' => $exclude]);
        $this->req['select'] = Json::compact([
            'source' => $noSource ? 'none' : ($source === [] ? null : $source),
            'vectors' => $vectors,
            'fields' => $fields,
        ]);
        return $this;
    }

    public function highlight(string $field, ?string $pre = null, ?string $post = null, ?int $fragmentSize = null, ?int $fragments = null): self
    {
        $this->req['highlight']['fields'][] = Json::compact([
            'field' => $field, 'preTag' => $pre, 'postTag' => $post,
            'fragmentSize' => $fragmentSize, 'numberOfFragments' => $fragments,
        ]);
        return $this;
    }

    public function groupBy(string $field, ?int $size = null, ?int $limit = null): self
    {
        $this->req['groupBy'] = Json::compact(['field' => $field, 'groupSize' => $size, 'limit' => $limit]);
        return $this;
    }

    /**
     * Graph expansion of the hits (`expand_spec`). `$direction` is `out`, `in`
     * or `both`; `$output` is `nested` or `flat`.
     *
     * @param ?list<string> $edgeTypes
     */
    public function expand(
        string $graph,
        ?string $fromField = null,
        ?int $hops = null,
        ?string $direction = null,
        ?array $edgeTypes = null,
        ?int $limitPerSeed = null,
        ?int $limit = null,
        ?string $seedLabel = null,
        ?int $seeds = null,
        ?string $output = null
    ): self {
        $this->req['expandSpec'] = Json::compact([
            'graph' => $graph, 'fromField' => $fromField, 'seedLabel' => $seedLabel, 'seeds' => $seeds,
            'hops' => $hops, 'limitPerSeed' => $limitPerSeed, 'limit' => $limit,
            'direction' => $direction === null ? null : 'EXPAND_DIRECTION_' . strtoupper($direction),
            'edgeTypes' => $edgeTypes,
            'output' => $output === null ? null : 'EXPAND_OUTPUT_' . strtoupper($output),
        ]);
        return $this;
    }

    /** `'strong'`, `'eventual'`, or a token (a string or `ConsistencyToken`) as `atLeast`. */
    public function consistency(string|\Stringable $level): self
    {
        $this->req['consistency'] = self::consistencyJson($level);
        return $this;
    }

    /** @return ?array<string, mixed> */
    public static function consistencyJson(string|\Stringable|array|null $level): ?array
    {
        return match (true) {
            $level === null => null,
            is_array($level) => $level,
            $level === 'strong' => ['freshness' => 'FRESHNESS_STRONG'],
            $level === 'eventual' => ['freshness' => 'FRESHNESS_EVENTUAL'],
            $level instanceof \Loams\ConsistencyToken => ['atLeast' => (string) $level],
            default => ['atLeast' => (string) \Loams\ConsistencyToken::parse((string) $level)],
        };
    }

    /** Whether the request's last sort key is the primary key, which `queryAll` needs. */
    public function hasTiebreak(): bool
    {
        return self::sortsByPk($this->req);
    }

    /** @param array<string, mixed> $request */
    public static function sortsByPk(array $request): bool
    {
        $sort = $request['sort'] ?? [];
        $last = $sort === [] ? null : $sort[array_key_last($sort)];
        return is_array($last) && array_key_exists('pk', $last);
    }

    /** The request exactly as built: what the builder corpus pins. @return array<string, mixed> */
    public function toArray(): array
    {
        return $this->req;
    }

    /** `toArray()`, checked: a weighted fusion needs one weight per retriever. @return array<string, mixed> */
    public function build(): array
    {
        return self::check($this->req);
    }

    /**
     * @param array<string, mixed> $request
     * @return array<string, mixed>
     */
    public static function check(array $request): array
    {
        $weights = $request['fusion']['weightedSum']['weights'] ?? null;
        $count = count($request['retrievers'] ?? []);
        if (is_array($weights) && count($weights) !== $count) {
            throw new \Loams\InvalidArgumentException(
                sprintf('a weighted fusion needs one weight per retriever: %d weights for %d retrievers', count($weights), $count),
                'invalid_argument',
                null,
                null,
                400,
                [['type' => 'loams.errors.v1.ErrorInfo', 'metadata' => ['field' => 'fusion.weighted_sum.weights']]]
            );
        }
        return $request;
    }

    /**
     * What the app modules send: `build()`. A retriever's `k` goes out only
     * when the caller gave one; the engine fills `max(10, offset + limit)`
     * itself (SDKP-15, SDKP-18).
     *
     * @param self|array<string, mixed> $request
     * @return array<string, mixed>
     */
    public static function wire(self|array $request): array
    {
        return self::check($request instanceof self ? $request->toArray() : $request);
    }

    public function jsonSerialize(): mixed
    {
        return $this->req;
    }

    /** @param array<string, mixed> $body */
    private function retriever(string $kind, array $body): self
    {
        $this->req['retrievers'][] = [$kind => $body];
        return $this;
    }
}
