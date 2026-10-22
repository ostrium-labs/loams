<?php

declare(strict_types=1);

namespace Loams\Builders;

/**
 * The `CreateCollectionRequest.schema` Struct (SDKP §2.6), in the shape the
 * server's schema parser reads (`kb_schema()` in `connect_collections.rs`).
 *
 *     Schema::new()->keyword('title')->text('body')->i64('n')->vector('embedding', 4)
 */
final class Schema implements \JsonSerializable
{
    /** @var list<array<string, mixed>> */
    private array $fields = [];
    /** @var list<array<string, mixed>> */
    private array $vectors = [];
    /** @var list<array<string, mixed>> */
    private array $sparse = [];
    private string $dynamic = 'ignore';
    private int $maxFields = 1000;

    public static function new(): self
    {
        return new self();
    }

    /** A full-text field; `positions` is what phrase queries need. */
    public function text(string $name, string $analyzer = 'standard', bool $positions = true, bool $indexed = true, bool $fast = false, ?string $sourcePath = null): self
    {
        return $this->field($name, ['text' => ['analyzer' => $analyzer, 'positions' => $positions]], $indexed, $fast, $sourcePath);
    }

    public function keyword(string $name, bool $indexed = true, bool $fast = true, ?string $sourcePath = null): self
    {
        return $this->field($name, 'keyword', $indexed, $fast, $sourcePath);
    }

    public function i64(string $name, bool $indexed = true, bool $fast = true, ?string $sourcePath = null): self
    {
        return $this->field($name, 'i64', $indexed, $fast, $sourcePath);
    }

    public function f64(string $name, bool $indexed = true, bool $fast = true, ?string $sourcePath = null): self
    {
        return $this->field($name, 'f64', $indexed, $fast, $sourcePath);
    }

    public function bool(string $name, bool $indexed = true, bool $fast = true, ?string $sourcePath = null): self
    {
        return $this->field($name, 'bool', $indexed, $fast, $sourcePath);
    }

    /** A date-time field (the schema's `date` kind). */
    public function timestamp(string $name, bool $indexed = true, bool $fast = true, ?string $sourcePath = null): self
    {
        return $this->field($name, 'date', $indexed, $fast, $sourcePath);
    }

    public function uuid(string $name, bool $indexed = true, bool $fast = true, ?string $sourcePath = null): self
    {
        return $this->field($name, 'uuid', $indexed, $fast, $sourcePath);
    }

    public function vector(string $name, int $dim, string $distance = 'cosine'): self
    {
        $this->vectors[] = ['name' => $name, 'dim' => $dim, 'distance' => $distance];
        return $this;
    }

    public function sparseVector(string $name, ?string $modifier = null): self
    {
        $this->sparse[] = Json::compact(['name' => $name, 'modifier' => $modifier]);
        return $this;
    }

    /** `ignore` (the builder's default), `strict` (the server's) or `map`. */
    public function dynamic(string $mode): self
    {
        if (!in_array($mode, ['ignore', 'strict', 'map'], true)) {
            throw new \InvalidArgumentException('dynamic must be ignore, strict or map');
        }
        $this->dynamic = $mode;
        return $this;
    }

    public function maxFields(int $count): self
    {
        $this->maxFields = $count;
        return $this;
    }

    /** @return array<string, mixed> */
    public function toArray(): array
    {
        return [
            'fields' => $this->fields,
            'vectors' => $this->vectors,
            'sparse_vectors' => $this->sparse,
            'dynamic' => $this->dynamic,
            'max_fields' => $this->maxFields,
        ];
    }

    public function jsonSerialize(): mixed
    {
        return $this->toArray();
    }

    /** @param string|array<string, mixed> $kind */
    private function field(string $name, string|array $kind, bool $indexed, bool $fast, ?string $sourcePath): self
    {
        $this->fields[] = [
            'name' => $name,
            'source_path' => $sourcePath ?? $name,
            'kind' => $kind,
            'indexed' => $indexed,
            'fast' => $fast,
        ];
        return $this;
    }
}
