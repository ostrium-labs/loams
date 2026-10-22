# Loams PHP SDK (preview)

> **Pre-release (preview).** This SDK is published as a preview of Loams. Its API
> can change between previews without notice: pin the exact version, and expect
> breaking changes until 1.0. Report problems at
> <https://github.com/ostrium-labs/loams/issues>.

## Install

```sh
composer require loams/loams:0.1.1
```

The package is `loams/loams` on Packagist. Composer has no `preview` stability,
so a prerelease such as `v0.1.0-preview.2` is tagged `v0.1.0-alpha.2` there, and
a project whose `minimum-stability` is `stable` must name it exactly (or add
`@alpha`). It needs PHP 8.2 or newer.

Packagist reads the read-only split repository
[`ostrium-labs/loams-php`](https://github.com/ostrium-labs/loams-php), which
`sdk-release.yml` updates from this directory on each release. Send changes here,
to `sdks/php` in `ostrium-labs/loams`, not to the split repository.

## Use

```php
<?php
require 'vendor/autoload.php';

$client = new Loams\Client('https://acme.loams.dev');
```

## Examples

Runnable examples (quickstart, approvals, status) are in [`examples/`](examples/).

## Build an app

The app-building modules are `namespaces()`, `collections()`, `documents()`,
`search()`, `vector()` and `graph()` (experimental), with the `Schema`, `Filter`
and `Search` builders in `Loams\Builders`.
[`examples/notes/notes.php`](examples/notes/notes.php) is the whole flow:

```php
use Loams\Builders\{Filter, Fusion, Schema, Search};
use Loams\Client;

// LOAMS_ENDPOINT, LOAMS_API_KEY (or LOAMS_TOKEN) and LOAMS_NAMESPACE.
$client = Client::fromEnv(sessionConsistency: true);
// or: new Client('http://127.0.0.1:8080', apiKey: '…', namespace: 'app', timeout: 30.0)

$client->namespaces()->create('app');
$client->collections()->create('notes', Schema::new()
    ->keyword('title')->text('body')->keyword('tags')->i64('n')->vector('embedding', 4));

$notes = $client->collection('notes');
$notes->upsert([['id' => 1, 'source' => ['title' => 'Refunds', 'body' => 'refund policy', 'tags' => ['policy'], 'n' => 1],
                 'vectors' => ['embedding' => [1, 0, 0, 0]]]]);

$hits = $notes->search(Search::on('notes')
    ->text('refund', ['body'])
    ->vector('embedding', [1, 0, 0, 0])
    ->fuse(Fusion::rrf(60))            // or Fusion::dbsf(), Fusion::weighted(0.7, 0.3)
    ->filter(Filter::and(Filter::eq('tags', 'policy'), Filter::range('n', gte: 1)))
    ->highlight('body')
    ->select(include: ['title'])
    ->limit(10))['hits'];
```

- **Answers are proto3 JSON arrays** (`lowerCamelCase` keys; 64-bit integers
  as strings). `Loams\Values::fromDocumentId($hit['pk'])` gives a key back as
  an int or a string.
- **Writes.** `documents()->upsert`, `delete`, `patch` and `write` (raw ops)
  answer a consistency `token`. `writeBatch($collection, $iterable, chunkRows:,
  chunkBytes:)` sends chunks one at a time, each with its own idempotency key,
  and returns a `Loams\BatchResult` (the merged `token`, `chunks`, `rows`); a
  failed chunk throws `Loams\BatchException` with `chunksApplied` and the
  `token` of the chunks before it.
- **Reads.** `documents()->get`, `count`, `scroll` / `scrollAll` (a
  `Generator`), `search()->query`, `queryAll` (follows `searchAfter`; the sort
  must end with `Sort::pk()`), `text($collection, $text, $fields)`,
  `hybrid($collection, $text, $vector, $field, $fields, $fusion)` and
  `vector()->search($collection, $vector, $field)`. Each
  takes `consistency:` (`'strong'`, `'eventual'` or a token). With
  `sessionConsistency: true` every token an answer carries is merged into the
  session and attached to later reads.
- **Graph** (experimental): `graph()->create`, `query` (GQL with PHP
  parameters), `queryBatch`, `explain`, `listAll`, `delete`;
  `Loams\Modules\Graph::records($res)` gives rows as arrays.
- **The default namespace** fills every request that names none; with neither,
  the call throws `Loams\InvalidArgumentException` before any network call.
  **Timeouts** default to 30 s per unary call (`timeout:` on the client,
  `options: ['timeout' => …]` per call, `null` for none); a spent budget throws
  `Loams\DeadlineExceededException` with reason `client_deadline_exceeded` and
  is never retried. Every call takes `options: ['idempotencyKey' => …]` too.
- **SQL.** `sql()->query($sql, [$1, …], maxRows:, consistency:)` answers
  `columns` and `rows`; `Loams\Modules\Sql::records($res)` gives rows as
  arrays. `queryArrow` answers the Arrow IPC bytes. SQL reads see the session's
  collection writes.
- **Streams.** `streams()->create`, `get`, `list` / `listAll`, `produce($stream,
  $records, partition:, producerId:, producerSeq:)` (a record is a string, an array
  sent as JSON, or `['key' => …, 'value' => …, 'headers' => […]]`; a repeated
  producer sequence is deduplicated), `fetch` (`Streams::records($res)` decodes
  it), `produceEvents` / `fetchEvents` (CloudEvents as arrays), and
  `subscribe($stream, partition:, fromOffset:)`, a `Subscription` to iterate,
  `cancel()` and resume from `offset()`.
- Durable execution has no module: use a Resonate SDK with
  `LOAMS_ENDPOINT/durable/` as its URL.

## Develop

```sh
composer install
vendor/bin/phpunit
LOAMS_TEST_ENDPOINT=http://127.0.0.1:8080 vendor/bin/phpunit  # plus the app scenario against `loams dev`
```

Licensed under Apache-2.0 (see `LICENSE`).
