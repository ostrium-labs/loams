import 'dart:convert';
import 'dart:typed_data';

import '../builders/json.dart';
import '../builders/search.dart';
import '../subscribe.dart';
import 'app.dart';

/// `sql`: one read-only statement over a namespace's collections, with
/// `$1…$n` parameters. SQL reads see the session's collection writes.
class SqlModule extends AppModule {
  SqlModule(super.ctx);

  /// The answer's `rows` are lists in `columns` order; see [records].
  Future<Map<String, dynamic>> query(String sql,
          {List<Object?> params = const [],
          int? maxRows,
          Object? consistency,
          String? namespace,
          CallOptions options = const CallOptions()}) async =>
      invoke('sql.query', _request(sql, params, maxRows, consistency, namespace), options: options);

  /// The same statement, answered as Arrow IPC bytes.
  Future<Uint8List> queryArrow(String sql,
      {List<Object?> params = const [],
      int? maxRows,
      Object? consistency,
      String? namespace,
      CallOptions options = const CallOptions()}) async {
    final answer =
        await invoke('sql.query_arrow', _request(sql, params, maxRows, consistency, namespace), options: options);
    return base64Decode(answer['arrowIpc'] as String? ?? '');
  }

  /// A `QueryResponse`'s rows as maps, column name to value.
  static List<Map<String, Object?>> records(Map<String, dynamic> response) {
    final names = [for (final c in (response['columns'] as List? ?? const [])) (c as Map)['name'] as String];
    return [
      for (final row in (response['rows'] as List? ?? const []))
        {for (var i = 0; i < names.length; i++) names[i]: i < (row as List).length ? row[i] : null}
    ];
  }

  static Map<String, dynamic> _request(
          String sql, List<Object?> params, int? maxRows, Object? consistency, String? namespace) =>
      {
        ...AppModule.ns(namespace),
        ...compact({
          'sql': sql,
          'params': params.isEmpty ? null : params,
          'maxRows': maxRows,
          'consistency': consistencyJson(consistency),
        })
      };
}

/// One fetched record, decoded.
class StreamRecord {
  final int offset;
  final Uint8List? key;
  final Uint8List? value;
  final Map<String, Uint8List?> headers;
  final int timestampMs;

  const StreamRecord(this.offset, this.key, this.value, this.headers, this.timestampMs);

  /// The value as UTF-8 text.
  String? get text => value == null ? null : utf8.decode(value!);

  /// The value as JSON.
  Object? get json => value == null ? null : jsonDecode(utf8.decode(value!));
}

/// `streams`: a namespace's logs. Records are bytes on the wire; the module
/// takes bytes, strings or JSON values (sent as UTF-8 JSON) and answers bytes.
class StreamsModule extends AppModule {
  StreamsModule(super.ctx);

  Future<Map<String, dynamic>> create(String name,
          {int partitions = 1, int? maxAgeMs, int? maxBytes, String? namespace, CallOptions options = const CallOptions()}) async {
    final retention = compact({'maxAgeMs': maxAgeMs?.toString(), 'maxBytes': maxBytes?.toString()});
    return invoke(
        'streams.create',
        {
          ...AppModule.ns(namespace),
          ...compact({'name': name, 'partitions': partitions == 0 ? null : partitions, 'retention': retention.isEmpty ? null : retention})
        },
        options: options);
  }

  Future<Map<String, dynamic>> get(String stream, {String? namespace, CallOptions options = const CallOptions()}) async =>
      invoke('streams.get', {...AppModule.ns(namespace), 'stream': stream}, options: options);

  Future<Map<String, dynamic>> list(
          {int? pageSize, String? pageToken, String? namespace, CallOptions options = const CallOptions()}) async =>
      invoke('streams.list', {...AppModule.ns(namespace), ...compact({'pageSize': pageSize, 'pageToken': pageToken})},
          options: options);

  Stream<Map<String, dynamic>> listAll({int? pageSize, String? namespace, CallOptions options = const CallOptions()}) =>
      paged('streams.list', {...AppModule.ns(namespace), ...compact({'pageSize': pageSize})}, 'streams', options);

  /// Appends [records] to one partition. A record is a value (bytes, a string,
  /// or a JSON value) or a [ProduceRecord]. With [producerId] and [producerSeq]
  /// a repeat is deduplicated (`duplicate`).
  Future<Map<String, dynamic>> produce(String stream, List<Object?> records,
          {int partition = 0,
          String? producerId,
          int? producerSeq,
          String? namespace,
          CallOptions options = const CallOptions()}) async =>
      invoke(
          'streams.produce',
          {
            ...AppModule.ns(namespace),
            ...compact({
              'stream': stream,
              'partition': partition == 0 ? null : partition,
              'records': [for (final r in records) _record(r)],
              'producerId': producerId,
              'producerSeq': producerSeq?.toString(),
            })
          },
          options: options);

  /// Records at and after [offset]; [records] decodes the answer.
  Future<Map<String, dynamic>> fetch(String stream,
          {int partition = 0,
          int offset = 0,
          int? maxRecords,
          int? maxBytes,
          int? maxWaitMs,
          String? namespace,
          CallOptions options = const CallOptions()}) async =>
      invoke(
          'streams.fetch',
          {
            ...AppModule.ns(namespace),
            ...compact({
              'stream': stream,
              'partition': partition == 0 ? null : partition,
              'offset': offset == 0 ? null : '$offset',
              'maxBytes': maxBytes,
              'maxRecords': maxRecords,
              'maxWaitMs': maxWaitMs,
            })
          },
          options: options);

  /// CloudEvents (maps) appended as one JSON batch.
  Future<Map<String, dynamic>> produceEvents(String stream, List<Map<String, Object?>> events,
          {int? partition, String? namespace, CallOptions options = const CallOptions()}) async =>
      invoke(
          'streams.produce_events',
          {
            ...AppModule.ns(namespace),
            ...compact({
              'stream': stream,
              'partition': partition,
              'jsonBatch': base64Encode(utf8.encode(jsonEncode(events))),
            })
          },
          options: options);

  /// CloudEvents at and after [offset], decoded from the answer's JSON batch.
  Future<List<Map<String, dynamic>>> fetchEvents(String stream,
      {int partition = 0,
      int offset = 0,
      int? maxBytes,
      int? maxWaitMs,
      String? namespace,
      CallOptions options = const CallOptions()}) async {
    final answer = await invoke(
        'streams.fetch_events',
        {
          ...AppModule.ns(namespace),
          ...compact({
            'stream': stream,
            'partition': partition == 0 ? null : partition,
            'offset': offset == 0 ? null : '$offset',
            'maxBytes': maxBytes,
            'maxWaitMs': maxWaitMs,
          })
        },
        options: options);
    final batch = answer['jsonBatch'] as String? ?? '';
    if (batch.isEmpty) return const [];
    return [for (final e in jsonDecode(utf8.decode(base64Decode(batch))) as List) Map<String, dynamic>.from(e as Map)];
  }

  /// R14: every record from [fromOffset] on, as a loop over [fetch]: listen to
  /// its `records`, `cancel()` it, resume from `offset`.
  Subscription<StreamRecord> subscribe(String stream,
          {int partition = 0,
          int fromOffset = 0,
          int? maxRecords,
          int? maxWaitMs,
          String? namespace,
          CallOptions options = const CallOptions()}) =>
      Subscription<StreamRecord>(
        (offset) async => records(await fetch(stream,
            partition: partition,
            offset: offset,
            maxRecords: maxRecords,
            maxWaitMs: maxWaitMs,
            namespace: namespace,
            options: options)),
        (r) => r.offset,
        fromOffset: fromOffset,
      );

  /// A `FetchResponse`'s records, decoded.
  static List<StreamRecord> records(Map<String, dynamic> response) => [
        for (final r in (response['records'] as List? ?? const []))
          StreamRecord(
            int.parse(((r as Map)['offset'] ?? '0').toString()),
            r['key'] == null ? null : base64Decode(r['key'] as String),
            r['value'] == null ? null : base64Decode(r['value'] as String),
            {
              for (final h in (r['headers'] as List? ?? const []))
                (h as Map)['key'] as String: h['value'] == null ? null : base64Decode(h['value'] as String)
            },
            int.parse((r['timestampMs'] ?? '0').toString()),
          )
      ];

  static Map<String, dynamic> _record(Object? record) {
    if (record is ProduceRecord) return record.toJson();
    return {'value': base64Encode(_bytes(record))};
  }
}

/// One record to produce: a key, a value and headers, each bytes, a string or a
/// JSON value.
class ProduceRecord {
  final Object? key;
  final Object? value;
  final Map<String, Object?> headers;

  const ProduceRecord({this.key, this.value, this.headers = const {}});

  Map<String, dynamic> toJson() => compact({
        'key': key == null ? null : base64Encode(_bytes(key)),
        'value': value == null ? null : base64Encode(_bytes(value)),
        'headers': headers.isEmpty
            ? null
            : [
                for (final e in headers.entries)
                  compact({'key': e.key, 'value': e.value == null ? null : base64Encode(_bytes(e.value))})
              ],
      });
}

List<int> _bytes(Object? value) {
  if (value is List<int>) return value;
  if (value is String) return utf8.encode(value);
  return utf8.encode(jsonEncode(value));
}
