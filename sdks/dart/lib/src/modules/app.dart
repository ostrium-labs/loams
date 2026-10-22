import 'dart:convert';

import '../batch.dart';
import '../builders/filter.dart';
import '../builders/json.dart';
import '../builders/schema.dart';
import '../builders/search.dart';
import '../consistency.dart';
import '../errors.dart';
import '../idempotency.dart';
import '../transport.dart';
import '../values.dart';

/// One app call: its RPC, its retry class (D610: `safe`, `keyed` or `never`),
/// whether its request names a namespace, and whether it is a read the
/// session applies to.
class AppCall {
  final String rpc;
  final String retry;
  final bool namespaced;
  final bool read;

  const AppCall(this.rpc, this.retry, {this.namespaced = true, this.read = false});

  /// The session's scope: the proto package, except that SQL reads (and adds
  /// to) the collection token, since it reads collections.
  String get scope {
    final service = rpc.substring(0, rpc.indexOf('/'));
    final package = service.substring(0, service.lastIndexOf('.'));
    return package == 'loams.sql.v1' ? 'loams.collection.v1' : package;
  }
}

/// The RPCs behind every app call (SDKP §2), `module.call` to its [AppCall].
/// Checked against `sdks/fixtures/facade-index.json` by
/// `dart_facade_matches_index`.
const appCalls = <String, AppCall>{
  'namespaces.create': AppCall('loams.collection.v1.NamespaceService/CreateNamespace', 'safe', namespaced: false),
  'namespaces.list': AppCall('loams.collection.v1.NamespaceService/ListNamespaces', 'safe', namespaced: false),
  'collections.create': AppCall('loams.collection.v1.CollectionService/CreateCollection', 'safe'),
  'collections.list': AppCall('loams.collection.v1.CollectionService/ListCollections', 'safe'),
  'collections.get': AppCall('loams.collection.v1.CollectionService/GetCollection', 'safe'),
  'collections.drop': AppCall('loams.collection.v1.CollectionService/DropCollection', 'safe'),
  'collections.add_fields': AppCall('loams.collection.v1.CollectionService/AddFields', 'safe'),
  'collections.list_versions': AppCall('loams.collection.v1.CollectionService/ListVersions', 'safe'),
  'collections.update_aliases': AppCall('loams.collection.v1.CollectionService/UpdateAliases', 'safe'),
  'collections.scan': AppCall('loams.collection.v1.CollectionService/Scan', 'never'),
  'collections.set_hot': AppCall('loams.collection.v1.CollectionService/SetHot', 'safe'),
  'collections.warm': AppCall('loams.collection.v1.CollectionService/WarmCollection', 'safe'),
  'documents.write': AppCall('loams.collection.v1.DocumentService/WriteDocuments', 'keyed'),
  'documents.get': AppCall('loams.collection.v1.DocumentService/GetDocuments', 'safe', read: true),
  'documents.scroll': AppCall('loams.collection.v1.DocumentService/ScrollDocuments', 'safe', read: true),
  'documents.count': AppCall('loams.collection.v1.DocumentService/CountDocuments', 'safe', read: true),
  'documents.delete_by_filter': AppCall('loams.collection.v1.DocumentService/DeleteByFilter', 'never', read: true),
  'documents.patch_by_filter': AppCall('loams.collection.v1.DocumentService/PatchByFilter', 'never', read: true),
  'search.query': AppCall('loams.collection.v1.QueryService/Search', 'safe', read: true),
  'vector.search': AppCall('loams.collection.v1.QueryService/Search', 'safe', read: true),
  'graph.create': AppCall('loams.graph.v1.GraphAdminService/CreateGraph', 'keyed'),
  'graph.get': AppCall('loams.graph.v1.GraphAdminService/GetGraph', 'safe'),
  'graph.list': AppCall('loams.graph.v1.GraphAdminService/ListGraphs', 'safe'),
  'graph.update': AppCall('loams.graph.v1.GraphAdminService/UpdateGraph', 'keyed', namespaced: false),
  'graph.delete': AppCall('loams.graph.v1.GraphAdminService/DeleteGraph', 'keyed'),
  'graph.schema': AppCall('loams.graph.v1.GraphAdminService/GetSchema', 'safe'),
  'graph.query': AppCall('loams.graph.v1.GraphService/Execute', 'keyed', read: true),
  'graph.query_batch': AppCall('loams.graph.v1.GraphService/ExecuteBatch', 'keyed'),
  'graph.explain': AppCall('loams.graph.v1.GraphService/Explain', 'safe', read: true),
  'sql.query': AppCall('loams.sql.v1.SqlService/Query', 'safe', read: true),
  'sql.query_arrow': AppCall('loams.sql.v1.SqlService/QueryArrow', 'safe', read: true),
  'streams.create': AppCall('loams.stream.v1.StreamService/CreateStream', 'safe'),
  'streams.get': AppCall('loams.stream.v1.StreamService/DescribeStream', 'safe'),
  'streams.list': AppCall('loams.stream.v1.StreamService/ListStreams', 'safe'),
  'streams.produce': AppCall('loams.stream.v1.StreamService/Produce', 'never'),
  'streams.fetch': AppCall('loams.stream.v1.StreamService/Fetch', 'safe'),
  'streams.produce_events': AppCall('loams.stream.v1.StreamService/ProduceCloudEvents', 'never'),
  'streams.fetch_events': AppCall('loams.stream.v1.StreamService/FetchCloudEvents', 'safe'),
};

/// The packages whose reads take the session's token (SDKP-17).
const sessionScopes = {'loams.collection.v1'};

/// What every call takes besides its arguments.
class CallOptions {
  /// This call's budget (R12); null is the client's.
  final Duration? timeout;

  /// The idempotency key of a keyed call (R3); null mints one.
  final String? idempotencyKey;

  /// Extra request headers.
  final Map<String, String> headers;

  const CallOptions({this.timeout, this.idempotencyKey, this.headers = const {}});
}

/// What the app modules need from the client.
abstract class AppContext {
  Transport get transport;
  String? get defaultNamespace;
  SessionStore? get session;
}

/// The call path every app module shares: proto3 JSON in, proto3 JSON out
/// (`lowerCamelCase` keys, 64-bit integers as strings), through the client's
/// transport, so retries, keys, token refresh, timeouts and errors are R1–R12.
abstract class AppModule {
  final AppContext _ctx;

  AppModule(this._ctx);

  /// Sends [request] to the call [name] (a key of [appCalls]).
  ///
  /// R11: a request that names no namespace gets the client's default, and
  /// with neither it is refused here, before any network call. R4: with
  /// `sessionConsistency`, a read that leaves `consistency` unset reads at
  /// least the session's token, and every token an answer carries is merged in.
  Future<Map<String, dynamic>> invoke(String name, Map<String, dynamic> request,
      {CallOptions options = const CallOptions(), String? retry}) async {
    final c = appCalls[name]!;
    var body = Map<String, dynamic>.from(request);
    if (c.namespaced) {
      body = withNamespace(body, c.rpc);
    }
    final session = _ctx.session;
    // SDKP-17: only collection (and SQL) reads take the session's token.
    if (c.read && session != null && sessionScopes.contains(c.scope)) {
      body = session.apply(c.scope, body);
    }
    final klass = retry ?? c.retry;
    String? key;
    if (klass == 'keyed') {
      key = options.idempotencyKey ?? body['idempotencyKey'] as String? ?? Idempotency.mintKey();
      body['idempotencyKey'] = key;
    }
    final res = await _ctx.transport.callWithRetry(
      '/${c.rpc}',
      {'content-type': 'application/json', 'connect-protocol-version': '1', ...options.headers},
      jsonEncode(body),
      isMutation: klass != 'safe',
      idempotencyKey: key,
      timeout: options.timeout,
    );
    final text = res.bodyText;
    final answer = text.isEmpty ? <String, dynamic>{} : jsonDecode(text) as Map<String, dynamic>;
    session?.observeAnswer(c.scope, answer);
    return answer;
  }

  Map<String, dynamic> withNamespace(Map<String, dynamic> request, String rpc) {
    final current = request['namespace'] as String?;
    if (current != null && current.isNotEmpty) {
      return request;
    }
    final fallback = _ctx.defaultNamespace;
    if (fallback == null || fallback.isEmpty) {
      throw InvalidArgumentException('$rpc needs a namespace: pass one or give the client one',
          code: 'invalid_argument', reason: 'namespace_required', httpStatus: 400);
    }
    return {...request, 'namespace': fallback};
  }

  static Map<String, dynamic> ns(String? namespace) => namespace == null ? {} : {'namespace': namespace};

  /// Every item of a paged list, following `nextPageToken`.
  Stream<Map<String, dynamic>> paged(
      String name, Map<String, dynamic> request, String items, CallOptions options) async* {
    var token = '';
    while (true) {
      final page = await invoke(name, {...request, if (token.isNotEmpty) 'pageToken': token}, options: options);
      for (final item in (page[items] as List? ?? const [])) {
        yield Map<String, dynamic>.from(item as Map);
      }
      final next = page['nextPageToken'] as String? ?? '';
      if (next.isEmpty || next == token) return;
      token = next;
    }
  }
}

/// Any app call by its facade-index name over its request's proto3 JSON:
/// what `LoamsClient.call` goes through.
class RawModule extends AppModule {
  RawModule(super.ctx);

  Future<Map<String, dynamic>> call(String name, Map<String, dynamic> request,
      {CallOptions options = const CallOptions()}) async {
    if (!appCalls.containsKey(name)) {
      throw ArgumentError.value(name, 'name', 'no app call');
    }
    return invoke(name, request, options: options);
  }
}

/// `namespaces`: the container every collection and stream lives in.
class NamespacesModule extends AppModule {
  NamespacesModule(super.ctx);

  Future<Map<String, dynamic>> create(String namespace, {CallOptions options = const CallOptions()}) async =>
      invoke('namespaces.create', {'namespace': namespace}, options: options);

  Future<Map<String, dynamic>> list(
          {int? pageSize, String? pageToken, CallOptions options = const CallOptions()}) async =>
      invoke('namespaces.list', compact({'pageSize': pageSize, 'pageToken': pageToken}), options: options);

  /// Every namespace, across pages.
  Stream<Map<String, dynamic>> listAll({int? pageSize, CallOptions options = const CallOptions()}) =>
      paged('namespaces.list', compact({'pageSize': pageSize}), 'namespaces', options);
}

/// `collections`: schema, aliases, versions and the hot tier.
class CollectionsModule extends AppModule {
  CollectionsModule(super.ctx);

  Future<Map<String, dynamic>> create(String name,
          {required Schema schema,
          int? partitions,
          String? namespace,
          CallOptions options = const CallOptions()}) async =>
      invoke(
          'collections.create',
          {
            ...AppModule.ns(namespace),
            ...compact({'name': name, 'schema': schema.toJson(), 'partitions': partitions})
          },
          options: options);

  Future<Map<String, dynamic>> get(String name, {String? namespace, CallOptions options = const CallOptions()}) async =>
      invoke('collections.get', {...AppModule.ns(namespace), 'collection': name}, options: options);

  Future<Map<String, dynamic>> list(
          {int? pageSize, String? pageToken, String? namespace, CallOptions options = const CallOptions()}) async =>
      invoke(
          'collections.list',
          {
            ...AppModule.ns(namespace),
            ...compact({'pageSize': pageSize, 'pageToken': pageToken})
          },
          options: options);

  Stream<Map<String, dynamic>> listAll({int? pageSize, String? namespace, CallOptions options = const CallOptions()}) =>
      paged(
          'collections.list',
          {
            ...AppModule.ns(namespace),
            ...compact({'pageSize': pageSize})
          },
          'collections',
          options);

  Future<Map<String, dynamic>> drop(String name,
          {String? namespace, CallOptions options = const CallOptions()}) async =>
      invoke('collections.drop', {...AppModule.ns(namespace), 'collection': name}, options: options);

  /// Additive schema change: the fields and vectors of [fields].
  Future<Map<String, dynamic>> addFields(String name, Schema fields,
      {String? namespace, CallOptions options = const CallOptions()}) async {
    final schema = fields.toJson();
    return invoke('collections.add_fields',
        {...AppModule.ns(namespace), 'collection': name, 'fields': schema['fields'], 'vectors': schema['vectors']},
        options: options);
  }

  Future<Map<String, dynamic>> listVersions(String name,
          {int? pageSize, String? pageToken, String? namespace, CallOptions options = const CallOptions()}) async =>
      invoke(
          'collections.list_versions',
          {
            ...AppModule.ns(namespace),
            ...compact({'collection': name, 'pageSize': pageSize, 'pageToken': pageToken})
          },
          options: options);

  /// `[{create: {alias:, collection:}}, {delete: {alias:}}]`.
  Future<Map<String, dynamic>> updateAliases(List<Map<String, dynamic>> actions,
          {String? namespace, CallOptions options = const CallOptions()}) async =>
      invoke('collections.update_aliases', {...AppModule.ns(namespace), 'actions': actions}, options: options);

  Future<Map<String, dynamic>> scan(String name,
          {Map<String, dynamic>? at, String? namespace, CallOptions options = const CallOptions()}) async =>
      invoke(
          'collections.scan',
          {
            ...AppModule.ns(namespace),
            ...compact({'collection': name, 'at': at})
          },
          options: options);

  Future<Map<String, dynamic>> setHot(String name,
          {bool vectors = false,
          bool text = false,
          bool fragments = false,
          String? namespace,
          CallOptions options = const CallOptions()}) async =>
      invoke(
          'collections.set_hot',
          {
            ...AppModule.ns(namespace),
            'collection': name,
            'hot': {'vectors': vectors, 'text': text, 'fragments': fragments}
          },
          options: options);

  Future<Map<String, dynamic>> warm(String name,
          {String? namespace, CallOptions options = const CallOptions()}) async =>
      invoke('collections.warm', {...AppModule.ns(namespace), 'collection': name}, options: options);
}

/// One document to write: `id` (an int, a String or `{uuid: …}`), its
/// `source` and its named `vectors`.
class Doc {
  final Object id;
  final Map<String, dynamic> source;
  final Map<String, List<num>>? vectors;

  const Doc(this.id, [this.source = const {}, this.vectors]);

  Map<String, dynamic> toJson() => compact({
        'id': Values.documentId(id),
        'source': source,
        'vectors': vectors?.map((k, v) => MapEntry(k, [for (final x in v) x.toDouble()])),
      });
}

/// `documents`: write, get, scroll, count and the filter writes.
class DocumentsModule extends AppModule {
  DocumentsModule(super.ctx);

  /// Raw write ops (`WriteOp` JSON); the answer's `token` is the write's
  /// consistency token.
  Future<Map<String, dynamic>> write(String collection, List<Map<String, dynamic>> ops,
          {bool reportExistence = false, String? namespace, CallOptions options = const CallOptions()}) async =>
      invoke('documents.write',
          {...AppModule.ns(namespace), 'collection': collection, 'ops': ops, 'reportExistence': reportExistence},
          options: options);

  Future<Map<String, dynamic>> upsert(String collection, Iterable<Doc> docs,
          {String? namespace, CallOptions options = const CallOptions()}) async =>
      write(
          collection,
          [
            for (final d in docs) {'upsert': d.toJson()}
          ],
          namespace: namespace,
          options: options);

  Future<Map<String, dynamic>> delete(String collection, List<Object> ids,
          {String? namespace, CallOptions options = const CallOptions()}) async =>
      write(
          collection,
          [
            for (final id in ids)
              {
                'delete': {'id': Values.documentId(id)}
              }
          ],
          namespace: namespace,
          options: options);

  /// Sets the keys of [set] and removes the keys named in [unset].
  Future<Map<String, dynamic>> patch(String collection, Object id,
          {Map<String, dynamic>? set,
          List<String>? unset,
          String? mode,
          String? namespace,
          CallOptions options = const CallOptions()}) async =>
      write(
          collection,
          [
            {
              'patch': compact({
                'id': Values.documentId(id),
                'mode': mode,
                'source': set,
                'deleteKeys': unset == null || unset.isEmpty ? null : unset,
              })
            }
          ],
          namespace: namespace,
          options: options);

  /// R13: [docs] in chunks, one in flight at a time, each with its own
  /// idempotency key. Answers a [BatchResult]; a failed chunk throws
  /// [BatchException].
  Future<BatchResult> writeBatch(String collection, Iterable<Doc> docs,
          {int chunkRows = Batch.defaultChunkRows,
          int chunkBytes = Batch.defaultChunkBytes,
          String? namespace,
          CallOptions options = const CallOptions()}) async =>
      Batch.run(
        docs.map((d) => {'upsert': d.toJson()}),
        (chunk, key) async => write(collection, chunk,
            namespace: namespace, options: CallOptions(timeout: options.timeout, idempotencyKey: key)),
        chunkRows: chunkRows,
        chunkBytes: chunkBytes,
      );

  /// [consistency] is `strong`, `eventual` or a token; [select] a projection
  /// (`{source: {include: [...]}}`).
  Future<Map<String, dynamic>> get(String collection, List<Object> ids,
          {Map<String, dynamic>? select,
          Object? consistency,
          String? namespace,
          CallOptions options = const CallOptions()}) async =>
      invoke(
          'documents.get',
          {
            ...AppModule.ns(namespace),
            ...compact({
              'collection': collection,
              'ids': [for (final id in ids) Values.documentId(id)],
              'select': select,
              'consistency': consistencyJson(consistency),
            })
          },
          options: options);

  /// One page. [where] is a typed [Filter]; [filter] the server's native
  /// filter JSON.
  Future<Map<String, dynamic>> scroll(String collection,
          {Filter? where,
          Object? filter,
          Object? after,
          int? limit,
          Map<String, dynamic>? select,
          Object? consistency,
          String? namespace,
          CallOptions options = const CallOptions()}) async =>
      invoke(
          'documents.scroll',
          {
            ...AppModule.ns(namespace),
            ...compact({
              'collection': collection,
              'where': where?.toJson(),
              'filter': filter,
              'after': after == null ? null : (after is Map ? after : Values.documentId(after)),
              'limit': limit,
              'select': select,
              'consistency': consistencyJson(consistency),
            })
          },
          options: options);

  /// Every document, page after page until the answer has no `next`.
  Stream<Map<String, dynamic>> scrollAll(String collection,
      {Filter? where,
      Object? filter,
      int? limit,
      Map<String, dynamic>? select,
      Object? consistency,
      String? namespace,
      CallOptions options = const CallOptions()}) async* {
    Object? after;
    while (true) {
      final page = await scroll(collection,
          where: where,
          filter: filter,
          after: after,
          limit: limit,
          select: select,
          consistency: consistency,
          namespace: namespace,
          options: options);
      for (final doc in (page['documents'] as List? ?? const [])) {
        yield Map<String, dynamic>.from(doc as Map);
      }
      final next = page['next'];
      if (next is! Map || next.isEmpty) return;
      after = Map<String, dynamic>.from(next);
    }
  }

  Future<Map<String, dynamic>> count(String collection,
          {Filter? where,
          Object? filter,
          Object? consistency,
          String? namespace,
          CallOptions options = const CallOptions()}) async =>
      invoke(
          'documents.count',
          {
            ...AppModule.ns(namespace),
            ...compact({
              'collection': collection,
              'where': where?.toJson(),
              'filter': filter,
              'consistency': consistencyJson(consistency),
            })
          },
          options: options);

  Future<Map<String, dynamic>> deleteByFilter(String collection,
          {Filter? where,
          Object? filter,
          int? maxRows,
          bool allowPartial = false,
          Map<String, dynamic>? cursor,
          String? namespace,
          CallOptions options = const CallOptions()}) async =>
      invoke(
          'documents.delete_by_filter',
          {
            ...AppModule.ns(namespace),
            ...compact({
              'collection': collection,
              'where': where?.toJson(),
              'filter': filter,
              'maxRows': maxRows?.toString(),
              'allowPartial': allowPartial,
              'cursor': cursor,
            })
          },
          options: options);

  Future<Map<String, dynamic>> patchByFilter(String collection, Map<String, dynamic> patch,
          {Filter? where,
          Object? filter,
          int? maxRows,
          bool allowPartial = false,
          Map<String, dynamic>? cursor,
          String? namespace,
          CallOptions options = const CallOptions()}) async =>
      invoke(
          'documents.patch_by_filter',
          {
            ...AppModule.ns(namespace),
            ...compact({
              'collection': collection,
              'patch': patch,
              'where': where?.toJson(),
              'filter': filter,
              'maxRows': maxRows?.toString(),
              'allowPartial': allowPartial,
              'cursor': cursor,
            })
          },
          options: options);
}

/// `search`: text, vector, sparse or hybrid, with filters, fusion,
/// highlights, `select` and pagination.
class SearchModule extends AppModule {
  SearchModule(super.ctx);

  /// [request] is a [Search] or the `SearchRequest`'s JSON, checked
  /// ([Search.build]) and sent as built.
  Future<Map<String, dynamic>> query(Object request, {CallOptions options = const CallOptions()}) async =>
      invoke('search.query', Search.wire(request), options: options);

  /// A text search: a match on one field, a multi-match on several, or a query string.
  Future<Map<String, dynamic>> text(String collection, String text,
      {List<String>? fields,
      Filter? filter,
      int? limit,
      String? namespace,
      CallOptions options = const CallOptions()}) async {
    var s = Search.on(collection, namespace: namespace).text(text, fields: fields);
    if (filter != null) s = s.filter(filter);
    if (limit != null) s = s.limit(limit);
    return query(s, options: options);
  }

  /// Text plus a vector on [field], fused ([Fusion.rrf] by default).
  Future<Map<String, dynamic>> hybrid(String collection, String text, List<num> vector,
      {required String field,
      List<String>? fields,
      Map<String, dynamic>? fusion,
      int? k,
      Filter? filter,
      int? limit,
      String? namespace,
      CallOptions options = const CallOptions()}) async {
    var s = Search.on(collection, namespace: namespace)
        .text(text, fields: fields, k: k)
        .vector(field, vector, k: k)
        .fuse(fusion ?? Fusion.rrf());
    if (filter != null) s = s.filter(filter);
    if (limit != null) s = s.limit(limit);
    return query(s, options: options);
  }

  /// Every hit, page after page through `searchAfter`, until a page has fewer
  /// than `limit` hits. The sort must end with `Sort.pk()`, or it throws
  /// [InvalidArgumentException] before any network call.
  Stream<Map<String, dynamic>> queryAll(Search request, {CallOptions options = const CallOptions()}) async* {
    if (!request.hasTiebreak) {
      throw InvalidArgumentException(
          "queryAll needs a sort that ends with the primary key, such as .sort(['score', 'pk'])",
          code: 'invalid_argument',
          httpStatus: 400);
    }
    var page = request.toJson();
    page['limit'] ??= Search.defaultK;
    final limit = page['limit'] as int;
    while (true) {
      final answer = await query(page, options: options);
      final hits = (answer['hits'] as List? ?? const []).map((h) => Map<String, dynamic>.from(h as Map)).toList();
      for (final hit in hits) {
        yield hit;
      }
      if (hits.isEmpty || hits.length < limit) return;
      page = {...page, 'searchAfter': List<Object?>.from(hits.last['sortValues'] as List? ?? const [])};
    }
  }

}

/// `vector`: the same RPC as `search.query`, with the vector shape as its required arguments.
class VectorModule extends AppModule {
  VectorModule(super.ctx);

  Future<Map<String, dynamic>> search(String collection, List<num> vector,
      {required String field,
      int? limit,
      int? k,
      Filter? filter,
      String? namespace,
      CallOptions options = const CallOptions()}) async {
    var s = Search.on(collection, namespace: namespace).vector(field, vector, k: k);
    if (filter != null) s = s.filter(filter);
    if (limit != null) s = s.limit(limit);
    return invoke('vector.search', Search.wire(s), options: options);
  }
}

/// `graph`: stored property graphs, queried with GQL. **Experimental**: the
/// package is `unstable`.
class GraphModule extends AppModule {
  GraphModule(super.ctx);

  static const _languages = {
    'gql': 'QUERY_LANGUAGE_GQL',
    'cypher': 'QUERY_LANGUAGE_CYPHER',
    'sql_pgq': 'QUERY_LANGUAGE_SQL_PGQ',
  };

  bool get unstable => true;

  Future<Map<String, dynamic>> create(String name,
          {Map<String, dynamic>? mapping,
          String? mode,
          String? namespace,
          CallOptions options = const CallOptions()}) async =>
      invoke(
          'graph.create',
          {
            ...AppModule.ns(namespace),
            ...compact(
                {'name': name, 'mapping': mapping, 'mode': mode == null ? null : 'GRAPH_MODE_${mode.toUpperCase()}'})
          },
          options: options);

  Future<Map<String, dynamic>> get(String name, {String? namespace, CallOptions options = const CallOptions()}) async =>
      invoke('graph.get', {...AppModule.ns(namespace), 'name': name}, options: options);

  Future<Map<String, dynamic>> list(
          {int? pageSize, String? pageToken, String? namespace, CallOptions options = const CallOptions()}) async =>
      invoke(
          'graph.list',
          {
            ...AppModule.ns(namespace),
            ...compact({'pageSize': pageSize, 'pageToken': pageToken})
          },
          options: options);

  Stream<Map<String, dynamic>> listAll({int? pageSize, String? namespace, CallOptions options = const CallOptions()}) =>
      paged(
          'graph.list',
          {
            ...AppModule.ns(namespace),
            ...compact({'pageSize': pageSize})
          },
          'graphs',
          options);

  /// [graph] is the `Graph` JSON (`name`, and `namespace` or the client's).
  Future<Map<String, dynamic>> update(Map<String, dynamic> graph,
          {required List<String> updateMask, int? expectedVersion, CallOptions options = const CallOptions()}) async =>
      invoke(
          'graph.update',
          compact({
            'graph': withNamespace(graph, appCalls['graph.update']!.rpc),
            'updateMask': updateMask.join(','),
            'expectedVersion': expectedVersion?.toString(),
          }),
          options: options);

  /// Answers an `Operation`.
  Future<Map<String, dynamic>> delete(String name,
          {String? namespace, CallOptions options = const CallOptions()}) async =>
      invoke('graph.delete', {...AppModule.ns(namespace), 'name': name}, options: options);

  Future<Map<String, dynamic>> schema(String name,
          {String? namespace, CallOptions options = const CallOptions()}) async =>
      invoke('graph.schema', {...AppModule.ns(namespace), 'name': name}, options: options);

  /// One statement (GQL by default) with Dart [parameters]. A read-only
  /// statement is retried as a read; a write carries an idempotency key.
  Future<Map<String, dynamic>> query(String graph, String statement,
          {Map<String, Object?> parameters = const {},
          bool readOnly = false,
          String language = 'gql',
          Object? consistency,
          int? maxRows,
          String? namespace,
          CallOptions options = const CallOptions()}) async =>
      invoke(
          'graph.query',
          {
            ...AppModule.ns(namespace),
            ...compact({
              'graph': graph,
              'statement': statement,
              'language': _languages[language] ?? language,
              'parameters': _params(parameters),
              'readOnly': readOnly,
              'consistency': _consistency(consistency),
              'maxRows': maxRows,
            })
          },
          options: options,
          retry: readOnly ? 'safe' : null);

  /// [statements]: `[{statement:, parameters:}]`, run in one batch.
  Future<Map<String, dynamic>> queryBatch(String graph, List<Map<String, Object?>> statements,
          {bool atomic = true,
          String language = 'gql',
          String? namespace,
          CallOptions options = const CallOptions()}) async =>
      invoke(
          'graph.query_batch',
          {
            ...AppModule.ns(namespace),
            'graph': graph,
            'statements': [
              for (final s in statements)
                {
                  'statement': s['statement'],
                  'parameters': _params(Map<String, Object?>.from(s['parameters'] as Map? ?? const {})),
                }
            ],
            'atomic': atomic,
            'language': _languages[language] ?? language,
          },
          options: options);

  Future<Map<String, dynamic>> explain(String graph, String statement,
          {Map<String, Object?> parameters = const {},
          bool profile = false,
          String language = 'gql',
          String? namespace,
          CallOptions options = const CallOptions()}) async =>
      invoke(
          'graph.explain',
          {
            ...AppModule.ns(namespace),
            'graph': graph,
            'statement': statement,
            'parameters': _params(parameters),
            'profile': profile,
            'language': _languages[language] ?? language,
          },
          options: options);

  /// An `ExecuteResponse`'s rows, column name to Dart value.
  static List<Map<String, Object?>> records(Map<String, dynamic> response) {
    final rows = response['rows'] as Map? ?? const {};
    final columns = List<String>.from(rows['columns'] as List? ?? const []);
    return [
      for (final row in (rows['rows'] as List? ?? const []))
        {
          for (var i = 0; i < columns.length; i++)
            columns[i]: i < ((row as Map)['values'] as List? ?? const []).length
                ? Values.fromGraphValue(Map<String, dynamic>.from((row['values'] as List)[i] as Map))
                : null
        }
    ];
  }

  static Map<String, dynamic> _params(Map<String, Object?> parameters) =>
      {for (final e in parameters.entries) e.key: Values.graphValue(e.value)};

  static Map<String, dynamic>? _consistency(Object? level) {
    if (level == null) return null;
    if (level == 'strong') return {'strong': <String, dynamic>{}};
    if (level == 'eventual') return {'eventual': <String, dynamic>{}};
    return {'atLeast': ConsistencyToken.parse(level.toString()).toString()};
  }
}
