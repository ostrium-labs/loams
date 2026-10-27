import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:loams/loams.dart';
import 'package:test/test.dart';

import 'builder_corpus.dart';

/// A scripted server: every request is recorded, every answer queued.
class Script {
  final requests = <http.Request>[];
  final answers = <Object>[];

  void ok([Map<String, dynamic> json = const {}]) => answers.add(http.Response(jsonEncode(json), 200));

  void fail(int status, String code, String message) =>
      answers.add(http.Response(jsonEncode({'code': code, 'message': message}), status));

  MockClient get client => MockClient((request) async {
        requests.add(request);
        if (answers.isEmpty) throw StateError('no queued answer');
        final next = answers.removeAt(0);
        if (next is Future<http.Response> Function()) return next();
        return next as http.Response;
      });

  Map<String, dynamic> sent(int i) => jsonDecode(requests[i].body) as Map<String, dynamic>;

  String method(int i) => requests[i].url.pathSegments.last;
}

LoamsClient clientOf(Script s,
        {String? namespace, bool session = false, Duration? timeout = Transport.defaultTimeout, int maxRetries = 3}) =>
    LoamsClient('http://127.0.0.1:4400',
        httpClient: s.client,
        namespace: namespace,
        sessionConsistency: session,
        timeout: timeout,
        maxRetries: maxRetries);

Object? canon(Object? json) => jsonDecode(jsonEncode(json));

void main() {
  test('dart_consistency_token_merge', () {
    expect(ConsistencyToken.parse('v1:s7/p3@918274').toString(), 'v1:s7/p3@918274');
    expect(ConsistencyToken.parse('v1:s2/p0@1,s1/p5@3').toString(), 'v1:s2/p0@1,s1/p5@3',
        reason: 'parse keeps the order');
    final merged = ConsistencyToken.parse('v1:s7/p3@10,s1/p0@5').merge('v1:s7/p3@4,s7/p1@2,s1/p0@9');
    expect(merged.toString(), 'v1:s1/p0@9,s7/p1@2,s7/p3@10');
    expect(merged.offset(BigInt.from(7), 3), BigInt.from(10));
    expect(merged.offset(BigInt.one, 1), isNull);

    final corpus = File('${BuilderCorpus.dir}/tokens.json');
    if (!corpus.existsSync()) {
      markTestSkipped('no token corpus at ${corpus.path}; the inline cases ran');
      return;
    }
    final data = jsonDecode(corpus.readAsStringSync()) as Map<String, dynamic>;
    for (final c in data['parse'] as List) {
      final token = ConsistencyToken.parse(c['text'] as String);
      expect(token.toString(), c['text'], reason: c['name'] as String);
      expect([
        for (final i in token.items) ['${i.stream}', i.partition, '${i.offset}']
      ], c['items'], reason: c['name'] as String);
    }
    for (final c in data['merge'] as List) {
      expect(ConsistencyToken.parse(c['a'] as String).merge(ConsistencyToken.parse(c['b'] as String)).toString(),
          c['expect'],
          reason: c['name'] as String);
    }
    for (final bad in data['malformed'] as List) {
      expect(() => ConsistencyToken.parse(bad as String), throwsA(isA<InvalidArgumentException>()), reason: bad);
    }
  });

  test('dart_session_read_your_writes', () async {
    final s = Script();
    final client = clientOf(s, namespace: 'app', session: true);
    s.ok({'token': 'v1:s1/p0@5'});
    s.ok({'readToken': 'v1:s1/p0@5,s1/p1@3'});
    s.ok();
    s.ok();
    await client.documents.upsert('notes', [
      const Doc(1, {'title': 'a'})
    ]);
    await client.documents.get('notes', [1]);
    expect(s.sent(1)['consistency'], {'atLeast': 'v1:s1/p0@5'});
    await client.documents.get('notes', [1]);
    expect(s.sent(2)['consistency'], {'atLeast': 'v1:s1/p0@5,s1/p1@3'});
    await client.documents.get('notes', [1], consistency: 'eventual');
    expect(s.sent(3)['consistency'], {'freshness': 'FRESHNESS_EVENTUAL'},
        reason: 'an explicit consistency is the caller\'s');

    // SDKP-17: a graph read never takes the session's token.
    s.ok();
    await client.graph.query('g', 'MATCH (n) RETURN n', readOnly: true);
    expect(s.sent(4).containsKey('consistency'), isFalse);

    final p = Script();
    final plain = clientOf(p, namespace: 'app');
    p.ok({'token': 'v1:s1/p0@5'});
    p.ok();
    await plain.documents.upsert('notes', [const Doc(1)]);
    await plain.documents.get('notes', [1]);
    expect(p.sent(1).containsKey('consistency'), isFalse, reason: 'off by default');
  });

  test('dart_default_namespace', () async {
    final s = Script();
    final client = clientOf(s, namespace: 'app');
    s.ok({'name': 'notes'});
    s.ok({'name': 'notes'});
    await client.collections.get('notes');
    expect(s.sent(0)['namespace'], 'app');
    await client.collections.get('notes', namespace: 'other');
    expect(s.sent(1)['namespace'], 'other');

    final b = Script();
    await expectLater(clientOf(b).collections.get('notes'),
        throwsA(isA<InvalidArgumentException>().having((e) => e.reason, 'reason', 'namespace_required')));
    expect(b.requests, isEmpty, reason: 'refused before any network call');
  });

  test('dart_timeout_deadline', () async {
    final s = Script();
    final client = clientOf(s, namespace: 'app', timeout: const Duration(seconds: 5));
    s.ok();
    s.ok();
    await client.collections.get('notes');
    final ms = int.parse(s.requests[0].headers['connect-timeout-ms']!);
    expect(ms > 0 && ms <= 5000, isTrue, reason: 'the client default goes out as connect-timeout-ms');
    await client.collections.get('notes', options: const CallOptions(timeout: Duration(milliseconds: 250)));
    expect(int.parse(s.requests[1].headers['connect-timeout-ms']!), lessThanOrEqualTo(250));

    final slow = Script();
    slow.answers.add(() async {
      await Future<void>.delayed(const Duration(milliseconds: 300));
      return http.Response('{}', 200);
    });
    slow.ok();
    final err = await clientOf(slow, namespace: 'app', timeout: const Duration(milliseconds: 50))
        .collections
        .get('notes')
        .then<Object?>((_) => null, onError: (Object e) => e);
    expect(err, isA<DeadlineExceededException>());
    expect((err as DeadlineExceededException).reason, 'client_deadline_exceeded');
    expect(slow.requests, hasLength(1), reason: 'a spent budget is never retried');

    final n = Script();
    n.ok();
    await clientOf(n, namespace: 'app', timeout: null).collections.get('notes');
    expect(n.requests[0].headers.containsKey('connect-timeout-ms'), isFalse);
  });

  test('dart_write_batch_chunks', () async {
    final docs = [
      for (var i = 1; i <= 5; i++) Doc(i, {'n': i})
    ];
    final s = Script();
    final client = clientOf(s, namespace: 'app');
    s.ok({'token': 'v1:s1/p0@2'});
    s.ok({'token': 'v1:s1/p1@2'});
    s.ok({'token': 'v1:s1/p0@5'});
    final result = await client.documents.writeBatch('notes', docs, chunkRows: 2);
    expect(result.token, 'v1:s1/p0@5,s1/p1@2');
    expect([result.chunks, result.rows], [3, 5]);
    expect([for (var i = 0; i < 3; i++) (s.sent(i)['ops'] as List).length], [2, 2, 1]);
    final keys = [for (var i = 0; i < 3; i++) s.requests[i].headers['idempotency-key']];
    expect(keys.toSet(), hasLength(3), reason: 'every chunk has its own key');
    expect(keys, [for (var i = 0; i < 3; i++) s.sent(i)['idempotencyKey']]);

    final f = Script();
    f.ok({'token': 'v1:s1/p0@2'});
    f.fail(503, 'unavailable', 'down');
    f.ok({'token': 'v1:s1/p0@4'});
    f.fail(400, 'invalid_argument', 'bad doc');
    final err = await clientOf(f, namespace: 'app')
        .documents
        .writeBatch('notes', docs, chunkRows: 2)
        .then<Object?>((_) => null, onError: (Object e) => e);
    expect(err, isA<BatchException>());
    final batch = err as BatchException;
    expect([batch.chunksApplied, batch.rowsApplied], [2, 4]);
    expect(batch.token, 'v1:s1/p0@4');
    expect(batch.reason, 'batch_partially_applied');
    expect(batch.code, 'aborted');
    expect(batch.cause.code, 'invalid_argument');
    expect(batch.metadata['chunks_applied'], '2');
    expect(f.requests[1].headers['idempotency-key'], f.requests[2].headers['idempotency-key'],
        reason: 'a retried chunk reuses its key');

    final b = Script();
    for (var i = 0; i < 5; i++) {
      b.ok();
    }
    await clientOf(b, namespace: 'app').documents.writeBatch('notes', docs, chunkBytes: 60);
    expect(b.requests.length, greaterThan(1), reason: 'the byte budget closes a chunk');
  });

  test('dart_streams_subscribe_resumes', () async {
    final log = [
      for (var i = 0; i < 5; i++) {'offset': i, 'value': 'r$i'}
    ];
    final fetched = <int>[];
    final sleeps = <int>[];
    final sub = Subscription<Map<String, Object>>((offset) async {
      fetched.add(offset);
      if (fetched.length == 2 || fetched.length == 3) return [];
      return log.where((r) => (r['offset'] as int) >= offset).take(2).toList();
    }, (r) => r['offset'] as int, fromOffset: 1, sleep: (d) async => sleeps.add(d.inMilliseconds));
    final got = <String>[];
    await for (final record in sub.records) {
      got.add(record['value'] as String);
      if (got.length == 4) break;
    }
    expect(got, ['r1', 'r2', 'r3', 'r4']);
    expect(sub.offset, 5, reason: 'the next offset is the last + 1');
    expect(fetched, [1, 3, 3, 3]);
    expect(sleeps, [100, 200], reason: 'an empty fetch backs off, doubling');

    final capped = <int>[];
    late Subscription<Map<String, Object>> empty;
    empty = Subscription<Map<String, Object>>((_) async => [], (r) => 0, sleep: (d) async {
      capped.add(d.inMilliseconds);
      if (capped.length == 6) empty.cancel();
    });
    expect(await empty.records.toList(), isEmpty);
    expect(capped, [100, 200, 400, 800, 1000, 1000], reason: 'the backoff stops at 1 s');
  });

  test('dart_builders_match_corpus', () {
    expect(
        Schema().text('title').keyword('tags').i64('n').vector('embedding', 4).toJson(),
        {
          'fields': [
            {
              'name': 'title',
              'source_path': 'title',
              'kind': {
                'text': {'analyzer': 'standard', 'positions': true}
              },
              'indexed': true,
              'fast': false
            },
            {'name': 'tags', 'source_path': 'tags', 'kind': 'keyword', 'indexed': true, 'fast': true},
            {'name': 'n', 'source_path': 'n', 'kind': 'i64', 'indexed': true, 'fast': true},
          ],
          'vectors': [
            {'name': 'embedding', 'dim': 4, 'distance': 'cosine'}
          ],
          'sparse_vectors': [],
          'dynamic': 'ignore',
          'max_fields': 1000,
        },
        reason: 'schema_notes');
    expect(
        Filter.ids([
          1,
          'k-str',
          {'uuid': '0190f5c4-6c1e-7b3a-9d2e-4f5a6b7c8d9e'}
        ]).toJson(),
        {
          'ids': [
            {'uint': '1'},
            'k-str',
            {'uuid': '0190f5c4-6c1e-7b3a-9d2e-4f5a6b7c8d9e'}
          ]
        });
    expect(canon(Filter.eq('tags', 'policy') & (Filter.range('n', gte: 2) | ~Filter.exists('archived'))), {
      'bool': {
        'must': [
          {
            'term': {'field': 'tags', 'value': 'policy'}
          },
          {
            'bool': {
              'should': [
                {
                  'range': {'field': 'n', 'gte': 2}
                },
                {
                  'bool': {
                    'mustNot': [
                      {
                        'exists': {'field': 'archived'}
                      }
                    ]
                  }
                }
              ]
            }
          }
        ]
      }
    });
    expect(jsonEncode(Fusion.rrf()), '{"rrf":{}}');
    expect(jsonEncode(Filter.all()), '{"matchAll":{}}');
    expect(Search.on('n').consistency('v1:s1/p0@2').toJson()['consistency'], {'atLeast': 'v1:s1/p0@2'});
    expect(() => Search.on('n').consistency('not-a-token'), throwsA(isA<InvalidArgumentException>()));
    expect(() => Search.on('n').text('x').vector('v', [1]).fuse(Fusion.weighted([0.7])).build(),
        throwsA(isA<InvalidArgumentException>()));
    final wire = Search.wire(Search.on('n').text('x').vector('v', [1], k: 3).limit(25));
    expect([for (final r in wire['retrievers'] as List) ((r as Map).values.first as Map)['k']], [null, 3],
        reason: 'k goes out only when given');
    final s1 = Search.on('n').limit(1);
    s1.limit(9);
    expect(s1.toJson()['limit'], 1, reason: 'a builder is immutable: each verb answers a new one');

    final cases = BuilderCorpus.cases();
    if (cases.isEmpty) {
      markTestSkipped('no builder corpus in ${BuilderCorpus.dir}; the inline cases ran');
      return;
    }
    for (final c in cases) {
      expect(BuilderCorpus.normalize(canon(BuilderCorpus.build(c))), BuilderCorpus.normalize(c['expect']),
          reason: c['name'] as String);
    }
  });

  test('dart_collection_handle_delegates', () async {
    final s = Script();
    final client = clientOf(s, namespace: 'app');
    final notes = client.collection('notes', namespace: 'other');
    s.ok();
    s.ok();
    s.ok({'count': '3'});
    s.ok();
    s.ok();
    s.ok();
    s.ok({'dropped': true});
    await notes.upsert([
      const Doc('a', {'x': 1})
    ]);
    await notes.get(['a']);
    expect((await notes.count())['count'], '3');
    await notes.search(Search.on('ignored').text('x', fields: ['body']));
    await notes.vectorSearch([1, 0], field: 'embedding');
    await notes.info();
    expect((await notes.drop())['dropped'], isTrue);
    expect([for (var i = 0; i < 7; i++) s.method(i)],
        ['WriteDocuments', 'GetDocuments', 'CountDocuments', 'Search', 'Search', 'GetCollection', 'DropCollection']);
    for (var i = 0; i < 7; i++) {
      expect(s.sent(i)['namespace'], 'other');
      expect(s.sent(i)['collection'], 'notes');
    }
  });

  test('dart_search_query_all_requires_tiebreak', () async {
    final s = Script();
    final client = clientOf(s, namespace: 'app');
    await expectLater(client.search.queryAll(Search.on('notes').text('x').sort(['score'])).toList(),
        throwsA(isA<InvalidArgumentException>()));
    expect(s.requests, isEmpty);
    Map<String, dynamic> hit(int pk, List<num> sv) => {
          'pk': {'uint': '$pk'},
          'sortValues': sv
        };
    s.ok({
      'hits': [
        hit(1, [0.9, 1]),
        hit(2, [0.8, 2])
      ]
    });
    s.ok({
      'hits': [
        hit(3, [0.7, 3])
      ]
    });
    final hits = await client.search.queryAll(Search.on('notes').text('x').sort(['score', 'pk']).limit(2)).toList();
    expect([for (final h in hits) Values.fromDocumentId(h['pk'] as Map<String, dynamic>)], [1, 2, 3]);
    expect(s.sent(1)['searchAfter'], [0.8, 2], reason: "the last hit's sortValues, as answered");
  });

  test('dart_sql_and_streams_send_the_recorded_bodies', () async {
    final s = Script();
    final client = clientOf(s, namespace: 'app');
    for (var i = 0; i < 7; i++) {
      s.ok({
        'columns': [
          {'name': 'tags'},
          {'name': 'c'}
        ],
        'rows': [
          ['policy', 2]
        ]
      });
    }
    final answer = await client.sql.query(r'SELECT tags, count(*) AS c FROM notes WHERE n > $1 GROUP BY tags ORDER BY tags', params: [1]);
    expect(s.sent(0), {'namespace': 'app', 'sql': r'SELECT tags, count(*) AS c FROM notes WHERE n > $1 GROUP BY tags ORDER BY tags', 'params': [1]});
    expect(SqlModule.records(answer), [
      {'tags': 'policy', 'c': 2}
    ]);
    await client.sql.query('SELECT title, n FROM notes ORDER BY n', maxRows: 2);
    expect(s.sent(1), {'namespace': 'app', 'sql': 'SELECT title, n FROM notes ORDER BY n', 'maxRows': 2});
    await client.streams.create('note-events');
    expect(s.sent(2), {'namespace': 'app', 'name': 'note-events', 'partitions': 1});
    await client.streams.produce('note-events', const [
      ProduceRecord(key: '1', value: {'note': 1, 'op': 'created'}),
      ProduceRecord(key: '5', value: {'note': 5, 'op': 'deleted'}),
    ], producerId: 'app-producer-1', producerSeq: 1);
    expect(s.sent(3), {
      'namespace': 'app',
      'stream': 'note-events',
      'records': [
        {'key': 'MQ==', 'value': 'eyJub3RlIjoxLCJvcCI6ImNyZWF0ZWQifQ=='},
        {'key': 'NQ==', 'value': 'eyJub3RlIjo1LCJvcCI6ImRlbGV0ZWQifQ=='},
      ],
      'producerId': 'app-producer-1',
      'producerSeq': '1',
    });
    await client.streams.fetch('note-events', maxRecords: 10);
    expect(s.sent(4), {'namespace': 'app', 'stream': 'note-events', 'maxRecords': 10}, reason: 'a zero partition and offset are omitted');
    await client.streams.fetch('note-events', offset: 2, maxRecords: 10);
    expect(s.sent(5), {'namespace': 'app', 'stream': 'note-events', 'offset': '2', 'maxRecords': 10});
    await client.streams.get('note-events');
    expect(s.sent(6), {'namespace': 'app', 'stream': 'note-events'});
  });

  test('dart_streams_module_subscribe', () async {
    final s = Script();
    final client = clientOf(s, namespace: 'app');
    s.ok({
      'records': [
        {'offset': '1', 'value': base64Encode(utf8.encode('a'))},
        {'offset': '2', 'value': base64Encode(utf8.encode('b'))}
      ]
    });
    s.ok({
      'records': [
        {'offset': '3', 'value': base64Encode(utf8.encode('c'))}
      ]
    });
    final sub = client.streams.subscribe('events', fromOffset: 1);
    final got = <String>[];
    await for (final record in sub.records) {
      got.add(record.text!);
      if (got.length == 3) break;
    }
    expect(got, ['a', 'b', 'c']);
    expect(sub.offset, 4);
    expect(s.sent(0)['offset'], '1');
    expect(s.sent(1)['offset'], '3', reason: 'the next fetch starts after the last record');
  });

  test('dart_facade_matches_index', () {
    final client = LoamsClient('http://127.0.0.1:1');
    final modules = <String, Object>{
      'namespaces': client.namespaces,
      'collections': client.collections,
      'documents': client.documents,
      'search': client.search,
      'vector': client.vector,
      'graph': client.graph,
      'sql': client.sql,
      'streams': client.streams,
    };
    for (final name in appCalls.keys) {
      expect(modules.containsKey(name.split('.').first), isTrue, reason: name);
      expect(appCalls[name]!.rpc, matches(RegExp(r'^loams\.(collection|graph|sql|stream)\.v1\.\w+Service/\w+$')));
    }
    final index = File('${BuilderCorpus.dir}/facade-index.json');
    if (!index.existsSync()) {
      markTestSkipped('no facade index at ${index.path}');
      return;
    }
    final calls = (jsonDecode(index.readAsStringSync()) as Map<String, dynamic>)['calls'] as List;
    for (final c in calls) {
      if (!modules.containsKey(c['module']) || c['stream'] == true) continue;
      final snake = (c['call'] as String).replaceAllMapped(RegExp(r'(?<!^)[A-Z]'), (m) => '_${m[0]!.toLowerCase()}');
      final key = '${c['module']}.$snake';
      expect(appCalls.containsKey(key), isTrue, reason: '$key is in the facade');
      expect(appCalls[key]!.rpc, c['rpc'], reason: key);
      if (c['retry'] != null) expect(appCalls[key]!.retry, c['retry'], reason: '$key retry class');
    }
  });
}
