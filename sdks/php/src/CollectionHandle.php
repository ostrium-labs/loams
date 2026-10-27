<?php

declare(strict_types=1);

namespace Loams;

use Loams\Builders\Filter;
use Loams\Builders\Search;

/**
 * `$client->collection('notes')` (SDKP §2.8): every document and search call
 * with the collection (and namespace) filled in. It holds no state of its own.
 */
final class CollectionHandle
{
    public function __construct(private readonly Client $client, public readonly string $name, public readonly ?string $namespace = null)
    {
    }

    /** @param iterable<array<string, mixed>> $docs @return array<string, mixed> */
    public function upsert(iterable $docs, array $options = []): array
    {
        return $this->client->documents()->upsert($this->name, $docs, $this->namespace, $options);
    }

    /** @param list<int|string|array<string, string>> $ids @return array<string, mixed> */
    public function get(array $ids, ?array $select = null, string|\Stringable|null $consistency = null, array $options = []): array
    {
        return $this->client->documents()->get($this->name, $ids, $select, $consistency, $this->namespace, $options);
    }

    /** @param list<int|string|array<string, string>> $ids @return array<string, mixed> */
    public function delete(array $ids, array $options = []): array
    {
        return $this->client->documents()->delete($this->name, $ids, $this->namespace, $options);
    }

    /** @return array<string, mixed> */
    public function patch(int|string|array $id, ?array $set = null, ?array $unset = null, array $options = []): array
    {
        return $this->client->documents()->patch($this->name, $id, $set, $unset, null, $this->namespace, $options);
    }

    /** @param iterable<array<string, mixed>> $docs */
    public function writeBatch(iterable $docs, int $chunkRows = Batch::DEFAULT_CHUNK_ROWS, int $chunkBytes = Batch::DEFAULT_CHUNK_BYTES, array $options = []): BatchResult
    {
        return $this->client->documents()->writeBatch($this->name, $docs, $chunkRows, $chunkBytes, $this->namespace, $options);
    }

    /** @return array<string, mixed> */
    public function count(?Filter $where = null, mixed $filter = null, string|\Stringable|null $consistency = null, array $options = []): array
    {
        return $this->client->documents()->count($this->name, $where, $filter, $consistency, $this->namespace, $options);
    }

    public function scrollAll(?Filter $where = null, mixed $filter = null, ?int $limit = null, array $options = []): \Generator
    {
        return $this->client->documents()->scrollAll($this->name, $where, $filter, $limit, null, null, $this->namespace, $options);
    }

    /**
     * `$builder`'s collection is set to this one; without one, a fresh
     * `Search::on($name)` is passed to `$configure`.
     *
     * @return array<string, mixed>
     */
    public function search(?Search $builder = null, ?callable $configure = null, array $options = []): array
    {
        $builder = $builder === null ? Search::on($this->name) : (clone $builder)->collection($this->name);
        if ($this->namespace !== null && $builder->namespaceName() === null) {
            $builder->namespace($this->namespace);
        }
        if ($configure !== null) {
            $configure($builder);
        }
        return $this->client->search()->query($builder, $options);
    }

    /** @param list<int|float> $vector @return array<string, mixed> */
    public function vectorSearch(array $vector, string $field, ?int $limit = null, ?Filter $filter = null, array $options = []): array
    {
        return $this->client->vector()->search($this->name, $vector, $field, $limit, null, $filter, $this->namespace, $options);
    }

    /** @return array<string, mixed> */
    public function drop(array $options = []): array
    {
        return $this->client->collections()->drop($this->name, $this->namespace, $options);
    }

    /** @return array<string, mixed> */
    public function info(array $options = []): array
    {
        return $this->client->collections()->get($this->name, $this->namespace, $options);
    }
}
