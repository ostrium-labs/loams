import 'dart:io';
import 'dart:math';

import 'package:loams/loams.dart';
import 'package:test/test.dart';

/// The SDKP app scenario through the public facade, against a running server:
/// `LOAMS_TEST_ENDPOINT=http://… dart test`. Skipped without one.
const notesData = [
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
    'body': 'shipping times and carriers',
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
  Doc(4, {
    'title': 'Returns',
    'body': 'returns need a receipt',
    'tags': ['policy', 'ops'],
    'n': 4
  }, {
    'embedding': [0.5, 0.5, 0, 0]
  }),
  Doc(5, {
    'title': 'Holidays',
    'body': 'holiday hours',
    'tags': ['ops'],
    'n': 5
  }, {
    'embedding': [0, 0, 1, 0]
  }),
];

Schema notesSchema() => Schema().keyword('title').text('body').keyword('tags').i64('n').vector('embedding', 4);

void main() {
  final endpoint = Platform.environment['LOAMS_TEST_ENDPOINT'];
  final skip = endpoint == null || endpoint.isEmpty ? 'LOAMS_TEST_ENDPOINT is not set' : null;
  late String ns;
  late LoamsClient client;

  setUp(() {
    if (skip != null) return;
    ns = 'parity-dart-${DateTime.now().millisecondsSinceEpoch}-${Random().nextInt(1000)}';
    client = LoamsClient(endpoint!,
        apiKey: Platform.environment['LOAMS_TEST_API_KEY'], namespace: ns, sessionConsistency: true);
  });

  tearDown(() {
    if (skip == null) client.close();
  });

  test('dart_live_app_collections', () async {
    expect(await client.instances.getInstance(), isNotEmpty);
    await client.namespaces.create(ns);
    await expectLater(client.namespaces.create(ns), throwsA(isA<AlreadyExistsException>()));
    expect(await client.namespaces.listAll().map((n) => n['namespace']).toList(), contains(ns));

    expect((await client.collections.create('notes', schema: notesSchema()))['name'], 'notes');
    expect((await client.collections.get('notes'))['name'], 'notes');
    await client.collections.create('scratch', schema: Schema().keyword('k'));
    final names = await client.collections.listAll(pageSize: 1).map((c) => c['name']).toList();
    expect(names..sort(), ['notes', 'scratch']);

    final key = Idempotency.mintKey();
    final first = await client.documents.upsert('notes', notesData, options: CallOptions(idempotencyKey: key));
    final again = await client.documents.upsert('notes', notesData, options: CallOptions(idempotencyKey: key));
    expect(again['token'], first['token'], reason: 'the same key answers the same write (R3)');

    final got = await client.documents.get('notes', [1, 2], consistency: first['token']);
    expect([for (final d in got['documents'] as List) (d as Map)['source']['title']], ['Refunds', 'Shipping']);

    await client.documents.patch('notes', 4, set: {'title': 'Returns and exchanges'});
    await client.documents.delete('notes', [5]);
    // The native filter JSON until P0a types `where`.
    final policy = await client.documents.count('notes', filter: {
      'term': {'field': 'tags', 'value': 'policy'}
    });
    expect(policy['count'], '3');
    final ids = await client.documents
        .scrollAll('notes', limit: 2)
        .map((d) => Values.fromDocumentId(d['id'] as Map<String, dynamic>))
        .toList();
    expect(ids..sort((a, b) => (a as int).compareTo(b as int)), [1, 2, 3, 4]);

    await expectLater(client.collections.get('missing'), throwsA(isA<NotFoundException>()));
    expect((await client.collections.drop('scratch'))['dropped'], isTrue);

    final handle = client.collection('notes');
    expect((await handle.info())['name'], 'notes');
    final result = await handle.writeBatch([
      for (var i = 10; i < 15; i++) Doc(i, {'n': i})
    ], chunkRows: 2);
    expect([result.chunks, result.rows], [3, 5]);
    expect((await handle.count(consistency: result.token))['count'], '9');
  }, skip: skip);

  test('dart_live_app_search', () async {
    await client.namespaces.create(ns);
    await client.collections.create('notes', schema: notesSchema());
    await client.documents.upsert('notes', notesData);

    final page = await client.search.query(Search.on('notes')
        .text('refund', fields: ['body'])
        .filter(Filter.eq('tags', 'policy'))
        .highlight('body')
        .select(include: ['title'])
        .limit(2)
        .sort(['score', 'pk']));
    final hits = page['hits'] as List;
    expect(hits, hasLength(2));
    expect(((hits.first as Map)['source'] as Map).keys, ['title']);
    expect((hits.first as Map)['highlight'], isNotEmpty);

    final all = await client.search
        .queryAll(Search.on('notes').text('refund', fields: ['body']).sort(['score', 'pk']).limit(1))
        .toList();
    expect(all, hasLength(2));

    final near = await client.vector.search('notes', [1, 0, 0, 0], field: 'embedding', limit: 2);
    expect(Values.fromDocumentId(((near['hits'] as List).first as Map)['pk'] as Map<String, dynamic>), 1);

    for (final fusion in [
      Fusion.rrf(60),
      Fusion.dbsf(),
      Fusion.weighted([0.7, 0.3])
    ]) {
      final res = await client.search.hybrid('notes', 'refund', [1, 0, 0, 0],
          field: 'embedding', fields: ['body'], fusion: fusion, filter: Filter.eq('tags', 'policy'), limit: 3);
      expect(res['hits'], isNotEmpty, reason: fusion.keys.first);
    }
    await expectLater(
        client.search.hybrid('notes', 'refund', [1, 0, 0, 0],
            field: 'embedding', fields: ['body'], fusion: Fusion.weighted([0.7])),
        throwsA(isA<InvalidArgumentException>()));
    final handleHits =
        await client.collection('notes').search(Search.on('notes').text('refund', fields: ['body']).limit(5));
    expect(handleHits['hits'], hasLength(2));
  }, skip: skip);

  test('dart_live_app_graph', () async {
    await client.namespaces.create(ns);
    try {
      await client.graph.create('notes_graph');
    } on LoamsException catch (e) {
      if (e.reason == 'feature_not_in_variant') {
        markTestSkipped('this build serves no graph');
        return;
      }
      rethrow;
    }
    await client.graph.query('notes_graph', r'INSERT (:Note {id: $id, title: $title})-[:TAGGED]->(:Tag {name: $tag})',
        parameters: {'id': 1, 'title': 'Refunds', 'tag': 'policy'});
    final read = await client.graph.query(
        'notes_graph', 'MATCH (n:Note)-[:TAGGED]->(t:Tag) RETURN n.title AS title, t.name AS tag',
        readOnly: true);
    expect(GraphModule.records(read), [
      {'title': 'Refunds', 'tag': 'policy'}
    ]);
    expect(await client.graph.listAll().map((g) => g['name']).toList(), contains('notes_graph'));
    expect(await client.graph.delete('notes_graph'), isNotNull);
  }, skip: skip);
}
