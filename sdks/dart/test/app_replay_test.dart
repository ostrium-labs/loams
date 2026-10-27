import 'dart:convert';
import 'dart:io';

import 'package:loams/loams.dart';
import 'package:test/test.dart';

import 'builder_corpus.dart';

/// The recorded app scenario (`sdks/fixtures/recorded/app/*.json`,
/// `app-scenario.md`), replayed through the public facade against the fixture
/// server (SDKP P0d). Every step goes through `LoamsClient.call` with its
/// recorded body; a step that names a builder case is first checked against that
/// builder; the answer is compared with the recording, its `volatile` fields
/// masked, and a refusal by its code. The fixtures replayed are added to
/// `results/dart.json`, which `check-languages.mjs --check dart --suite app`
/// reads.
void main() {
  Process? server;
  late String endpoint;

  setUpAll(() async {
    final dir = BuilderCorpus.dir;
    server = await Process.start('node', ['${Directory(dir).absolute.parent.path}/conformance/fixture-server.mjs', '--port', '0', '--fixtures', dir]);
    final line = await server!.stdout.transform(utf8.decoder).transform(const LineSplitter()).firstWhere((l) => l.contains('"url":'));
    endpoint = RegExp(r'"url":"([^"]+)"').firstMatch(line)!.group(1)!;
  });

  tearDownAll(() => server?.kill());

  Future<void> run(String name) async {
    final file = File('${BuilderCorpus.dir}/recorded/app/$name.json');
    if (!file.existsSync()) {
      markTestSkipped('no recording for $name');
      return;
    }
    final failures = await replay(endpoint, jsonDecode(file.readAsStringSync()) as Map<String, dynamic>);
    expect(failures, isEmpty, reason: failures.join('\n'));
    record(name);
  }

  // Spelled out: `check-languages.mjs` looks for each name in this file.
  test('dart_app_collections', () => run('app_collections'));
  test('dart_app_search', () => run('app_search'));
  test('dart_app_graph', () => run('app_graph'));
  test('dart_app_sql', () => run('app_sql'));
  test('dart_app_streams', () => run('app_streams'));
}

Future<List<String>> replay(String endpoint, Map<String, dynamic> fixture) async {
  final name = fixture['name'] as String;
  final volatile = List<String>.from(fixture['volatile'] as List? ?? const []);
  final client = LoamsClient(endpoint, maxRetries: 1);
  final failures = <String>[];
  try {
    final steps = fixture['steps'] as List;
    for (var index = 0; index < steps.length; index++) {
      final step = steps[index] as Map<String, dynamic>;
      final label = '$name step $index (${step['call']})';
      final body = (step['request'] as Map)['body'] as String? ?? '';
      final request = body.isEmpty ? <String, dynamic>{} : jsonDecode(body) as Map<String, dynamic>;
      if (step['builder'] != null) {
        final problem = checkBuilder(step['builder'] as String, request);
        if (problem != null) failures.add('$label: $problem');
      }
      final expected = step['response'] as Map<String, dynamic>;
      final answered = expected['status'] == 200;
      try {
        final answer = await client.call(step['call'] as String, request,
            options: CallOptions(headers: {'loams-fixture-name': name, 'loams-fixture-step': '$index'}));
        if (!answered) {
          failures.add('$label: answered, but the recording is a ${(expected['body'] as Map)['code']}');
          continue;
        }
        final diff = compare(mask(expected['body'], volatile), mask(answer, volatile), r'$');
        if (diff != null) failures.add('$label: $diff');
      } on LoamsException catch (e) {
        final want = (expected['body'] as Map?)?['code'];
        if (answered) {
          failures.add('$label: refused (${e.code}) where the recording answered: ${e.message}');
        } else if (e.code != want) {
          failures.add('$label: refused with ${e.code}, the recording with $want');
        }
      }
    }
  } finally {
    client.close();
  }
  return failures;
}

String? checkBuilder(String caseName, Map<String, dynamic> request) {
  final file = File('${BuilderCorpus.dir}/builders/$caseName.json');
  if (!file.existsSync()) return 'no builder case $caseName';
  final c = jsonDecode(file.readAsStringSync()) as Map<String, dynamic>;
  final built = jsonDecode(jsonEncode(BuilderCorpus.build(c)));
  final Object? part = switch (c['builder']) {
    'schema' => request['schema'],
    'filter' => request['where'] ?? request['filter'],
    _ => Map.of(request)..remove('namespace'),
  };
  return compare(part, built, r'$') == null
      ? null
      : 'the $caseName builder builds ${jsonEncode(built)}, the recording sent ${jsonEncode(part)}';
}

Object? mask(Object? value, List<String> volatile) {
  if (value is Map) {
    return {
      for (final e in value.entries)
        if (!volatile.contains(e.key)) e.key: mask(e.value, volatile)
    };
  }
  if (value is List) return [for (final v in value) mask(v, volatile)];
  return value;
}

/// Null when equal; numbers compare to float32 precision.
String? compare(Object? want, Object? got, String at) {
  if (want is Map) {
    if (got is! Map) return '$at: ${jsonEncode(got)} is not an object';
    for (final k in {...want.keys, ...got.keys}) {
      final p = compare(want[k], got[k], '$at.$k');
      if (p != null) return p;
    }
    return null;
  }
  if (want is List) {
    if (got is! List || got.length != want.length) return '$at: ${jsonEncode(got)} is not ${want.length} items';
    for (var i = 0; i < want.length; i++) {
      final p = compare(want[i], got[i], '$at[$i]');
      if (p != null) return p;
    }
    return null;
  }
  if (want is num) {
    final ok = got is num && (want - got).abs() <= 1e-6 * (want.abs() > 1 ? want.abs() : 1);
    return ok ? null : '$at: $got, recorded $want';
  }
  return want == got ? null : '$at: ${jsonEncode(got)}, recorded ${jsonEncode(want)}';
}

void record(String fixture) {
  final file = File('${BuilderCorpus.dir}/results/dart.json');
  file.parent.createSync(recursive: true);
  final report = file.existsSync()
      ? jsonDecode(file.readAsStringSync()) as Map<String, dynamic>
      : <String, dynamic>{'language': 'dart', 'transport': 'connect', 'ran': <String>[]};
  report['ran'] = ({...List<String>.from(report['ran'] as List? ?? const []), fixture}.toList()..sort());
  file.writeAsStringSync('${const JsonEncoder.withIndent('  ').convert(report)}\n');
}
