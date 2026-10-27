import 'dart:convert';
import 'dart:typed_data';
import 'collection_handle.dart';
import 'consistency.dart';
import 'envelopes.dart';
import 'errors.dart';
import 'idempotency.dart';
import 'streams.dart';
import 'token_source.dart';
import 'transport.dart';
import 'modules/app.dart';
import 'modules/data.dart';
import 'package:http/http.dart' as http;
import 'dart:io' show Platform;

class InstanceService {
  final Transport _transport;
  InstanceService(this._transport);

  Future<Map<String, dynamic>> getInstance([Map<String, dynamic> request = const {}]) async {
    final headers = {
      'content-type': 'application/json',
      'connect-protocol-version': '1',
    };
    final res = await _transport.callWithRetry('/loams.instance.v1.InstanceService/GetInstance', headers, request);
    return jsonDecode(res.bodyText) as Map<String, dynamic>;
  }

  Future<Map<String, dynamic>> whoAmI() async {
    final headers = {
      'content-type': 'application/json',
      'connect-protocol-version': '1',
    };
    final res = await _transport.callWithRetry('/loams.instance.v1.InstanceService/WhoAmI', headers, '{}');
    return jsonDecode(res.bodyText) as Map<String, dynamic>;
  }
}

class LiveService {
  final Transport _transport;
  LiveService(this._transport);

  Future<Map<String, dynamic>> query(Map<String, dynamic> request) async {
    final headers = {
      'content-type': 'application/json',
      'connect-protocol-version': '1',
    };
    final res = await _transport.callWithRetry('/loams.live.v1.LiveService/Query', headers, request);
    return jsonDecode(res.bodyText) as Map<String, dynamic>;
  }

  Future<StreamHandle> watch() async {
    final headers = {
      'content-type': 'application/connect+json',
      'connect-protocol-version': '1',
    };
    final body = Envelopes.pack(Uint8List.fromList(utf8.encode('{}')));
    final res = await _transport.execute('/loams.live.v1.LiveService/Watch', headers, body);
    if (res.status != 200) {
      throw ErrorParser.parse(res.status, res.bodyBytes);
    }
    final frames = Envelopes.split(res.bodyBytes);
    return StreamHandle(frames);
  }
}

class ApprovalService {
  final Transport _transport;
  ApprovalService(this._transport);

  Future<Map<String, dynamic>> decideApproval(Map<String, dynamic> request, {String? idempotencyKey}) async {
    final key = idempotencyKey ?? request['idempotencyKey'] ?? request['idempotency_key'] ?? Idempotency.mintKey();
    final payload = Map<String, dynamic>.from(request);
    payload['idempotencyKey'] = key;

    final headers = {
      'content-type': 'application/json',
      'connect-protocol-version': '1',
    };
    final res = await _transport.callWithRetry(
      '/loams.approvals.v1.ApprovalService/DecideApproval',
      headers,
      payload,
      isMutation: true,
      idempotencyKey: key,
    );
    return jsonDecode(res.bodyText) as Map<String, dynamic>;
  }

  Future<StreamHandle> watchApprovals({String? cursor}) async {
    final headers = {
      'content-type': 'application/connect+json',
      'connect-protocol-version': '1',
    };
    final reqObj = cursor != null ? {'cursor': cursor} : <String, dynamic>{};
    final body = Envelopes.pack(Uint8List.fromList(utf8.encode(jsonEncode(reqObj))));
    final res = await _transport.execute('/loams.approvals.v1.ApprovalService/WatchApprovals', headers, body);
    if (res.status != 200) {
      throw ErrorParser.parse(res.status, res.bodyBytes);
    }
    final frames = Envelopes.split(res.bodyBytes);
    return StreamHandle(frames, lastCursor: cursor);
  }

  Future<Map<String, dynamic>> listApprovals([Map<String, dynamic> request = const {}]) async {
    final headers = {
      'content-type': 'application/json',
      'connect-protocol-version': '1',
    };
    final res = await _transport.callWithRetry('/loams.approvals.v1.ApprovalService/ListApprovals', headers, request);
    return jsonDecode(res.bodyText) as Map<String, dynamic>;
  }
}

class DeviceService {
  final Transport _transport;
  DeviceService(this._transport);

  Future<Map<String, dynamic>> sendTestNotification([Map<String, dynamic> request = const {}]) async {
    final headers = {
      'content-type': 'application/json',
      'connect-protocol-version': '1',
    };
    final res =
        await _transport.callWithRetry('/loams.devices.v1.DeviceService/SendTestNotification', headers, request);
    return jsonDecode(res.bodyText) as Map<String, dynamic>;
  }
}

class LoamsClient implements AppContext, AppModules {
  @override
  final Transport transport;
  late final InstanceService instances;
  late final LiveService live;
  late final ApprovalService approvals;
  late final DeviceService devices;

  /// The default namespace (R11): fills every app request that names none.
  @override
  final String? defaultNamespace;

  /// The session store when `sessionConsistency` is on (R4), else null.
  @override
  final SessionStore? session;

  late final NamespacesModule namespaces;
  @override
  late final CollectionsModule collections;
  @override
  late final DocumentsModule documents;
  @override
  late final SearchModule search;
  @override
  late final VectorModule vector;

  /// Experimental: `loams.graph.v1` is unstable.
  late final GraphModule graph;

  late final SqlModule sql;
  late final StreamsModule streams;

  /// [apiKey] is sent as `Authorization: Bearer` (R1); [namespace] is the
  /// default namespace (R11); [timeout] a unary call's budget, null for none
  /// (R12); [sessionConsistency] turns the session store on (R4); [httpClient]
  /// replaces the default `http.Client`.
  LoamsClient(
    String endpoint, {
    TokenSource? tokenSource,
    String? apiKey,
    String? namespace,
    Duration? timeout = Transport.defaultTimeout,
    bool sessionConsistency = false,
    int maxRetries = 3,
    http.Client? httpClient,
  })  : transport = Transport(endpoint,
            tokenSource: tokenSource ?? (apiKey == null || apiKey.isEmpty ? null : StaticTokenSource(apiKey)),
            maxRetries: maxRetries,
            timeout: timeout,
            httpClient: httpClient),
        defaultNamespace = namespace,
        session = sessionConsistency ? SessionStore() : null {
    instances = InstanceService(transport);
    live = LiveService(transport);
    approvals = ApprovalService(transport);
    devices = DeviceService(transport);
    namespaces = NamespacesModule(this);
    collections = CollectionsModule(this);
    documents = DocumentsModule(this);
    search = SearchModule(this);
    vector = VectorModule(this);
    graph = GraphModule(this);
    sql = SqlModule(this);
    streams = StreamsModule(this);
  }

  /// A client from the environment (R1): `LOAMS_ENDPOINT`, `LOAMS_API_KEY` or
  /// `LOAMS_TOKEN`, and `LOAMS_NAMESPACE`.
  factory LoamsClient.fromEnv({
    Duration? timeout = Transport.defaultTimeout,
    bool sessionConsistency = false,
    Map<String, String>? environment,
  }) {
    final env = environment ?? Platform.environment;
    final endpoint = env['LOAMS_ENDPOINT'] ?? '';
    if (endpoint.isEmpty) {
      throw InvalidArgumentException('LOAMS_ENDPOINT is not set', code: 'invalid_argument', httpStatus: 400);
    }
    final key = env['LOAMS_API_KEY'] ?? '';
    final token = env['LOAMS_TOKEN'] ?? '';
    final ns = env['LOAMS_NAMESPACE'] ?? '';
    return LoamsClient(
      endpoint,
      apiKey: key.isNotEmpty ? key : (token.isNotEmpty ? token : null),
      namespace: ns.isEmpty ? null : ns,
      timeout: timeout,
      sessionConsistency: sessionConsistency,
    );
  }

  late final RawModule _raw = RawModule(this);

  /// Any facade call by its facade-index name (`documents.write`,
  /// `instance.get_instance`) over its request's proto3 JSON.
  Future<Map<String, dynamic>> call(String name, Map<String, dynamic> request,
      {CallOptions options = const CallOptions()}) async {
    if (name == 'instance.get_instance' || name == 'instance.who_am_i') {
      final method = name == 'instance.get_instance' ? 'GetInstance' : 'WhoAmI';
      final res = await transport.callWithRetry(
        '/loams.instance.v1.InstanceService/$method',
        {'content-type': 'application/json', 'connect-protocol-version': '1', ...options.headers},
        request.isEmpty ? '{}' : jsonEncode(request),
        timeout: options.timeout,
      );
      return jsonDecode(res.bodyText) as Map<String, dynamic>;
    }
    return _raw.call(name, request, options: options);
  }

  /// A handle on one collection (SDKP §2.8).
  CollectionHandle collection(String name, {String? namespace}) => CollectionHandle(this, name, namespace: namespace);

  void close() {
    transport.close();
  }
}
