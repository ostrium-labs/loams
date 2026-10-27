# Loams Dart SDK (preview)

> **Pre-release (preview).** This SDK is published as a preview of Loams. Its API
> can change between previews without notice: pin the exact version, and expect
> breaking changes until 1.0. Report problems at
> <https://github.com/ostrium-labs/loams/issues>.

## Install

The `v0.1.2` source tag is published. The 0.1.2 package is not currently
available from pub.dev, so install the SDK from the tagged monorepo source:

```yaml
dependencies:
  loams:
    git:
      url: https://github.com/ostrium-labs/loams.git
      ref: v0.1.2
      path: sdks/dart
```

It works with Dart 3 and Flutter.

## Use

```dart
import 'package:loams/loams.dart';
```

## Build an app

The app-building modules are `namespaces`, `collections`, `documents`,
`search`, `vector` and `graph` (experimental), with the `Schema`, `Filter` and
`Search` builders. [`example/notes/main.dart`](example/notes/main.dart) is the
whole flow:

See the [notes example instructions](example/notes/README.md) to run it from
the matching source checkout.

```dart
import 'package:loams/loams.dart';

// LOAMS_ENDPOINT, LOAMS_API_KEY (or LOAMS_TOKEN) and LOAMS_NAMESPACE.
final client = LoamsClient.fromEnv(sessionConsistency: true);
// or: LoamsClient('http://127.0.0.1:8080', apiKey: '…', namespace: 'app', timeout: Duration(seconds: 30))

await client.namespaces.create('app');
await client.collections.create('notes',
    schema: Schema().keyword('title').text('body').keyword('tags').i64('n').vector('embedding', 4));

final notes = client.collection('notes');
await notes.upsert(const [
  Doc(1, {'title': 'Refunds', 'body': 'refund policy', 'tags': ['policy'], 'n': 1}, {'embedding': [1, 0, 0, 0]}),
]);

final page = await notes.search(Search.on('notes')
    .text('refund', fields: ['body'])
    .vector('embedding', [1, 0, 0, 0])
    .fuse(Fusion.rrf(60)) // or Fusion.dbsf(), Fusion.weighted([0.7, 0.3])
    .filter(Filter.eq('tags', 'policy') & Filter.range('n', gte: 1))
    .highlight('body')
    .select(include: ['title'])
    .limit(10));
```

- **Answers are proto3 JSON maps** (`lowerCamelCase` keys; 64-bit integers as
  strings). `Values.fromDocumentId(hit['pk'])` gives a key back as an int or a String.
- **Writes.** `documents.upsert`, `delete`, `patch` and `write` (raw ops) answer a
  consistency `token`. `documents.writeBatch(collection, docs, chunkRows:, chunkBytes:)`
  sends chunks one at a time, each with its own idempotency key, and answers a
  `BatchResult` (`token`, `chunks`, `rows`); a failed chunk throws a
  `BatchException` with `chunksApplied` and the `token` of the chunks before it.
- **Reads.** `documents.get`, `count`, `scroll` and `scrollAll` (a `Stream`),
  `search.query`, `search.queryAll` (a `Stream` that follows `searchAfter`; the
  sort must end with `'pk'`), `search.text`, `search.hybrid(…, field:, fields:,
  fusion:)` and `vector.search(…, field:)`. Each takes `consistency:` (`'strong'`,
  `'eventual'` or a token). With `sessionConsistency: true` every token an answer
  carries is merged into the session and attached to later reads.
- **Graph** (experimental): `graph.create`, `query` (GQL with Dart `parameters`;
  `readOnly: true` retries as a read), `queryBatch`, `explain`, `listAll`,
  `delete`; `GraphModule.records(res)` gives rows as maps.
- **The default namespace** fills every request that names none; with neither,
  the call throws `InvalidArgumentException` before any network call.
  **Timeouts**: 30 s per unary call (`timeout:` on the client, `CallOptions(timeout:)`
  per call, `null` for none); a spent budget throws `DeadlineExceededException`
  with reason `client_deadline_exceeded` and is never retried.
- **SQL.** `sql.query(sql, params: [...], maxRows:, consistency:)` answers
  `columns` and `rows`; `SqlModule.records(res)` gives rows as maps.
  `queryArrow` answers the Arrow IPC bytes. SQL reads see the session's
  collection writes.
- **Streams.** `streams.create`, `get`, `list` / `listAll`, `produce(stream,
  records, partition:, producerId:, producerSeq:)` (a record is bytes, a string, a
  JSON value or a `ProduceRecord`; a repeated producer sequence is deduplicated),
  `fetch` (`StreamsModule.records(res)` decodes it into `StreamRecord`s),
  `produceEvents` / `fetchEvents` (CloudEvents as maps), and `subscribe(stream,
  partition:, fromOffset:)`, whose `records` is a `Stream` that resumes from
  `offset`.
- Durable execution has no module: use a Resonate SDK with `LOAMS_ENDPOINT/durable/`.

## Develop

```sh
dart pub get
dart analyze
dart test
dart pub publish --dry-run   # what sdk-release.yml checks
```

Licensed under Apache-2.0 (see `LICENSE`).
