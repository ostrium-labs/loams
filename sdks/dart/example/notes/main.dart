// A notes app on Loams: a namespace, a collection with a schema, documents,
// a hybrid search with a filter, and a graph query.
//
//   LOAMS_ENDPOINT=http://127.0.0.1:8080 LOAMS_NAMESPACE=app dart run example/notes/main.dart

import 'package:loams/loams.dart';

Future<void> main() async {
  // With sessionConsistency, every read sees this client's writes.
  final client = LoamsClient.fromEnv(sessionConsistency: true);
  try {
    final instance = await client.instances.getInstance();
    print('engine API versions: ${instance['apiVersions']}');
    try {
      await client.namespaces.create(client.defaultNamespace!);
    } on AlreadyExistsException {
      // Created by an earlier run.
    }
    try {
      await client.collections.create('notes',
          schema: Schema().keyword('title').text('body').keyword('tags').i64('n').vector('embedding', 4));
    } on AlreadyExistsException {
      // Created by an earlier run.
    }

    final notes = client.collection('notes');
    await notes.upsert(const [
      Doc(1, {
        'title': 'Refunds',
        'body': 'refund policy for orders',
        'tags': ['policy'],
        'n': 1
      }, {
        'embedding': [1, 0, 0, 0]
      }),
      Doc(2, {
        'title': 'Shipping',
        'body': 'shipping times',
        'tags': ['ops'],
        'n': 2
      }, {
        'embedding': [0, 1, 0, 0]
      }),
      Doc(3, {
        'title': 'Refund window',
        'body': 'the refund window is thirty days',
        'tags': ['policy'],
        'n': 3
      }, {
        'embedding': [0.9, 0.1, 0, 0]
      }),
    ]);

    final page = await notes.search(Search.on('notes')
        // k is explicit so the example also runs on a 0.1.0 engine, which has no default k.
        .text('refund', fields: ['body'], k: 10)
        .vector('embedding', [1, 0, 0, 0], k: 10)
        .fuse(Fusion.rrf(60))
        .filter(Filter.eq('tags', 'policy'))
        .highlight('body')
        .select(include: ['title'])
        .limit(5));
    for (final hit in (page['hits'] as List? ?? const [])) {
      final h = hit as Map<String, dynamic>;
      print(
          '${Values.fromDocumentId(h['pk'] as Map<String, dynamic>)}: ${(h['source'] as Map)['title']} (${h['score']})');
    }
    print('${(await notes.count())['count']} notes');

    final nearest = await client.vector.search('notes', [1, 0, 0, 0], field: 'embedding', limit: 3);
    print('vector hits: ${(nearest['hits'] as List? ?? const []).length}');

    // Graph is experimental and not in every build.
    try {
      await client.graph.create('notes_graph');
      await client.graph.query('notes_graph', r'INSERT (:Note {id: $id})-[:TAGGED]->(:Tag {name: $tag})',
          parameters: {'id': 1, 'tag': 'policy'});
      final rows = await client.graph
          .query('notes_graph', 'MATCH (n:Note)-[:TAGGED]->(t:Tag) RETURN n.id AS id, t.name AS tag', readOnly: true);
      print(GraphModule.records(rows));
      await client.graph.delete('notes_graph');
    } on LoamsException catch (e) {
      if (e.reason != 'feature_not_in_variant') rethrow;
      print('this server has no graph');
    }

    final sqlResponse = await client.sql.query(
        'SELECT n, title FROM notes WHERE n >= \$1 ORDER BY n', params: [2]);
    print('SQL rows: ${SqlModule.records(sqlResponse)}');

    await client.streams.create('notes_events', partitions: 1);
    final produced = await client.streams.produce('notes_events', [
      {'kind': 'created', 'note': 1},
      {'kind': 'patched', 'note': 2},
    ], producerId: 'dart-notes', producerSeq: 1);
    final start = int.parse((produced['baseOffset'] ?? '0').toString());
    final subscription = client.streams.subscribe('notes_events', fromOffset: start, maxWaitMs: 100);
    await for (final event in subscription.records) {
      print('stream event at offset ${event.offset}: ${event.json}');
      subscription.cancel();
      break;
    }
  } finally {
    client.close();
  }
}
