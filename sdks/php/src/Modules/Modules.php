<?php

declare(strict_types=1);

namespace Loams\Modules;

use Loams\Batch;
use Loams\Builders\Filter;
use Loams\Builders\Fusion;
use Loams\Builders\Json;
use Loams\Builders\Schema;
use Loams\Builders\Search as SearchBuilder;
use Loams\InvalidArgumentException;
use Loams\Values;

/** `namespaces`: the container every collection and stream lives in. */
final class Namespaces extends Base
{
    /** @param array<string, mixed> $options @return array<string, mixed> */
    public function create(string $namespace, array $options = []): array
    {
        return $this->rpc('namespaces.create', ['namespace' => $namespace], $options);
    }

    /** @param array<string, mixed> $options @return array<string, mixed> */
    public function list(?int $pageSize = null, ?string $pageToken = null, array $options = []): array
    {
        return $this->rpc('namespaces.list', Json::compact(['pageSize' => $pageSize, 'pageToken' => $pageToken]), $options);
    }

    /** Every namespace, across pages. @param array<string, mixed> $options */
    public function listAll(?int $pageSize = null, array $options = []): \Generator
    {
        return $this->paged('namespaces.list', Json::compact(['pageSize' => $pageSize]), 'namespaces', $options);
    }
}

/** `collections`: schema, aliases, versions and the hot tier. */
final class Collections extends Base
{
    /**
     * @param Schema|array<string, mixed> $schema
     * @param array<string, mixed> $options
     * @return array<string, mixed>
     */
    public function create(string $name, Schema|array $schema, ?int $partitions = null, ?string $namespace = null, array $options = []): array
    {
        return $this->rpc('collections.create', self::ns($namespace) + Json::compact([
            'name' => $name, 'schema' => Json::of($schema), 'partitions' => $partitions,
        ]), $options);
    }

    /** @param array<string, mixed> $options @return array<string, mixed> */
    public function get(string $name, ?string $namespace = null, array $options = []): array
    {
        return $this->rpc('collections.get', self::ns($namespace) + ['collection' => $name], $options);
    }

    /** @param array<string, mixed> $options @return array<string, mixed> */
    public function list(?int $pageSize = null, ?string $pageToken = null, ?string $namespace = null, array $options = []): array
    {
        return $this->rpc('collections.list', self::ns($namespace) + Json::compact(['pageSize' => $pageSize, 'pageToken' => $pageToken]), $options);
    }

    /** @param array<string, mixed> $options */
    public function listAll(?int $pageSize = null, ?string $namespace = null, array $options = []): \Generator
    {
        return $this->paged('collections.list', self::ns($namespace) + Json::compact(['pageSize' => $pageSize]), 'collections', $options);
    }

    /** @param array<string, mixed> $options @return array<string, mixed> */
    public function drop(string $name, ?string $namespace = null, array $options = []): array
    {
        return $this->rpc('collections.drop', self::ns($namespace) + ['collection' => $name], $options);
    }

    /**
     * Additive schema change: the schema builder's field and vector arrays.
     *
     * @param Schema|list<array<string, mixed>> $fields
     * @param list<array<string, mixed>> $vectors
     * @param array<string, mixed> $options
     * @return array<string, mixed>
     */
    public function addFields(string $name, Schema|array $fields = [], array $vectors = [], ?string $namespace = null, array $options = []): array
    {
        if ($fields instanceof Schema) {
            $schema = $fields->toArray();
            $fields = $schema['fields'];
            $vectors = $vectors === [] ? $schema['vectors'] : $vectors;
        }
        return $this->rpc('collections.add_fields', self::ns($namespace) + [
            'collection' => $name, 'fields' => $fields, 'vectors' => $vectors,
        ], $options);
    }

    /** @param array<string, mixed> $options @return array<string, mixed> */
    public function listVersions(string $name, ?int $pageSize = null, ?string $pageToken = null, ?string $namespace = null, array $options = []): array
    {
        return $this->rpc('collections.list_versions', self::ns($namespace) + Json::compact([
            'collection' => $name, 'pageSize' => $pageSize, 'pageToken' => $pageToken,
        ]), $options);
    }

    /**
     * `[['create' => ['alias' => …, 'collection' => …]], ['delete' => ['alias' => …]]]`.
     *
     * @param list<array<string, mixed>> $actions
     * @param array<string, mixed> $options
     * @return array<string, mixed>
     */
    public function updateAliases(array $actions, ?string $namespace = null, array $options = []): array
    {
        return $this->rpc('collections.update_aliases', self::ns($namespace) + ['actions' => $actions], $options);
    }

    /** @param ?array<string, mixed> $at @param array<string, mixed> $options @return array<string, mixed> */
    public function scan(string $name, ?array $at = null, ?string $namespace = null, array $options = []): array
    {
        return $this->rpc('collections.scan', self::ns($namespace) + Json::compact(['collection' => $name, 'at' => $at]), $options);
    }

    /** @param array<string, mixed> $options @return array<string, mixed> */
    public function setHot(string $name, bool $vectors = false, bool $text = false, bool $fragments = false, ?string $namespace = null, array $options = []): array
    {
        return $this->rpc('collections.set_hot', self::ns($namespace) + [
            'collection' => $name, 'hot' => ['vectors' => $vectors, 'text' => $text, 'fragments' => $fragments],
        ], $options);
    }

    /** @param array<string, mixed> $options @return array<string, mixed> */
    public function warm(string $name, ?string $namespace = null, array $options = []): array
    {
        return $this->rpc('collections.warm', self::ns($namespace) + ['collection' => $name], $options);
    }
}

/** `documents`: write, get, scroll, count and the filter writes. */
final class Documents extends Base
{
    /**
     * Raw write ops (`WriteOp` JSON). The answer's `token` is the write's
     * consistency token.
     *
     * @param list<array<string, mixed>> $ops
     * @param array<string, mixed> $options
     * @return array<string, mixed>
     */
    public function write(string $collection, array $ops, bool $reportExistence = false, ?string $namespace = null, array $options = []): array
    {
        return $this->rpc('documents.write', self::ns($namespace) + [
            'collection' => $collection, 'ops' => $ops, 'reportExistence' => $reportExistence,
        ], $options);
    }

    /**
     * @param iterable<array<string, mixed>> $docs `[['id' => …, 'source' => […], 'vectors' => […]]]`
     * @param array<string, mixed> $options
     * @return array<string, mixed>
     */
    public function upsert(string $collection, iterable $docs, ?string $namespace = null, array $options = []): array
    {
        $ops = [];
        foreach ($docs as $doc) {
            $ops[] = ['upsert' => self::document($doc)];
        }
        return $this->write($collection, $ops, false, $namespace, $options);
    }

    /** @param list<int|string|array<string, string>> $ids @param array<string, mixed> $options @return array<string, mixed> */
    public function delete(string $collection, array $ids, ?string $namespace = null, array $options = []): array
    {
        $ops = array_map(static fn ($id) => ['delete' => ['id' => Values::documentId($id)]], array_values($ids));
        return $this->write($collection, $ops, false, $namespace, $options);
    }

    /**
     * Sets the keys of `$set` and removes the keys named in `$unset`.
     *
     * @param ?array<string, mixed> $set
     * @param ?list<string> $unset
     * @param array<string, mixed> $options
     * @return array<string, mixed>
     */
    public function patch(string $collection, int|string|array $id, ?array $set = null, ?array $unset = null, ?string $mode = null, ?string $namespace = null, array $options = []): array
    {
        $patch = Json::compact([
            'id' => Values::documentId($id), 'mode' => $mode,
            'source' => $set === null ? null : Values::object($set), 'deleteKeys' => $unset,
        ]);
        return $this->write($collection, [['patch' => $patch]], false, $namespace, $options);
    }

    /**
     * R13: `$docs` in chunks, one in flight at a time, each with its own
     * idempotency key. Returns a `BatchResult` (the merged token, the chunks
     * and the rows); a failed chunk throws `BatchException`.
     *
     * @param iterable<array<string, mixed>> $docs
     * @param array<string, mixed> $options
     */
    public function writeBatch(
        string $collection,
        iterable $docs,
        int $chunkRows = Batch::DEFAULT_CHUNK_ROWS,
        int $chunkBytes = Batch::DEFAULT_CHUNK_BYTES,
        ?string $namespace = null,
        array $options = []
    ): \Loams\BatchResult {
        $ops = (static function () use ($docs): \Generator {
            foreach ($docs as $doc) {
                yield ['upsert' => self::document($doc)];
            }
        })();
        return Batch::run($ops, $chunkRows, $chunkBytes, fn (array $chunk, string $key) =>
            $this->write($collection, $chunk, false, $namespace, ['idempotencyKey' => $key] + $options));
    }

    /**
     * @param list<int|string|array<string, string>> $ids
     * @param ?array<string, mixed> $select a projection: `['source' => ['include' => […]]]`
     * @param array<string, mixed> $options
     * @return array<string, mixed>
     */
    public function get(string $collection, array $ids, ?array $select = null, string|\Stringable|null $consistency = null, ?string $namespace = null, array $options = []): array
    {
        return $this->rpc('documents.get', self::ns($namespace) + Json::compact([
            'collection' => $collection, 'ids' => array_map([Values::class, 'documentId'], array_values($ids)),
            'select' => $select, 'consistency' => SearchBuilder::consistencyJson($consistency),
        ]), $options);
    }

    /**
     * One page. `$where` is a typed `Filter`; `$filter` the server's native filter JSON.
     *
     * @param array<string, mixed> $options
     * @return array<string, mixed>
     */
    public function scroll(
        string $collection,
        ?Filter $where = null,
        mixed $filter = null,
        int|string|array|null $after = null,
        ?int $limit = null,
        ?array $select = null,
        string|\Stringable|null $consistency = null,
        ?string $namespace = null,
        array $options = []
    ): array {
        return $this->rpc('documents.scroll', self::filtered(self::ns($namespace) + Json::compact([
            'collection' => $collection, 'after' => $after === null ? null : Values::documentId($after),
            'limit' => $limit, 'select' => $select, 'consistency' => SearchBuilder::consistencyJson($consistency),
        ]), $where, $filter), $options);
    }

    /**
     * Every document, page after page, until the answer has no `next`.
     *
     * @param array<string, mixed> $options
     */
    public function scrollAll(string $collection, ?Filter $where = null, mixed $filter = null, ?int $limit = null, ?array $select = null, string|\Stringable|null $consistency = null, ?string $namespace = null, array $options = []): \Generator
    {
        $after = null;
        do {
            $page = $this->scroll($collection, $where, $filter, $after, $limit, $select, $consistency, $namespace, $options);
            foreach ($page['documents'] ?? [] as $doc) {
                yield $doc;
            }
            $after = $page['next'] ?? null;
        } while ($after !== null && $after !== []);
    }

    /** @param array<string, mixed> $options @return array<string, mixed> */
    public function count(string $collection, ?Filter $where = null, mixed $filter = null, string|\Stringable|null $consistency = null, ?string $namespace = null, array $options = []): array
    {
        return $this->rpc('documents.count', self::filtered(self::ns($namespace) + Json::compact([
            'collection' => $collection, 'consistency' => SearchBuilder::consistencyJson($consistency),
        ]), $where, $filter), $options);
    }

    /** @param array<string, mixed> $options @return array<string, mixed> */
    public function deleteByFilter(string $collection, ?Filter $where = null, mixed $filter = null, ?int $maxRows = null, bool $allowPartial = false, ?array $cursor = null, ?string $namespace = null, array $options = []): array
    {
        return $this->rpc('documents.delete_by_filter', self::filtered(self::ns($namespace) + Json::compact([
            'collection' => $collection, 'maxRows' => $maxRows === null ? null : (string) $maxRows,
            'allowPartial' => $allowPartial, 'cursor' => $cursor,
        ]), $where, $filter), $options);
    }

    /** @param array<string, mixed> $patch @param array<string, mixed> $options @return array<string, mixed> */
    public function patchByFilter(string $collection, array $patch, ?Filter $where = null, mixed $filter = null, ?int $maxRows = null, bool $allowPartial = false, ?array $cursor = null, ?string $namespace = null, array $options = []): array
    {
        return $this->rpc('documents.patch_by_filter', self::filtered(self::ns($namespace) + Json::compact([
            'collection' => $collection, 'patch' => $patch, 'maxRows' => $maxRows === null ? null : (string) $maxRows,
            'allowPartial' => $allowPartial, 'cursor' => $cursor,
        ]), $where, $filter), $options);
    }

    /**
     * One document as a `WriteDocument`'s JSON.
     *
     * @param array<string, mixed> $doc
     * @return array<string, mixed>
     */
    public static function document(array $doc): array
    {
        if (!array_key_exists('id', $doc)) {
            throw InvalidArgumentException::client('a document needs an id');
        }
        return Json::compact([
            'id' => Values::documentId($doc['id']),
            'source' => Values::object($doc['source'] ?? []),
            'vectors' => isset($doc['vectors']) ? Values::object($doc['vectors']) : null,
            'sparseVectors' => isset($doc['sparseVectors']) ? Values::object($doc['sparseVectors']) : null,
        ]);
    }

    /**
     * The typed `where` (SDKP-3) and the native `filter`.
     *
     * @param array<string, mixed> $request
     * @return array<string, mixed>
     */
    private static function filtered(array $request, ?Filter $where, mixed $filter): array
    {
        if ($where !== null) {
            $request['where'] = $where->toArray();
        }
        if ($filter !== null) {
            $request['filter'] = $filter;
        }
        return $request;
    }
}

/** `search`: text, vector, sparse or hybrid, with filters, fusion, highlights, `select` and pagination. */
final class Search extends Base
{
    /**
     * `$request` is a `Search` builder or the `SearchRequest`'s JSON,
     * checked (`Search::build()`) and sent as built.
     *
     * @param SearchBuilder|array<string, mixed> $request
     * @param array<string, mixed> $options
     * @return array<string, mixed>
     */
    public function query(SearchBuilder|array $request, array $options = []): array
    {
        return $this->rpc('search.query', SearchBuilder::wire($request), $options);
    }

    /**
     * A text search: a match on one field, a multi-match on several, or a query string.
     *
     * @param ?list<string> $fields
     * @param array<string, mixed> $options
     * @return array<string, mixed>
     */
    public function text(string $collection, string $text, ?array $fields = null, ?Filter $filter = null, ?int $limit = null, ?string $namespace = null, array $options = []): array
    {
        $s = SearchBuilder::on($collection, $namespace)->text($text, $fields);
        self::finish($s, $filter, $limit);
        return $this->query($s, $options);
    }

    /**
     * Text plus a vector on `$field`, fused (`Fusion::rrf()` by default).
     *
     * @param list<int|float> $vector
     * @param ?list<string> $fields the text fields
     * @param ?array<string, mixed> $fusion
     * @param array<string, mixed> $options
     * @return array<string, mixed>
     */
    public function hybrid(string $collection, string $text, array $vector, string $field, ?array $fields = null, ?array $fusion = null, ?int $k = null, ?Filter $filter = null, ?int $limit = null, ?string $namespace = null, array $options = []): array
    {
        $s = SearchBuilder::on($collection, $namespace)->text($text, $fields, k: $k)->vector($field, $vector, $k)->fuse($fusion ?? Fusion::rrf());
        self::finish($s, $filter, $limit);
        return $this->query($s, $options);
    }

    /**
     * Every hit, page after page through `searchAfter`, until a page has
     * fewer than `limit` hits. The request must sort with the `pk`
     * tiebreaker, or it is refused here, before any network call.
     *
     * @param SearchBuilder|array<string, mixed> $request
     * @param array<string, mixed> $options
     */
    public function queryAll(SearchBuilder|array $request, array $options = []): \Generator
    {
        $page = Json::of($request);
        if (!SearchBuilder::sortsByPk($page)) {
            throw InvalidArgumentException::client("queryAll needs a sort that ends with the primary key, such as ->sort('score', 'pk')", 'search_after_needs_tiebreak');
        }
        $page['limit'] ??= SearchBuilder::DEFAULT_K;
        $limit = (int) $page['limit'];
        while (true) {
            $res = $this->query($page, $options);
            $hits = $res['hits'] ?? [];
            foreach ($hits as $hit) {
                yield $hit;
            }
            if ($hits === [] || count($hits) < $limit) {
                return;
            }
            $page['searchAfter'] = $hits[count($hits) - 1]['sortValues'] ?? [];
        }
    }

    private static function finish(SearchBuilder $s, ?Filter $filter, ?int $limit): void
    {
        if ($filter !== null) {
            $s->filter($filter);
        }
        if ($limit !== null) {
            $s->limit($limit);
        }
    }
}

/** `vector`: the same RPC as `search.query`, with the vector shape as its required arguments (Q613). */
final class Vector extends Base
{
    /** @param list<int|float> $vector @param array<string, mixed> $options @return array<string, mixed> */
    public function search(string $collection, array $vector, string $field, ?int $limit = null, ?int $k = null, ?Filter $filter = null, ?string $namespace = null, array $options = []): array
    {
        $s = SearchBuilder::on($collection, $namespace)->vector($field, $vector, $k);
        if ($filter !== null) {
            $s->filter($filter);
        }
        if ($limit !== null) {
            $s->limit($limit);
        }
        return $this->rpc('vector.search', SearchBuilder::wire($s), $options);
    }
}

/** `graph`: stored property graphs, queried with GQL. Experimental: the package is `unstable`. */
final class Graph extends Base
{
    private const LANGUAGES = ['gql' => 'QUERY_LANGUAGE_GQL', 'cypher' => 'QUERY_LANGUAGE_CYPHER', 'sql_pgq' => 'QUERY_LANGUAGE_SQL_PGQ'];

    public function isUnstable(): bool
    {
        return true;
    }

    /** @param ?array<string, mixed> $mapping @param array<string, mixed> $options @return array<string, mixed> */
    public function create(string $name, ?array $mapping = null, ?string $mode = null, ?string $namespace = null, array $options = []): array
    {
        return $this->rpc('graph.create', self::ns($namespace) + Json::compact([
            'name' => $name, 'mapping' => $mapping, 'mode' => $mode === null ? null : 'GRAPH_MODE_' . strtoupper($mode),
        ]), $options);
    }

    /** @param array<string, mixed> $options @return array<string, mixed> */
    public function get(string $name, ?string $namespace = null, array $options = []): array
    {
        return $this->rpc('graph.get', self::ns($namespace) + ['name' => $name], $options);
    }

    /** @param array<string, mixed> $options @return array<string, mixed> */
    public function list(?int $pageSize = null, ?string $pageToken = null, ?string $namespace = null, array $options = []): array
    {
        return $this->rpc('graph.list', self::ns($namespace) + Json::compact(['pageSize' => $pageSize, 'pageToken' => $pageToken]), $options);
    }

    /** @param array<string, mixed> $options */
    public function listAll(?int $pageSize = null, ?string $namespace = null, array $options = []): \Generator
    {
        return $this->paged('graph.list', self::ns($namespace) + Json::compact(['pageSize' => $pageSize]), 'graphs', $options);
    }

    /**
     * `$graph` is the `Graph` JSON (`name`, and `namespace` or the client's).
     *
     * @param array<string, mixed> $graph
     * @param list<string> $updateMask
     * @param array<string, mixed> $options
     * @return array<string, mixed>
     */
    public function update(array $graph, array $updateMask, ?int $expectedVersion = null, array $options = []): array
    {
        $graph = $this->withNamespace($graph, Calls::CALLS['graph.update']['rpc']);
        return $this->rpc('graph.update', Json::compact([
            'graph' => $graph, 'updateMask' => implode(',', $updateMask),
            'expectedVersion' => $expectedVersion === null ? null : (string) $expectedVersion,
        ]), $options);
    }

    /** Answers an `Operation`. @param array<string, mixed> $options @return array<string, mixed> */
    public function delete(string $name, ?string $namespace = null, array $options = []): array
    {
        return $this->rpc('graph.delete', self::ns($namespace) + ['name' => $name], $options);
    }

    /** @param array<string, mixed> $options @return array<string, mixed> */
    public function schema(string $name, ?string $namespace = null, array $options = []): array
    {
        return $this->rpc('graph.schema', self::ns($namespace) + ['name' => $name], $options);
    }

    /**
     * One statement (GQL by default). `$parameters` are PHP values. A
     * read-only statement is safe to retry; a write carries an idempotency key.
     *
     * @param array<string, mixed> $parameters
     * @param array<string, mixed> $options
     * @return array<string, mixed>
     */
    public function query(string $graph, string $statement, array $parameters = [], bool $readOnly = false, string $language = 'gql', string|\Stringable|null $consistency = null, ?int $maxRows = null, ?string $namespace = null, array $options = []): array
    {
        $request = self::ns($namespace) + Json::compact([
            'graph' => $graph, 'statement' => $statement, 'language' => self::LANGUAGES[$language] ?? $language,
            'parameters' => self::params($parameters), 'readOnly' => $readOnly,
            'consistency' => self::consistency($consistency), 'maxRows' => $maxRows,
        ]);
        return $this->rpc('graph.query', $request, $options + ($readOnly ? ['retry' => 'safe'] : []));
    }

    /**
     * `$statements`: `[['statement' => …, 'parameters' => […]]]`, in one batch.
     *
     * @param list<array<string, mixed>> $statements
     * @param array<string, mixed> $options
     * @return array<string, mixed>
     */
    public function queryBatch(string $graph, array $statements, bool $atomic = true, string $language = 'gql', ?string $namespace = null, array $options = []): array
    {
        $list = array_map(static fn (array $s) => [
            'statement' => $s['statement'], 'parameters' => self::params($s['parameters'] ?? []),
        ], $statements);
        return $this->rpc('graph.query_batch', self::ns($namespace) + [
            'graph' => $graph, 'statements' => $list, 'atomic' => $atomic, 'language' => self::LANGUAGES[$language] ?? $language,
        ], $options);
    }

    /** @param array<string, mixed> $parameters @param array<string, mixed> $options @return array<string, mixed> */
    public function explain(string $graph, string $statement, array $parameters = [], bool $profile = false, string $language = 'gql', ?string $namespace = null, array $options = []): array
    {
        return $this->rpc('graph.explain', self::ns($namespace) + [
            'graph' => $graph, 'statement' => $statement, 'parameters' => self::params($parameters),
            'profile' => $profile, 'language' => self::LANGUAGES[$language] ?? $language,
        ], $options);
    }

    /**
     * An `ExecuteResponse`'s rows as arrays, column name to PHP value.
     *
     * @param array<string, mixed> $response
     * @return list<array<string, mixed>>
     */
    public static function records(array $response): array
    {
        $columns = $response['rows']['columns'] ?? [];
        $out = [];
        foreach ($response['rows']['rows'] ?? [] as $row) {
            $values = array_map([Values::class, 'fromGraphValue'], $row['values'] ?? []);
            $out[] = array_combine($columns, array_pad($values, count($columns), null));
        }
        return $out;
    }

    /** @param array<string, mixed> $parameters */
    private static function params(array $parameters): array|\stdClass
    {
        if ($parameters === []) {
            return Json::empty();
        }
        $out = [];
        foreach ($parameters as $k => $v) {
            $out[(string) $k] = Values::graphValue($v);
        }
        return $out;
    }

    /** @return ?array<string, mixed> */
    private static function consistency(string|\Stringable|null $level): ?array
    {
        return match (true) {
            $level === null => null,
            $level === 'strong' => ['strong' => Json::empty()],
            $level === 'eventual' => ['eventual' => Json::empty()],
            default => ['atLeast' => (string) $level],
        };
    }
}
