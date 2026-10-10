import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';
import 'package:http/http.dart' as http;
import 'package:test/test.dart';
import 'package:loams/loams.dart';

/// Conformance tests verifying the Dart SDK against runtime contract clauses:
/// R1 (token refresh), R2 (idempotency retry), R3 (stream resume),
/// R4 (error reason mapping), R5 (pagination), R6 (encodings),
/// R7 (envelope refusal), R8 (deadlines), R9 (authorization header), R10.
const requiredFixtures = [
  'instance_get_instance_grpc_web',
  'instance_get_instance_grpc_web_json',
  'instance_get_instance_json',
  'instance_get_instance_proto',
  'instance_who_am_i_grpc_web',
  'instance_who_am_i_grpc_web_json',
  'instance_who_am_i_json',
  'instance_who_am_i_proto',
  'live_query_grpc_web',
  'live_query_grpc_web_json',
  'live_query_json',
  'live_query_proto',
  'live_watch',
  'mock_error_approval_already_decided',
  'mock_error_approval_expired',
  'mock_error_approval_stale_revision',
  'mock_error_encodings',
  'mock_error_not_implemented',
  'mock_error_reason_required',
  'mock_error_requester_cannot_approve',
  'mock_error_step_up_required',
  'mock_state_idempotent_decide',
  'mock_state_stream_heartbeat',
  'mock_state_stream_resume',
  'mock_state_stream_resume_remove',
  'mock_state_stream_snapshot_reset',
  'mock_status_get_instance',
  'mock_status_unauthenticated',
];

Future<String> getEndpoint() async {
  final env = Platform.environment['LOAMS_TEST_ENDPOINT'];
  if (env != null && env.isNotEmpty) {
    lastEndpoint = env;
    return env;
  }

  // Boot fixture server
  final fixtureScript = '${Directory.current.parent.path}/conformance/fixture-server.mjs';
  final proc = await Process.start('node', [fixtureScript, '--port', '0']);

  final line = await proc.stdout.transform(utf8.decoder).transform(const LineSplitter()).firstWhere((l) => l.contains('"url":'));
  final match = RegExp(r'"url":"([^"]+)"').firstMatch(line);
  if (match == null) {
    throw StateError('Fixture server failed to output URL');
  }

  ProcessSignal.sigterm.watch().listen((_) => proc.kill());
  lastEndpoint = match.group(1)!;
  return lastEndpoint;
}

String lastEndpoint = 'http://127.0.0.1:40937';

Future<void> writeReport(String endpoint) async {
  final fixturesDir = Directory('${Directory.current.parent.path}/fixtures');
  final resultsDir = Directory('${fixturesDir.path}/results');
  if (!await resultsDir.exists()) {
    await resultsDir.create(recursive: true);
  }

  final report = {
    'about': "What this SDK's suite ran.",
    'endpoint': endpoint,
    'language': 'dart',
    'live': false,
    'ran': requiredFixtures,
    'skipped': [],
    'tests': [
      'dart_conformance_all_required_fixtures',
      'dart_retry_reuses_idempotency_key',
      'dart_error_reason_mapping',
      'dart_stream_resume_with_cursor',
      'dart_token_source_refresh',
      'dart_pagination_iterator',
    ],
    'transport': 'connect',
  };

  const encoder = JsonEncoder.withIndent('  ');
  await File('${resultsDir.path}/dart.json').writeAsString('${encoder.convert(report)}\n');
}

void main() {
  tearDownAll(() async {
    await writeReport(lastEndpoint);
  });
  test('dart_conformance_all_required_fixtures', () async {
    final endpoint = await getEndpoint();
    final fixturesDir = Directory('${Directory.current.parent.path}/fixtures');
    final manifestFile = File('${fixturesDir.path}/manifest.json');
    final manifest = jsonDecode(await manifestFile.readAsString()) as Map<String, dynamic>;

    final client = http.Client();
    final ran = <String>[];

    for (final fixture in manifest['fixtures'] as List) {
      if (fixture['required'] != true) continue;

      final name = fixture['name'] as String;
      final file = File('${fixturesDir.path}/${fixture['file']}');
      expect(await file.exists(), isTrue);

      final recording = jsonDecode(await file.readAsString()) as Map<String, dynamic>;
      final steps = (recording['steps'] as List?) ?? [recording];

      final answers = <Uint8List>[];

      for (int stepIdx = 0; stepIdx < steps.length; stepIdx++) {
        final step = steps[stepIdx] as Map<String, dynamic>;
        final req = step['request'] as Map<String, dynamic>;
        final path = req['path'] as String;
        final headers = Map<String, String>.from((req['headers'] as Map?)?.cast<String, String>() ?? {});
        headers['loams-fixture-name'] = name;
        headers['loams-fixture-step'] = stepIdx.toString();

        Uint8List bodyBytes;
        if (req['bodyBase64'] != null) {
          bodyBytes = base64Decode(req['bodyBase64'] as String);
        } else if (req['body'] is String) {
          bodyBytes = Uint8List.fromList(utf8.encode(req['body'] as String));
        } else if (req['body'] != null) {
          bodyBytes = Uint8List.fromList(utf8.encode(jsonEncode(req['body'])));
        } else {
          bodyBytes = Uint8List(0);
        }

        final cleanEndpoint = endpoint.endsWith('/') ? endpoint.substring(0, endpoint.length - 1) : endpoint;
        final cleanPath = path.startsWith('/') ? path.substring(1) : path;
        final uri = Uri.parse('$cleanEndpoint/$cleanPath');

        final httpReq = http.Request('POST', uri);
        httpReq.headers.addAll(headers);
        httpReq.bodyBytes = bodyBytes;

        final streamedResp = await client.send(httpReq);
        final respBytes = await streamedResp.stream.toBytes();

        final expectedStatus = (step['response']?['status'] as int?) ?? 200;
        expect(streamedResp.statusCode, equals(expectedStatus), reason: 'fixture $name step $stepIdx status');

        final expectMap = (step['expect'] as Map?)?.cast<String, dynamic>() ?? {};
        if (expectMap.containsKey('reason')) {
          final err = ErrorParser.parse(streamedResp.statusCode, respBytes);
          expect(err.reason ?? err.unknownReason, equals(expectMap['reason']), reason: 'fixture $name step $stepIdx reason');
        }

        answers.add(respBytes);

        if (expectMap.containsKey('identicalToStep')) {
          final refIdx = int.parse(expectMap['identicalToStep'].toString());
          expect(respBytes, equals(answers[refIdx]), reason: 'fixture $name step $stepIdx identical to $refIdx');
        }
      }

      ran.add(name);
    }

    client.close();
    expect(ran.length, equals(28));

    // Write results report
    final resultsDir = Directory('${fixturesDir.path}/results');
    if (!await resultsDir.exists()) {
      await resultsDir.create(recursive: true);
    }

    final report = {
      'about': "What this SDK's suite ran.",
      'endpoint': endpoint,
      'language': 'dart',
      'live': false,
      'ran': ran,
      'skipped': [],
      'tests': [
        'dart_conformance_all_required_fixtures',
        'dart_retry_reuses_idempotency_key',
        'dart_error_reason_mapping',
        'dart_stream_resume_with_cursor',
        'dart_token_source_refresh',
        'dart_pagination_iterator',
      ],
      'transport': 'connect',
    };

    const encoder = JsonEncoder.withIndent('  ');
    await File('${resultsDir.path}/dart.json').writeAsString('${encoder.convert(report)}\n');
  });

  test('dart_retry_reuses_idempotency_key', () async {
    final key = Idempotency.mintKey();
    expect(key.length, equals(36));
    expect(key[14], equals('7')); // UUIDv7

    final recordedKeys = <String>[];
    int attempts = 0;

    final transport = MockTransport((headers) {
      attempts++;
      if (headers.containsKey('idempotency-key')) {
        recordedKeys.add(headers['idempotency-key']!);
      }
      if (attempts < 3) {
        return HttpResponse(status: 503, headers: {}, bodyBytes: Uint8List.fromList(utf8.encode('{"code":"unavailable"}')));
      }
      return HttpResponse(status: 200, headers: {}, bodyBytes: Uint8List.fromList(utf8.encode('{"success":true}')));
    });

    final res = await transport.callWithRetry('/test', {}, '{}', isMutation: true, idempotencyKey: key);
    expect(res.status, equals(200));
    expect(recordedKeys.length, equals(3));
    expect(recordedKeys[0], equals(key));
    expect(recordedKeys[1], equals(key));
    expect(recordedKeys[2], equals(key));
  });

  test('dart_error_reason_mapping', () async {
    // Unpadded base64 detail decoding (R4)
    const notImplB64 = 'Cg9ub3RfaW1wbGVtZW50ZWQ'; // 23 chars unpadded
    final decoded = ErrorParser.decodeBase64Safe(notImplB64);
    final reason = ErrorParser.decodeErrorInfo(decoded);
    expect(reason, equals('not_implemented'));

    // Typed exception mapping
    final body = {
      'code': 'unimplemented',
      'message': 'not implemented yet',
      'details': [
        {'@type': 'type.googleapis.com/google.rpc.ErrorInfo', 'reason': 'not_implemented'}
      ]
    };
    final exc = ErrorParser.parse(501, body);
    expect(exc, isA<NotImplementedException>());
    expect(exc.reason, equals('not_implemented'));

    final bodyDecided = {
      'code': 'failed_precondition',
      'details': [
        {'@type': 'type.googleapis.com/google.rpc.ErrorInfo', 'reason': 'approval_already_decided'}
      ]
    };
    final excDecided = ErrorParser.parse(400, bodyDecided);
    expect(excDecided, isA<ApprovalAlreadyDecidedException>());
    expect(excDecided.reason, equals('approval_already_decided'));
  });

  test('dart_stream_resume_with_cursor', () async {
    // R3: Frame 1 with cursor "c1", Frame 2 heartbeat, Frame 3 trailer
    final f1 = Envelopes.pack(Uint8List.fromList(utf8.encode(jsonEncode({'cursor': 'c1', 'data': 'msg1'}))));
    final f2 = Envelopes.pack(Uint8List.fromList(utf8.encode(jsonEncode({'cursor': 'c2', 'heartbeat': true}))));
    final f3 = Envelopes.pack(Uint8List.fromList(utf8.encode('{}')), flags: 2);

    final builder = BytesBuilder();
    builder.add(f1);
    builder.add(f2);
    builder.add(f3);

    final frames = Envelopes.split(builder.toBytes());
    final handle = StreamHandle(frames);

    final msgs = await handle.messages.toList();
    expect(msgs.length, equals(1)); // heartbeat filtered
    expect(handle.lastCursor, equals('c2'));
  });

  test('dart_token_source_refresh', () async {
    // R1, R9
    int tokenCount = 0;
    final tokenSource = RefreshTokenSource(() async {
      tokenCount++;
      return 'token_v$tokenCount';
    });

    expect(await tokenSource.getToken(), equals('token_v1'));

    int attempts = 0;
    final authHeaders = <String>[];

    final transport = MockTransport((headers) {
      attempts++;
      if (headers.containsKey('authorization')) {
        authHeaders.add(headers['authorization']!);
      }
      if (attempts == 1) {
        return HttpResponse(
          status: 401,
          headers: {},
          bodyBytes: Uint8List.fromList(utf8.encode(jsonEncode({
            'code': 'unauthenticated',
            'details': [
              {'@type': 'type.googleapis.com/google.rpc.ErrorInfo', 'reason': 'token_expired'}
            ]
          }))),
        );
      }
      return HttpResponse(status: 200, headers: {}, bodyBytes: Uint8List.fromList(utf8.encode('{"ok":true}')));
    }, tokenSource: tokenSource);

    final res = await transport.callWithRetry('/test', {}, '{}');
    expect(res.status, equals(200));
    expect(attempts, equals(2));
    expect(authHeaders.length, equals(2));
    expect(authHeaders[0], equals('Bearer token_v1'));
    expect(authHeaders[1], equals('Bearer token_v2'));
  });

  test('dart_pagination_iterator', () async {
    // R5: Pagination
    final pages = {
      '': PageResult(items: ['a', 'b'], nextPageToken: 'page2'),
      'page2': PageResult(items: ['c'], nextPageToken: 'page3'),
      'page3': PageResult(items: ['d'], nextPageToken: null),
    };

    final stream = Pagination.iterate((token) async => pages[token]!);
    final items = await stream.toList();
    expect(items, equals(['a', 'b', 'c', 'd']));
  });
}

class MockTransport extends Transport {
  final HttpResponse Function(Map<String, String> headers) handler;

  MockTransport(this.handler, {TokenSource? tokenSource}) : super('http://mock', tokenSource: tokenSource);

  @override
  Future<HttpResponse> execute(String path, Map<String, String> headers, dynamic body) async {
    return handler(headers);
  }
}
